//! A small persistent facade for common embedded vector-database operations.

use crate::{
    Collection, CollectionConfig, Database, DeleteSelector, Error, Filter, GetRequest, GetResult,
    MutationResult, ObjectId, Point, PointId, Query, QueryResult, Record, Result, ScoredPoint,
    SnapshotMutation, WriteResult,
};
use git2::ErrorCode;
use std::fs;
use std::path::Path;
use std::thread;

const WRITE_ATTEMPTS: usize = 8;

/// Opens or creates a persistent embedded vector database.
///
/// Existing bare and non-bare Git repositories are opened as-is. A missing path
/// or existing empty directory is initialized as a bare repository. Existing
/// nonempty directories that are not Git repositories are rejected.
pub fn open(path: impl AsRef<Path>) -> Result<Store> {
    Store::open(path)
}

/// A persistent embedded vector database with lazy collection handles.
#[derive(Clone, Debug)]
pub struct Store {
    database: Database,
}

/// A named collection that is created with an inferred dimension on first use.
#[derive(Clone, Debug)]
pub struct CollectionHandle {
    database: Database,
    name: String,
}

impl Store {
    /// Opens or safely creates a database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let database = match Database::open(path) {
            Ok(database) => database,
            Err(_) if !path.exists() => Database::init_bare(path)?,
            Err(_) if path.is_dir() && directory_is_empty(path)? => Database::init_bare(path)?,
            Err(open_error) if path.is_dir() => {
                return Err(Error::Invalid(format!(
                "database path {} is a nonempty directory but not a Git repository: {open_error}",
                path.display()
            )))
            }
            Err(open_error) => return Err(open_error),
        };
        Ok(Self { database })
    }

    /// Returns a lazy handle for a named collection.
    ///
    /// This method performs no repository write. A missing collection is
    /// created by its first nonempty [`CollectionHandle::upsert`].
    pub fn collection(&self, name: impl Into<String>) -> CollectionHandle {
        CollectionHandle {
            database: self.database.clone(),
            name: name.into(),
        }
    }

    /// Returns collection names in canonical ascending order.
    pub fn list_collections(&self) -> Result<Vec<String>> {
        self.database.list_collections()
    }

    /// Returns the advanced named-database API.
    pub fn advanced(&self) -> &Database {
        &self.database
    }
}

impl CollectionHandle {
    /// Adds or replaces points, creating a missing collection on first write.
    ///
    /// The first write infers the collection dimension from its points. All
    /// vectors in that batch must be nonempty and have the same dimension.
    pub fn upsert(&self, points: impl IntoIterator<Item = Point>) -> Result<WriteResult> {
        self.upsert_with_vector_space(points, None)
    }

