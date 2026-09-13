#!/usr/bin/env python3
"""Pinned, local-only MiniLM inference. stdin: JSON strings; stdout: vectors."""
import hashlib
import json
import os
import platform
from pathlib import Path
import sys

# This program never resolves a remote model or imports a hub client.
import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer
import tokenizers

MODEL = Path(os.environ.get("GIT_VDB_MODEL_DIR", "/model"))
LOCK = json.loads((MODEL / "model.lock.json").read_text())
SETTINGS = {
    "model": LOCK,
    "adapter_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    "onnxruntime": ort.__version__,
    "tokenizers": tokenizers.__version__,
    "numpy": np.__version__,
    "python": platform.python_version(),
    "machine": platform.machine(),
    "graph_optimization": "ORT_ENABLE_EXTENDED",
    "execution": "ORT_SEQUENTIAL",
    "document_prefix": "",
    "query_prefix": "",
    "output_dtype": "float32",
}
IDENTITY = "minilm/" + hashlib.sha256(
    json.dumps(SETTINGS, sort_keys=True, separators=(",", ":")).encode()
).hexdigest()

if sys.argv[1:] == ["--identity"]:
    print(IDENTITY)
    sys.exit(0)

for name, spec in LOCK["files"].items():
    if hashlib.sha256((MODEL / name).read_bytes()).hexdigest() != spec["sha256"]:
        raise ValueError(f"model artifact checksum mismatch: {name}")

texts = json.load(sys.stdin)
if not isinstance(texts, list) or len(texts) > 32 or any(
    not isinstance(text, str) for text in texts
):
    raise ValueError("expected a batch of at most 32 strings")
if not texts:
    print("[]")
    sys.exit(0)

tokenizer = Tokenizer.from_file(str(MODEL / "tokenizer.json"))
tokenizer.enable_truncation(max_length=LOCK["max_tokens"])
tokenizer.enable_padding(pad_id=0, pad_token="[PAD]")
encodings = tokenizer.encode_batch(texts)
inputs = {
    "input_ids": np.asarray([item.ids for item in encodings], dtype=np.int64),
    "attention_mask": np.asarray([item.attention_mask for item in encodings], dtype=np.int64),
    "token_type_ids": np.asarray([item.type_ids for item in encodings], dtype=np.int64),
}
options = ort.SessionOptions()
options.intra_op_num_threads = LOCK["threads"]
options.inter_op_num_threads = 1
options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_EXTENDED
session = ort.InferenceSession(str(MODEL / "model.onnx"), sess_options=options,
                               providers=[LOCK["provider"]])
accepted = {item.name for item in session.get_inputs()}
hidden = session.run(None, {name: value for name, value in inputs.items()
                            if name in accepted})[0]
mask = inputs["attention_mask"][:, :, None].astype(np.float32)
pooled = (hidden * mask).sum(axis=1) / np.maximum(mask.sum(axis=1), 1e-9)
vectors = pooled / np.maximum(np.linalg.norm(pooled, axis=1, keepdims=True), 1e-12)
vectors = vectors.astype(np.float32)
if vectors.shape != (len(texts), LOCK["dimension"]) or not np.isfinite(vectors).all():
    raise ValueError("model produced invalid embeddings")
json.dump(vectors.tolist(), sys.stdout, allow_nan=False, separators=(",", ":"))
sys.stdout.write("\n")
