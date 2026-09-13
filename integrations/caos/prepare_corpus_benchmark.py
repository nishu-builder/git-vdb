#!/usr/bin/env python3
"""Write the predeclared four workloads for the pinned Caos source corpus."""
import argparse
import copy
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("snapshot", type=Path, help="Materialized result/snapshot directory")
parser.add_argument("query_vector", type=Path)
args = parser.parse_args()
manifest = json.loads((args.snapshot/"manifest.json").read_text())
assert manifest["source_tree"] == "67fe4d4d391d57ecaac07c4023621f45d6f6bc79"
query = {"vector":json.loads(args.query_vector.read_text()), "limit":10,
         "filter":None, "with_payload":True, "with_vector":False,
         "expected_vector_space":manifest["model"],
         "params":{"exact":False,"probes":1,"candidate_limit":32}}
queries = {"selective":copy.deepcopy(query)}
query["params"] = {"exact":False,"probes":64,"candidate_limit":4096}
queries["broad"] = copy.deepcopy(query)
query["filter"] = {"must":[{"key":"path","match":{"value":"README.md"}}]}
queries["filtered"] = copy.deepcopy(query)
query["filter"] = None
query["params"] = {"exact":True,"probes":0,"candidate_limit":0}
queries["exact"] = query
(args.snapshot/"queries.json").write_text(json.dumps(queries,sort_keys=True,separators=(",",":")))
