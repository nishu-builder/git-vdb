//! Selective reads of canonical format-2 snapshots from external object stores.
//!
//! Sources must pin immutable content to the supplied root. The reader validates
//! objects it visits, but does not authenticate a directory or validate unvisited
//! objects. Use `Snapshot::validate` for full canonical validation.
use crate::filter::matches_filter;
use crate::root::{decode_meta, validate_config, validate_query, RootMeta};
use crate::root_v2::{
    cosine_f64, decode_codebook, decode_ids, decode_payloads, decode_posting, decode_vectors,
    validate_ivf_meta,
};
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

/// Reads objects relative to one immutable canonical snapshot root.
///
/// Implementations must return stable bytes and immediate entry names for the
/// lifetime of a reader, and bind them to the root supplied to `SnapshotReader::open`.
/// Network stores can fetch one blob or tree per call instead of materializing
/// the complete snapshot. Paths use forward slashes and never traverse parents.
pub trait SnapshotSource {
    /// Read one complete blob at a root-relative path.
    fn read_blob(&self, path: &str) -> Result<Vec<u8>>;
    /// List immediate entry names in a root-relative directory.
    fn list(&self, path: &str) -> Result<Vec<String>>;
}

/// A materialized directory source. The caller must keep its contents immutable.
///
/// This source rejects symlinks in accessed paths. It does not recompute the Git
/// root; use `Snapshot::open_directory` when importing untrusted directories.
#[derive(Clone, Debug)]
pub struct DirectorySource {
    directory: PathBuf,
}
impl DirectorySource {
    /// Open a directory containing a materialized snapshot.
    pub fn new(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref().canonicalize()?;
        if !directory.is_dir() {
            return Err(Error::Invalid("snapshot source is not a directory".into()));
        }
        Ok(Self { directory })
    }
    fn path(&self, relative: &str) -> Result<PathBuf> {
        let mut path = self.directory.clone();
        for component in Path::new(relative).components() {
            let Component::Normal(name) = component else {
                return Err(Error::Invalid("invalid snapshot object path".into()));
            };
            path.push(name);
            if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
                return Err(Error::Corrupt(
                    "snapshot object path contains a symlink".into(),
                ));
            }
        }
        Ok(path)
    }
}
impl SnapshotSource for DirectorySource {
    fn read_blob(&self, path: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.path(path)?)?)
    }
    fn list(&self, path: &str) -> Result<Vec<String>> {
        std::fs::read_dir(self.path(path)?)?
            .map(|entry| {
                entry?
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::Corrupt("non-UTF8 snapshot entry".into()))
            })
            .collect()
    }
}

/// A source reading a pinned tree directly from a local Git object database.
///
/// No refs are read or changed. Git verifies object identities when loading
/// objects; callers must pass the same root to `SnapshotReader::open`.
pub struct GitSource {
    repository: git2::Repository,
    root: git2::Oid,
}
impl GitSource {
    /// Open an existing repository and pin the supplied tree object.
    pub fn new(repository: impl AsRef<Path>, root: &ObjectId) -> Result<Self> {
        let repository = git2::Repository::open(repository)?;
        let root = git2::Oid::from_str(root.as_ref())?;
        repository.find_tree(root)?;
        Ok(Self { repository, root })
    }
    fn object(&self, path: &str) -> Result<git2::Object<'_>> {
        let entry = self
            .repository
            .find_tree(self.root)?
            .get_path(Path::new(path))?;
        Ok(entry.to_object(&self.repository)?)
    }
}
impl SnapshotSource for GitSource {
    fn read_blob(&self, path: &str) -> Result<Vec<u8>> {
        let object = self.object(path)?;
        let blob = object
            .as_blob()
            .ok_or_else(|| Error::Corrupt("expected snapshot blob".into()))?;
        Ok(blob.content().to_vec())
    }
    fn list(&self, path: &str) -> Result<Vec<String>> {
        let object = self.object(path)?;
        let tree = object
            .as_tree()
            .ok_or_else(|| Error::Corrupt("expected snapshot tree".into()))?;
        tree.iter()
            .map(|entry| {
                entry
                    .name()
                    .map(str::to_owned)
                    .map_err(|_| Error::Corrupt("non-UTF8 snapshot entry".into()))
            })
            .collect()
    }
}

/// Object requests made by a reader, including opening its metadata.
///
/// These count returned blob bytes, not compressed network traffic, tree bytes,
/// cache misses, or physical disk I/O. Sources can measure those separately.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotReadStats {
    /// Number of complete blob reads.
    pub blob_reads: usize,
    /// Sum of bytes returned by blob reads.
    pub blob_bytes: u64,
    /// Number of directory listings.
    pub directory_reads: usize,
}

