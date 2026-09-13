#!/usr/bin/env python3
"""Profile the actual packaged provider command without changing its source."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import resource
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("embed_command")
parser.add_argument("output", type=Path)
args = parser.parse_args()
description = json.loads(subprocess.check_output([args.embed_command, "--describe"]))
texts = ["How are network requests retried after temporary failures?",
         "Retry failed HTTP requests with exponential backoff and jitter.",
         "Bake a chocolate cake in the oven."]
runs = []
for _ in range(5):
    started = time.perf_counter_ns()
    data = subprocess.check_output([args.embed_command], input=json.dumps(texts).encode())
    elapsed = time.perf_counter_ns()-started
    vectors = json.loads(data)
    assert all(len(vector) == 384 for vector in vectors)
    similarities = [sum(x*y for x,y in zip(vectors[0], vector)) for vector in vectors[1:]]
    assert similarities[0] > similarities[1]+0.2
    runs.append({"elapsed_ns":elapsed, "sha256":hashlib.sha256(data).hexdigest(),
                 "similarities":similarities})
assert len({run["sha256"] for run in runs}) == 1
peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
if platform.system() == "Darwin":
    peak /= 1024
value = {"provider":description, "texts":texts, "runs":runs, "peak_child_rss_kib":peak,
         "note":"Elapsed time includes provider process startup, imports, artifact verification, tokenization, model initialization, inference and serialization. Peak RSS is the maximum across child processes."}
args.output.write_text(json.dumps(value, indent=2)+"\n")
print(json.dumps(value))
