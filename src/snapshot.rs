use crate::filter::matches_filter;
use crate::root::{
    build_root, build_root_reusing, count_root, get_root, query_root_with_cache, read_meta,
    read_stored_points, validate_config, validate_point, validate_root,
};
use crate::{
    CollectionConfig, CountResult, Error, GetRequest, GetResult, ObjectId, Point, PointId, Query,
    QueryResult, Result, SnapshotInfo, SnapshotMutation, ValidationReport,
};
use git2::{ObjectType, Oid, Repository};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tempfile::TempDir;

const TREE_MODE: i32 = 0o040000;
const BLOB_MODE: i32 = 0o100644;

/// A ref-free engine for deterministic immutable collection roots.
///
/// The engine writes and reads Git objects but never creates a commit, updates a
/// ref, consults repository history, or reads the clock. Callers are responsible
/// for retaining returned roots, for example through an external content store
/// or the named collection adapter.
#[derive(Clone)]
pub struct SnapshotEngine {
    object_database: PathBuf,
    temporary: Option<Arc<TempDir>>,
}

/// An immutable collection root opened independently of collection refs.
#[derive(Clone)]
pub struct Snapshot {
    object_database: PathBuf,
    root: Oid,
    points: Arc<OnceLock<Vec<Point>>>,
    temporary: Option<Arc<TempDir>>,
}

impl fmt::Debug for SnapshotEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotEngine")
            .field("object_database", &self.object_database)
            .field("temporary", &self.temporary.is_some())
            .finish()
    }
}

impl fmt::Debug for Snapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Snapshot")
            .field("root", &self.root)
            .field("object_database", &self.object_database)
            .field("temporary", &self.temporary.is_some())
            .finish()
    }
}

