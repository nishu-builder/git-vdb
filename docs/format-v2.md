# git-vdb format version 2

This document is normative for roots whose `meta.json.format_version` is `2`.
All multibyte binary integers are little-endian. Git entries use mode `100644`
for blobs and `040000` for trees. Git's tree encoding supplies bytewise path
ordering. Readers reject missing, extra, malformed, or non-canonical content.

## Root tree

```text
meta.json
points/
  ids/<shard-3hex>.bin
  payloads/<shard-3hex>.bin
  vectors/<shard-3hex>.f32le
index/ivf-flat-v2/
  codebook.bin
  sample.bin
  postings/<centroid-4hex>.bin
```

Empty point shards do not exist. The three point trees contain identical shard
sets. The postings tree contains one blob for every centroid, including empty
postings. An empty collection has empty point and postings trees, a zero-row
sample, and a zero-centroid codebook.

## Metadata

`meta.json` is compact UTF-8 JSON with object keys sorted lexicographically, no
insignificant whitespace, and finite numbers. It records:

- `format_version`: `2`;
- `point_count` and positive `dimension`;
- `distance`: `"cosine"`;
- nullable application-defined `vector_space`;
- `vector_codec`: `"f32le-sharded-v2"`;
- `git_object_format`: `"sha1"`;
- `index`: approximate query defaults; its LSH construction fields are retained
  for format-version-1 compatibility and do not affect v2 IVF construction;
- `ivf`: `shard_bits`, `centroid_count`, `training_sample_limit`, and
  `training_iterations`.

The canonical v2 construction constants are 6 shard bits, an 8,192-point
training-sample limit, four Lloyd iterations, and at most 4,096 centroids.
For example, an empty two-dimensional collection with default query settings
has these exact metadata bytes:

```json
{"dimension":2,"distance":"cosine","format_version":2,"git_object_format":"sha1","index":{"default_candidate_limit":10000,"default_probes":96,"full_scan_threshold":1000,"projection_seed":7451614797069836849,"signature_bits":12,"tables":12},"ivf":{"centroid_count":0,"shard_bits":6,"training_iterations":4,"training_sample_limit":8192},"point_count":0,"vector_codec":"f32le-sharded-v2","vector_space":null}
```

## Typed IDs, sharding, and row order

The canonical bytes of a string ID are `s`, NUL, then its unmodified UTF-8. The
canonical bytes of an unsigned ID are `u`, NUL, then its u64 big-endian value.
Typed IDs are distinct. Their SHA-256 digest determines both training order and
placement. A point's shard is the high six bits of the digest's first byte.

Rows within each shard ascend by complete canonical ID bytes. Consequently all
string IDs precede all unsigned IDs, strings use bytewise UTF-8 order, and
unsigned IDs use numeric order. The `(shard, row)` pair is the stable reference
stored by the index.

## Point shard blobs

ID and payload blobs have this common envelope:

```text
magic[8] | row_count:u32 | offsets[row_count + 1]:u32 | body[...]
```

The first offset is zero, offsets are nondecreasing, and the last offset equals
the body length. No trailing bytes are allowed. ID magic is `GTV2IDS\0`; each ID
row is either `0 | utf8_len:u32 | utf8` or `1 | value:u64`. Payload magic is
`GTV2PAY\0`; each row is the compact, key-sorted canonical JSON object.

Vector blobs are:

```text
"GTV2VEC\0" | dimension:u32 | row_count:u32 |
row-major finite f32 IEEE-754 bit patterns
```

The dimension equals metadata, and the three blobs for a shard have identical
nonzero row counts.

## Deterministic IVF-flat index

The training sample contains the `min(point_count, 8192)` points with the lowest
typed-ID SHA-256 digests, ordered by digest and then canonical ID. `sample.bin`
is `GTV2SMP\0`, a u32 row count, then for each row: canonical-ID byte length as
u32, canonical-ID bytes, and SHA-256 of the row's concatenated little-endian f32
vector bit patterns.

For a nonempty collection, first round `sqrt(point_count)` to the nearest
integer, with ties upward. Clamp that value to `[1, 4096]`, then choose its
nearest power of two, again with ties upward. This is `centroid_count`.

Initial centroid `i` is an actual sample vector at position
`i * (sample_count - 1) / (centroid_count - 1)` using integer division; the
single-centroid position is zero. Four Lloyd iterations follow. Each sample is
assigned to the centroid with greatest f64 cosine score, ties going to the
lowest centroid number. Component sums are f64 in sample order and nonempty
centroid means are rounded to f32. Empty centroids retain their prior vector.

`codebook.bin` is:

```text
"GTV2IVF\0" | dimension:u32 | centroid_count:u32 |
row-major finite f32 centroid bit patterns
```

Every point is assigned by the same f64 cosine and tie rule. Posting
`<centroid-4hex>.bin` is `GTV2PST\0`, a u32 count, then ascending `(shard:u16,
row:u32)` pairs. Every point appears in exactly one posting.

Cosine is zero when either vector has zero norm. Otherwise it is the f64 dot
product divided by the product of f64 norms, accumulated in component order.

## Query semantics

Exact search scores every filter-eligible point. Approximate search ranks all
centroids by descending cosine and centroid number, then visits the requested
prefix. Zero probes means the metadata default for unfiltered queries and all
centroids for filtered queries. Zero candidate limit means the metadata
default. The candidate limit counts filter-eligible vectors actually scored.
Winners sort by descending f64 score and canonical typed ID, then expose the
score rounded to f32.

Query caches are derived accelerators keyed by the immutable root and are not
part of persisted identity. Full validation ignores caches and recomputes the
sample, centroids, assignments, and postings from authoritative point shards.

## Mutations, history, and compatibility

Construction is a pure function of the final configuration and point set.
Incremental mutations rewrite affected point shards and changed index blobs,
but their resulting root must equal a clean build byte for byte. Named writes
create objects before atomically compare-and-swapping the collection ref;
historical roots remain immutable.

New collections and snapshots emit format version 2. Readers dispatch solely
on `meta.json.format_version`; existing format-version-1 roots remain readable,
validatable, and mutable without changing their canonical bytes. The separate
format-version-1 specification remains normative for those roots and is exposed
in crate documentation as [`crate::format_v1`].
