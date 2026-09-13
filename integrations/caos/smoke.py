#!/usr/bin/env python3
"""Real Caos cache/provenance checks. Run only in a clean, disposable clone."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import subprocess
import time

SCOPE = "tests/fixtures/caos-source"
QUESTION = "How are requests retried after temporary network failures?"

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("caos", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--runner-log", required=True, type=Path,
                        help="Runner log belonging to this isolated Caos stack")
    args = parser.parse_args()
    cli = str(args.caos.resolve())
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    def git(*arguments):
        return subprocess.check_output(["git", *arguments])
    root = Path(git("rev-parse", "--show-toplevel").decode().strip())
    assert Path.cwd() == root, "run from the disposable clone root"
    assert not git("status", "--porcelain").strip(), "use a clean disposable clone"
    assert not output.is_relative_to(root), "keep output outside the source tree"
    assert args.runner_log.is_file(), "runner log must belong to the tested stack"
    def command(*arguments):
        return subprocess.run([cli, *arguments], capture_output=True, text=True, check=True)
    def tree_id(text):
        match = re.search(r"^tree ([0-9a-f]{40})$", text, re.M)
        assert match, text
        return match[1]
    def walk(node):
        if not node:
            return
        yield node
        for child in node.get("children", []):
            yield from walk(child)
    records = []
    def run(name, *, question=QUESTION, scope=SCOPE, source=None, salt=None, chunk_chars=None, direct=False):
        tool = tree_id(command("eval-path", "caos-tools/semantic-search").stdout)
        kvs = ["--in:@=." if source is None else "--in:hash="+source,
               "--query="+question, "--path="+scope, "--limit=100"]
        if salt is not None:
            kvs.append("--run-salt="+salt)
        if chunk_chars is not None:
            kvs.append("--chunk-chars="+str(chunk_chars))
        request = command("prepare-request", "--base:hash="+tool, *kvs).stdout.strip()
        offset = args.runner_log.stat().st_size
        started = time.monotonic()
        result = command("run", "--base:hash="+tool, *kvs) if direct else command("run-tool", "semantic-search", *kvs)
        elapsed = time.monotonic()-started
        result_id = tree_id(result.stdout)
        trace = json.loads(command("status", "--all", request).stdout)
        with args.runner_log.open("rb") as log:
            log.seek(offset)
            dispatched = set(re.findall(rb"arg_tree ([0-9a-f]{40}) -> container", log.read()))
        executed = Counter()
        stages = []
        seen = set()
        for node in walk(trace):
            if node["arg_tree"] in seen:
                continue
            seen.add(node["arg_tree"])
            did_run = node["arg_tree"].encode() in dispatched
            if did_run:
                executed[node["name"]] += 1
            stages.append({**node, "children": [], "executed_in_call":did_run})
        destination = output/name
        command("get", result_id, str(destination))
        value = json.loads((destination/"results.json").read_text())
        assert (destination/"snapshot/index/meta.json").is_file()
        for hit in value["hits"]:
            payload = hit["payload"]
            location = value["source_tree"]+":"+payload["path"]
            blob = git("rev-parse", location).decode().strip()
            assert blob == payload["blob"], (name, "blob provenance mismatch")
            text = git("show", location).decode()
            excerpt = "".join(text.splitlines(keepends=True)[payload["start_line"]-1:payload["end_line"]])
            assert payload["document"] in excerpt, (name, "line provenance mismatch")
        record = {"name":name, "seconds":elapsed, "request":request, "result":result_id,
                  "document_jobs":executed["embed"], "query_embedding_jobs":executed["embed-query"],
                  "executed":dict(executed), "source_tree":value["source_tree"],
                  "index_root":value["index_root"], "model":value["model"], "reads":value["reads"],
                  "paths":[hit["payload"]["path"] for hit in value["hits"]], "stages":stages}
        (output/(name+"-trace.json")).write_text(json.dumps(trace, indent=2)+"\n")
        (output/(name+"-command.log")).write_text(result.stderr)
        records.append(record)
        (output/"runs.json").write_text(json.dumps(records, indent=2)+"\n")
        print(json.dumps({key:record[key] for key in ["name","seconds","document_jobs","query_embedding_jobs","result"]}), flush=True)
        return record, value

    fixture = root/SCOPE
    model_path = root/"integrations/caos/model.lock.json"
    originals = {path: (path.is_symlink(), path.readlink() if path.is_symlink() else path.read_bytes())
                 for path in fixture.iterdir()}
    model_bytes = model_path.read_bytes()
    def stage():
        subprocess.run(["git", "add", "--all", SCOPE, str(model_path)], check=True)
    try:
        baseline, value = run("baseline")
        assert baseline["document_jobs"] == 4, baseline # includes one empty-file job
        assert len(value["hits"]) == 4
        assert sum(path.endswith(("retry.py", "retry-copy.py")) for path in baseline["paths"]) == 2
        repeated, _ = run("repeat")
        assert repeated["result"] == baseline["result"] and repeated["document_jobs"] == 0
        agent, _ = run("agent-dispatch", direct=True)
        assert agent["result"] == baseline["result"] and not agent["executed"]
        new_query, _ = run("new-query", question="How are historical snapshots retained?")
        assert new_query["document_jobs"] == 0 and new_query["query_embedding_jobs"] == 1
        for repetition in range(5):
            fresh, _ = run("fresh-"+str(repetition), salt="smoke-"+str(repetition)+"-"+str(time.time_ns()))
            assert fresh["document_jobs"] == 4
            assert fresh["result"] == baseline["result"], "salt changed semantic output"
        (fixture/"retry.py").write_text((fixture/"retry.py").read_text()+"\n# An added retry diagnostic records the final failure.\n")
        stage()
        edited, _ = run("edit")
        assert edited["document_jobs"] == 1
        (fixture/"history.rs").rename(fixture/"past.rs")
        stage()
        renamed, _ = run("rename")
        assert renamed["document_jobs"] == 0 and all(not path.endswith("/history.rs") for path in renamed["paths"])
        (fixture/"retry-copy.py").unlink()
        stage()
        deleted, _ = run("delete")
        assert deleted["document_jobs"] == 0 and all(not path.endswith("/retry-copy.py") for path in deleted["paths"])
        historical, _ = run("historical", source=baseline["source_tree"])
        assert historical["result"] == baseline["result"]
        chunked, _ = run("chunk-change", chunk_chars=64)
        assert chunked["document_jobs"] == 4 and chunked["model"] == baseline["model"]
        model = json.loads(model_bytes)
        model["max_tokens"] = 128
        model_path.write_text(json.dumps(model, indent=2)+"\n")
        stage()
        changed, _ = run("model-change")
        assert changed["document_jobs"] == 4 and changed["model"] != baseline["model"]
    finally:
        for path in fixture.iterdir():
            path.unlink()
        for path, (is_link, content) in originals.items():
            if is_link:
                path.symlink_to(content)
            else:
                path.write_bytes(content)
        model_path.write_bytes(model_bytes)
        stage()
        assert not git("status", "--porcelain").strip(), "fixture restoration left changes"
    summary = {"git_vdb":git("rev-parse", "HEAD").decode().strip(),
               "caos":"8e44b8f51d97155c2287f47e6ae57eb4d4a23e20",
               "runs":[{key:value for key,value in record.items() if key != "stages"} for record in records],
               "raw_sha256":hashlib.sha256((output/"runs.json").read_bytes()).hexdigest(),
               "note":"Document jobs include the empty-file job; it does not call the model. Fresh runs use a unique propagated salt. Agent dispatch uses the identical resolved ArgTree without an LLM provider."}
    (output/"summary.json").write_text(json.dumps(summary, indent=2)+"\n")

if __name__ == "__main__":
    main()
