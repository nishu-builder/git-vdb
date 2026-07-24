#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! A local vector database stored entirely in Git.
//!
//! The common path opens one directory, selects a collection, writes points,
//! and searches. A missing database and collection are created on first write,
//! and vector dimensions are inferred from that write.
//!
//! ```
//! use git_vdb::{open, Point};
//!
//! # fn main() -> git_vdb::Result<()> {
//! # let temporary = tempfile::TempDir::new()?;
//! let db = open(temporary.path().join("vectors.git"))?;
//! let docs = db.collection("docs");
//! docs.upsert([
//!     Point::new("east", [1.0, 0.0]),
//!     Point::new("north", [0.0, 1.0]),
//! ])?;
//! let hits = docs.search([0.9, 0.1], 1)?;
//! assert_eq!(hits[0].id.to_string(), "east");
//! # Ok(())
//! # }
//! ```
//!
//! Use [`Store`] and [`CollectionHandle`] for ordinary embedded use. Use
//! [`Database`] and [`Collection`] for explicitly configured collections,
//! filters, detailed query statistics, history, and compare-and-swap writes.
//! Use [`SnapshotEngine`] when another system owns naming and persistence.
//!
//! Equivalent configuration and points produce the same canonical Git root,
//! independent of insertion order, repository path, history, or timestamps.
//! The crate's semantic version and persisted [`mod@format`] version are
//! separate compatibility boundaries; new roots use format version 2 and
//! format-version-1 roots remain readable and mutable.

pub mod adapter;
mod codec;
mod filter;
pub mod model;
mod root;
mod root_v2;
pub mod snapshot;
mod store;
pub mod text;

/// The normative persisted format-version-2 specification.
#[doc = include_str!("../docs/format-v2.md")]
pub mod format {}

/// The legacy persisted format-version-1 specification.
#[doc = include_str!("../docs/format.md")]
pub mod format_v1 {}

/// Design notes for immutable root snapshots and named collection history.
#[doc = include_str!("../docs/snapshots.md")]
pub mod snapshots {}

/// Task-oriented guides whose Rust examples are checked as doctests.
pub mod guides {
    /// Create and query a persistent database.
    #[doc = include_str!("../docs/quickstart.md")]
    pub mod quickstart {}

    /// Safely create, reopen, and reuse a database.
    #[doc = include_str!("../docs/persistence.md")]
    pub mod persistence {}

    /// Filter metadata through the detailed query API.
    #[doc = include_str!("../docs/filtering.md")]
    pub mod filtering {}

    /// Read collection history and transport refs with Git.
    #[doc = include_str!("../docs/history.md")]
    pub mod history {}

    /// Keep embedding-model identities consistent.
    #[doc = include_str!("../docs/embeddings.md")]
    pub mod embeddings {}

    /// Map common Chroma concepts onto the git-vdb API.
    #[doc = include_str!("../docs/chroma-migration.md")]
    pub mod chroma_migration {}

    /// Connect document and vector frameworks through the public API or CLI.
    #[doc = include_str!("../docs/integrations.md")]
    pub mod integrations {}
}

pub use adapter::{Collection, Database};
pub use model::{
    CollectionConfig, CollectionInfo, Condition, CountResult, DeleteSelector, DiffResult, Distance,
    Filter, GetRequest, GetResult, HistoryEntry, IndexConfig, JsonObject, MatchValue,
    MutationResult, ObjectId, ObjectStats, Point, PointId, Query, QueryMode, QueryParams,
    QueryResult, QueryStats, Range, Record, ScoredPoint, SnapshotInfo, SnapshotMutation,
    ValidationReport, WriteResult,
};
pub use snapshot::{Snapshot, SnapshotEngine};
pub use store::{open, CollectionHandle, Store};
pub use text::{Document, DocumentHit, Embedder, TextCollection, TextQuery};
#[cfg(feature = "fastembed")]
pub use text::{FastEmbedInitOptions, FastEmbedModel, FastEmbedder};

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
    /// An embedding provider could not initialize or generate vectors.
    #[error("embedding error: {0}")]
    Embedding(String),
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
