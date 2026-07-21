mod adapter;
mod codec;
mod filter;
mod model;
mod root;
mod snapshot;

pub use adapter::{Collection, Database};
pub use model::*;
pub use snapshot::{Snapshot, SnapshotEngine};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("collection not found: {0}")]
    CollectionNotFound(String),
    #[error("collection already exists: {0}")]
    CollectionExists(String),
    #[error("stale collection root: expected {expected}, actual {actual}")]
    StaleRoot {
        expected: ObjectId,
        actual: ObjectId,
    },
    #[error("read-only historical collection")]
    ReadOnly,
    #[error("corrupt collection: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, Error>;
