#!/usr/bin/env python3
"""Standalone deterministic format-v2 layout and IVF-flat prototype.

This is benchmark code, not a stable reader or writer. It writes canonical
candidate blobs into a temporary bare Git repository so root equality, object
count, logical bytes, packing, and transfer can be measured before production
format obligations are accepted.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import statistics
import struct
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

import numpy as np


SHARD_BITS = 12
SAMPLE_LIMIT = 1024
TRAINING_ITERATIONS = 4
PROBES = 8
CANDIDATE_LIMIT = 10_000


@dataclass
class PrototypeIndex:
    ids: np.ndarray
    points: np.ndarray
    hashes: list[bytes]
    shards: np.ndarray
    order: np.ndarray
    shard_rows: np.ndarray
    sample_indices: np.ndarray
    centroids: np.ndarray
    assignments: np.ndarray
    postings: list[np.ndarray]


class GitWriter:
    def __init__(self, path: Path):
        self.path = path
        subprocess.run(["git", "init", "--bare", str(path)], check=True, capture_output=True)
        self.blob_sizes: dict[str, int] = {}
        self.staging = tempfile.TemporaryDirectory(prefix="git-vdb-format2-blobs-")
        self.batch = 0

    def command(self, *args: str, input_bytes: bytes | None = None) -> str:
        return subprocess.run(
            ["git", f"--git-dir={self.path}", *args],
            input=input_bytes,
            check=True,
            capture_output=True,
        ).stdout.decode().strip()

    def blob(self, data: bytes) -> str:
        oid = self.command("hash-object", "-w", "--stdin", input_bytes=data)
        self.blob_sizes.setdefault(oid, len(data))
        return oid

    def blobs(self, values: dict[str, bytes]) -> dict[str, str]:
        self.batch += 1
        directory = Path(self.staging.name) / str(self.batch)
        directory.mkdir()
        names = sorted(values, key=lambda name: name.encode())
        paths = []
        for name in names:
            path = directory / name
            path.write_bytes(values[name])
            paths.append(path)
        completed = subprocess.run(
            ["git", f"--git-dir={self.path}", "hash-object", "-w", "--stdin-paths"],
            input=("\n".join(str(path) for path in paths) + "\n").encode(),
            check=True,
            capture_output=True,
        )
        oids = completed.stdout.decode().splitlines()
        if len(oids) != len(names):
            raise RuntimeError(f"git hash-object returned {len(oids)} IDs for {len(names)} blobs")
        output = dict(zip(names, oids, strict=True))
        for name, oid in output.items():
            self.blob_sizes.setdefault(oid, len(values[name]))
        return output

    def tree(self, entries: list[tuple[str, str, str]]) -> str:
        encoded = b"".join(
            f"{mode} {kind} {oid}\t{name}\n".encode()
            for mode, kind, oid, name in sorted(entries, key=lambda entry: entry[3].encode())
        )
        return self.command("mktree", input_bytes=encoded)

    def blob_tree(self, blobs: dict[str, bytes]) -> str:
        return self.tree(
            [
                ("100644", "blob", oid, name)
                for name, oid in self.blobs(blobs).items()
            ]
        )

    def root_blobs(self, root: str) -> set[str]:
        output = self.command("ls-tree", "-r", root)
        return {
            line.split(maxsplit=3)[2]
            for line in output.splitlines()
            if line and len(line.split(maxsplit=3)) == 4
        }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-spec", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repository", type=Path)
    return parser.parse_args()


def read_vectors(path: Path, count: int, dimension: int) -> np.ndarray:
    values = np.fromfile(path, dtype="<f4")
    expected = count * dimension
    if values.size != expected:
        raise ValueError(f"{path} has {values.size} components, expected {expected}")
    points = values.reshape(count, dimension)
    if not np.isfinite(points).all():
        raise ValueError(f"{path} contains non-finite components")
    return points


def id_hash(identifier: int) -> bytes:
    return hashlib.sha256(b"u\0" + identifier.to_bytes(8, "big")).digest()


def normalized(rows: np.ndarray) -> np.ndarray:
    rows64 = rows.astype(np.float64, copy=False)
    norms = np.linalg.norm(rows64, axis=1, keepdims=True)
    return np.divide(rows64, norms, out=np.zeros_like(rows64), where=norms != 0)


def assign(points: np.ndarray, centroids: np.ndarray, batch_size: int = 8192) -> np.ndarray:
    centroid_rows = normalized(centroids)
    output = np.empty(points.shape[0], dtype=np.uint16)
    for start in range(0, points.shape[0], batch_size):
        stop = min(points.shape[0], start + batch_size)
        similarities = normalized(points[start:stop]) @ centroid_rows.T
        # np.argmax chooses the first (lowest) centroid on a tie.
        output[start:stop] = np.argmax(similarities, axis=1).astype(np.uint16)
    return output


def train(points: np.ndarray, sample_indices: np.ndarray, centroid_count: int) -> np.ndarray:
    sample = points[sample_indices]
    if centroid_count == 1:
        positions = np.array([0], dtype=np.int64)
    else:
        positions = np.linspace(0, sample.shape[0] - 1, centroid_count, dtype=np.int64)
    centroids = sample[positions].astype("<f4", copy=True)
    for _ in range(TRAINING_ITERATIONS):
        assignments = assign(sample, centroids, batch_size=sample.shape[0])
        updated = centroids.copy()
        for centroid in range(centroid_count):
            members = sample[assignments == centroid]
            if members.size:
                # Canonical sample order is retained by boolean selection.
                updated[centroid] = np.sum(members, axis=0, dtype=np.float64) / members.shape[0]
        centroids = updated.astype("<f4", copy=False)
    return centroids


def build_index(ids: np.ndarray, points: np.ndarray) -> PrototypeIndex:
    hashes = [id_hash(int(identifier)) for identifier in ids]
    shards = np.fromiter(
        ((digest[0] << 4) | (digest[1] >> 4) for digest in hashes),
        dtype=np.uint16,
        count=len(hashes),
    )
    order = np.array(
        sorted(range(len(ids)), key=lambda index: (int(shards[index]), int(ids[index]))),
        dtype=np.int64,
    )
    shard_rows = np.empty(len(ids), dtype=np.uint16)
    previous_shard = None
    row = 0
    for index in order:
        shard = int(shards[index])
        if shard != previous_shard:
            previous_shard = shard
            row = 0
        if row > np.iinfo(np.uint16).max:
            raise ValueError(f"prototype shard {shard:03x} exceeds u16 rows")
        shard_rows[index] = row
        row += 1

    sample_order = sorted(range(len(ids)), key=lambda index: (hashes[index], int(ids[index])))
    sample_indices = np.array(sample_order[: min(SAMPLE_LIMIT, len(ids))], dtype=np.int64)
    centroid_count = min(4096, max(1, round(math.sqrt(len(ids)))))
    centroids = train(points, sample_indices, centroid_count)
    assignments = assign(points, centroids)
    order_position = np.empty(len(ids), dtype=np.int64)
    order_position[order] = np.arange(len(ids), dtype=np.int64)
    postings = []
    for centroid in range(centroid_count):
        members = np.flatnonzero(assignments == centroid)
        postings.append(members[np.argsort(order_position[members], kind="stable")])
    return PrototypeIndex(
        ids=ids,
        points=points,
        hashes=hashes,
        shards=shards,
        order=order,
        shard_rows=shard_rows,
        sample_indices=sample_indices,
        centroids=centroids,
        assignments=assignments,
        postings=postings,
    )


def encode_ids(index: PrototypeIndex, members: np.ndarray) -> bytes:
    ids = index.ids[members].astype("<u8", copy=False)
    return b"GTV2IDS\0" + struct.pack("<I", len(members)) + ids.tobytes()


def encode_payloads(index: PrototypeIndex, members: np.ndarray) -> bytes:
    values = [
        json.dumps(
            {"selectivity_bucket": int(index.ids[member]) % 1000},
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
        for member in members
    ]
    offsets = [0]
    body = bytearray()
    for value in values:
        body.extend(value)
        offsets.append(len(body))
    return (
        b"GTV2PAY\0"
        + struct.pack("<I", len(members))
        + np.asarray(offsets, dtype="<u4").tobytes()
        + bytes(body)
    )


def encode_vectors(index: PrototypeIndex, members: np.ndarray) -> bytes:
    vectors = index.points[members].astype("<f4", copy=False)
    return (
        b"GTV2VEC\0"
        + struct.pack("<II", vectors.shape[1], vectors.shape[0])
        + vectors.tobytes(order="C")
    )


def encode_codebook(index: PrototypeIndex) -> bytes:
    return (
        b"GTV2IVF\0"
        + struct.pack("<II", index.centroids.shape[1], index.centroids.shape[0])
        + index.centroids.astype("<f4", copy=False).tobytes(order="C")
    )


def encode_sample(index: PrototypeIndex) -> bytes:
    identifiers = index.ids[index.sample_indices].astype("<u8", copy=False)
    return b"GTV2SMP\0" + struct.pack("<I", len(identifiers)) + identifiers.tobytes()


def encode_posting(index: PrototypeIndex, members: np.ndarray) -> bytes:
    body = bytearray(b"GTV2PST\0" + struct.pack("<I", len(members)))
    for member in members:
        body.extend(struct.pack("<HH", int(index.shards[member]), int(index.shard_rows[member])))
    return bytes(body)


def write_root(writer: GitWriter, index: PrototypeIndex) -> str:
    ids: dict[str, bytes] = {}
    payloads: dict[str, bytes] = {}
    vectors: dict[str, bytes] = {}
    for shard in np.unique(index.shards[index.order]):
        members = index.order[index.shards[index.order] == shard]
        name = f"{int(shard):03x}.bin"
        ids[name] = encode_ids(index, members)
        payloads[name] = encode_payloads(index, members)
        vectors[f"{int(shard):03x}.f32le"] = encode_vectors(index, members)
    points_tree = writer.tree(
        [
            ("040000", "tree", writer.blob_tree(ids), "ids"),
            ("040000", "tree", writer.blob_tree(payloads), "payloads"),
            ("040000", "tree", writer.blob_tree(vectors), "vectors"),
        ]
    )
    posting_blobs = {
        f"{centroid:04x}.bin": encode_posting(index, members)
        for centroid, members in enumerate(index.postings)
    }
    postings_tree = writer.blob_tree(posting_blobs)
    ivf_tree = writer.tree(
        [
            ("100644", "blob", writer.blob(encode_codebook(index)), "codebook.bin"),
            ("100644", "blob", writer.blob(encode_sample(index)), "sample.bin"),
            ("040000", "tree", postings_tree, "postings"),
        ]
    )
    index_tree = writer.tree([("040000", "tree", ivf_tree, "ivf-flat-v2")])
    meta = json.dumps(
        {
            "candidate_limit": CANDIDATE_LIMIT,
            "centroid_count": len(index.centroids),
            "dimension": index.points.shape[1],
            "distance": "cosine",
            "format_version": 2,
            "point_count": len(index.ids),
            "probes": PROBES,
            "shard_bits": SHARD_BITS,
            "training_iterations": TRAINING_ITERATIONS,
            "training_sample_limit": SAMPLE_LIMIT,
            "vector_codec": "f32le-v2-prototype",
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    return writer.tree(
        [
            ("100644", "blob", writer.blob(meta), "meta.json"),
            ("040000", "tree", points_tree, "points"),
            ("040000", "tree", index_tree, "index"),
        ]
    )


def score(query: np.ndarray, points: np.ndarray) -> np.ndarray:
    query64 = query.astype(np.float64)
    points64 = points.astype(np.float64, copy=False)
    denominator = np.linalg.norm(points64, axis=1) * np.linalg.norm(query64)
    return np.divide(
        points64 @ query64,
        denominator,
        out=np.zeros(points64.shape[0], dtype=np.float64),
        where=denominator != 0,
    )


def rank(query: np.ndarray, index: PrototypeIndex, eligible: np.ndarray | None = None) -> np.ndarray:
    if eligible is None:
        eligible = np.arange(len(index.ids), dtype=np.int64)
    scores = score(query, index.points[eligible])
    return eligible[np.lexsort((index.ids[eligible], -scores))]


def approximate_candidates(query: np.ndarray, index: PrototypeIndex) -> np.ndarray:
    centroid_scores = score(query, index.centroids)
    selected = np.lexsort((np.arange(len(index.centroids)), -centroid_scores))[:PROBES]
    candidates = np.concatenate([index.postings[centroid] for centroid in selected])
    if len(candidates) > CANDIDATE_LIMIT:
        candidates = candidates[:CANDIDATE_LIMIT]
    return candidates


def query_metrics(index: PrototypeIndex, queries: np.ndarray, ks: list[int]) -> dict:
    maximum_k = max(ks)
    exact_us = []
    approximate_us = []
    recall = {str(k): [] for k in ks}
    result_count = {str(k): [] for k in ks}
    exact_results = []
    approximate_results = []
    for query in queries:
        started = time.perf_counter_ns()
        exact = rank(query, index)[:maximum_k]
        exact_us.append((time.perf_counter_ns() - started) // 1000)
        started = time.perf_counter_ns()
        candidates = approximate_candidates(query, index)
        approximate = rank(query, index, candidates)[:maximum_k]
        approximate_us.append((time.perf_counter_ns() - started) // 1000)
        exact_scores = score(query, index.points[exact])
        approximate_scores = score(query, index.points[approximate])
        exact_results.append(
            [
                {"id": int(index.ids[row]), "score": float(value)}
                for row, value in zip(exact, exact_scores, strict=True)
            ]
        )
        approximate_results.append(
            [
                {"id": int(index.ids[row]), "score": float(value)}
                for row, value in zip(approximate, approximate_scores, strict=True)
            ]
        )
        for k in ks:
            wanted = set(index.ids[exact[:k]].tolist())
            found = set(index.ids[approximate[:k]].tolist())
            recall[str(k)].append(len(wanted & found) / max(1, len(wanted)))
            result_count[str(k)].append(len(approximate) >= min(k, len(index.ids)))
    return {
        "exact_query_us": exact_us,
        "approximate_query_us": approximate_us,
        "recall": {key: statistics.fmean(values) for key, values in recall.items()},
        "result_count_ok": {key: all(values) for key, values in result_count.items()},
        "exact_results": exact_results,
        "approximate_results": approximate_results,
    }


def filtered_metrics(index: PrototypeIndex, queries: np.ndarray, ks: list[int], selectivity: float):
    maximum_k = max(ks)
    eligible = np.flatnonzero(index.ids % 1000 < round(selectivity * 1000))
    recalls = {str(k): [] for k in ks}
    counts = {str(k): [] for k in ks}
    for query in queries:
        exact = rank(query, index, eligible)[:maximum_k]
        candidate_set = approximate_candidates(query, index)
        candidate_set = candidate_set[
            index.ids[candidate_set] % 1000 < round(selectivity * 1000)
        ]
        approximate = rank(query, index, candidate_set)[:maximum_k]
        for k in ks:
            expected_count = min(k, len(exact))
            wanted = set(index.ids[exact[:expected_count]].tolist())
            found = set(index.ids[approximate[:expected_count]].tolist())
            recalls[str(k)].append(len(wanted & found) / max(1, expected_count))
            counts[str(k)].append(len(approximate) >= expected_count)
    return {
        "recall": {key: statistics.fmean(values) for key, values in recalls.items()},
        "result_count_ok": {key: all(values) for key, values in counts.items()},
    }


def directory_bytes(path: Path) -> int:
    return sum(entry.stat().st_size for entry in path.rglob("*") if entry.is_file())


def main() -> None:
    args = parse_args()
    spec = json.loads(args.run_spec.read_text())
    if spec["schema_version"] != 1:
        raise ValueError("run spec schema_version must be 1")
    points = read_vectors(Path(spec["points_path"]), spec["point_count"], spec["dimension"])
    queries = read_vectors(Path(spec["queries_path"]), spec["query_count"], spec["dimension"])
    ids = np.arange(len(points), dtype=np.uint64)

    temporary = None
    if args.repository is None:
        temporary = tempfile.TemporaryDirectory(prefix="git-vdb-format2-")
        repository = Path(temporary.name) / "objects.git"
    else:
        repository = args.repository
    writer = GitWriter(repository)

    started = time.perf_counter_ns()
    index = build_index(ids, points)
    index_build_us = (time.perf_counter_ns() - started) // 1000
    write_started = time.perf_counter_ns()
    root = write_root(writer, index)
    write_us = (time.perf_counter_ns() - write_started) // 1000
    build_us = (time.perf_counter_ns() - started) // 1000
    base_blobs = writer.root_blobs(root)
    base_logical_blob_bytes = sum(writer.blob_sizes[oid] for oid in base_blobs)
    base_loose_repository_bytes = directory_bytes(repository)

    reversed_index = build_index(ids[::-1].copy(), points[::-1].copy())
    reversed_root = write_root(writer, reversed_index)
    mutation_count = max(1, round(len(points) * 0.01))
    mutated_points = points.copy()
    mutated_points[:mutation_count, 0] += np.float32(0.001)
    mutation_started = time.perf_counter_ns()
    mutated_index = build_index(ids, mutated_points)
    mutated_root = write_root(writer, mutated_index)
    mutation_full_rebuild_us = (time.perf_counter_ns() - mutation_started) // 1000
    reversed_mutated_index = build_index(ids[::-1].copy(), mutated_points[::-1].copy())
    reversed_mutated_root = write_root(writer, reversed_mutated_index)
    mutated_blobs = writer.root_blobs(mutated_root)
    queries_report = query_metrics(index, queries, spec["k"])
    filtered = {
        str(selectivity): filtered_metrics(index, queries, spec["k"], selectivity)
        for selectivity in spec["filter_selectivities"]
    }
    report = {
        "schema_version": 1,
        "prototype_format_version": 2,
        "root": root,
        "reversed_input_root": reversed_root,
        "reversed_input_root_equal": root == reversed_root,
        "build_us": build_us,
        "index_build_us": index_build_us,
        "git_write_us": write_us,
        "logical_blob_bytes": base_logical_blob_bytes,
        "unique_blobs": len(base_blobs),
        "loose_repository_bytes": base_loose_repository_bytes,
        "repository_bytes_after_determinism_and_mutation_checks": directory_bytes(repository),
        "point_count": len(points),
        "dimension": points.shape[1],
        "centroid_count": len(index.centroids),
        "nonempty_shards": int(len(np.unique(index.shards))),
        "training_sample_count": len(index.sample_indices),
        "mutation_1_percent": {
            "points": mutation_count,
            "full_rebuild_us": mutation_full_rebuild_us,
            "root": mutated_root,
            "reversed_input_root": reversed_mutated_root,
            "reversed_input_root_equal": mutated_root == reversed_mutated_root,
            "shared_blobs": len(base_blobs & mutated_blobs),
            "base_blobs": len(base_blobs),
            "mutated_blobs": len(mutated_blobs),
            "shared_logical_blob_bytes": sum(
                writer.blob_sizes[oid] for oid in base_blobs & mutated_blobs
            ),
            "base_logical_blob_bytes": sum(writer.blob_sizes[oid] for oid in base_blobs),
            "mutated_logical_blob_bytes": sum(writer.blob_sizes[oid] for oid in mutated_blobs),
        },
        "queries": queries_report,
        "filtered": filtered,
        "limitations": [
            "prototype uses NumPy floating-point reductions; cross-platform root equality is a required external gate",
            "mutation currently measures a canonical full rebuild; incremental changed-shard construction is not implemented",
            "packing, transfer, concurrency, and phase RSS are not yet measured by this command",
            "prototype point codec currently supports the benchmark uint64 ID domain",
        ],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    if temporary is not None:
        temporary.cleanup()


if __name__ == "__main__":
    # Prevent implicit multi-threaded BLAS from making timing and reduction order
    # depend on the host's default thread pool.
    os.environ.setdefault("OMP_NUM_THREADS", "1")
    os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
    os.environ.setdefault("MKL_NUM_THREADS", "1")
    main()
