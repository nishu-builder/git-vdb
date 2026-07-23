#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! A deterministic, Git-native embedded vector database.
//!
//! `git-vdb` stores vectors, payloads, metadata, and search indexes as immutable
//! Git objects. A canonical Git tree identifies the complete database state;
//! commits and refs are an optional naming and history layer above that state.
//! Equivalent configuration and points therefore produce the same root tree,
//! independent of insertion order, repository path, history, or timestamps.
//!
//! Two APIs expose the same storage engine:
//!
//! - [`SnapshotEngine`] works directly with immutable root IDs and never creates
//!   commits or updates refs.
//! - [`Database`] and [`Collection`] manage named collections under
//!   `refs/git-vdb/collections/*`, including history and compare-and-swap writes.
//!
//! # Immutable snapshot example
//!
//! ```
//! use git_vdb::{CollectionConfig, Point, Query, SnapshotEngine};
//!
//! # fn main() -> git_vdb::Result<()> {
//! let engine = SnapshotEngine::ephemeral()?;
//! let snapshot = engine.build(
//!     CollectionConfig::new(2),
//!     vec![
//!         Point::new("east", [1.0, 0.0]),
//!         Point::new("north", [0.0, 1.0]),
//!     ],
//! )?;
//!
//! let result = snapshot.query(Query::exact([0.9, 0.1], 1))?;
//! assert_eq!(result.points[0].id.to_string(), "east");
//! assert_eq!(result.root, snapshot.root());
//! # Ok(())
//! # }
//! ```
//!
//! # Semantics and compatibility
//!
//! Exact cosine search scores every eligible point. Approximate search uses the
//! root's deterministic IVF-flat index and may omit globally better points when
//! probe or candidate limits are exhausted; [`QueryResult::stats`] reports the
//! work performed. Reads and validation never advance refs. Named writes create
//! immutable objects first and then atomically compare-and-swap the collection
//! ref, so [`Error::StaleRoot`] cannot silently overwrite a concurrent writer.
//!
//! The crate's semantic version and its persisted [`mod@format`] version are
//! separate compatibility boundaries. New roots use format version 2; existing
//! format-version-1 roots remain readable and canonical regardless of physical
//! Git packing.

pub mod adapter;
mod codec;
mod filter;
pub mod model;
mod root;
mod root_v2;
pub mod snapshot;
mod store;

/// The normative persisted format-version-2 specification.
#[doc = include_str!("../docs/format-v2.md")]
pub mod format {}

/// The legacy persisted format-version-1 specification.
#[doc = include_str!("../docs/format.md")]
pub mod format_v1 {}

/// Design notes for immutable root snapshots and named collection history.
#[doc = include_str!("../docs/snapshots.md")]
pub mod snapshots {}

pub use adapter::{Collection, Database};
pub use model::{
    CollectionConfig, CollectionInfo, Condition, CountResult, DeleteSelector, DiffResult, Distance,
    Filter, GetRequest, GetResult, HistoryEntry, IndexConfig, JsonObject, MatchValue, ObjectId,
    ObjectStats, Point, PointId, Query, QueryMode, QueryParams, QueryResult, QueryStats, Range,
    Record, ScoredPoint, SnapshotInfo, SnapshotMutation, ValidationReport, WriteResult,
};
pub use snapshot::{Snapshot, SnapshotEngine};
pub use store::{open, CollectionHandle, Store};

use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
/// An error returned by a `git-vdb` operation.
pub enum Error {
    /// The underlying Git object database returned an error.
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),
    /// JSON serialization or deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// A filesystem operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The request or persisted value violated a declared invariant.
    #[error("invalid request: {0}")]
    Invalid(String),
    /// A requested named collection does not exist.
    #[error("collection not found: {0}")]
    CollectionNotFound(String),
    /// A collection could not be created because its name already exists.
    #[error("collection already exists: {0}")]
    CollectionExists(String),
    /// A named write expected a different current collection root.
    #[error("stale collection root: expected {expected}, actual {actual}")]
    StaleRoot {
        /// Root supplied by the writer as its compare-and-swap precondition.
        expected: ObjectId,
        /// Root resolved from the collection ref when the write was attempted.
        actual: ObjectId,
    },
    /// A write was attempted through an immutable historical collection view.
    #[error("read-only historical collection")]
    ReadOnly,
    /// Stored Git objects do not form a valid canonical collection root.
    #[error("corrupt collection: {0}")]
    Corrupt(String),
}

/// The result type returned by `git-vdb` operations.
pub type Result<T> = std::result::Result<T, Error>;
