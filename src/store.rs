use crate::codec::{
    canonical_json, decode_id, decode_payload, decode_vector, encode_id, encode_vector, id_hash,
    validate_vector_components,
};
use crate::filter::matches_filter;
use crate::*;
use git2::{Commit, ObjectType, Oid, Repository, Signature, Tree};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const FORMAT_VERSION: u32 = 1;
const TREE_MODE: i32 = 0o040000;
const BLOB_MODE: i32 = 0o100644;

type BucketEntries = BTreeMap<usize, BTreeMap<String, BTreeMap<String, Vec<(String, Oid)>>>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RootMeta {
    format_version: u32,
    point_count: usize,
    dimension: usize,
    distance: Distance,
    vector_space: Option<String>,
    vector_codec: String,
    git_object_format: String,
    index: IndexConfig,
}

impl RootMeta {
    fn new(config: &CollectionConfig, point_count: usize) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            point_count,
            dimension: config.dimension,
            distance: config.distance,
            vector_space: config.vector_space.clone(),
            vector_codec: "f32le-v1".into(),
            git_object_format: "sha1".into(),
            index: config.index.clone(),
        }
    }

    fn config(&self) -> CollectionConfig {
        CollectionConfig {
            dimension: self.dimension,
            distance: self.distance,
            vector_space: self.vector_space.clone(),
            index: self.index.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Snapshot {
    root: Oid,
    commit: Option<Oid>,
}

#[derive(Clone, Debug)]
struct StoredPoint {
    point: Point,
    tree: Oid,
}

#[derive(Clone, Debug)]
pub struct Database {
    path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Collection {
    db: Database,
    name: String,
    historical: Option<Snapshot>,
}

impl Database {
    pub fn init(path: impl AsRef<Path>) -> Result<Self> {
        Self::init_with_options(path, false)
    }

    pub fn init_bare(path: impl AsRef<Path>) -> Result<Self> {
        Self::init_with_options(path, true)
    }

    pub fn init_with_options(path: impl AsRef<Path>, bare: bool) -> Result<Self> {
        let path = path.as_ref();
        if bare {
            Repository::init_bare(path)?;
        } else {
            Repository::init(path)?;
        }
        Self::open(path)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let repository = Repository::open(path.as_ref())?;
        Ok(Self {
            path: repository.path().to_path_buf(),
        })
    }

    fn repo(&self) -> Result<Repository> {
        Ok(Repository::open(&self.path)?)
    }

    pub fn create_collection(
        &self,
        name: impl AsRef<str>,
        config: CollectionConfig,
    ) -> Result<Collection> {
        let name = name.as_ref();
        validate_collection_name(name)?;
        validate_config(&config)?;
        let repo = self.repo()?;
        let ref_name = collection_ref(name);
        if repo.find_reference(&ref_name).is_ok() {
            return Err(Error::CollectionExists(name.into()));
        }
        let root = build_root(&repo, &config, &BTreeMap::new())?;
        let commit = create_commit(&repo, root, None, &format!("create collection {name}"))?;
        repo.reference(&ref_name, commit, false, "git-vdb create collection")?;
        Ok(Collection {
            db: self.clone(),
            name: name.into(),
            historical: None,
        })
    }

    pub fn get_or_create_collection(
        &self,
        name: impl AsRef<str>,
        config: CollectionConfig,
    ) -> Result<Collection> {
        match self.collection(name.as_ref()) {
            Ok(collection) => {
                let actual = collection.info()?.config;
                if actual != config {
                    return Err(Error::Invalid(format!(
                        "collection exists with a different configuration: {actual:?}"
                    )));
                }
                Ok(collection)
            }
            Err(Error::CollectionNotFound(_)) => self.create_collection(name, config),
            Err(error) => Err(error),
        }
    }

    pub fn collection(&self, name: impl AsRef<str>) -> Result<Collection> {
        let name = name.as_ref();
        validate_collection_name(name)?;
        let repo = self.repo()?;
        if repo.find_reference(&collection_ref(name)).is_err() {
            return Err(Error::CollectionNotFound(name.into()));
        }
        Ok(Collection {
            db: self.clone(),
            name: name.into(),
            historical: None,
        })
    }

    pub fn list_collections(&self) -> Result<Vec<String>> {
        let repo = self.repo()?;
        let mut names = Vec::new();
        for reference in repo.references_glob("refs/git-vdb/collections/*")? {
            let reference = reference?;
            if let Some(name) = reference.name()?.strip_prefix("refs/git-vdb/collections/") {
                names.push(name.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn delete_collection(&self, name: impl AsRef<str>) -> Result<ObjectId> {
        let collection = self.collection(name.as_ref())?;
        let root = collection.root()?;
        let repo = self.repo()?;
        repo.find_reference(&collection_ref(name.as_ref()))?
            .delete()?;
        Ok(root)
    }
}

impl Collection {
    fn repo(&self) -> Result<Repository> {
        self.db.repo()
    }

    fn snapshot(&self, repo: &Repository) -> Result<Snapshot> {
        if let Some(snapshot) = self.historical {
            return Ok(snapshot);
        }
        current_snapshot(repo, &self.name)
    }

    pub fn root(&self) -> Result<ObjectId> {
        let repo = self.repo()?;
        Ok(self.snapshot(&repo)?.root.into())
    }

    pub fn at(&self, revision: impl AsRef<str>) -> Result<Self> {
        let repo = self.repo()?;
        let snapshot = resolve_snapshot(&repo, revision.as_ref())?;
        read_meta(&repo, snapshot.root)?;
        Ok(Self {
            db: self.db.clone(),
            name: self.name.clone(),
            historical: Some(snapshot),
        })
    }

    pub fn info(&self) -> Result<CollectionInfo> {
        let repo = self.repo()?;
        let snapshot = self.snapshot(&repo)?;
        let meta = read_meta(&repo, snapshot.root)?;
        Ok(CollectionInfo {
            root: snapshot.root.into(),
            name: self.name.clone(),
            point_count: meta.point_count,
            config: meta.config(),
            read_only: self.historical.is_some(),
        })
    }

    pub fn upsert(&self, points: Vec<Point>) -> Result<WriteResult> {
        self.upsert_expect(points, None)
    }

    pub fn upsert_expect(
        &self,
        points: Vec<Point>,
        expected_root: Option<ObjectId>,
    ) -> Result<WriteResult> {
        if self.historical.is_some() {
            return Err(Error::ReadOnly);
        }
        if points.is_empty() {
            return Err(Error::Invalid("upsert batch must not be empty".into()));
        }
        let repo = self.repo()?;
        let snapshot = current_snapshot(&repo, &self.name)?;
        check_expected_root(snapshot.root, expected_root.as_ref())?;
        let meta = read_meta(&repo, snapshot.root)?;
        let config = meta.config();
        let mut existing = read_all_points(&repo, snapshot.root)?;
        let mut batch_ids = BTreeSet::new();
        for point in &points {
            validate_point(point, &config)?;
            if !batch_ids.insert(point.id.clone()) {
                return Err(Error::Invalid(format!(
                    "upsert batch contains duplicate point ID {}",
                    point.id
                )));
            }
        }
        let affected_points = points.len();
        for point in points {
            existing.insert(point.id.clone(), point);
        }
        let root = build_root(&repo, &config, &existing)?;
        advance_collection(
            &repo,
            &self.name,
            snapshot,
            root,
            &format!("upsert {affected_points} points"),
        )?;
        Ok(WriteResult {
            root: root.into(),
            affected_points,
        })
    }

    pub fn delete(&self, selector: DeleteSelector) -> Result<WriteResult> {
        self.delete_expect(selector, None)
    }

    pub fn delete_expect(
        &self,
        selector: DeleteSelector,
        expected_root: Option<ObjectId>,
    ) -> Result<WriteResult> {
        if self.historical.is_some() {
            return Err(Error::ReadOnly);
        }
        if selector.ids.is_empty() && selector.filter.is_none() {
            return Err(Error::Invalid("delete selector must not be empty".into()));
        }
        let repo = self.repo()?;
        let snapshot = current_snapshot(&repo, &self.name)?;
        check_expected_root(snapshot.root, expected_root.as_ref())?;
        let config = read_meta(&repo, snapshot.root)?.config();
        let mut existing = read_all_points(&repo, snapshot.root)?;
        let ids: BTreeSet<_> = selector.ids.into_iter().collect();
        let before = existing.len();
        existing.retain(|id, point| {
            let id_match = ids.contains(id);
            let filter_match = selector
                .filter
                .as_ref()
                .is_some_and(|filter| matches_filter(filter, id, &point.payload));
            !(id_match || filter_match)
        });
        let affected_points = before - existing.len();
        let root = build_root(&repo, &config, &existing)?;
        advance_collection(
            &repo,
            &self.name,
            snapshot,
            root,
            &format!("delete {affected_points} points"),
        )?;
        Ok(WriteResult {
            root: root.into(),
            affected_points,
        })
    }

    pub fn get(&self, request: GetRequest) -> Result<GetResult> {
        let repo = self.repo()?;
        let snapshot = self.snapshot(&repo)?;
        let points = read_all_points(&repo, snapshot.root)?;
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
            root: snapshot.root.into(),
            points: records,
        })
    }

    pub fn count(&self, filter: Option<Filter>) -> Result<CountResult> {
        let repo = self.repo()?;
        let snapshot = self.snapshot(&repo)?;
        let meta = read_meta(&repo, snapshot.root)?;
        let count = if let Some(filter) = filter {
            read_all_points(&repo, snapshot.root)?
                .values()
                .filter(|point| matches_filter(&filter, &point.id, &point.payload))
                .count()
        } else {
            meta.point_count
        };
        Ok(CountResult {
            root: snapshot.root.into(),
            count,
        })
    }

    pub fn query(&self, query: Query) -> Result<QueryResult> {
        let repo = self.repo()?;
        let snapshot = self.snapshot(&repo)?;
        let meta = read_meta(&repo, snapshot.root)?;
        validate_query(&query, &meta)?;
        let exact = query
            .params
            .exact
            .unwrap_or(meta.point_count <= meta.index.full_scan_threshold);
        if exact {
            exact_query(&repo, snapshot.root, &meta, query)
        } else {
            approximate_query(&repo, snapshot.root, &meta, query)
        }
    }

    pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let repo = self.repo()?;
        let mut commit_id = self
            .snapshot(&repo)?
            .commit
            .ok_or_else(|| Error::Invalid("a root tree has no commit history".into()))?;
        let mut history = Vec::new();
        while history.len() < limit {
            let commit = repo.find_commit(commit_id)?;
            let parent = commit.parent_id(0).ok();
            history.push(HistoryEntry {
                commit: commit.id().into(),
                root: commit.tree_id().into(),
                parent: parent.map(Into::into),
                message: commit.message().unwrap_or_default().to_owned(),
                time_seconds: commit.time().seconds(),
            });
            let Some(parent) = parent else { break };
            commit_id = parent;
        }
        Ok(history)
    }

    pub fn diff(
        &self,
        left_revision: impl AsRef<str>,
        right_revision: impl AsRef<str>,
    ) -> Result<DiffResult> {
        let repo = self.repo()?;
        let left = resolve_snapshot(&repo, left_revision.as_ref())?;
        let right = resolve_snapshot(&repo, right_revision.as_ref())?;
        let left_points = read_stored_points(&repo, left.root)?;
        let right_points = read_stored_points(&repo, right.root)?;
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut changed = Vec::new();
        for (id, point) in &left_points {
            match right_points.get(id) {
                None => removed.push(id.clone()),
                Some(right_point) if right_point.tree != point.tree => changed.push(id.clone()),
                _ => {}
            }
        }
        for id in right_points.keys() {
            if !left_points.contains_key(id) {
                added.push(id.clone());
            }
        }
        let left_meta = read_meta(&repo, left.root)?;
        let right_meta = read_meta(&repo, right.root)?;
        let left_index = root_entry_oid(&repo, left.root, "index")?;
        let right_index = root_entry_oid(&repo, right.root, "index")?;
        let left_objects = object_sizes(&repo, left.root)?;
        let right_objects = object_sizes(&repo, right.root)?;
        let shared_ids: BTreeSet<_> = left_objects
            .keys()
            .filter(|id| right_objects.contains_key(id))
            .copied()
            .collect();
        Ok(DiffResult {
            left_root: left.root.into(),
            right_root: right.root.into(),
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

    pub fn validate(&self, full: bool) -> Result<ValidationReport> {
        let repo = self.repo()?;
        let snapshot = self.snapshot(&repo)?;
        let meta = read_meta(&repo, snapshot.root)?;
        validate_config(&meta.config()).map_err(|error| Error::Corrupt(error.to_string()))?;
        let points = read_stored_points(&repo, snapshot.root)?;
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
            let actual = read_bucket_entries(&repo, snapshot.root, &meta.index)?;
            checked_buckets = actual.len();
            if expected != actual {
                return Err(Error::Corrupt(
                    "LSH bucket entries do not match authoritative points".into(),
                ));
            }
        }
        Ok(ValidationReport {
            root: snapshot.root.into(),
            full,
            point_count: points.len(),
            checked_buckets,
            valid: true,
        })
    }
}

fn validate_collection_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || name.starts_with('.')
        || name.ends_with('.')
        || name.contains("..")
    {
        return Err(Error::Invalid(format!("invalid collection name {name:?}")));
    }
    Ok(())
}

fn validate_config(config: &CollectionConfig) -> Result<()> {
    if config.dimension == 0 || config.dimension > u32::MAX as usize {
        return Err(Error::Invalid(
            "dimension must be between 1 and u32::MAX".into(),
        ));
    }
    if config.index.tables == 0
        || config.index.signature_bits == 0
        || config.index.signature_bits > 64
        || config.index.default_probes == 0
        || config.index.default_candidate_limit == 0
    {
        return Err(Error::Invalid(
            "index tables, signature bits, probes, and candidate limit must be positive; signature bits must not exceed 64".into(),
        ));
    }
    Ok(())
}

fn validate_point(point: &Point, config: &CollectionConfig) -> Result<()> {
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

fn collection_ref(name: &str) -> String {
    format!("refs/git-vdb/collections/{name}")
}

fn current_snapshot(repo: &Repository, name: &str) -> Result<Snapshot> {
    let reference = repo
        .find_reference(&collection_ref(name))
        .map_err(|_| Error::CollectionNotFound(name.into()))?;
    let commit = reference.peel_to_commit()?;
    Ok(Snapshot {
        root: commit.tree_id(),
        commit: Some(commit.id()),
    })
}

fn resolve_snapshot(repo: &Repository, revision: &str) -> Result<Snapshot> {
    let object = repo.revparse_single(revision)?;
    match object.kind() {
        Some(ObjectType::Commit) => {
            let commit = object.peel_to_commit()?;
            Ok(Snapshot {
                root: commit.tree_id(),
                commit: Some(commit.id()),
            })
        }
        Some(ObjectType::Tree) => Ok(Snapshot {
            root: object.id(),
            commit: None,
        }),
        _ => Err(Error::Invalid(format!(
            "revision {revision:?} is not a commit or tree"
        ))),
    }
}

fn check_expected_root(actual: Oid, expected: Option<&ObjectId>) -> Result<()> {
    if let Some(expected) = expected {
        if expected.0 != actual.to_string() {
            return Err(Error::StaleRoot {
                expected: expected.clone(),
                actual: actual.into(),
            });
        }
    }
    Ok(())
}

fn signature() -> Result<Signature<'static>> {
    Ok(Signature::now("git-vdb", "git-vdb@localhost")?)
}

fn create_commit(
    repo: &Repository,
    root: Oid,
    parent: Option<&Commit<'_>>,
    message: &str,
) -> Result<Oid> {
    let tree = repo.find_tree(root)?;
    let signature = signature()?;
    let parents: Vec<&Commit<'_>> = parent.into_iter().collect();
    Ok(repo.commit(None, &signature, &signature, message, &tree, &parents)?)
}

fn advance_collection(
    repo: &Repository,
    name: &str,
    old: Snapshot,
    root: Oid,
    message: &str,
) -> Result<()> {
    let old_commit_id = old
        .commit
        .ok_or_else(|| Error::Corrupt("current collection ref is not a commit".into()))?;
    let old_commit = repo.find_commit(old_commit_id)?;
    let new_commit = create_commit(repo, root, Some(&old_commit), message)?;
    repo.reference_matching(
        &collection_ref(name),
        new_commit,
        true,
        old_commit_id,
        "git-vdb atomic collection update",
    )
    .map_err(|error| {
        if error.code() == git2::ErrorCode::Modified {
            let actual = current_snapshot(repo, name)
                .map(|snapshot| snapshot.root.into())
                .unwrap_or_else(|_| ObjectId("unknown".into()));
            Error::StaleRoot {
                expected: old.root.into(),
                actual,
            }
        } else {
            Error::Git(error)
        }
    })?;
    Ok(())
}

fn build_root(
    repo: &Repository,
    config: &CollectionConfig,
    points: &BTreeMap<PointId, Point>,
) -> Result<Oid> {
    let projections = lsh_projections(&config.index, config.dimension);
    let mut point_entries: BTreeMap<String, Vec<(String, Oid)>> = BTreeMap::new();
    let mut bucket_entries: BucketEntries = BTreeMap::new();
    for point in points.values() {
        validate_point(point, config)?;
        let hash = id_hash(&point.id);
        let point_tree = write_point_tree(repo, point)?;
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
    let meta_blob = repo.blob(&canonical_json(&RootMeta::new(config, points.len()))?)?;
    let mut root = repo.treebuilder(None)?;
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

fn read_meta(repo: &Repository, root: Oid) -> Result<RootMeta> {
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
    if meta.format_version != FORMAT_VERSION
        || meta.vector_codec != "f32le-v1"
        || meta.git_object_format != "sha1"
    {
        return Err(Error::Corrupt(format!(
            "unsupported format metadata: version {}, vector codec {}, object format {}",
            meta.format_version, meta.vector_codec, meta.git_object_format
        )));
    }
    Ok(meta)
}

fn read_all_points(repo: &Repository, root: Oid) -> Result<BTreeMap<PointId, Point>> {
    Ok(read_stored_points(repo, root)?
        .into_iter()
        .map(|(id, stored)| (id, stored.point))
        .collect())
}

fn read_stored_points(repo: &Repository, root: Oid) -> Result<BTreeMap<PointId, StoredPoint>> {
    let points_oid = root_entry_oid(repo, root, "points")?;
    let points_tree = repo.find_tree(points_oid)?;
    let mut result = BTreeMap::new();
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
            let point = read_point_tree(repo, point_entry.id())?;
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
                        tree: point_entry.id(),
                    },
                )
                .is_some()
            {
                return Err(Error::Corrupt("duplicate typed point ID".into()));
            }
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
    let points = read_all_points(repo, root)?;
    let mut scored = Vec::new();
    let mut vectors_scored = 0;
    for point in points.values() {
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
            payload: query.with_payload.then_some(point.payload.clone()),
            vector: query.with_vector.then_some(point.vector.clone()),
        });
    }
    sort_and_truncate(&mut scored, query.limit);
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
    points.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.id.cmp(&right.id))
    });
    points.truncate(limit);
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
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
        (dot / (left_norm.sqrt() * right_norm.sqrt())) as f32
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
