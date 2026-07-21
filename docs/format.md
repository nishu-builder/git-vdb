# git-vdb format version 1

This document is normative for roots whose `meta.json.format_version` is `1`.
All path names are UTF-8, all blobs use Git mode `100644`, and all child trees
use mode `040000`. Git's tree encoder supplies bytewise name ordering. Empty
directories are represented by the standard empty Git tree object, so an empty
collection still contains `points`, `index/lsh-v1`, and `meta.json`.

## Root

```text
meta.json
points/<first-2-sha256-hex>/<sha256-hex>/
  id.json
  payload.json
  vector.f32le
index/lsh-v1/<table-4hex>/<first-2-signature-hex>/<signature-hex>/<id-hash>
```

The final index entry is mode `040000` and its object ID is the exact same point
tree used below `points`; it is not a copied record.

`meta.json`, `id.json`, and `payload.json` are compact UTF-8 JSON with object keys
sorted lexicographically, no insignificant whitespace, and the serialization
provided by `serde_json` without its `preserve_order` feature. Non-finite JSON
numbers are rejected. Readers reject non-canonical metadata and payload bytes.

`id.json` is exactly one of:

```json
{"type":"string","value":"the original UTF-8 string"}
{"type":"uint","value":42}
```

The storage hash is lowercase SHA-256 hex over a typed byte sequence. A string
hash input is byte `0x73`, byte `0x00`, then the unmodified UTF-8 bytes. An
unsigned integer hash input is byte `0x75`, byte `0x00`, then its eight-byte
big-endian value. Thus integer `42` and string `"42"` cannot collide by type.

`meta.json` contains these complete fields:

```json
{"dimension":2,"distance":"cosine","format_version":1,"git_object_format":"sha1","index":{"default_candidate_limit":10000,"default_probes":96,"full_scan_threshold":1000,"projection_seed":7451614797069836849,"signature_bits":12,"tables":12},"point_count":0,"vector_codec":"f32le-v1","vector_space":null}
```

The example is illustrative but canonical for that configuration. Version 1
uses SHA-1 Git repositories because that is the object format supported by the
embedded libgit2 dependency.

## Vector bytes

`vector.f32le` is:

1. eight ASCII bytes `GTVDBV01`;
2. dimension as an unsigned 32-bit little-endian integer;
3. exactly `dimension` IEEE-754 binary32 bit patterns, each little-endian.

All components must be finite. Signed zero and other finite bit patterns are
preserved. The hand-authored two-vector fixture `[1.0, -2.5]` is:

```text
4754564442563031020000000000803f000020c0
```

## LSH

Version 1 defaults are 12 tables, 12 bits per signature, projection seed
`0x6769742d76646231`, full-scan threshold 1,000, 96 total probes, and 10,000
unique candidates. Every value is stored in `meta.json`; environment variables
cannot alter it.

For `(seed, table, bit, dimension-index)`, hash the concatenation of:

```text
"git-vdb/lsh-v1/projection\0"
seed as u64 little-endian
table as u64 little-endian
bit as u64 little-endian
dimension-index as u64 little-endian
```

with SHA-256. An even low bit in the first digest byte selects coefficient
`-1.0`; an odd low bit selects `+1.0`. Accumulate the dot product in `f64` in
ascending dimension order. A nonnegative result sets the signature bit. This
is a deterministic Rademacher random-hyperplane family and avoids platform
dependent PRNG and Gaussian implementations.

Signatures are lowercase, zero-padded hexadecimal with `ceil(bits/4)` digits.
Tables use four lowercase hexadecimal digits. Signature fanout uses the first
two signature digits (or all digits for a one-digit signature).

Queries first generate exact-signature probes for each table. They then visit
Hamming distance 1, 2, and so on. At a distance, tables ascend first; within a
table, flip masks are combinations of bit positions in lexicographic
combination order, with bit 0 first. The configured/requested probe count bounds
the total across all tables. Candidate point hashes are deduplicated and
accepted in bucket tree order until the unique candidate limit. Candidates are
scored with exact cosine similarity and sorted by descending score, then typed
canonical ID bytes ascending.

## Materialized snapshots

A root can be written as an ordinary directory containing the exact tree above.
Importing the directory uses regular files as mode `100644` blobs and directories
as mode `040000` trees, so its computed tree ID is identical to the original
root. The directory contains no `.git` metadata. Index paths materialize their
referenced point trees as files, so filesystem exports do not preserve Git's
object-level deduplication.

## Named-collection adapter

`refs/git-vdb/collections/<name>` points to a commit whose tree is the current
root. A mutation creates a parented commit and advances the ref with libgit2's
reference-matching transaction against the previously observed commit. Root
trees are deterministic. Commit author, committer, timestamp, message, and
object ID are deliberately not part of database identity.

Readers accept either a commit or root tree object ID. A commit resolves to its
tree. The immutable snapshot API is stricter: it accepts only the full object ID
of a tree and never resolves commits or refs. Format meaning is determined solely
by the root metadata version.