impl SnapshotEngine {
    /// Opens an existing bare or non-bare Git object database without reading a
    /// collection ref.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let repository = Repository::open(path)?;
        Ok(Self {
            object_database: repository.path().to_path_buf(),
            temporary: None,
        })
    }

    /// Initializes a bare Git object database for immutable snapshots.
    pub fn init(path: impl AsRef<Path>) -> Result<Self> {
        let repository = Repository::init_bare(path)?;
        Ok(Self {
            object_database: repository.path().to_path_buf(),
            temporary: None,
        })
    }

    /// Creates an isolated temporary object database whose lifetime is retained
    /// by the engine and snapshots returned from it.
    pub fn ephemeral() -> Result<Self> {
        let temporary = Arc::new(TempDir::new()?);
        let repository = Repository::init_bare(temporary.path())?;
        Ok(Self {
            object_database: repository.path().to_path_buf(),
            temporary: Some(temporary),
        })
    }

    fn repo(&self) -> Result<Repository> {
        Ok(Repository::open(&self.object_database)?)
    }

    /// Builds a canonical root from a complete point set.
    pub fn build(&self, config: CollectionConfig, points: Vec<Point>) -> Result<Snapshot> {
        validate_config(&config)?;
        let points = canonical_point_set(points, &config)?;
        let repo = self.repo()?;
        let root = build_root(&repo, &config, &points)?;
        Ok(self.snapshot(root, Some(points)))
    }

    /// Applies an ordered mutation batch to a root and returns the new root.
    ///
    /// Mutation order is significant. Duplicate upserts for the same typed ID
    /// within one call are rejected. No ref or commit is created.
    pub fn apply(
        &self,
        previous_root: impl AsRef<str>,
        mutations: Vec<SnapshotMutation>,
    ) -> Result<Snapshot> {
        if mutations.is_empty() {
            return Err(Error::Invalid(
                "snapshot mutation batch must not be empty".into(),
            ));
        }
        let repo = self.repo()?;
        let previous_root = exact_root(&repo, previous_root.as_ref())?;
        let config = read_meta(&repo, previous_root)?.config();
        let stored_points = read_stored_points(&repo, previous_root)?;
        let mut points = BTreeMap::new();
        let mut reusable_point_trees = BTreeMap::new();
        for (id, stored) in stored_points {
            reusable_point_trees.insert(id.clone(), stored.tree);
            points.insert(id, stored.point);
        }
        let mut upsert_ids = BTreeSet::new();

        for mutation in mutations {
            match mutation {
                SnapshotMutation::Upsert { point } => {
                    validate_point(&point, &config)?;
                    if !upsert_ids.insert(point.id.clone()) {
                        return Err(Error::Invalid(format!(
                            "snapshot mutation batch contains duplicate upsert ID {}",
                            point.id
                        )));
                    }
                    reusable_point_trees.remove(&point.id);
                    points.insert(point.id.clone(), point);
                }
                SnapshotMutation::DeleteIds { ids } => {
                    if ids.is_empty() {
                        return Err(Error::Invalid(
                            "snapshot delete_ids mutation must not be empty".into(),
                        ));
                    }
                    for id in ids {
                        points.remove(&id);
                    }
                }
                SnapshotMutation::DeleteFilter { filter } => {
                    points.retain(|id, point| !matches_filter(&filter, id, &point.payload));
                }
            }
        }

        let root = build_root_reusing(&repo, &config, &points, &reusable_point_trees)?;
        Ok(self.snapshot(root, Some(points)))
    }

    /// Opens an exact tree object ID without resolving refs or commits.
    pub fn open_snapshot(&self, root: impl AsRef<str>) -> Result<Snapshot> {
        let repo = self.repo()?;
        let root = exact_root(&repo, root.as_ref())?;
        read_meta(&repo, root)?;
        Ok(self.snapshot(root, None))
    }

    /// Imports a materialized canonical tree into this engine's object database.
    pub fn import_directory(&self, path: impl AsRef<Path>) -> Result<Snapshot> {
        let path = path.as_ref();
        if !path.is_dir() {
            return Err(Error::Invalid(format!(
                "materialized snapshot is not a directory: {}",
                path.display()
            )));
        }
        let repo = self.repo()?;
        let root = import_directory(&repo, path)?;
        read_meta(&repo, root)?;
        Ok(self.snapshot(root, None))
    }

    /// Queries an exact root ID without first constructing a named collection.
    pub fn query(&self, root: impl AsRef<str>, query: Query) -> Result<QueryResult> {
        let repo = self.repo()?;
        let root = exact_root(&repo, root.as_ref())?;
        read_meta(&repo, root)?;
        query_root_with_cache(&repo, root, query, None)
    }

    /// Retrieves records from an exact root ID.
    pub fn get(&self, root: impl AsRef<str>, request: GetRequest) -> Result<GetResult> {
        self.open_snapshot(root)?.get(request)
    }

    /// Counts records in an exact root ID.
    pub fn count(
        &self,
        root: impl AsRef<str>,
        filter: Option<crate::Filter>,
    ) -> Result<CountResult> {
        self.open_snapshot(root)?.count(filter)
    }

    /// Validates an exact root ID.
    pub fn validate(&self, root: impl AsRef<str>, full: bool) -> Result<ValidationReport> {
        self.open_snapshot(root)?.validate(full)
    }

    /// Builds a snapshot without a caller-provided Git repository and writes its
    /// canonical files into a new materialized directory.
    pub fn build_directory(
        path: impl AsRef<Path>,
        config: CollectionConfig,
        points: Vec<Point>,
    ) -> Result<Snapshot> {
        let engine = Self::ephemeral()?;
        let snapshot = engine.build(config, points)?;
        snapshot.materialize(path.as_ref())?;
        Snapshot::open_directory(path)
    }

    fn snapshot(&self, root: Oid, points: Option<BTreeMap<PointId, Point>>) -> Snapshot {
        let cache = OnceLock::new();
        if let Some(points) = points {
            cache
                .set(points.into_values().collect())
                .expect("new snapshot point cache must be empty");
        }
        Snapshot {
            object_database: self.object_database.clone(),
            root,
            points: Arc::new(cache),
            temporary: self.temporary.clone(),
        }
    }
}

impl Snapshot {
    /// Imports a materialized canonical tree into an isolated temporary object
    /// database and opens the computed root. The source directory is not changed.
    pub fn open_directory(path: impl AsRef<Path>) -> Result<Self> {
        let engine = SnapshotEngine::ephemeral()?;
        engine.import_directory(path)
    }

    fn repo(&self) -> Result<Repository> {
        Ok(Repository::open(&self.object_database)?)
    }

    /// Returns the deterministic Git tree ID that identifies this snapshot.
    pub fn root(&self) -> ObjectId {
        self.root.into()
    }

    /// Returns configuration and point-count metadata for this root.
    pub fn info(&self) -> Result<SnapshotInfo> {
        let repo = self.repo()?;
        let meta = read_meta(&repo, self.root)?;
        let point_count = count_root(&repo, self.root, None)?.count;
        Ok(SnapshotInfo {
            root: self.root(),
            point_count,
            config: meta.config(),
        })
    }

    pub fn get(&self, request: GetRequest) -> Result<GetResult> {
        get_root(&self.repo()?, self.root, request)
    }

