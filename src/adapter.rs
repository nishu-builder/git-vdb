use crate::root::{
    count_root, diff_roots, get_root, query_root_with_cache, read_meta, validate_config,
    validate_root, SearchView,
};
use crate::*;
use git2::{Commit, Oid, Repository, Signature};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, Debug)]
struct ResolvedSnapshot {
    root: Oid,
    commit: Option<Oid>,
}

/// A Git repository that manages named, mutable collection refs.
#[derive(Clone, Debug)]
pub struct Database {
    pub(crate) path: PathBuf,
}

/// A named collection view backed by the Git commit/ref adapter.
#[derive(Clone, Debug)]
pub struct Collection {
    db: Database,
    name: String,
    historical: Option<ResolvedSnapshot>,
    query_cache: Arc<Mutex<CollectionQueryCache>>,
}

#[derive(Debug)]
struct CollectionQueryCache {
    root: Option<Oid>,
    points: Arc<OnceLock<SearchView>>,
}

impl Default for CollectionQueryCache {
    fn default() -> Self {
        Self {
            root: None,
            points: Arc::new(OnceLock::new()),
        }
    }
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
        let root = SnapshotEngine::open(&self.path)?
            .build(config, Vec::new())?
            .oid();
        let commit = create_commit(&repo, root, None, &format!("create collection {name}"))?;
        repo.reference(&ref_name, commit, false, "git-vdb create collection")?;
        Ok(Collection {
            db: self.clone(),
            name: name.into(),
            historical: None,
            query_cache: Default::default(),
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
            query_cache: Default::default(),
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

    fn snapshot(&self, repo: &Repository) -> Result<ResolvedSnapshot> {
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
            query_cache: Default::default(),
        })
    }

    pub fn info(&self) -> Result<CollectionInfo> {
        let repo = self.repo()?;
        let snapshot = self.snapshot(&repo)?;
        let meta = read_meta(&repo, snapshot.root)?;
        Ok(CollectionInfo {
            root: snapshot.root.into(),
            name: self.name.clone(),
            point_count: meta.point_count(),
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
        let affected_points = points.len();
        let mutations = points.into_iter().map(SnapshotMutation::upsert).collect();
        let root = SnapshotEngine::open(&self.db.path)?
            .apply(snapshot.root.to_string(), mutations)?
            .oid();
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
        let before = read_meta(&repo, snapshot.root)?.point_count();
        let mut mutations = Vec::new();
        if !selector.ids.is_empty() {
            mutations.push(SnapshotMutation::delete_ids(selector.ids));
        }
        if let Some(filter) = selector.filter {
            mutations.push(SnapshotMutation::delete_filter(filter));
        }
        let new_snapshot =
            SnapshotEngine::open(&self.db.path)?.apply(snapshot.root.to_string(), mutations)?;
        let root = new_snapshot.oid();
        let affected_points = before - new_snapshot.info()?.point_count;
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
        get_root(&repo, snapshot.root, request)
    }

    pub fn count(&self, filter: Option<Filter>) -> Result<CountResult> {
        let repo = self.repo()?;
        let snapshot = self.snapshot(&repo)?;
        count_root(&repo, snapshot.root, filter)
    }

    pub fn query(&self, query: Query) -> Result<QueryResult> {
        let repo = self.repo()?;
        let snapshot = self.snapshot(&repo)?;
        let points = {
            let mut cache = self
                .query_cache
                .lock()
                .map_err(|_| Error::Invalid("collection query cache lock is poisoned".into()))?;
            if cache.root != Some(snapshot.root) {
                cache.root = Some(snapshot.root);
                cache.points = Arc::new(OnceLock::new());
            }
            cache.points.clone()
        };
        query_root_with_cache(&repo, snapshot.root, query, Some(&points))
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
        diff_roots(&repo, left.root, right.root)
    }

    pub fn validate(&self, full: bool) -> Result<ValidationReport> {
        let repo = self.repo()?;
        let snapshot = self.snapshot(&repo)?;
        validate_root(&repo, snapshot.root, full)
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

fn collection_ref(name: &str) -> String {
    format!("refs/git-vdb/collections/{name}")
}

fn current_snapshot(repo: &Repository, name: &str) -> Result<ResolvedSnapshot> {
    let reference = repo
        .find_reference(&collection_ref(name))
        .map_err(|_| Error::CollectionNotFound(name.into()))?;
    let commit = reference.peel_to_commit()?;
    Ok(ResolvedSnapshot {
        root: commit.tree_id(),
        commit: Some(commit.id()),
    })
}

fn resolve_snapshot(repo: &Repository, revision: &str) -> Result<ResolvedSnapshot> {
    let object = repo.revparse_single(revision)?;
    match object.kind() {
        Some(git2::ObjectType::Commit) => {
            let commit = object.peel_to_commit()?;
            Ok(ResolvedSnapshot {
                root: commit.tree_id(),
                commit: Some(commit.id()),
            })
        }
        Some(git2::ObjectType::Tree) => Ok(ResolvedSnapshot {
            root: object.id(),
            commit: None,
        }),
        _ => Err(Error::Invalid("revision is not a commit or tree".into())),
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
    old: ResolvedSnapshot,
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