    pub(crate) fn upsert_with_vector_space(
        &self,
        points: impl IntoIterator<Item = Point>,
        vector_space: Option<&str>,
    ) -> Result<WriteResult> {
        let points: Vec<Point> = points.into_iter().collect();
        let dimension = inferred_dimension(&points)?;

        for _ in 0..WRITE_ATTEMPTS {
            let collection = match self.database.collection(&self.name) {
                Ok(collection) => collection,
                Err(Error::CollectionNotFound(_)) => {
                    let mut config = CollectionConfig::new(dimension);
                    config.vector_space = vector_space.map(str::to_owned);
                    match self.database.create_collection(&self.name, config) {
                        Ok(collection) => collection,
                        Err(Error::CollectionExists(_)) => {
                            thread::yield_now();
                            continue;
                        }
                        Err(Error::Git(error)) if retryable_ref_error(&error) => {
                            thread::yield_now();
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            };

            if let Some(vector_space) = vector_space {
                let actual = collection.info()?.config.vector_space;
                if actual.as_deref() != Some(vector_space) {
                    return Err(Error::Invalid(format!(
                        "collection {:?} uses vector space {:?}, expected {:?}",
                        self.name, actual, vector_space
                    )));
                }
            }

            match collection.upsert(points.clone()) {
                Ok(result) => return Ok(result),
                Err(Error::StaleRoot { .. }) => thread::yield_now(),
                Err(Error::Git(error)) if retryable_ref_error(&error) => thread::yield_now(),
                Err(error) => return Err(error),
            }
        }

        Err(Error::Invalid(format!(
            "collection {:?} changed repeatedly while applying the first write; retry the upsert",
            self.name
        )))
    }

    /// Searches for the nearest points using the collection's automatic mode.
    ///
    /// Payload metadata is included in every returned winner. Use
    /// [`CollectionHandle::advanced`] for filters, stored vectors, immutable
    /// roots, execution statistics, or explicit search tuning.
    pub fn search(
        &self,
        vector: impl IntoIterator<Item = f32>,
        limit: usize,
    ) -> Result<Vec<ScoredPoint>> {
        Ok(self
            .advanced()?
            .query(Query::new(vector, limit).with_payload())?
            .points)
    }

    /// Executes a detailed vector query without leaving the common facade.
    pub fn query(&self, query: Query) -> Result<QueryResult> {
        self.advanced()?.query(query)
    }

    /// Executes detailed vector queries in input order.
    pub fn query_batch(
        &self,
        queries: impl IntoIterator<Item = Query>,
    ) -> Result<Vec<QueryResult>> {
        let collection = self.advanced()?;
        queries
            .into_iter()
            .map(|query| collection.query(query))
            .collect()
    }

    /// Retrieves records using IDs, filters, pagination, and include controls.
    pub fn get(&self, request: GetRequest) -> Result<GetResult> {
        self.advanced()?.get(request)
    }

    /// Retrieves points by typed ID with payload metadata included.
    pub fn get_ids(&self, ids: impl IntoIterator<Item = PointId>) -> Result<Vec<Record>> {
        Ok(self
            .advanced()?
            .get(GetRequest {
                ids: ids.into_iter().collect(),
                with_payload: true,
                ..GetRequest::default()
            })?
            .points)
    }

    /// Deletes points by typed ID. Missing IDs are ignored.
    pub fn delete_ids(&self, ids: impl IntoIterator<Item = PointId>) -> Result<WriteResult> {
        self.advanced()?.delete(DeleteSelector {
            ids: ids.into_iter().collect(),
            ..DeleteSelector::default()
        })
    }

    /// Deletes records selected by IDs, a filter, or both.
    pub fn delete(&self, selector: DeleteSelector) -> Result<WriteResult> {
        self.advanced()?.delete(selector)
    }

    /// Applies an ordered batch of upserts and deletions in one ref update.
    pub fn apply(
        &self,
        mutations: impl IntoIterator<Item = SnapshotMutation>,
    ) -> Result<MutationResult> {
        self.advanced()?.apply(mutations.into_iter().collect())
    }

    /// Returns the number of points in the collection.
    pub fn count(&self) -> Result<usize> {
        Ok(self.advanced()?.count(None)?.count)
    }

    /// Returns the number of points matching a payload or ID filter.
    pub fn count_where(&self, filter: Filter) -> Result<usize> {
        Ok(self.advanced()?.count(Some(filter))?.count)
    }

    /// Returns the current immutable collection root.
    pub fn root(&self) -> Result<ObjectId> {
        self.advanced()?.root()
    }

    /// Restores a historical root as a new history-preserving commit.
    pub fn restore(&self, revision: impl AsRef<str>) -> Result<WriteResult> {
        self.advanced()?.restore(revision)
    }

    /// Returns the first `limit` canonically ordered points with payloads.
    pub fn peek(&self, limit: usize) -> Result<Vec<Record>> {
        Ok(self
            .advanced()?
            .get(GetRequest {
                limit: Some(limit),
                with_payload: true,
                ..GetRequest::default()
            })?
            .points)
    }

    /// Opens the existing collection through the detailed named API.
    pub fn advanced(&self) -> Result<Collection> {
        self.database.collection(&self.name)
    }
}

fn directory_is_empty(path: &Path) -> Result<bool> {
    Ok(fs::read_dir(path)?.next().is_none())
}

fn inferred_dimension(points: &[Point]) -> Result<usize> {
    let Some(first) = points.first() else {
        return Err(Error::Invalid("upsert batch must not be empty".into()));
    };
    let dimension = first.vector.len();
    if dimension == 0 {
        return Err(Error::Invalid("point vector must not be empty".into()));
    }
    if let Some(point) = points.iter().find(|point| point.vector.len() != dimension) {
        return Err(Error::Invalid(format!(
            "point {} has dimension {}, expected {dimension}",
            point.id,
            point.vector.len()
        )));
    }
    Ok(dimension)
}

fn retryable_ref_error(error: &git2::Error) -> bool {
    matches!(
        error.code(),
        ErrorCode::Exists | ErrorCode::Locked | ErrorCode::Modified
    )
}