    pub fn count(&self, filter: Option<crate::Filter>) -> Result<CountResult> {
        count_root(&self.repo()?, self.root, filter)
    }

    pub fn query(&self, query: Query) -> Result<QueryResult> {
        let repo = self.repo()?;
        query_root_with_cache(&repo, self.root, query, Some(&self.points))
    }

    /// Applies mutations using this snapshot's object database without creating
    /// collection history or refs.
    pub fn apply(&self, mutations: Vec<SnapshotMutation>) -> Result<Snapshot> {
        SnapshotEngine {
            object_database: self.object_database.clone(),
            temporary: self.temporary.clone(),
        }
        .apply(self.root.to_string(), mutations)
    }

    pub fn validate(&self, full: bool) -> Result<ValidationReport> {
        validate_root(&self.repo()?, self.root, full)
    }

    /// Writes this exact Git tree as ordinary files and directories.
    ///
    /// The target must not already exist. A sibling staging directory is renamed
    /// into place only after every object has been read successfully.
    pub fn materialize(&self, target: impl AsRef<Path>) -> Result<()> {
        let target = target.as_ref();
        if target.exists() {
            return Err(Error::Invalid(format!(
                "materialization target already exists: {}",
                target.display()
            )));
        }
        let parent = target
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let staging = tempfile::Builder::new()
            .prefix(".git-vdb-snapshot-")
            .tempdir_in(parent)?;
        materialize_tree(&self.repo()?, self.root, staging.path())?;
        fs::rename(staging.path(), target).map_err(|error| {
            Error::Invalid(format!(
                "cannot publish materialized snapshot {} as {}: {error}",
                staging.path().display(),
                target.display()
            ))
        })?;
        Ok(())
    }

    pub(crate) fn oid(&self) -> Oid {
        self.root
    }
}

fn canonical_point_set(
    points: Vec<Point>,
    config: &CollectionConfig,
) -> Result<BTreeMap<PointId, Point>> {
    let mut canonical = BTreeMap::new();
    for point in points {
        validate_point(&point, config)?;
        if canonical.insert(point.id.clone(), point).is_some() {
            return Err(Error::Invalid(
                "snapshot build contains a duplicate typed point ID".into(),
            ));
        }
    }
    Ok(canonical)
}

fn exact_root(repo: &Repository, root: &str) -> Result<Oid> {
    let oid = Oid::from_str(root)
        .map_err(|_| Error::Invalid(format!("invalid snapshot root object ID {root:?}")))?;
    repo.find_tree(oid)
        .map_err(|_| Error::Invalid(format!("snapshot root {root} is not a tree object")))?;
    Ok(oid)
}

fn materialize_tree(repo: &Repository, tree_oid: Oid, path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    let tree = repo.find_tree(tree_oid)?;
    for entry in &tree {
        let name = entry
            .name()
            .map_err(|_| Error::Corrupt("Git tree entry name is not UTF-8".into()))?;
        validate_tree_name(name)?;
        let destination = path.join(name);
        match entry.kind() {
            Some(ObjectType::Tree) => materialize_tree(repo, entry.id(), &destination)?,
            Some(ObjectType::Blob) => {
                fs::write(destination, repo.find_blob(entry.id())?.content())?
            }
            kind => {
                return Err(Error::Corrupt(format!(
                    "unsupported object kind {kind:?} in snapshot tree"
                )));
            }
        }
    }
    Ok(())
}

fn validate_tree_name(name: &str) -> Result<()> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\']) {
        return Err(Error::Corrupt(format!(
            "unsafe path name in snapshot tree: {name:?}"
        )));
    }
    Ok(())
}

fn import_directory(repo: &Repository, path: &Path) -> Result<Oid> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::result::Result<Vec<PathBuf>, std::io::Error>>()?;
    entries.sort_by(|left, right| left.file_name().cmp(&right.file_name()));

    let mut tree = repo.treebuilder(None)?;
    for path in entries {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error::Invalid("snapshot path names must be valid UTF-8".into()))?;
        let file_type = fs::symlink_metadata(&path)?.file_type();
        if file_type.is_symlink() {
            return Err(Error::Invalid(format!(
                "materialized snapshots cannot contain symlinks: {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            tree.insert(name, import_directory(repo, &path)?, TREE_MODE)?;
        } else if file_type.is_file() {
            tree.insert(name, repo.blob(&fs::read(&path)?)?, BLOB_MODE)?;
        } else {
            return Err(Error::Invalid(format!(
                "unsupported materialized snapshot entry: {}",
                path.display()
            )));
        }
    }
    Ok(tree.write()?)
}