/// A query reader that fetches only visited format-2 objects.
///
/// Existing `Snapshot` APIs retain support for both persisted formats. This
/// optional reader supports format 2 only and creates no repository or refs.
/// Decoded shards are reused within a query and dropped afterwards; caching
/// between queries belongs to the source.
pub struct SnapshotReader<S> {
    source: S,
    root: ObjectId,
    meta: RootMeta,
    reads: SnapshotReadStats,
}
enum CandidateGroup {
    Exact(Vec<(u16, u32)>),
    Centroid(usize),
}

struct Shard {
    ids: Vec<PointId>,
    vectors: Vec<Vec<f32>>,
    payloads: Option<Vec<JsonObject>>,
}
impl<S: SnapshotSource> SnapshotReader<S> {
    /// Open metadata for an immutable root served by `source`.
    pub fn open(root: ObjectId, source: S) -> Result<Self> {
        if root.0.len() != 40
            || root
                .0
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        {
            return Err(Error::Invalid(
                "snapshot root must be a full lowercase SHA-1 object ID".into(),
            ));
        }
        let bytes = source.read_blob("meta.json")?;
        let meta = decode_meta(&bytes)?;
        if meta.format_version != 2 {
            return Err(Error::Invalid(
                "selective snapshot reads require format 2".into(),
            ));
        }
        validate_config(&meta.config()).map_err(|error| Error::Corrupt(error.to_string()))?;
        validate_ivf_meta(&meta)?;
        Ok(Self {
            source,
            root,
            meta,
            reads: SnapshotReadStats {
                blob_reads: 1,
                blob_bytes: bytes.len() as u64,
                directory_reads: 0,
            },
        })
    }
    /// Collection configuration declared by this root.
    pub fn config(&self) -> CollectionConfig {
        self.meta.config()
    }
    /// Cumulative object requests, including metadata read during opening.
    pub fn read_stats(&self) -> SnapshotReadStats {
        self.reads
    }
    /// Borrow the underlying source, for example to inspect transport counters.
    pub fn source(&self) -> &S {
        &self.source
    }
    fn blob(&mut self, path: &str) -> Result<Vec<u8>> {
        let bytes = self.source.read_blob(path)?;
        self.reads.blob_reads += 1;
        self.reads.blob_bytes += bytes.len() as u64;
        Ok(bytes)
    }
    fn shard(&mut self, shards: &mut BTreeMap<u16, Shard>, shard: u16) -> Result<()> {
        if let std::collections::btree_map::Entry::Vacant(entry) = shards.entry(shard) {
            let ids = decode_ids(&self.blob(&format!("points/ids/{shard:03x}.bin"))?, shard)?;
            let vectors =
                decode_vectors(&self.blob(&format!("points/vectors/{shard:03x}.f32le"))?)?;
            if ids.is_empty()
                || ids.len() != vectors.len()
                || vectors
                    .iter()
                    .any(|vector| vector.len() != self.meta.dimension)
            {
                return Err(Error::Corrupt(
                    "invalid format-2 shard row count or dimension".into(),
                ));
            }
            entry.insert(Shard {
                ids,
                vectors,
                payloads: None,
            });
        }
        Ok(())
    }
    fn payloads(&mut self, shard: u16, data: &mut Shard) -> Result<()> {
        if data.payloads.is_none() {
            let payloads =
                decode_payloads(&self.blob(&format!("points/payloads/{shard:03x}.bin"))?)?;
            if payloads.len() != data.ids.len() {
                return Err(Error::Corrupt("format-2 payload row count mismatch".into()));
            }
            data.payloads = Some(payloads);
        }
        Ok(())
    }
    /// Query with the same ranking, filter, budget and tie semantics as `Snapshot::query`.
    ///
    /// Approximate searches read the codebook, selected postings and candidate
    /// shards. Unfiltered queries defer payload reads until winners are known.
    /// Exact searches read every ID/vector shard. A filter may require payload
    /// shards for all discovered candidates.
    pub fn query(&mut self, query: Query) -> Result<QueryResult> {
        validate_query(&query, &self.meta)?;
        let exact = query
            .params
            .exact
            .unwrap_or(self.meta.point_count <= self.meta.index.full_scan_threshold);
        let mut shards = BTreeMap::new();
        let mut groups = Vec::new();
        let mut probes = 0;
        let mut centroid_count = 0;
        if exact {
            let mut names = self.source.list("points/ids")?;
            self.reads.directory_reads += 1;
            names.sort();
            if names.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(Error::Corrupt("duplicate shard entry".into()));
            }
            let mut rows = Vec::new();
            for name in names {
                let shard = name
                    .strip_suffix(".bin")
                    .and_then(|value| u16::from_str_radix(value, 16).ok())
                    .filter(|&shard| shard < 64 && name == format!("{shard:03x}.bin"))
                    .ok_or_else(|| Error::Corrupt("non-canonical format-2 shard name".into()))?;
                self.shard(&mut shards, shard)?;
                rows.extend((0..shards[&shard].ids.len()).map(|row| (shard, row as u32)));
            }
            if rows.len() != self.meta.point_count {
                return Err(Error::Corrupt("format-2 point count mismatch".into()));
            }
            groups.push(CandidateGroup::Exact(rows));
        } else {
            let dimension = self.meta.dimension;
            let codebook =
                decode_codebook(&self.blob("index/ivf-flat-v2/codebook.bin")?, dimension)?;
            centroid_count = codebook.len();
            if centroid_count
                != self
                    .meta
                    .ivf
                    .as_ref()
                    .expect("validated IVF metadata")
                    .centroid_count
            {
                return Err(Error::Corrupt(
                    "format-2 codebook centroid count mismatch".into(),
                ));
            }
            probes = if query.params.probes == 0 {
                if query.filter.is_some() {
                    centroid_count
                } else {
                    self.meta.index.default_probes
                }
            } else {
                query.params.probes
            }
            .min(centroid_count);
            let mut centroids = codebook
                .iter()
                .enumerate()
                .map(|(id, vector)| (id, cosine_f64(&query.vector, vector)))
                .collect::<Vec<_>>();
            centroids.sort_by(|left, right| {
                right
                    .1
                    .total_cmp(&left.1)
                    .then_with(|| left.0.cmp(&right.0))
            });
            // Load postings inside the scoring loop so a candidate budget also
            // avoids requests for subsequent buckets.
            groups = centroids
                .into_iter()
                .take(probes)
                .map(|(id, _)| CandidateGroup::Centroid(id))
                .collect();
        }
        let candidate_limit = if query.params.candidate_limit == 0 {
            self.meta.index.default_candidate_limit
        } else {
            query.params.candidate_limit
        };
        let mut stats = QueryStats {
            mode: if exact {
                QueryMode::Exact
            } else {
                QueryMode::Approximate
            },
            collection_points: self.meta.point_count,
            buckets_probed: 0,
            candidates_discovered: 0,
            vectors_scored: 0,
            probe_limit_exhausted: false,
            candidate_limit_exhausted: false,
        };
        let mut ranked = Vec::new();
        let mut visited = BTreeSet::new();
        'groups: for group in groups {
            let rows = match group {
                CandidateGroup::Exact(rows) => rows,
                CandidateGroup::Centroid(centroid) => {
                    let rows = decode_posting(
                        &self.blob(&format!("index/ivf-flat-v2/postings/{centroid:04x}.bin"))?,
                    )?;
                    stats.buckets_probed += 1;
                    rows
                }
            };
            for (shard, row) in rows {
                if !visited.insert((shard, row)) {
                    return Err(Error::Corrupt("duplicate format-2 posting row".into()));
                }
                stats.candidates_discovered += 1;
                self.shard(&mut shards, shard)?;
                let data = shards.get_mut(&shard).expect("loaded shard");
                let row = row as usize;
                let id = data
                    .ids
                    .get(row)
                    .ok_or_else(|| Error::Corrupt("posting row outside shard".into()))?
                    .clone();
                if let Some(filter) = &query.filter {
                    self.payloads(shard, data)?;
                    if !matches_filter(
                        filter,
                        &id,
                        &data.payloads.as_ref().expect("loaded payloads")[row],
                    ) {
                        continue;
                    }
                }
                if !exact && stats.vectors_scored >= candidate_limit {
                    stats.candidate_limit_exhausted = true;
                    break 'groups;
                }
                stats.vectors_scored += 1;
                ranked.push((
                    cosine_f64(&query.vector, &data.vectors[row]),
                    id,
                    shard,
                    row,
                ));
            }
        }
        stats.probe_limit_exhausted =
            !exact && stats.buckets_probed == probes && probes < centroid_count;
        ranked.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        ranked.truncate(query.limit);
        let mut points = Vec::with_capacity(ranked.len());
        for (score, id, shard, row) in ranked {
            let data = shards.get_mut(&shard).expect("loaded winner shard");
            let payload = if query.with_payload {
                self.payloads(shard, data)?;
                Some(data.payloads.as_ref().expect("loaded payloads")[row].clone())
            } else {
                None
            };
            points.push(ScoredPoint {
                id,
                score: score as f32,
                payload,
                vector: query.with_vector.then(|| data.vectors[row].clone()),
            });
        }
        Ok(QueryResult {
            root: self.root.clone(),
            points,
            stats,
        })
    }
}
