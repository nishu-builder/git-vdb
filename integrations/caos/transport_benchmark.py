#!/usr/bin/env python3
"""Measure five fresh search stages through http_meter on an otherwise idle stack."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("caos", type=Path)
parser.add_argument("result_id", help="A semantic-search result already stored in Caos")
parser.add_argument("query", help="The original natural-language query")
parser.add_argument("output", type=Path)
parser.add_argument("--meter-log", type=Path, required=True)
args = parser.parse_args()
cli = str(args.caos.resolve())
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=True)
def command(*arguments):
    return subprocess.check_output([cli, *arguments], text=True)
def oid(text, kind):
    match = re.search(r"^"+kind+r" ([0-9a-f]{40})$", text, re.M)
    assert match, text
    return match[1]
command("get", args.result_id, str(output/"source"))
source = json.loads((output/"source/results.json").read_text())
index = subprocess.check_output(["git", "rev-parse", args.result_id+":snapshot"], text=True).strip()
base = oid(command("eval-path", "caos-tools/semantic-search"), "tree")
vector = oid(command("run", str(output/"query-vector.json"), "--base:hash="+base,
                     "--stage=embed-query", "--in="+args.query), "blob")
runs = []
for repetition in range(5):
    kvs = ["--base:hash="+base, "--stage=search", "--index:hash="+index,
           "--result:hash="+vector, "--source-tree="+source["source_tree"],
           "--source-commit="+(source["source_commit"] or ""), "--scope=.", "--limit=10",
           "--run-salt=transport-"+str(time.time_ns())]
    request = command("prepare-request", *kvs).strip()
    offset = args.meter_log.stat().st_size
    started = time.perf_counter_ns()
    result = oid(command("run", str(output/str(repetition)), *kvs), "tree")
    elapsed = time.perf_counter_ns()-started
    with args.meter_log.open("rb") as log:
        log.seek(offset)
        traffic = [json.loads(line) for line in log if line.strip()]
    value = json.loads((output/str(repetition)/"results.json").read_text())
    objects = {entry["object"] for entry in value["objects"]}
    gets = [entry for entry in traffic if entry["method"] == "GET" and entry["status"] == 200]
    index_gets = [entry for entry in gets if entry["path"].removeprefix("/object/") in objects]
    observed = {entry["path"].removeprefix("/object/") for entry in index_gets}
    assert objects <= observed, ("index objects bypassed meter", objects-observed)
    trace = json.loads(command("status", "--all", request))
    assert "started" in trace and not trace.get("reused", False), "search was not freshly executed"
    (output/(str(repetition)+"-traffic.json")).write_text(json.dumps(traffic, indent=2)+"\n")
    (output/(str(repetition)+"-trace.json")).write_text(json.dumps(trace, indent=2)+"\n")
    runs.append({"elapsed_ns":elapsed, "request":request, "result":result,
                 "index_object_gets":len(index_gets), "unique_index_objects":len(objects),
                 "index_response_body_bytes":sum(entry["response_bytes"] for entry in index_gets),
                 "all_get_response_body_bytes":sum(entry["response_bytes"] for entry in gets),
                 "logical_reads":value["reads"], "stats":value["stats"]})
assert len({run["result"] for run in runs}) == 1
value = {"source_result":args.result_id, "query":args.query, "tool":base,
         "query_vector":vector, "runs":runs,
         "meter_log_sha256":hashlib.sha256(args.meter_log.read_bytes()).hexdigest(),
         "note":"Five forced-fresh search stages, reused index and query vector. HTTP response bodies include object framing but exclude headers and TCP. Stack must otherwise be idle. Root/object identity checks include every visited index object."}
(output/"summary.json").write_text(json.dumps(value, indent=2)+"\n")
print(json.dumps(value, indent=2))
