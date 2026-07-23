//! Canonical root-tree construction, reads, search, and validation.

use crate::codec::{
    canonical_json, decode_id, decode_payload, decode_vector, encode_id, encode_vector, id_hash,
    validate_vector_components,
};
use crate::filter::matches_filter;
use crate::*;
use git2::{ObjectType, Oid, Repository, Tree};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::sync::OnceLock;

const FORMAT_VERSION_V1: u32 = 1;
const FORMAT_VERSION_V2: u32 = 2;
const TREE_MODE: i32 = 0o040000;
const BLOB_MODE: i32 = 0o100644;

#[cfg(test)]
type BucketEntries = BTreeMap<usize, BTreeMap<String, BTreeMap<String, Vec<(String, Oid)>>>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RootMeta {
    pub(crate) format_version: u32,
    pub(crate) point_count: usize,
    pub(crate) dimension: usize,
    pub(crate) distance: Distance,
    pub(crate) vector_space: Option<String>,
    pub(crate) vector_codec: String,
    pub(crate) git_object_format: String,
    pub(crate) index: IndexConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ivf: Option<IvfConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct IvfConfig {
    pub(crate) shard_bits: usize,
    pub(crate) centroid_count: usize,
    pub(crate) training_sample_limit: usize,
    pub(crate) training_iterations: usize,
}

impl RootMeta {
    fn new_v1(config: &CollectionConfig, point_count: usize) -> Self {
        Self {
            format_version: FORMAT_VERSION_V1,
            point_count,
            dimension: config.dimension,
            distance: config.distance,
            vector_space: config.vector_space.clone(),
            vector_codec: "f32le-v1".into(),
            git_object_format: "sha1".into(),
            index: config.index.clone(),
            ivf: None,
        }
    }

    pub(crate) fn new_v2(config: &CollectionConfig, point_count: usize, ivf: IvfConfig) -> Self {
        Self {
            format_version: FORMAT_VERSION_V2,
            point_count,
            dimension: config.dimension,
            distance: config.distance,
            vector_space: config.vector_space.clone(),
            vector_codec: "f32le-sharded-v2".into(),
            git_object_format: "sha1".into(),
            index: config.index.clone(),
            ivf: Some(ivf),
        }
    }

    pub(crate) fn config(&self) -> CollectionConfig {
        CollectionConfig {
            dimension: self.dimension,
            distance: self.distance,
            vector_space: self.vector_space.clone(),
            index: self.index.clone(),
        }
    }

    pub(crate) fn point_count(&self) -> usize {
        self.point_count
    }

    pub(crate) fn format_version(&self) -> u32 {
        self.format_version
    }
}
#[derive(Clone, Debug)]
pub(crate) struct StoredPoint {
    pub(crate) point: Point,
    pub(crate) tree: Oid,
}

#[derive(Debug)]
pub(crate) struct SearchView {
    pub(crate) points: Vec<Point>,
    point_trees: OnceLock<Vec<(Oid, usize)>>,
    v2_rows: OnceLock<BTreeMap<(u16, u32), usize>>,
    pub(crate) v2_index: OnceLock<crate::root_v2::V2SearchIndex>,
}

impl SearchView {
    pub(crate) fn new(points: Vec<Point>) -> Self {
        Self {
            points,
            point_trees: OnceLock::new(),
            v2_rows: OnceLock::new(),
            v2_index: OnceLock::new(),
        }
    }

    pub(crate) fn point_for_v2_row(&self, shard: u16, row: u32) -> Result<&Point> {
        if self.v2_rows.get().is_none() {
            let mut grouped = BTreeMap::<u16, Vec<(PointId, usize)>>::new();
            for (index, point) in self.points.iter().enumerate() {
                let shard = crate::root_v2::shard_for_id(&point.id);
                grouped
                    .entry(shard)
                    .or_default()
                    .push((point.id.clone(), index));
            }
            let mut rows = BTreeMap::new();
            for (shard, mut points) in grouped {
                points.sort_by(|left, right| left.0.cmp(&right.0));
                for (row, (_, index)) in points.into_iter().enumerate() {
                    rows.insert(
                        (
                            shard,
                            u32::try_from(row).map_err(|_| {
                                Error::Corrupt("format-2 shard row exceeds u32".into())
                            })?,
                        ),
                        index,
                    );
                }
            }
            let _ = self.v2_rows.set(rows);
        }
        let index = self
            .v2_rows
            .get()
            .expect("format-2 row lookup was initialized")
            .get(&(shard, row))
            .copied()
            .ok_or_else(|| Error::Corrupt("format-2 posting references a missing row".into()))?;
        Ok(&self.points[index])
    }

    fn point_for_tree(&self, repo: &Repository, root: Oid, tree: Oid) -> Result<&Point> {
        if self.point_trees.get().is_none() {
            let mut indices = self
                .points
                .iter()
                .enumerate()
                .map(|(index, point)| (id_hash(&point.id), index))
                .collect::<BTreeMap<_, _>>();
            let mut point_trees = Vec::with_capacity(indices.len());
            for (hash, point_tree) in point_tree_entries(repo, root)? {
                let index = indices.remove(&hash).ok_or_else(|| {
                    Error::Corrupt("root point tree does not match cached points".into())
                })?;
                point_trees.push((point_tree, index));
            }
            if !indices.is_empty() {
                return Err(Error::Corrupt(
                    "cached points do not match the root point tree".into(),
                ));
            }
            point_trees.sort_unstable_by_key(|(oid, _)| *oid);
            let _ = self.point_trees.set(point_trees);
        }
        let point_trees = self
            .point_trees
            .get()
            .expect("search-view point-tree lookup was initialized");
        let index = point_trees
            .binary_search_by_key(&tree, |(oid, _)| *oid)
            .map_err(|_| Error::Corrupt("bucket points outside the root point tree".into()))?;
        Ok(&self.points[point_trees[index].1])
    }
}

pub(crate) struct PointChange {
    pub(crate) old: Option<Point>,
    pub(crate) new: Option<Point>,
}

pub(crate) fn diff_roots(repo: &Repository, left: Oid, right: Oid) -> Result<DiffResult> {
    let left_points = read_stored_points(repo, left)?;
    let right_points = read_stored_points(repo, right)?;
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for (id, point) in &left_points {
        match right_points.get(id) {
            None => removed.push(id.clone()),
            Some(right_point) if right_point.point != point.point => changed.push(id.clone()),
            _ => {}
        }
    }
    for id in right_points.keys() {
        if !left_points.contains_key(id) {
            added.push(id.clone());
        }
    }
    let left_meta = read_meta(repo, left)?;
    let right_meta = read_meta(repo, right)?;
    let left_index = root_entry_oid(repo, left, "index")?;
    let right_index = root_entry_oid(repo, right, "index")?;
    let left_objects = object_sizes(repo, left)?;
    let right_objects = object_sizes(repo, right)?;
    let shared_ids: BTreeSet<_> = left_objects
        .keys()
        .filter(|id| right_objects.contains_key(id))
        .copied()
        .collect();
    Ok(DiffResult {
        left_root: left.into(),
        right_root: right.into(),
        added,
        removed,
        changed,
        configuration_changed: left_meta.config() != right_meta.config(),
        buckets_changed: left_index != right_index,
        shared: stats_for(&left_objects, shared_ids.iter().copied()),
        left_unique: stats_for(
            &left_objects,
            left_objects
                .keys()
                .filter(|id| !shared_ids.contains(id))
                .copied(),
        ),
        right_unique: stats_for(
            &right_objects,
            right_objects
                .keys()
                .filter(|id| !shared_ids.contains(id))
                .copied(),
        ),
    })
}

pub(crate) fn get_root(repo: &Repository, root: Oid, request: GetRequest) -> Result<GetResult> {
    let points = read_all_points(repo, root)?;
    let selected_ids: BTreeSet<_> = request.ids.into_iter().collect();
    let mut records = points
        .into_values()
        .filter(|point| {
            (selected_ids.is_empty() || selected_ids.contains(&point.id))
                && request
                    .filter
                    .as_ref()
                    .is_none_or(|filter| matches_filter(filter, &point.id, &point.payload))
        })
        .skip(request.offset)
        .take(request.limit.unwrap_or(usize::MAX))
        .map(|point| Record {
            id: point.id,
            payload: request.with_payload.then_some(point.payload),
            vector: request.with_vector.then_some(point.vector),
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(GetResult {
        root: root.into(),
        points: records,
    })
}

pub(crate) fn count_root(
    repo: &Repository,
    root: Oid,
    filter: Option<Filter>,
) -> Result<CountResult> {
    let meta = read_meta(repo, root)?;
    let count = if let Some(filter) = filter {
        read_all_points(repo, root)?
            .values()
            .filter(|point| matches_filter(&filter, &point.id, &point.payload))
            .count()
    } else {
        meta.point_count
    };
    Ok(CountResult {
        root: root.into(),
        count,
    })
}

pub(crate) fn query_root_with_cache(
    repo: &Repository,
    root: Oid,
    query: Query,
    cached_points: Option<&OnceLock<SearchView>>,
) -> Result<QueryResult> {
    let meta = read_meta(repo, root)?;
    validate_query(&query, &meta)?;
    let exact = query
        .params
        .exact
        .unwrap_or(meta.point_count <= meta.index.full_scan_threshold);
    if exact {
        if let Some(cache) = cached_points {
            if cache.get().is_none() {
                let points = read_all_points(repo, root)?.into_values().collect();
                let _ = cache.set(SearchView::new(points));
            }
            exact_query_points(
                root,
                &meta,
                query,
                &cache
                    .get()
                    .expect("snapshot point cache was initialized")
                    .points,
            )
        } else if meta.format_version == FORMAT_VERSION_V1 {
            exact_query(repo, root, &meta, query)
        } else {
            let points = read_all_points(repo, root)?
                .into_values()
                .collect::<Vec<_>>();
            exact_query_points(root, &meta, query, &points)
        }
    } else if meta.format_version == FORMAT_VERSION_V2 {
        crate::root_v2::approximate_query(repo, root, &meta, query, cached_points)
    } else {
        let cached_points = cached_points.and_then(OnceLock::get);
        approximate_query(repo, root, &meta, query, cached_points)
    }
}

pub(crate) fn validate_root(repo: &Repository, root: Oid, full: bool) -> Result<ValidationReport> {
    let meta = read_meta(repo, root)?;
    if meta.format_version == FORMAT_VERSION_V2 {
        return crate::root_v2::validate_root(repo, root, &meta, full);
    }
    validate_config(&meta.config()).map_err(|error| Error::Corrupt(error.to_string()))?;
    validate_v1_index(&meta.index).map_err(|error| Error::Corrupt(error.to_string()))?;
    let points = read_stored_points(repo, root)?;
    if points.len() != meta.point_count {
        return Err(Error::Corrupt(format!(
            "metadata point count {} does not match tree count {}",
            meta.point_count,
            points.len()
        )));
    }
    let mut checked_buckets = 0;
    if full {
        let projections = lsh_projections(&meta.index, meta.dimension);
        let mut expected = BTreeSet::new();
        for (id, stored) in &points {
            validate_point(&stored.point, &meta.config())
                .map_err(|error| Error::Corrupt(error.to_string()))?;
            let hash = id_hash(id);
            for (table, hyperplanes) in projections.iter().enumerate() {
                let signature = lsh_signature_with(&stored.point.vector, hyperplanes);
                expected.insert((table, signature, hash.clone(), stored.tree));
            }
        }
        let actual = read_bucket_entries(repo, root, &meta.index)?;
        checked_buckets = actual.len();
        if expected != actual {
            return Err(Error::Corrupt(
                "LSH bucket entries do not match authoritative points".into(),
            ));
        }
    }
    Ok(ValidationReport {
        root: root.into(),
        full,
        point_count: points.len(),
        checked_buckets,
        valid: true,
    })
}

pub(crate) fn validate_config(config: &CollectionConfig) -> Result<()> {
    if config.dimension == 0 || config.dimension > u32::MAX as usize {
        return Err(Error::Invalid(
            "dimension must be between 1 and u32::MAX".into(),
        ));
    }
    if config.index.default_probes == 0 || config.index.default_candidate_limit == 0 {
        return Err(Error::Invalid(
            "index probes and candidate limit must be positive".into(),
        ));
    }
    Ok(())
}

fn validate_v1_index(index: &IndexConfig) -> Result<()> {
    if index.tables == 0 || index.signature_bits == 0 || index.signature_bits > 64 {
        return Err(Error::Invalid(
            "format-1 index tables and signature bits must be positive; signature bits must not exceed 64".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_point(point: &Point, config: &CollectionConfig) -> Result<()> {
    if point.vector.len() != config.dimension {
        return Err(Error::Invalid(format!(
            "point {} has dimension {}, expected {}",
            point.id,
            point.vector.len(),
            config.dimension
        )));
    }
    validate_vector_components(&point.vector)?;
    canonical_json(&point.payload)?;
    Ok(())
}

fn validate_query(query: &Query, meta: &RootMeta) -> Result<()> {
    if query.vector.len() != meta.dimension {
        return Err(Error::Invalid(format!(
            "query has dimension {}, expected {}",
            query.vector.len(),
            meta.dimension
        )));
    }
    validate_vector_components(&query.vector)?;
    if let Some(expected) = &query.expected_vector_space {
        if meta.vector_space.as_ref() != Some(expected) {
            return Err(Error::Invalid(format!(
                "vector-space mismatch: expected {expected:?}, collection is {:?}",
                meta.vector_space
            )));
        }
    }
    Ok(())
}

pub(crate) fn build_root(
    repo: &Repository,
    config: &CollectionConfig,
    points: &BTreeMap<PointId, Point>,
) -> Result<Oid> {
    crate::root_v2::build_root(repo, config, points)
}

#[cfg(test)]
fn build_root_v1(
    repo: &Repository,
    config: &CollectionConfig,
    points: &BTreeMap<PointId, Point>,
    reusable_point_trees: &BTreeMap<PointId, Oid>,
) -> Result<Oid> {
    let projections = lsh_projections(&config.index, config.dimension);
    let mut point_entries: BTreeMap<String, Vec<(String, Oid)>> = BTreeMap::new();
    let mut bucket_entries: BucketEntries = BTreeMap::new();
    for point in points.values() {
        validate_point(point, config)?;
        let hash = id_hash(&point.id);
        let point_tree = reusable_point_trees
            .get(&point.id)
            .copied()
            .map_or_else(|| write_point_tree(repo, point), Ok)?;
        point_entries
            .entry(hash[..2].into())
            .or_default()
            .push((hash.clone(), point_tree));
        for (table, hyperplanes) in projections.iter().enumerate() {
            let signature = lsh_signature_with(&point.vector, hyperplanes);
            let signature_name = signature_name(signature, config.index.signature_bits);
            bucket_entries
                .entry(table)
                .or_default()
                .entry(signature_name[..2.min(signature_name.len())].into())
                .or_default()
                .entry(signature_name)
                .or_default()
                .push((hash.clone(), point_tree));
        }
    }

    let points_tree = write_points_tree(repo, point_entries)?;
    let lsh_tree = write_lsh_tree(repo, bucket_entries)?;
    let mut index_builder = repo.treebuilder(None)?;
    index_builder.insert("lsh-v1", lsh_tree, TREE_MODE)?;
    let index_tree = index_builder.write()?;
    let meta_blob = repo.blob(&canonical_json(&RootMeta::new_v1(config, points.len()))?)?;
    let mut root = repo.treebuilder(None)?;
    root.insert("index", index_tree, TREE_MODE)?;
    root.insert("meta.json", meta_blob, BLOB_MODE)?;
    root.insert("points", points_tree, TREE_MODE)?;
    Ok(root.write()?)
}

pub(crate) fn update_root(
    repo: &Repository,
    previous_root: Oid,
    config: &CollectionConfig,
    final_point_count: usize,
    changes: &BTreeMap<PointId, PointChange>,
) -> Result<Oid> {
    if changes.is_empty() {
        return Ok(previous_root);
    }
    let meta = read_meta(repo, previous_root)?;
    if meta.format_version == FORMAT_VERSION_V2 {
        return crate::root_v2::update_root(repo, previous_root, config, changes);
    }
    update_root_v1(repo, previous_root, config, final_point_count, changes)
}

fn update_root_v1(
    repo: &Repository,
    previous_root: Oid,
    config: &CollectionConfig,
    final_point_count: usize,
    changes: &BTreeMap<PointId, PointChange>,
) -> Result<Oid> {
    let projections = lsh_projections(&config.index, config.dimension);
    let mut new_trees = BTreeMap::new();
    for (id, change) in changes {
        if let Some(point) = &change.new {
            new_trees.insert(id.clone(), write_point_tree(repo, point)?);
        }
    }

    let old_points_oid = root_entry_oid(repo, previous_root, "points")?;
    let old_points_tree = repo.find_tree(old_points_oid)?;
    let mut points_builder = repo.treebuilder(Some(&old_points_tree))?;
    let mut point_prefixes: BTreeMap<String, Vec<(&PointId, &PointChange)>> = BTreeMap::new();
    for (id, change) in changes {
        let hash = id_hash(id);
        point_prefixes
            .entry(hash[..2].to_owned())
            .or_default()
            .push((id, change));
    }
    for (prefix, prefix_changes) in point_prefixes {
        let old_prefix = old_points_tree.get_name(&prefix).map(|entry| entry.id());
        let old_prefix_tree = old_prefix.map(|oid| repo.find_tree(oid)).transpose()?;
        let mut prefix_builder = repo.treebuilder(old_prefix_tree.as_ref())?;
        for (id, change) in prefix_changes {
            let hash = id_hash(id);
            if change.new.is_some() {
                prefix_builder.insert(&hash, new_trees[id], TREE_MODE)?;
            } else {
                prefix_builder.remove(&hash)?;
            }
        }
        if prefix_builder.is_empty() {
            points_builder.remove(&prefix)?;
        } else {
            points_builder.insert(&prefix, prefix_builder.write()?, TREE_MODE)?;
        }
    }
    let points_tree = points_builder.write()?;

    type EntryUpdates = BTreeMap<String, Option<Oid>>;
    type BucketUpdates = BTreeMap<usize, BTreeMap<String, EntryUpdates>>;
    type PrefixUpdates = BTreeMap<String, Vec<(String, EntryUpdates)>>;
    let mut bucket_updates = BucketUpdates::new();
    for (id, change) in changes {
        let hash = id_hash(id);
        for (table, hyperplanes) in projections.iter().enumerate() {
            let old_signature = change
                .old
                .as_ref()
                .map(|point| lsh_signature_with(&point.vector, hyperplanes));
            let new_signature = change
                .new
                .as_ref()
                .map(|point| lsh_signature_with(&point.vector, hyperplanes));
            if let Some(signature) = old_signature {
                if new_signature != Some(signature) {
                    bucket_updates
                        .entry(table)
                        .or_default()
                        .entry(signature_name(signature, config.index.signature_bits))
                        .or_default()
                        .insert(hash.clone(), None);
                }
            }
            if let Some(signature) = new_signature {
                bucket_updates
                    .entry(table)
                    .or_default()
                    .entry(signature_name(signature, config.index.signature_bits))
                    .or_default()
                    .insert(hash.clone(), Some(new_trees[id]));
            }
        }
    }

    let old_index_oid = root_entry_oid(repo, previous_root, "index")?;
    let old_index_tree = repo.find_tree(old_index_oid)?;
    let old_lsh_oid = tree_child_oid(repo, old_index_oid, "lsh-v1")?;
    let old_lsh_tree = repo.find_tree(old_lsh_oid)?;
    let mut lsh_builder = repo.treebuilder(Some(&old_lsh_tree))?;
    for (table, signatures) in bucket_updates {
        let table_name = format!("{table:04x}");
        let old_table = old_lsh_tree.get_name(&table_name).map(|entry| entry.id());
        let old_table_tree = old_table.map(|oid| repo.find_tree(oid)).transpose()?;
        let mut table_builder = repo.treebuilder(old_table_tree.as_ref())?;
        let mut prefixes = PrefixUpdates::new();
        for (signature, updates) in signatures {
            prefixes
                .entry(signature[..2.min(signature.len())].to_owned())
                .or_default()
                .push((signature, updates));
        }
        for (prefix, signatures) in prefixes {
            let old_prefix = old_table_tree
                .as_ref()
                .and_then(|tree| tree.get_name(&prefix))
                .map(|entry| entry.id());
            let old_prefix_tree = old_prefix.map(|oid| repo.find_tree(oid)).transpose()?;
            let mut prefix_builder = repo.treebuilder(old_prefix_tree.as_ref())?;
            for (signature, updates) in signatures {
                let old_bucket = old_prefix_tree
                    .as_ref()
                    .and_then(|tree| tree.get_name(&signature))
                    .map(|entry| entry.id());
                let old_bucket_tree = old_bucket.map(|oid| repo.find_tree(oid)).transpose()?;
                let mut bucket_builder = repo.treebuilder(old_bucket_tree.as_ref())?;
                for (hash, point_tree) in updates {
                    if let Some(point_tree) = point_tree {
                        bucket_builder.insert(&hash, point_tree, TREE_MODE)?;
                    } else {
                        bucket_builder.remove(&hash)?;
                    }
                }
                if bucket_builder.is_empty() {
                    prefix_builder.remove(&signature)?;
                } else {
                    prefix_builder.insert(&signature, bucket_builder.write()?, TREE_MODE)?;
                }
            }
            if prefix_builder.is_empty() {
                table_builder.remove(&prefix)?;
            } else {
                table_builder.insert(&prefix, prefix_builder.write()?, TREE_MODE)?;
            }
        }
        if table_builder.is_empty() {
            lsh_builder.remove(&table_name)?;
        } else {
            lsh_builder.insert(&table_name, table_builder.write()?, TREE_MODE)?;
        }
    }
    let lsh_tree = lsh_builder.write()?;
    let mut index_builder = repo.treebuilder(Some(&old_index_tree))?;
    index_builder.insert("lsh-v1", lsh_tree, TREE_MODE)?;
    let index_tree = index_builder.write()?;
    let meta_blob = repo.blob(&canonical_json(&RootMeta::new_v1(
        config,
        final_point_count,
    ))?)?;
    let old_root_tree = repo.find_tree(previous_root)?;
    let mut root = repo.treebuilder(Some(&old_root_tree))?;
    root.insert("index", index_tree, TREE_MODE)?;
    root.insert("meta.json", meta_blob, BLOB_MODE)?;
    root.insert("points", points_tree, TREE_MODE)?;
    Ok(root.write()?)
}

fn write_point_tree(repo: &Repository, point: &Point) -> Result<Oid> {
    let id = repo.blob(&encode_id(&point.id)?)?;
    let vector = repo.blob(&encode_vector(&point.vector)?)?;
    let payload = repo.blob(&canonical_json(&point.payload)?)?;
    let mut builder = repo.treebuilder(None)?;
    builder.insert("id.json", id, BLOB_MODE)?;
    builder.insert("payload.json", payload, BLOB_MODE)?;
    builder.insert("vector.f32le", vector, BLOB_MODE)?;
    Ok(builder.write()?)
}

#[cfg(test)]
fn write_points_tree(
    repo: &Repository,
    prefixes: BTreeMap<String, Vec<(String, Oid)>>,
) -> Result<Oid> {
    let mut root = repo.treebuilder(None)?;
    for (prefix, entries) in prefixes {
        let mut prefix_builder = repo.treebuilder(None)?;
        for (hash, oid) in entries {
            prefix_builder.insert(&hash, oid, TREE_MODE)?;
        }
        root.insert(&prefix, prefix_builder.write()?, TREE_MODE)?;
    }
    Ok(root.write()?)
}

#[cfg(test)]
fn write_lsh_tree(repo: &Repository, tables: BucketEntries) -> Result<Oid> {
    let mut lsh = repo.treebuilder(None)?;
    for (table, prefixes) in tables {
        let mut table_builder = repo.treebuilder(None)?;
        for (prefix, signatures) in prefixes {
            let mut prefix_builder = repo.treebuilder(None)?;
            for (signature, entries) in signatures {
                let mut bucket_builder = repo.treebuilder(None)?;
                for (hash, point_tree) in entries {
                    bucket_builder.insert(&hash, point_tree, TREE_MODE)?;
                }
                prefix_builder.insert(&signature, bucket_builder.write()?, TREE_MODE)?;
            }
            table_builder.insert(&prefix, prefix_builder.write()?, TREE_MODE)?;
        }
        lsh.insert(format!("{table:04x}"), table_builder.write()?, TREE_MODE)?;
    }
    Ok(lsh.write()?)
}

pub(crate) fn read_meta(repo: &Repository, root: Oid) -> Result<RootMeta> {
    let root_tree = repo.find_tree(root)?;
    if root_tree.get_name("points").is_none() || root_tree.get_name("index").is_none() {
        return Err(Error::Corrupt(
            "root must contain meta.json, points, and index".into(),
        ));
    }
    let bytes = read_named_blob(repo, &root_tree, "meta.json")?;
    let meta: RootMeta = serde_json::from_slice(&bytes)?;
    if canonical_json(&meta)? != bytes {
        return Err(Error::Corrupt("meta.json is not canonical JSON".into()));
    }
    let supported = match meta.format_version {
        FORMAT_VERSION_V1 => meta.vector_codec == "f32le-v1" && meta.ivf.is_none(),
        FORMAT_VERSION_V2 => meta.vector_codec == "f32le-sharded-v2" && meta.ivf.is_some(),
        _ => false,
    };
    if !supported || meta.git_object_format != "sha1" {
        return Err(Error::Corrupt(format!(
            "unsupported format metadata: version {}, vector codec {}, object format {}",
            meta.format_version, meta.vector_codec, meta.git_object_format
        )));
    }
    Ok(meta)
}

pub(crate) fn read_all_points(repo: &Repository, root: Oid) -> Result<BTreeMap<PointId, Point>> {
    if read_meta(repo, root)?.format_version == FORMAT_VERSION_V2 {
        return crate::root_v2::read_all_points(repo, root);
    }
    Ok(read_stored_points_v1(repo, root)?
        .into_iter()
        .map(|(id, stored)| (id, stored.point))
        .collect())
}

pub(crate) fn read_point_by_id(
    repo: &Repository,
    root: Oid,
    id: &PointId,
) -> Result<Option<Point>> {
    if read_meta(repo, root)?.format_version == FORMAT_VERSION_V2 {
        return crate::root_v2::read_point_by_id(repo, root, id);
    }
    let hash = id_hash(id);
    let points_oid = root_entry_oid(repo, root, "points")?;
    let points_tree = repo.find_tree(points_oid)?;
    let Some(prefix_entry) = points_tree.get_name(&hash[..2]) else {
        return Ok(None);
    };
    ensure_tree_entry(&prefix_entry, "point hash prefix")?;
    let prefix_tree = repo.find_tree(prefix_entry.id())?;
    let Some(point_entry) = prefix_tree.get_name(&hash) else {
        return Ok(None);
    };
    ensure_tree_entry(&point_entry, "point")?;
    let point = read_point_tree(repo, point_entry.id())?;
    if &point.id != id {
        return Err(Error::Corrupt(format!(
            "point ID hash does not match path for {}",
            point.id
        )));
    }
    Ok(Some(point))
}

pub(crate) fn read_stored_points(
    repo: &Repository,
    root: Oid,
) -> Result<BTreeMap<PointId, StoredPoint>> {
    if read_meta(repo, root)?.format_version == FORMAT_VERSION_V2 {
        return Ok(crate::root_v2::read_all_points(repo, root)?
            .into_iter()
            .map(|(id, point)| {
                (
                    id,
                    StoredPoint {
                        point,
                        tree: Oid::ZERO_SHA1,
                    },
                )
            })
            .collect());
    }
    read_stored_points_v1(repo, root)
}

fn read_stored_points_v1(repo: &Repository, root: Oid) -> Result<BTreeMap<PointId, StoredPoint>> {
    let mut result = BTreeMap::new();
    for (hash, point_tree) in point_tree_entries(repo, root)? {
        let point = read_point_tree(repo, point_tree)?;
        if id_hash(&point.id) != hash {
            return Err(Error::Corrupt(format!(
                "point ID hash does not match path for {}",
                point.id
            )));
        }
        let id = point.id.clone();
        if result
            .insert(
                id,
                StoredPoint {
                    point,
                    tree: point_tree,
                },
            )
            .is_some()
        {
            return Err(Error::Corrupt("duplicate typed point ID".into()));
        }
    }
    Ok(result)
}

fn point_tree_entries(repo: &Repository, root: Oid) -> Result<Vec<(String, Oid)>> {
    let points_oid = root_entry_oid(repo, root, "points")?;
    let points_tree = repo.find_tree(points_oid)?;
    let mut result = Vec::new();
    for prefix_entry in &points_tree {
        ensure_tree_entry(&prefix_entry, "point hash prefix")?;
        let prefix = prefix_entry.name().unwrap_or_default();
        let prefix_tree = repo.find_tree(prefix_entry.id())?;
        for point_entry in &prefix_tree {
            ensure_tree_entry(&point_entry, "point")?;
            let hash = point_entry.name().unwrap_or_default();
            if hash.len() != 64 || !hash.starts_with(prefix) {
                return Err(Error::Corrupt(format!(
                    "invalid point hash path {prefix}/{hash}"
                )));
            }
            result.push((hash.to_owned(), point_entry.id()));
        }
    }
    Ok(result)
}

fn read_point_tree(repo: &Repository, oid: Oid) -> Result<Point> {
    let tree = repo.find_tree(oid)?;
    Ok(Point {
        id: decode_id(&read_named_blob(repo, &tree, "id.json")?)?,
        vector: decode_vector(&read_named_blob(repo, &tree, "vector.f32le")?)?,
        payload: decode_payload(&read_named_blob(repo, &tree, "payload.json")?)?,
    })
}

fn read_point_parts(
    repo: &Repository,
    oid: Oid,
    needs_payload: bool,
) -> Result<(PointId, Vec<f32>, Option<JsonObject>)> {
    let tree = repo.find_tree(oid)?;
    let id = decode_id(&read_named_blob(repo, &tree, "id.json")?)?;
    let vector = decode_vector(&read_named_blob(repo, &tree, "vector.f32le")?)?;
    let payload = needs_payload
        .then(|| {
            read_named_blob(repo, &tree, "payload.json").and_then(|bytes| decode_payload(&bytes))
        })
        .transpose()?;
    Ok((id, vector, payload))
}

fn read_named_blob(repo: &Repository, tree: &Tree<'_>, name: &str) -> Result<Vec<u8>> {
    let entry = tree
        .get_name(name)
        .ok_or_else(|| Error::Corrupt(format!("missing {name}")))?;
    if entry.kind() != Some(ObjectType::Blob) {
        return Err(Error::Corrupt(format!("{name} is not a blob")));
    }
    Ok(repo.find_blob(entry.id())?.content().to_vec())
}

fn root_entry_oid(repo: &Repository, root: Oid, name: &str) -> Result<Oid> {
    let tree = repo.find_tree(root)?;
    let entry = tree
        .get_name(name)
        .ok_or_else(|| Error::Corrupt(format!("missing root entry {name}")))?;
    if entry.kind() != Some(ObjectType::Tree) {
        return Err(Error::Corrupt(format!("root entry {name} is not a tree")));
    }
    Ok(entry.id())
}

fn ensure_tree_entry(entry: &git2::TreeEntry<'_>, context: &str) -> Result<()> {
    if entry.kind() != Some(ObjectType::Tree) {
        return Err(Error::Corrupt(format!("{context} entry is not a tree")));
    }
    Ok(())
}

fn exact_query(repo: &Repository, root: Oid, meta: &RootMeta, query: Query) -> Result<QueryResult> {
    let needs_payload = query.filter.is_some() || query.with_payload;
    let mut scored = Vec::new();
    let mut vectors_scored = 0;
    for (hash, point_tree) in point_tree_entries(repo, root)? {
        let (id, vector, payload) = read_point_parts(repo, point_tree, needs_payload)?;
        if id_hash(&id) != hash {
            return Err(Error::Corrupt(format!(
                "point ID hash does not match path for {id}"
            )));
        }
        if query.filter.as_ref().is_some_and(|filter| {
            !matches_filter(
                filter,
                &id,
                payload.as_ref().expect("payload requested for filter"),
            )
        }) {
            continue;
        }
        vectors_scored += 1;
        let ranking_score = cosine_f64(&query.vector, &vector);
        scored.push(RankedScoredPoint {
            ranking_score,
            point: ScoredPoint {
                id,
                score: ranking_score as f32,
                payload: query.with_payload.then(|| payload.unwrap_or_default()),
                vector: query.with_vector.then_some(vector),
            },
        });
    }
    select_ranked_and_truncate(&mut scored, query.limit);
    let scored = scored.into_iter().map(|ranked| ranked.point).collect();
    Ok(QueryResult {
        root: root.into(),
        points: scored,
        stats: QueryStats {
            mode: QueryMode::Exact,
            collection_points: meta.point_count,
            buckets_probed: 0,
            candidates_discovered: meta.point_count,
            vectors_scored,
            probe_limit_exhausted: false,
            candidate_limit_exhausted: false,
        },
    })
}

fn exact_query_points(
    root: Oid,
    meta: &RootMeta,
    query: Query,
    points: &[Point],
) -> Result<QueryResult> {
    let mut winners = BinaryHeap::with_capacity(query.limit.min(points.len()));
    let mut vectors_scored = 0;
    for point in points {
        if query
            .filter
            .as_ref()
            .is_some_and(|filter| !matches_filter(filter, &point.id, &point.payload))
        {
            continue;
        }
        vectors_scored += 1;
        let ranking_score = cosine_f64(&query.vector, &point.vector);
        if query.limit == 0 {
            continue;
        }
        let candidate = ScoredPointRef {
            point,
            ranking_score,
        };
        if winners.len() < query.limit {
            winners.push(candidate);
        } else if winners
            .peek()
            .is_some_and(|worst| candidate.cmp(worst) == Ordering::Less)
        {
            winners.pop();
            winners.push(candidate);
        }
    }
    let mut winners = winners.into_vec();
    winners.sort_by(ScoredPointRef::cmp);
    let scored = winners
        .into_iter()
        .map(|winner| ScoredPoint {
            id: winner.point.id.clone(),
            score: winner.ranking_score as f32,
            payload: query.with_payload.then(|| winner.point.payload.clone()),
            vector: query.with_vector.then(|| winner.point.vector.clone()),
        })
        .collect();
    Ok(QueryResult {
        root: root.into(),
        points: scored,
        stats: QueryStats {
            mode: QueryMode::Exact,
            collection_points: meta.point_count,
            buckets_probed: 0,
            candidates_discovered: meta.point_count,
            vectors_scored,
            probe_limit_exhausted: false,
            candidate_limit_exhausted: false,
        },
    })
}

fn approximate_query(
    repo: &Repository,
    root: Oid,
    meta: &RootMeta,
    query: Query,
    cached_points: Option<&SearchView>,
) -> Result<QueryResult> {
    let probes = if query.params.probes == 0 {
        meta.index.default_probes
    } else {
        query.params.probes
    };
    let candidate_limit = if query.params.candidate_limit == 0 {
        meta.index.default_candidate_limit
    } else {
        query.params.candidate_limit
    };
    let probe_sequence = lsh_probe_sequence(&query.vector, &meta.index, probes);
    let index_oid = root_entry_oid(repo, root, "index")?;
    let lsh_oid = tree_child_oid(repo, index_oid, "lsh-v1")?;
    let mut candidates = BTreeMap::<String, Oid>::new();
    let mut buckets_probed = 0;
    let mut candidate_limit_exhausted = false;
    for (table, signature) in &probe_sequence {
        buckets_probed += 1;
        if let Some(bucket) =
            find_bucket(repo, lsh_oid, *table, *signature, meta.index.signature_bits)?
        {
            let tree = repo.find_tree(bucket)?;
            for entry in &tree {
                ensure_tree_entry(&entry, "bucket point")?;
                let hash = entry.name().unwrap_or_default().to_owned();
                if candidates.contains_key(&hash) {
                    continue;
                }
                if candidates.len() >= candidate_limit {
                    candidate_limit_exhausted = true;
                    break;
                }
                candidates.insert(hash, entry.id());
            }
        }
        if candidate_limit_exhausted {
            break;
        }
    }
    let candidates_discovered = candidates.len();
    let needs_payload = query.filter.is_some() || query.with_payload;
    let mut scored = Vec::new();
    let mut vectors_scored = 0;
    for (hash, point_tree) in candidates {
        if let Some(cache) = cached_points {
            let point = cache.point_for_tree(repo, root, point_tree)?;
            if id_hash(&point.id) != hash {
                return Err(Error::Corrupt(
                    "bucket entry points to a mismatched point".into(),
                ));
            }
            if query
                .filter
                .as_ref()
                .is_some_and(|filter| !matches_filter(filter, &point.id, &point.payload))
            {
                continue;
            }
            vectors_scored += 1;
            scored.push(ScoredPoint {
                id: point.id.clone(),
                score: cosine(&query.vector, &point.vector),
                payload: query.with_payload.then(|| point.payload.clone()),
                vector: query.with_vector.then(|| point.vector.clone()),
            });
            continue;
        }
        let (id, vector, payload) = read_point_parts(repo, point_tree, needs_payload)?;
        if id_hash(&id) != hash {
            return Err(Error::Corrupt(
                "bucket entry points to a mismatched point".into(),
            ));
        }
        if query.filter.as_ref().is_some_and(|filter| {
            !matches_filter(
                filter,
                &id,
                payload.as_ref().expect("payload requested for filter"),
            )
        }) {
            continue;
        }
        vectors_scored += 1;
        scored.push(ScoredPoint {
            id,
            score: cosine(&query.vector, &vector),
            payload: query.with_payload.then(|| payload.unwrap_or_default()),
            vector: query.with_vector.then_some(vector),
        });
    }
    sort_and_truncate(&mut scored, query.limit);
    Ok(QueryResult {
        root: root.into(),
        points: scored,
        stats: QueryStats {
            mode: QueryMode::Approximate,
            collection_points: meta.point_count,
            buckets_probed,
            candidates_discovered,
            vectors_scored,
            probe_limit_exhausted: buckets_probed == probes,
            candidate_limit_exhausted,
        },
    })
}

fn sort_and_truncate(points: &mut Vec<ScoredPoint>, limit: usize) {
    points.sort_by(compare_scored_points);
    points.truncate(limit);
}

#[cfg(test)]
fn select_and_truncate(points: &mut Vec<ScoredPoint>, limit: usize) {
    if limit == 0 {
        points.clear();
        return;
    }
    if points.len() > limit {
        points.select_nth_unstable_by(limit, compare_scored_points);
        points.truncate(limit);
    }
    points.sort_by(compare_scored_points);
}

fn compare_scored_points(left: &ScoredPoint, right: &ScoredPoint) -> std::cmp::Ordering {
    compare_score_and_id(left.score, &left.id, right.score, &right.id)
}

fn compare_score_and_id(
    left_score: f32,
    left_id: &PointId,
    right_score: f32,
    right_id: &PointId,
) -> Ordering {
    right_score
        .total_cmp(&left_score)
        .then_with(|| left_id.cmp(right_id))
}

struct RankedScoredPoint {
    point: ScoredPoint,
    ranking_score: f64,
}

fn select_ranked_and_truncate(points: &mut Vec<RankedScoredPoint>, limit: usize) {
    if limit == 0 {
        points.clear();
        return;
    }
    if points.len() > limit {
        points.select_nth_unstable_by(limit, compare_ranked_points);
        points.truncate(limit);
    }
    points.sort_by(compare_ranked_points);
}

fn compare_ranked_points(left: &RankedScoredPoint, right: &RankedScoredPoint) -> Ordering {
    compare_rank_and_id(
        left.ranking_score,
        &left.point.id,
        right.ranking_score,
        &right.point.id,
    )
}

fn compare_rank_and_id(
    left_score: f64,
    left_id: &PointId,
    right_score: f64,
    right_id: &PointId,
) -> Ordering {
    right_score
        .total_cmp(&left_score)
        .then_with(|| left_id.cmp(right_id))
}

struct ScoredPointRef<'a> {
    point: &'a Point,
    ranking_score: f64,
}

impl PartialEq for ScoredPointRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ScoredPointRef<'_> {}

impl PartialOrd for ScoredPointRef<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredPointRef<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_rank_and_id(
            self.ranking_score,
            &self.point.id,
            other.ranking_score,
            &other.point.id,
        )
    }
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    cosine_f64(left, right) as f32
}

fn cosine_f64(left: &[f32], right: &[f32]) -> f64 {
    let mut dot = 0.0_f64;
    let mut left_norm = 0.0_f64;
    let mut right_norm = 0.0_f64;
    for (&left, &right) in left.iter().zip(right) {
        let left = left as f64;
        let right = right as f64;
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    }
}

#[cfg(test)]
fn lsh_signature(vector: &[f32], config: &IndexConfig, table: usize) -> u64 {
    let projections = (0..config.signature_bits)
        .map(|bit| {
            (0..vector.len())
                .map(|dimension| projection_sign(config.projection_seed, table, bit, dimension))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    lsh_signature_with(vector, &projections)
}

fn lsh_projections(config: &IndexConfig, dimension: usize) -> Vec<Vec<Vec<f64>>> {
    (0..config.tables)
        .map(|table| {
            (0..config.signature_bits)
                .map(|bit| {
                    (0..dimension)
                        .map(|dimension| {
                            projection_sign(config.projection_seed, table, bit, dimension)
                        })
                        .collect()
                })
                .collect()
        })
        .collect()
}

fn lsh_signature_with(vector: &[f32], projections: &[Vec<f64>]) -> u64 {
    let mut signature = 0_u64;
    for (bit, hyperplane) in projections.iter().enumerate() {
        let projection = vector
            .iter()
            .zip(hyperplane)
            .map(|(component, sign)| *component as f64 * sign)
            .sum::<f64>();
        if projection >= 0.0 {
            signature |= 1_u64 << bit;
        }
    }
    signature
}

fn projection_sign(seed: u64, table: usize, bit: usize, dimension: usize) -> f64 {
    let mut hasher = Sha256::new();
    hasher.update(b"git-vdb/lsh-v1/projection\0");
    hasher.update(seed.to_le_bytes());
    hasher.update((table as u64).to_le_bytes());
    hasher.update((bit as u64).to_le_bytes());
    hasher.update((dimension as u64).to_le_bytes());
    if hasher.finalize()[0] & 1 == 0 {
        -1.0
    } else {
        1.0
    }
}

fn signature_name(signature: u64, bits: usize) -> String {
    format!("{signature:0width$x}", width = bits.div_ceil(4))
}

fn lsh_probe_sequence(vector: &[f32], config: &IndexConfig, limit: usize) -> Vec<(usize, u64)> {
    let projections = lsh_projections(config, vector.len());
    let exact: Vec<_> = projections
        .iter()
        .map(|table| lsh_signature_with(vector, table))
        .collect();
    let mut probes = Vec::with_capacity(limit);
    for distance in 0..=config.signature_bits {
        let masks = hamming_masks(config.signature_bits, distance, limit);
        for (table, signature) in exact.iter().enumerate() {
            for mask in &masks {
                if probes.len() == limit {
                    return probes;
                }
                probes.push((table, signature ^ mask));
            }
        }
    }
    probes
}

fn hamming_masks(bits: usize, distance: usize, limit: usize) -> Vec<u64> {
    fn visit(
        bits: usize,
        left: usize,
        next: usize,
        mask: u64,
        limit: usize,
        output: &mut Vec<u64>,
    ) {
        if output.len() >= limit {
            return;
        }
        if left == 0 {
            output.push(mask);
            return;
        }
        for bit in next..bits {
            visit(
                bits,
                left - 1,
                bit + 1,
                mask | (1_u64 << bit),
                limit,
                output,
            );
        }
    }
    let mut masks = Vec::new();
    visit(bits, distance, 0, 0, limit, &mut masks);
    masks
}

fn tree_child_oid(repo: &Repository, parent: Oid, name: &str) -> Result<Oid> {
    let tree = repo.find_tree(parent)?;
    let entry = tree
        .get_name(name)
        .ok_or_else(|| Error::Corrupt(format!("missing tree path component {name}")))?;
    if entry.kind() != Some(ObjectType::Tree) {
        return Err(Error::Corrupt(format!(
            "tree path component {name} is not a tree"
        )));
    }
    Ok(entry.id())
}

fn find_bucket(
    repo: &Repository,
    lsh: Oid,
    table: usize,
    signature: u64,
    bits: usize,
) -> Result<Option<Oid>> {
    let signature = signature_name(signature, bits);
    let components = [
        format!("{table:04x}"),
        signature[..2.min(signature.len())].to_owned(),
        signature,
    ];
    let mut current = lsh;
    for component in components {
        let tree = repo.find_tree(current)?;
        let Some(entry) = tree.get_name(&component) else {
            return Ok(None);
        };
        if entry.kind() != Some(ObjectType::Tree) {
            return Err(Error::Corrupt(format!(
                "bucket path {component} is not a tree"
            )));
        }
        current = entry.id();
    }
    Ok(Some(current))
}

fn read_bucket_entries(
    repo: &Repository,
    root: Oid,
    config: &IndexConfig,
) -> Result<BTreeSet<(usize, u64, String, Oid)>> {
    let lsh = tree_child_oid(repo, root_entry_oid(repo, root, "index")?, "lsh-v1")?;
    let lsh_tree = repo.find_tree(lsh)?;
    let mut result = BTreeSet::new();
    for table_entry in &lsh_tree {
        ensure_tree_entry(&table_entry, "LSH table")?;
        let table = usize::from_str_radix(table_entry.name().unwrap_or_default(), 16)
            .map_err(|_| Error::Corrupt("invalid LSH table name".into()))?;
        let table_tree = repo.find_tree(table_entry.id())?;
        for prefix_entry in &table_tree {
            ensure_tree_entry(&prefix_entry, "LSH signature prefix")?;
            let prefix_tree = repo.find_tree(prefix_entry.id())?;
            for signature_entry in &prefix_tree {
                ensure_tree_entry(&signature_entry, "LSH signature")?;
                let signature_name_value = signature_entry.name().unwrap_or_default();
                let signature = u64::from_str_radix(signature_name_value, 16)
                    .map_err(|_| Error::Corrupt("invalid LSH signature".into()))?;
                if signature_name(signature, config.signature_bits) != signature_name_value {
                    return Err(Error::Corrupt("non-canonical LSH signature".into()));
                }
                let bucket = repo.find_tree(signature_entry.id())?;
                for point_entry in &bucket {
                    ensure_tree_entry(&point_entry, "LSH bucket point")?;
                    result.insert((
                        table,
                        signature,
                        point_entry.name().unwrap_or_default().to_owned(),
                        point_entry.id(),
                    ));
                }
            }
        }
    }
    Ok(result)
}

fn object_sizes(repo: &Repository, root: Oid) -> Result<BTreeMap<Oid, usize>> {
    fn walk(repo: &Repository, oid: Oid, output: &mut BTreeMap<Oid, usize>) -> Result<()> {
        if output.contains_key(&oid) {
            return Ok(());
        }
        let odb = repo.odb()?;
        let object = odb.read(oid)?;
        output.insert(oid, object.data().len());
        if object.kind() == ObjectType::Tree {
            let tree = repo.find_tree(oid)?;
            for entry in &tree {
                walk(repo, entry.id(), output)?;
            }
        }
        Ok(())
    }
    let mut output = BTreeMap::new();
    walk(repo, root, &mut output)?;
    Ok(output)
}

fn stats_for(sizes: &BTreeMap<Oid, usize>, ids: impl IntoIterator<Item = Oid>) -> ObjectStats {
    let mut stats = ObjectStats::default();
    for id in ids {
        stats.objects += 1;
        stats.bytes += sizes.get(&id).copied().unwrap_or_default();
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Collection, Database};
    use serde_json::json;
    use tempfile::TempDir;

    fn point(id: impl Into<PointId>, vector: [f32; 2], topic: &str) -> Point {
        Point {
            id: id.into(),
            vector: vector.into(),
            payload: json!({"topic": topic}).as_object().unwrap().clone(),
        }
    }

    fn database() -> (TempDir, Database, Collection) {
        let temp = TempDir::new().unwrap();
        let db = Database::init(temp.path()).unwrap();
        let collection = db
            .create_collection(
                "notes",
                CollectionConfig {
                    dimension: 2,
                    ..CollectionConfig::default()
                },
            )
            .unwrap();
        (temp, db, collection)
    }

    #[test]
    fn point_operations_and_exact_scores() {
        let (_temp, _db, collection) = database();
        collection
            .upsert(vec![
                point("b", [1.0, 0.0], "rust"),
                point("a", [1.0, 0.0], "rust"),
                point(7_u64, [0.0, 1.0], "other"),
            ])
            .unwrap();
        let result = collection
            .query(Query {
                vector: vec![1.0, 0.0],
                limit: 3,
                with_payload: true,
                params: QueryParams {
                    exact: Some(true),
                    ..QueryParams::default()
                },
                ..Query::default()
            })
            .unwrap();
        assert_eq!(result.points[0].id, PointId::from("a"));
        assert_eq!(result.points[1].id, PointId::from("b"));
        assert_eq!(result.points[0].score, 1.0);
        assert_eq!(collection.count(None).unwrap().count, 3);
        assert!(collection.validate(true).unwrap().valid);
    }

    #[test]
    fn exact_top_k_selection_matches_full_sort_for_ties_and_typed_ids() {
        let points = vec![
            scored("b", 1.0),
            scored("a", 1.0),
            scored(7_u64, 1.0),
            scored("zero", 0.0),
            scored(8_u64, -0.0),
            scored("low", -1.0),
        ];
        for limit in 0..=points.len() + 1 {
            let mut expected = points.clone();
            sort_and_truncate(&mut expected, limit);
            let mut actual = points.clone();
            select_and_truncate(&mut actual, limit);
            assert_eq!(
                actual.iter().map(|point| &point.id).collect::<Vec<_>>(),
                expected.iter().map(|point| &point.id).collect::<Vec<_>>()
            );
            assert_eq!(
                actual
                    .iter()
                    .map(|point| point.score.to_bits())
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|point| point.score.to_bits())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn bounded_exact_search_preserves_ties_zero_vectors_and_filters() {
        let (_temp, _db, collection) = database();
        collection
            .upsert(vec![
                point("b", [1.0, 0.0], "keep"),
                point("a", [1.0, 0.0], "keep"),
                point(7_u64, [1.0, 0.0], "drop"),
                point(8_u64, [0.0, 0.0], "keep"),
            ])
            .unwrap();
        let filtered = collection
            .query(Query {
                vector: vec![1.0, 0.0],
                limit: 2,
                filter: Some(Filter::must([Condition::matches("topic", "keep")])),
                params: QueryParams {
                    exact: Some(true),
                    ..QueryParams::default()
                },
                ..Query::default()
            })
            .unwrap();
        assert_eq!(
            filtered
                .points
                .iter()
                .map(|point| &point.id)
                .collect::<Vec<_>>(),
            vec![&PointId::from("a"), &PointId::from("b")]
        );

        let zero = collection
            .query(Query {
                vector: vec![0.0, 0.0],
                limit: 10,
                params: QueryParams {
                    exact: Some(true),
                    ..QueryParams::default()
                },
                ..Query::default()
            })
            .unwrap();
        let mut expected = vec![
            PointId::from("a"),
            PointId::from("b"),
            PointId::from(7_u64),
            PointId::from(8_u64),
        ];
        expected.sort();
        assert_eq!(
            zero.points
                .iter()
                .map(|point| point.id.clone())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(zero.points.iter().all(|point| point.score == 0.0));

        let empty = collection
            .query(Query {
                vector: vec![1.0, 0.0],
                limit: 0,
                params: QueryParams {
                    exact: Some(true),
                    ..QueryParams::default()
                },
                ..Query::default()
            })
            .unwrap();
        assert!(empty.points.is_empty());
        assert_eq!(empty.stats.vectors_scored, 4);
    }

    #[test]
    fn exact_ranking_uses_f64_before_public_score_rounding() {
        let temp = TempDir::new().unwrap();
        let db = Database::init(temp.path()).unwrap();
        let collection = db
            .create_collection(
                "near-ties",
                CollectionConfig {
                    dimension: 3,
                    ..CollectionConfig::default()
                },
            )
            .unwrap();
        collection
            .upsert(vec![
                Point {
                    id: "z-higher".into(),
                    vector: vec![0.35979405, -0.9917066, 0.13241175],
                    payload: JsonObject::new(),
                },
                Point {
                    id: "a-lower".into(),
                    vector: vec![0.7231045, 0.25723642, -0.009842582],
                    payload: JsonObject::new(),
                },
            ])
            .unwrap();
        let result = collection
            .query(Query {
                vector: vec![0.731, -0.281, 0.619],
                limit: 2,
                params: QueryParams {
                    exact: Some(true),
                    ..QueryParams::default()
                },
                ..Query::default()
            })
            .unwrap();
        assert_eq!(result.points[0].id, PointId::from("z-higher"));
        assert_eq!(result.points[1].id, PointId::from("a-lower"));
        assert_eq!(
            result.points[0].score.to_bits(),
            result.points[1].score.to_bits()
        );
    }

    fn scored(id: impl Into<PointId>, score: f32) -> ScoredPoint {
        ScoredPoint {
            id: id.into(),
            score,
            payload: None,
            vector: None,
        }
    }

    #[test]
    fn roots_are_independent_of_input_order_and_repository_kind() {
        let temp_a = TempDir::new().unwrap();
        let temp_b = TempDir::new().unwrap();
        let db_a = Database::init(temp_a.path()).unwrap();
        let db_b = Database::init_bare(temp_b.path()).unwrap();
        let config = CollectionConfig {
            dimension: 2,
            ..CollectionConfig::default()
        };
        let a = db_a.create_collection("c", config.clone()).unwrap();
        let b = db_b.create_collection("c", config).unwrap();
        let one = point("one", [1.0, 0.0], "x");
        let two = point("two", [0.0, 1.0], "y");
        let root_a = a.upsert(vec![one.clone(), two.clone()]).unwrap().root;
        let root_b = b.upsert(vec![two, one]).unwrap().root;
        assert_eq!(root_a, root_b);
    }

    #[test]
    fn legacy_v1_roots_remain_readable_valid_and_mutable() {
        let temp = TempDir::new().unwrap();
        let repo = Repository::init_bare(temp.path()).unwrap();
        let config = CollectionConfig::new(2);
        let original = point("old", [1.0, 0.0], "v1");
        let points = BTreeMap::from([(original.id.clone(), original)]);
        let root = build_root_v1(&repo, &config, &points, &BTreeMap::new()).unwrap();
        assert_eq!(root.to_string(), "b02e98c1f919ec065ac342812a5ccf6b62bc4217");
        let meta = read_meta(&repo, root).unwrap();
        assert_eq!(meta.format_version, FORMAT_VERSION_V1);
        assert_eq!(read_all_points(&repo, root).unwrap().len(), 1);
        assert!(validate_root(&repo, root, true).unwrap().valid);
        drop(repo);

        let engine = SnapshotEngine::open(temp.path()).unwrap();
        assert_eq!(
            engine
                .open_snapshot(root.to_string())
                .unwrap()
                .info()
                .unwrap()
                .format_version,
            1
        );
        let next = engine
            .apply(
                root.to_string(),
                vec![SnapshotMutation::upsert(point(
                    "new",
                    [0.0, 1.0],
                    "still-v1",
                ))],
            )
            .unwrap();
        let repo = Repository::open_bare(temp.path()).unwrap();
        assert_eq!(read_meta(&repo, next.oid()).unwrap().format_version, 1);
        assert_eq!(next.info().unwrap().point_count, 2);
        assert!(next.validate(true).unwrap().valid);
    }

    #[test]
    fn historical_reads_and_compare_and_swap() {
        let (_temp, _db, collection) = database();
        let old = collection.root().unwrap();
        collection
            .upsert_expect(vec![point("a", [1.0, 0.0], "x")], Some(old.clone()))
            .unwrap();
        let historical = collection.at(&old).unwrap();
        assert_eq!(historical.count(None).unwrap().count, 0);
        assert!(matches!(
            collection.upsert_expect(vec![point("b", [0.0, 1.0], "x")], Some(old)),
            Err(Error::StaleRoot { .. })
        ));
    }

    #[test]
    fn named_query_cache_tracks_roots_across_clones_and_handles() {
        let (_temp, db, collection) = database();
        collection
            .upsert(vec![point("a", [1.0, 0.0], "first")])
            .unwrap();
        let exact = || Query {
            vector: vec![1.0, 0.0],
            limit: 10,
            params: QueryParams {
                exact: Some(true),
                ..QueryParams::default()
            },
            ..Query::default()
        };
        let approximate = || Query {
            vector: vec![1.0, 0.0],
            limit: 10,
            params: QueryParams {
                exact: Some(false),
                ..QueryParams::default()
            },
            ..Query::default()
        };
        assert_eq!(collection.query(exact()).unwrap().points.len(), 1);
        assert_eq!(
            collection.query(approximate()).unwrap().root,
            collection.root().unwrap()
        );

        collection
            .clone()
            .upsert(vec![point("b", [0.9, 0.1], "clone")])
            .unwrap();
        assert_eq!(collection.query(exact()).unwrap().points.len(), 2);
        assert_eq!(
            collection.query(approximate()).unwrap().root,
            collection.root().unwrap()
        );

        db.collection("notes")
            .unwrap()
            .upsert(vec![point("c", [0.8, 0.2], "other-handle")])
            .unwrap();
        assert_eq!(collection.query(exact()).unwrap().points.len(), 3);
        assert_eq!(
            collection.query(approximate()).unwrap().root,
            collection.root().unwrap()
        );
    }

    #[test]
    fn approximate_mode_scores_a_subset() {
        let (_temp, _db, collection) = database();
        let points = (0..100)
            .map(|id| point(id as u64, [id as f32 + 1.0, 1.0], "x"))
            .collect();
        collection.upsert(points).unwrap();
        let result = collection
            .query(Query {
                vector: vec![10.0, 1.0],
                params: QueryParams {
                    exact: Some(false),
                    probes: 1,
                    candidate_limit: 10,
                },
                ..Query::default()
            })
            .unwrap();
        assert_eq!(result.stats.mode, QueryMode::Approximate);
        assert!(result.stats.vectors_scored <= 10);
        assert!(result.stats.buckets_probed <= 1);
    }

    #[test]
    fn approximate_search_view_matches_uncached_results() {
        let (temp, _db, collection) = database();
        let points = (0..100)
            .map(|id| {
                point(
                    id as u64,
                    [id as f32 + 1.0, (id % 7) as f32 - 3.0],
                    if id % 2 == 0 { "even" } else { "odd" },
                )
            })
            .collect();
        collection.upsert(points).unwrap();
        let query = Query {
            vector: vec![10.0, 1.0],
            limit: 20,
            filter: Some(Filter::must([Condition::matches("topic", "even")])),
            with_payload: true,
            with_vector: true,
            params: QueryParams {
                exact: Some(false),
                probes: 8,
                candidate_limit: 75,
            },
            ..Query::default()
        };
        let repo = Repository::open(temp.path()).unwrap();
        let root = Oid::from_str(collection.root().unwrap().as_ref()).unwrap();
        let uncached = query_root_with_cache(&repo, root, query.clone(), None).unwrap();
        assert_eq!(uncached.points.len(), 20);
        assert_eq!(uncached.stats.vectors_scored, 50);
        let cold_cache = OnceLock::new();
        let cold_cached =
            query_root_with_cache(&repo, root, query.clone(), Some(&cold_cache)).unwrap();
        assert!(cold_cache.get().is_some());
        let cache = OnceLock::new();
        let mut exact_warmup = query.clone();
        exact_warmup.params.exact = Some(true);
        query_root_with_cache(&repo, root, exact_warmup, Some(&cache)).unwrap();
        let cached = query_root_with_cache(&repo, root, query.clone(), Some(&cache)).unwrap();
        let cached_again = query_root_with_cache(&repo, root, query, Some(&cache)).unwrap();
        assert_eq!(
            serde_json::to_value(&cold_cached).unwrap(),
            serde_json::to_value(&uncached).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&cached).unwrap(),
            serde_json::to_value(&uncached).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&cached_again).unwrap(),
            serde_json::to_value(&uncached).unwrap()
        );
    }

    #[test]
    fn lsh_signature_and_probe_order_golden() {
        let config = IndexConfig {
            tables: 2,
            signature_bits: 4,
            ..IndexConfig::default()
        };
        let vector = [1.0, -2.0, 0.5];
        assert_eq!(lsh_signature(&vector, &config, 0), 0x0);
        assert_eq!(lsh_signature(&vector, &config, 1), 0x4);
        assert_eq!(
            lsh_probe_sequence(&vector, &config, 6),
            vec![(0, 0), (1, 4), (0, 1), (0, 2), (0, 4), (0, 8)]
        );
    }
}
