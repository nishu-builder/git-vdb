from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import time
from pathlib import Path

import h5py
import numpy as np


ROOT = Path(__file__).resolve().parents[2]
SEED = 0x676974766462626D


class SplitMix64:
    def __init__(self, seed: int):
        self.state = seed

    def next(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & ((1 << 64) - 1)
        value = self.state
        value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & ((1 << 64) - 1)
        value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & ((1 << 64) - 1)
        return value ^ (value >> 31)

    def signed(self) -> np.float32:
        unit = np.float32(self.next() >> 40) / np.float32(1 << 24)
        return np.float32(unit * np.float32(2.0) - np.float32(1.0))


def generate_synthetic(case: dict, directory: Path) -> tuple[Path, Path]:
    rng = SplitMix64(SEED)
    count = case["points"]
    dimension = case["dimension"]
    query_count = case["queries"]
    if case["dataset"] == "synthetic_uniform":
        points = np.empty((count, dimension), dtype="<f4")
        queries = np.empty((query_count, dimension), dtype="<f4")
        for row in range(count):
            for column in range(dimension):
                points[row, column] = rng.signed()
        for row in range(query_count):
            for column in range(dimension):
                queries[row, column] = rng.signed()
    else:
        clusters = min(32, max(1, count))
        centers = np.empty((clusters, dimension), dtype="<f4")
        for row in range(clusters):
            for column in range(dimension):
                centers[row, column] = rng.signed()
        points = np.empty((count, dimension), dtype="<f4")
        for row in range(count):
            for column in range(dimension):
                points[row, column] = np.float32(
                    centers[row % clusters, column]
                    + rng.signed() * np.float32(0.08)
                )
        queries = np.empty((query_count, dimension), dtype="<f4")
        for row in range(query_count):
            for column in range(dimension):
                queries[row, column] = np.float32(
                    centers[row % clusters, column]
                    + rng.signed() * np.float32(0.04)
                )
    return write_vectors(directory, points, queries)


def generate_real(case: dict, directory: Path, cache: Path) -> tuple[Path, Path]:
    datasets = json.loads((ROOT / "benchmarks/lancedb/datasets.json").read_text())
    definition = datasets["real"][case["dataset"]]
    source = cache / "glove-25-angular.hdf5"
    if not source.exists():
        subprocess.run(
            ["curl", "-fL", "--retry", "3", "-o", str(source), definition["url"]],
            check=True,
        )
    actual = sha256_file(source)
    if actual != definition["sha256"]:
        raise ValueError(f"real dataset checksum mismatch: {actual}")
    with h5py.File(source) as dataset:
        points = np.asarray(dataset["train"][: case["points"]], dtype="<f4")
        queries = np.asarray(dataset["test"][: case["queries"]], dtype="<f4")
    if points.shape[1] != case["dimension"]:
        raise ValueError(f"real dataset dimension is {points.shape[1]}")
    return write_vectors(directory, points, queries)


def write_vectors(directory: Path, points: np.ndarray, queries: np.ndarray):
    directory.mkdir(parents=True, exist_ok=True)
    points_path = directory / "points.f32le"
    queries_path = directory / "queries.f32le"
    points.astype("<f4", copy=False).tofile(points_path)
    queries.astype("<f4", copy=False).tofile(queries_path)
    return points_path, queries_path


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def number_key(value: float) -> str:
    return format(value, "g")


def command_output(*args: str) -> str:
    return subprocess.run(args, cwd=ROOT, check=True, text=True, capture_output=True).stdout.strip()


def metadata(workload_path: Path, workload: dict) -> dict:
    memory = None
    if platform.system() == "Darwin":
        memory = int(command_output("sysctl", "-n", "hw.memsize"))
        cpu = command_output("sysctl", "-n", "machdep.cpu.brand_string")
    else:
        cpu = platform.processor()
        try:
            memory = int(os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES"))
        except (ValueError, OSError):
            pass
    return {
        "schema_version": 1,
        "workload": workload,
        "workload_path": str(workload_path.relative_to(ROOT)),
        "workload_sha256": sha256_file(workload_path),
        "dependency_lock_sha256": sha256_file(ROOT / "benchmarks/lancedb/uv.lock"),
        "git_revision": command_output("git", "rev-parse", "HEAD"),
        "git_dirty": bool(command_output("git", "status", "--porcelain")),
        "os": platform.platform(),
        "architecture": platform.machine(),
        "cpu": cpu,
        "memory_bytes": memory,
        "rustc": command_output("rustc", "--version", "--verbose"),
        "python": sys.version,
        "cold_warm_protocol": "Each repetition is a cold runner process. Each process performs one unmeasured exact and approximate warmup query before recording warm-query samples.",
        "clock": "monotonic wall time (perf_counter/Instant)",
    }


def validate_workload(workload: dict) -> None:
    if workload.get("schema_version") != 1:
        raise ValueError("workload schema_version must be 1")
    if not workload.get("cases") or workload.get("repetitions", 0) < 1:
        raise ValueError("workload must have cases and positive repetitions")
    required_steps = {"k", "mutation_fractions", "filter_selectivities", "concurrency"}
    if set(workload.get("steps", {})) != required_steps:
        raise ValueError(f"steps must contain exactly {sorted(required_steps)}")


def run_command(command: list[str], stderr_path: Path) -> None:
    completed = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
    stderr_path.write_text(completed.stderr)
    if completed.returncode:
        raise subprocess.CalledProcessError(
            completed.returncode, command, completed.stdout, completed.stderr
        )


def oracle(points: np.ndarray, query: np.ndarray, limit: int, selectivity=None):
    eligible = np.arange(points.shape[0], dtype=np.int64)
    selected = points
    if selectivity is not None:
        mask = eligible % 1000 < round(selectivity * 1000)
        eligible = eligible[mask]
        selected = points[mask]
    query64 = query.astype(np.float64)
    points64 = selected.astype(np.float64)
    denominator = np.linalg.norm(points64, axis=1) * np.linalg.norm(query64)
    scores = np.divide(
        points64 @ query64,
        denominator,
        out=np.zeros(points64.shape[0], dtype=np.float64),
        where=denominator != 0,
    )
    order = np.lexsort((eligible, -scores))[:limit]
    return eligible[order], scores[order]


def assess_results(points, queries, results, ks, selectivity=None):
    exact_id_agreement = {str(k): [] for k in ks}
    score_error = []
    for query, actual in zip(queries, results, strict=True):
        wanted_ids, wanted_scores = oracle(points, query, max(ks), selectivity)
        actual_ids = np.array([row["id"] for row in actual], dtype=np.int64)
        actual_scores = np.array([row["score"] for row in actual], dtype=np.float64)
        for k in ks:
            expected_count = min(k, wanted_ids.size)
            exact_id_agreement[str(k)].append(
                bool(np.array_equal(actual_ids[:expected_count], wanted_ids[:expected_count]))
                and actual_ids.size >= expected_count
            )
        comparable = min(actual_scores.size, wanted_scores.size)
        if comparable:
            score_error.extend(np.abs(actual_scores[:comparable] - wanted_scores[:comparable]))
    return {
        "id_agreement": {key: all(values) for key, values in exact_id_agreement.items()},
        "max_score_error": max(score_error, default=0.0),
    }


def recall(points, queries, approximate, ks, selectivity=None):
    recalls = {str(k): [] for k in ks}
    counts_ok = {str(k): [] for k in ks}
    for query, actual in zip(queries, approximate, strict=True):
        wanted_ids, _ = oracle(points, query, max(ks), selectivity)
        actual_ids = [row["id"] for row in actual]
        for k in ks:
            expected_count = min(k, wanted_ids.size)
            wanted = set(wanted_ids[:expected_count].tolist())
            found = set(actual_ids[:expected_count])
            recalls[str(k)].append(len(wanted & found) / max(1, expected_count))
            counts_ok[str(k)].append(len(actual_ids) >= expected_count)
    return {
        "recall": {key: float(np.mean(values)) for key, values in recalls.items()},
        "result_count_ok": {key: all(values) for key, values in counts_ok.items()},
    }


def percentiles(samples: list[int]) -> dict:
    return {
        "p50_us": float(np.percentile(samples, 50)),
        "p95_us": float(np.percentile(samples, 95)),
        "p99_us": float(np.percentile(samples, 99)),
    }


def summarize(case: dict, points, queries, git_reports, lance_reports, steps):
    summary = {"case": case, "engines": {}}
    for engine, reports in (("git-vdb", git_reports), ("lancedb", lance_reports)):
        if engine == "git-vdb":
            cores = [report["snapshot_core"] for report in reports]
            build = [core["build_us"] for core in cores]
            exact_times = sum((core["exact_query_us"] for core in cores), [])
            approximate_times = sum((core["approximate_query_us"] for core in cores), [])
            exact_results = cores[0]["exact_results"]
            approximate_results = cores[0]["approximate_results"]
            disk = [core["on_disk_bytes"] for core in cores]
        else:
            build = [report["build_us"] + report["index_build_us"] for report in reports]
            exact_times = sum((report["exact_query_us"] for report in reports), [])
            approximate_times = sum((report["approximate_query_us"] for report in reports), [])
            exact_results = reports[0]["exact_results"]
            approximate_results = reports[0]["approximate_results"]
            disk = [report["on_disk_bytes"] for report in reports]
        engine_summary = {
            "build": percentiles(build),
            "exact_query": percentiles(exact_times),
            "approximate_query": percentiles(approximate_times),
            "on_disk_bytes_median": float(np.median(disk)),
            "bytes_per_point": float(np.median(disk) / case["points"]),
            "exact_correctness": assess_results(points, queries, exact_results, steps["k"]),
            "approximate": recall(points, queries, approximate_results, steps["k"]),
            "filtered": {},
            "mutations": {},
        }
        if engine == "git-vdb":
            adapters = [report["named_adapter"] for report in reports]
            engine_summary["named_adapter"] = {
                "build": percentiles([adapter["build_us"] for adapter in adapters]),
                "exact_query": percentiles(
                    sum((adapter["exact_query_us"] for adapter in adapters), [])
                ),
                "approximate_query": percentiles(
                    sum((adapter["approximate_query_us"] for adapter in adapters), [])
                ),
                "historical_read": percentiles(
                    [adapter["historical_read_us"] for adapter in adapters]
                ),
                "exact_correctness": assess_results(
                    points, queries, adapters[0]["exact_results"], steps["k"]
                ),
                "approximate": recall(
                    points, queries, adapters[0]["approximate_results"], steps["k"]
                ),
            }
        for selectivity in steps["filter_selectivities"]:
            filtered = (
                reports[0]["snapshot_core"]["filtered"][number_key(selectivity)]
                if engine == "git-vdb"
                else reports[0]["filtered"][number_key(selectivity)]
            )
            engine_summary["filtered"][str(selectivity)] = {
                "exact_correctness": assess_results(
                    points, queries, filtered["exact_results"], steps["k"], selectivity
                ),
                "approximate": recall(
                    points,
                    queries,
                    filtered["approximate_results"],
                    steps["k"],
                    selectivity,
                ),
            }
        for fraction in steps["mutation_fractions"]:
            samples = [
                (
                    report["snapshot_core"]["mutations"][number_key(fraction)]
                    if engine == "git-vdb"
                    else report["mutations"][number_key(fraction)]
                )
                for report in reports
            ]
            engine_summary["mutations"][str(fraction)] = {
                "upsert": percentiles([sample["upsert_us"] for sample in samples]),
                "delete": percentiles([sample["delete_us"] for sample in samples]),
            }
        summary["engines"][engine] = engine_summary
    return summary


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--workload", type=Path, default=ROOT / "benchmarks/lancedb/workloads/smoke.json"
    )
    parser.add_argument("--output", type=Path)
    parser.add_argument("--case", action="append", dest="cases")
    args = parser.parse_args()
    workload_path = args.workload.resolve()
    workload = json.loads(workload_path.read_text())
    validate_workload(workload)
    timestamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    output = (args.output or ROOT / "target/lancedb-results" / f"{workload['name']}-{timestamp}").resolve()
    output.mkdir(parents=True, exist_ok=False)
    cache = ROOT / "target/lancedb-cache"
    cache.mkdir(parents=True, exist_ok=True)
    (output / "metadata.json").write_text(
        json.dumps(metadata(workload_path, workload), indent=2, sort_keys=True)
    )

    subprocess.run(
        ["cargo", "build", "--release", "--example", "lancedb_git_vdb_runner"],
        cwd=ROOT,
        check=True,
    )
    rust_runner = ROOT / "target/release/examples/lancedb_git_vdb_runner"
    summaries = []
    for case in workload["cases"]:
        if args.cases and case["name"] not in args.cases:
            continue
        case_dir = output / case["name"]
        data_dir = case_dir / "data"
        case_dir.mkdir(parents=True)
        if case["dataset"].startswith("synthetic_"):
            points_path, queries_path = generate_synthetic(case, data_dir)
        else:
            points_path, queries_path = generate_real(case, data_dir, cache)
        case_metadata = {
            "points_sha256": sha256_file(points_path),
            "queries_sha256": sha256_file(queries_path),
        }
        (case_dir / "dataset.json").write_text(json.dumps(case_metadata, indent=2))
        spec = {
            "schema_version": 1,
            "case_name": case["name"],
            "dimension": case["dimension"],
            "point_count": case["points"],
            "query_count": case["queries"],
            "points_path": str(points_path),
            "queries_path": str(queries_path),
            **workload["steps"],
        }
        spec_path = case_dir / "run.json"
        spec_path.write_text(json.dumps(spec, indent=2))
        git_reports = []
        lance_reports = []
        for repetition in range(workload["repetitions"]):
            git_output = case_dir / f"git-vdb-{repetition}.json"
            run_command(
                [str(rust_runner), str(spec_path), str(git_output)],
                case_dir / f"git-vdb-{repetition}.stderr",
            )
            git_reports.append(json.loads(git_output.read_text()))
            lance_output = case_dir / f"lancedb-{repetition}.json"
            run_command(
                [sys.executable, str(ROOT / "benchmarks/lancedb/lancedb_runner.py"), str(spec_path), str(lance_output)],
                case_dir / f"lancedb-{repetition}.stderr",
            )
            lance_reports.append(json.loads(lance_output.read_text()))
        points = np.fromfile(points_path, dtype="<f4").reshape(case["points"], case["dimension"])
        queries = np.fromfile(queries_path, dtype="<f4").reshape(case["queries"], case["dimension"])
        summaries.append(summarize(case, points, queries, git_reports, lance_reports, workload["steps"]))
    (output / "summary.json").write_text(json.dumps(summaries, indent=2, sort_keys=True))
    print(output)


if __name__ == "__main__":
    main()
