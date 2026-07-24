"""Small Python client for the git-vdb JSON CLI."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
from typing import Any, Iterable, Mapping, Sequence

__all__ = ["Collection", "GitVdbError", "PersistentClient"]


class GitVdbError(RuntimeError):
    """A failed git-vdb operation."""


class PersistentClient:
    """A persistent local git-vdb client."""

    def __init__(self, path: str | os.PathLike[str], executable: str | None = None):
        self.path = Path(path)
        self.executable = executable or os.environ.get("GIT_VDB_BIN") or shutil.which("git-vdb")
        if not self.executable:
            raise GitVdbError(
                "git-vdb executable not found; install it or set GIT_VDB_BIN"
            )

    def get_or_create_collection(self, name: str) -> "Collection":
        return Collection(self, name)

    def get_collection(self, name: str) -> "Collection":
        collection = Collection(self, name)
        collection.count()
        return collection

    def list_collections(self) -> list[dict[str, Any]]:
        return self._run("collection", "list")["collections"]

    def delete_collection(self, name: str) -> None:
        self._run("collection", "delete", name)

    def doctor(self) -> dict[str, Any]:
        return self._run("doctor")

    def _run(self, *arguments: str, stdin: str | None = None) -> Any:
        command = [str(self.executable), "--db", str(self.path), *arguments]
        completed = subprocess.run(
            command,
            input=stdin,
            text=True,
            capture_output=True,
            check=False,
        )
        if completed.returncode:
            message = completed.stderr.strip() or completed.stdout.strip()
            raise GitVdbError(message or f"git-vdb exited with {completed.returncode}")
        output = completed.stdout.strip()
        return json.loads(output) if output else None


class Collection:
    """A named collection with Chroma-compatible operation names."""

    def __init__(self, client: PersistentClient, name: str):
        self._client = client
        self.name = name

    def upsert(
        self,
        *,
        ids: Sequence[str | int],
        embeddings: Sequence[Sequence[float]],
        metadatas: Sequence[Mapping[str, Any] | None] | None = None,
        documents: Sequence[str | None] | None = None,
        batch_size: int = 1000,
    ) -> None:
        length = len(ids)
        if len(embeddings) != length:
            raise ValueError("ids and embeddings must have the same length")
        if metadatas is not None and len(metadatas) != length:
            raise ValueError("ids and metadatas must have the same length")
        if documents is not None and len(documents) != length:
            raise ValueError("ids and documents must have the same length")
        rows = []
        for index, identifier in enumerate(ids):
            payload = dict(metadatas[index] or {}) if metadatas is not None else {}
            if documents is not None and documents[index] is not None:
                payload["document"] = documents[index]
            rows.append(
                json.dumps(
                    {
                        "id": identifier,
                        "vector": list(embeddings[index]),
                        "payload": payload,
                    },
                    separators=(",", ":"),
                )
            )
        self._client._run(
            "upsert",
            self.name,
            "-",
            "--batch-size",
            str(batch_size),
            stdin="\n".join(rows) + ("\n" if rows else ""),
        )

    def query(
        self,
        *,
        query_embeddings: Sequence[Sequence[float]] | None = None,
        query_texts: Sequence[str] | None = None,
        n_results: int = 10,
        where: Mapping[str, Any] | None = None,
        where_document: Mapping[str, Any] | None = None,
    ) -> dict[str, list[list[Any]]]:
        if (query_embeddings is None) == (query_texts is None):
            raise ValueError("supply exactly one of query_embeddings or query_texts")
        filter_value = _where_filter(where, where_document)
        batches = []
        values: Iterable[Sequence[float] | str] = query_embeddings or query_texts or []
        for value in values:
            arguments = ["search", self.name, "--limit", str(n_results), "--with-payload"]
            if isinstance(value, str):
                arguments.extend(["--text", value])
            else:
                arguments.extend(["--vector", json.dumps(list(value))])
            if filter_value is not None:
                arguments.extend(["--filter", json.dumps(filter_value, separators=(",", ":"))])
            result = self._client._run(*arguments)
            points = result if isinstance(result, list) else result["points"]
            batches.append(points)
        return {
            "ids": [[point["id"] for point in batch] for batch in batches],
            "documents": [
                [
                    point.get("document", point.get("payload", {}).get("document"))
                    for point in batch
                ]
                for batch in batches
            ],
            "metadatas": [
                [
                    point.get("metadata", _metadata_without_document(point.get("payload")))
                    for point in batch
                ]
                for batch in batches
            ],
            "distances": [
                [1.0 - float(point["score"]) for point in batch] for batch in batches
            ],
        }

    def get(
        self,
        *,
        ids: Sequence[str | int] | None = None,
        where: Mapping[str, Any] | None = None,
        limit: int | None = None,
        offset: int = 0,
    ) -> dict[str, list[Any]]:
        arguments = ["get", self.name, "--with-payload", "--offset", str(offset)]
        if ids:
            arguments.extend(["--ids", *(json.dumps(value) for value in ids)])
        if where:
            arguments.extend(["--filter", json.dumps(_where_filter(where, None))])
        if limit is not None:
            arguments.extend(["--limit", str(limit)])
        points = self._client._run(*arguments)["points"]
        return {
            "ids": [point["id"] for point in points],
            "documents": [point.get("payload", {}).get("document") for point in points],
            "metadatas": [_metadata_without_document(point.get("payload")) for point in points],
        }

    def count(self) -> int:
        return int(self._client._run("count", self.name)["count"])

    def delete(
        self,
        *,
        ids: Sequence[str | int] | None = None,
        where: Mapping[str, Any] | None = None,
    ) -> None:
        if bool(ids) == bool(where):
            raise ValueError("supply exactly one of ids or where")
        arguments = ["delete", self.name]
        if ids:
            arguments.extend(["--ids", *(json.dumps(value) for value in ids)])
        else:
            arguments.extend(["--filter", json.dumps(_where_filter(where, None))])
        self._client._run(*arguments)


def _metadata_without_document(payload: Mapping[str, Any] | None) -> dict[str, Any] | None:
    if payload is None:
        return None
    metadata = dict(payload)
    metadata.pop("document", None)
    return metadata


def _where_filter(
    where: Mapping[str, Any] | None,
    where_document: Mapping[str, Any] | None,
) -> dict[str, Any] | None:
    must = _conditions(where) if where else []
    if where_document:
        if "$contains" in where_document:
            must.append({"document_contains": str(where_document["$contains"])})
        elif "$regex" in where_document:
            must.append({"document_regex": str(where_document["$regex"])})
        else:
            raise ValueError("where_document supports $contains or $regex")
    return {"must": must} if must else None


def _conditions(where: Mapping[str, Any]) -> list[dict[str, Any]]:
    conditions: list[dict[str, Any]] = []
    for key, value in where.items():
        if key == "$and":
            for nested in value:
                conditions.extend(_conditions(nested))
            continue
        if key == "$or":
            conditions.append({"should": _conditions_list(value)})
            continue
        if not isinstance(value, Mapping):
            conditions.append({"key": key, "match": {"value": value}})
            continue
        if "$eq" in value:
            conditions.append({"key": key, "match": {"value": value["$eq"]}})
        elif "$in" in value:
            conditions.append({"key": key, "any": list(value["$in"])})
        elif "$nin" in value:
            conditions.append({"key": key, "none": list(value["$nin"])})
        elif "$contains" in value:
            conditions.append({"key": key, "contains": value["$contains"]})
        else:
            bounds = {
                operator[1:]: operand
                for operator, operand in value.items()
                if operator in {"$gt", "$gte", "$lt", "$lte"}
            }
            if not bounds:
                raise ValueError(f"unsupported where operator for {key!r}")
            conditions.append({"key": key, "range": bounds})
    return conditions


def _conditions_list(values: Iterable[Mapping[str, Any]]) -> list[dict[str, Any]]:
    return [condition for value in values for condition in _conditions(value)]
