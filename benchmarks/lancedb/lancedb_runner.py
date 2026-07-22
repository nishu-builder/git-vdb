from __future__ import annotations

import json
import math
import shutil
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import lancedb
from lancedb.index import IvfFlat
import numpy as np
import pyarrow as pa


def elapsed_us(started: int) -> int:
    return (time.perf_counter_ns() - started) // 1_000


def number_key(value: float) -> str:
    return format(value, "g")


def read_spec(path: Path) -> dict:
    spec = json.loads(path.read_text())
    if spec["schema_version"] != 1:
        raise ValueError(f"unsupported harness schema version {spec['schema_version']}")
    return spec


def load_vectors(path: str, rows: int, dimension: int) -> np.ndarray:
    vectors = np.fromfile(path, dtype="<f4")
    expected = rows * dimension
    if vectors.size != expected:
        raise ValueError(f"{path} has {vectors.size} components, expected {expected}")
    return vectors.reshape(rows, dimension)


def arrow_table(vectors: np.ndarray) -> pa.Table:
    rows, dimension = vectors.shape
    vector_array = pa.FixedSizeListArray.from_arrays(
        pa.array(vectors.reshape(-1), type=pa.float32()), dimension
    )
    return pa.table(
        {
            "id": pa.array(np.arange(rows, dtype=np.int64)),
            "vector": vector_array,
            "selectivity_bucket": pa.array(
                np.arange(rows, dtype=np.int64) % 1000
            ),
        }
    )


def normalized_results(rows: list[dict]) -> list[dict]:
    result = [
        {"id": int(row["id"]), "score": float(1.0 - row["_distance"])}
        for row in rows
    ]
    result.sort(key=lambda item: (-item["score"], item["id"]))
    return result


def query_all(table, queries: np.ndarray, limit: int, exact: bool, predicate=None):
    durations = []
    results = []
    for vector in queries:
        builder = (
            table.search(vector)
            .metric("cosine")
            .select(["id", "_distance"])
            .limit(limit)
        )
        if exact:
            builder = builder.bypass_vector_index()
        else:
            builder = builder.nprobes(8)
        if predicate is not None:
            builder = builder.where(predicate, prefilter=True)
        started = time.perf_counter_ns()
        rows = builder.to_list()
        durations.append(elapsed_us(started))
        results.append(normalized_results(rows))
    return durations, results


def query_throughput(table, queries, limit, exact, concurrencies):
    measurements = {}
    for workers in concurrencies:
        started = time.perf_counter_ns()
        with ThreadPoolExecutor(max_workers=workers) as executor:
            futures = [
                executor.submit(
                    query_all,
                    table,
                    queries[worker::workers],
                    limit,
                    exact,
                )
                for worker in range(workers)
            ]
            for future in futures:
                future.result()
        wall_us = elapsed_us(started)
        measurements[str(workers)] = {
            "queries": len(queries),
            "wall_us": wall_us,
            "queries_per_second": len(queries) * 1_000_000.0 / wall_us,
        }
    return measurements


def directory_bytes(path: Path) -> int:
    return sum(item.stat().st_size for item in path.rglob("*") if item.is_file())


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: lancedb_runner.py INPUT.json OUTPUT.json")
    spec = read_spec(Path(sys.argv[1]))
    points = load_vectors(spec["points_path"], spec["point_count"], spec["dimension"])
    queries = load_vectors(spec["queries_path"], spec["query_count"], spec["dimension"])
    data = arrow_table(points)
    maximum_k = max(spec["k"])

    with tempfile.TemporaryDirectory(prefix="git-vdb-lancedb-") as temporary:
        database_path = Path(temporary)
        started = time.perf_counter_ns()
        database = lancedb.connect(database_path)
        setup_us = elapsed_us(started)
        started = time.perf_counter_ns()
        table = database.create_table("benchmark", data=data)
        build_us = elapsed_us(started)
        partitions = max(1, min(256, round(math.sqrt(spec["point_count"]))))
        started = time.perf_counter_ns()
        table.create_index(
            "vector",
            config=IvfFlat(distance_type="cosine", num_partitions=partitions),
        )
        index_build_us = elapsed_us(started)
        baseline_on_disk_bytes = directory_bytes(database_path)

        # One unmeasured query per mode faults code and data before warm samples.
        query_all(table, queries[:1], maximum_k, True)
        query_all(table, queries[:1], maximum_k, False)
        exact_query_us, exact_results = query_all(
            table, queries, maximum_k, True
        )
        approximate_query_us, approximate_results = query_all(
            table, queries, maximum_k, False
        )
        throughput = {
            "exact": query_throughput(
                table, queries, maximum_k, True, spec["concurrency"]
            ),
            "approximate": query_throughput(
                table, queries, maximum_k, False, spec["concurrency"]
            ),
        }

        filtered = {}
        for selectivity in spec["filter_selectivities"]:
            threshold = round(selectivity * 1000)
            predicate = f"selectivity_bucket < {threshold}"
            exact_us, exact = query_all(
                table, queries, maximum_k, True, predicate
            )
            approximate_us, approximate = query_all(
                table, queries, maximum_k, False, predicate
            )
            filtered[number_key(selectivity)] = {
                "exact_query_us": exact_us,
                "approximate_query_us": approximate_us,
                "exact_results": exact,
                "approximate_results": approximate,
            }

        mutations = {}
        for index, fraction in enumerate(spec["mutation_fractions"]):
            count = max(1, min(spec["point_count"], round(spec["point_count"] * fraction)))
            mutation_table = database.create_table(f"mutation_{index}", data=data)
            changed = points[:count].copy()
            changed[:, 0] += np.float32(0.001)
            changed_data = arrow_table(changed)
            started = time.perf_counter_ns()
            (
                mutation_table.merge_insert("id")
                .when_matched_update_all()
                .when_not_matched_insert_all()
                .execute(changed_data)
            )
            upsert_us = elapsed_us(started)
            delete_table = database.create_table(f"delete_{index}", data=data)
            started = time.perf_counter_ns()
            delete_table.delete(f"id < {count}")
            delete_us = elapsed_us(started)
            mutations[number_key(fraction)] = {
                "points": count,
                "upsert_us": upsert_us,
                "delete_us": delete_us,
            }

        report = {
            "schema_version": 1,
            "engine": "lancedb",
            "engine_version": lancedb.__version__,
            "case_name": spec["case_name"],
            "point_count": spec["point_count"],
            "dimension": spec["dimension"],
            "query_count": spec["query_count"],
            "k": spec["k"],
            "setup_us": setup_us,
            "build_us": build_us,
            "index_build_us": index_build_us,
            "index": {
                "type": "IVF_FLAT",
                "partitions": partitions,
                "nprobes": 8,
            },
            "exact_query_us": exact_query_us,
            "approximate_query_us": approximate_query_us,
            "exact_results": exact_results,
            "approximate_results": approximate_results,
            "throughput": throughput,
            "filtered": filtered,
            "mutations": mutations,
            "on_disk_bytes": baseline_on_disk_bytes,
            "semantic_differences": [
                "LanceDB table versions are not Git roots or collection refs.",
                "LanceDB integer IDs use signed int64 in this common-subset harness; git-vdb stores the same nonnegative values as uint64.",
            ],
        }
        Path(sys.argv[2]).write_text(json.dumps(report, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
