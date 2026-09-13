#!/usr/bin/env python3
"""Run fixed queries in five fresh processes per reader, with an independent oracle."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics
import struct
import subprocess

def f32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]

def cosine(left, right):
    a = [f32(x) for x in left]
    b = [f32(x) for x in right]
    denominator = math.sqrt(sum(x*x for x in a)) * math.sqrt(sum(x*x for x in b))
    return sum(x*y for x,y in zip(a,b)) / denominator if denominator else 0.0

def accepted(point, filters):
    if not filters:
        return True
    assert not filters.get("should") and not filters.get("must_not"), "benchmark only supports declared equality filters"
    for condition in filters.get("must", []):
        assert set(condition) == {"key", "match"}, condition
        if point["payload"].get(condition["key"]) != condition["match"]["value"]:
            return False
    return True

def distribution(values):
    return {"min": min(values), "median": statistics.median(values), "max": max(values)}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    points = json.loads((args.corpus / "points.json").read_text())
    queries = json.loads((args.corpus / "queries.json").read_text())
    raw, summary = [], {}
    for name, query in queries.items():
        ranked = [(cosine(query["vector"], point["vector"]), point["id"]) for point in points if accepted(point, query.get("filter"))]
        ranked.sort(key=lambda pair: (-pair[0], (0, pair[1]) if isinstance(pair[1], str) else (1, pair[1])))
        oracle = [pair[1] for pair in ranked[:query["limit"]]]
        query_path = args.output / (name + ".json")
        query_path.write_text(json.dumps(query))
        samples = {"eager": [], "lazy": []}
        expected = None
        for repetition in range(5):
            for mode in (["eager", "lazy"] if repetition % 2 == 0 else ["lazy", "eager"]):
                record = json.loads(subprocess.check_output([str(args.binary.resolve()), mode, str(args.corpus.resolve()), str(query_path.resolve())]))
                result = record["result"]
                if expected is None:
                    expected = result
                assert result == expected, (name, mode, "baseline mismatch")
                ids = [point["id"] for point in result["points"]]
                if query.get("params", {}).get("exact") is True:
                    assert ids == oracle, (name, "independent exact oracle mismatch")
                    for hit, (score, _) in zip(result["points"], ranked):
                        assert abs(hit["score"] - score) < 1e-6
                assert all(accepted(point, query.get("filter")) for point in result["points"])
                record.update(workload=name, repetition=repetition, recall=len(set(ids) & set(oracle))/len(oracle) if oracle else 1.0)
                samples[mode].append(record)
                raw.append(record)
        summary[name] = {}
        for mode, records in samples.items():
            cold = [(r["setup_ns"]+r["query_ns"][0])/1e6 for r in records]
            warm = [n/1e6 for r in records for n in r["query_ns"][1:]]
            memory = [int(r["peak_memory"].split()[1]) for r in records if r["peak_memory"]]
            summary[name][mode] = {
                "fresh_process_ms": distribution(cold), "warm_query_ms": distribution(warm),
                "peak_rss_kib": distribution(memory) if memory else None,
                "logical_reads": records[0]["reads"],
                "recall": records[0]["recall"], "stats": records[0]["result"]["stats"],
            }
    raw_path = args.output/"raw.json"
    raw_path.write_text(json.dumps(raw, sort_keys=True, separators=(",", ":")))
    summary["provenance"] = {
        "root": (args.corpus/"root").read_text().strip(), "repetitions":5,
        "raw_sha256":hashlib.sha256(raw_path.read_bytes()).hexdigest(),
        "points_sha256":hashlib.sha256((args.corpus/"points.json").read_bytes()).hexdigest(),
        "queries_sha256":hashlib.sha256((args.corpus/"queries.json").read_bytes()).hexdigest(),
        "notes":"Fresh processes; filesystem cache is not dropped. Logical blob bytes exclude trees, compression and Git import writes.",
    }
    (args.output/"summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True)+"\n")
    print(json.dumps(summary, indent=2))
if __name__ == "__main__":
    main()
