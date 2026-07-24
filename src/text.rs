//! Provider-independent document embedding and text search.

use crate::{
    CollectionHandle, DeleteSelector, Error, Filter, JsonObject, MutationResult, Point, PointId,
    Query, QueryParams, Result, ScoredPoint, SnapshotMutation, Store, WriteResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(feature = "fastembed")]
use std::sync::Mutex;
#[cfg(feature = "fastembed")]
use std::{env, path::PathBuf};

#[cfg(feature = "fastembed")]
pub use fastembed::{EmbeddingModel as FastEmbedModel, TextInitOptions as FastEmbedInitOptions};

/// Converts text batches into vectors from one stable model space.
pub trait Embedder {
    /// Returns a stable identity such as a provider, model, and revision.
    fn model_id(&self) -> &str;

    /// Embeds every input string in the same order.
    fn embed(&self, input: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// A local FastEmbed text model, available with the `fastembed` feature.
///
/// The model is downloaded on first initialization and cached for offline use.
/// Ordinary `git-vdb` builds do not include FastEmbed or an ONNX runtime.
#[cfg(feature = "fastembed")]
pub struct FastEmbedder {
    model: Mutex<fastembed::TextEmbedding>,
    model_id: String,
}

#[cfg(feature = "fastembed")]
impl FastEmbedder {
    /// Initializes FastEmbed's default English model.
    pub fn try_new() -> Result<Self> {
        Self::try_with_model(FastEmbedModel::default())
    }

    /// Initializes one of FastEmbed's supported text models.
    pub fn try_with_model(model: FastEmbedModel) -> Result<Self> {
        let mut options = FastEmbedInitOptions::new(model);
        if env::var_os("FASTEMBED_CACHE_DIR").is_none() {
            if let Some(cache) = default_fastembed_cache_dir() {
                options = options.with_cache_dir(cache);
            }
        }
        Self::try_from_options(options)
    }

    /// Initializes a model with explicit FastEmbed cache, runtime, and length options.
    pub fn try_from_options(options: FastEmbedInitOptions) -> Result<Self> {
        let model_id = format!("fastembed/{}@5.17.3", options.model_name);
        let model = fastembed::TextEmbedding::try_new(options)
            .map_err(|error| Error::Embedding(error.to_string()))?;
        Ok(Self {
            model: Mutex::new(model),
            model_id,
        })
    }
}

#[cfg(feature = "fastembed")]
fn default_fastembed_cache_dir() -> Option<PathBuf> {
    if let Some(cache) = env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(cache).join("git-vdb/fastembed"));
    }
    if let Some(cache) = env::var_os("LOCALAPPDATA") {
        return Some(PathBuf::from(cache).join("git-vdb/fastembed"));
    }
    env::var_os("HOME").map(|home| {
        let home = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            home.join("Library/Caches/git-vdb/fastembed")
        } else {
            home.join(".cache/git-vdb/fastembed")
        }
    })
}

#[cfg(feature = "fastembed")]
impl Embedder for FastEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn embed(&self, input: &[String]) -> Result<Vec<Vec<f32>>> {
        self.model
            .lock()
            .map_err(|_| Error::Embedding("FastEmbed model lock was poisoned".into()))?
            .embed(input, None)
            .map_err(|error| Error::Embedding(error.to_string()))
    }
}

/// A text document with a typed ID and optional JSON metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    /// Stable document identifier.
    pub id: PointId,
    /// Text embedded for similarity search.
    pub text: String,
    /// Application-defined metadata stored with the text.
    pub metadata: JsonObject,
}

/// A typed document similarity-search result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentHit {
    /// Stable document identifier.
    pub id: PointId,
    /// Stored document text.
    pub document: String,
    /// Application metadata, excluding the internal stored-document field.
    pub metadata: JsonObject,
    /// Descending cosine similarity score.
    pub score: f32,
}

/// A text similarity query with optional filtering and execution controls.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextQuery {
    /// Text embedded for similarity search.
    pub text: String,
    /// Maximum number of documents returned.
    pub limit: usize,
    /// Optional metadata, ID, or document filter.
    pub filter: Option<Filter>,
    /// Exact or approximate execution controls.
    pub params: QueryParams,
}

impl TextQuery {
    /// Creates a text query returning at most ten documents.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            limit: 10,
            filter: None,
            params: QueryParams::default(),
        }
    }

    /// Sets the maximum number of returned documents.
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Restricts the query with a metadata, ID, or document filter.
    #[must_use]
    pub fn with_filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Replaces exact or approximate execution controls.
    #[must_use]
    pub fn with_params(mut self, params: QueryParams) -> Self {
        self.params = params;
        self
    }
}

impl Document {
    /// Creates a document with empty metadata.
    pub fn new(id: impl Into<PointId>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            metadata: JsonObject::new(),
        }
    }

    /// Serializes object-shaped document metadata.
    pub fn with_metadata(mut self, metadata: impl Serialize) -> Result<Self> {
        match serde_json::to_value(metadata)? {
            Value::Object(metadata) => {
                self.metadata = metadata;
                Ok(self)
            }
            _ => Err(Error::Invalid(
                "document metadata must serialize to a JSON object".into(),
            )),
        }
    }
}

/// A collection that embeds documents and text queries with one model.
#[derive(Clone, Debug)]
pub struct TextCollection<E> {
    collection: CollectionHandle,
    embedder: E,
    model_id: String,
}

impl Store {
    /// Opens a text collection bound to the embedder's stable model identity.
    pub fn text_collection<E: Embedder>(
        &self,
        name: impl Into<String>,
        embedder: E,
    ) -> Result<TextCollection<E>> {
        TextCollection::new(self.collection(name), embedder)
    }
}

impl<E: Embedder> TextCollection<E> {
    fn new(collection: CollectionHandle, embedder: E) -> Result<Self> {
        let model_id = embedder.model_id().trim().to_owned();
        if model_id.is_empty() {
            return Err(Error::Invalid(
                "embedding model identity must not be empty".into(),
            ));
        }
        if let Ok(existing) = collection.advanced() {
            let actual = existing.info()?.config.vector_space;
            if actual.as_deref() != Some(model_id.as_str()) {
                return Err(Error::Invalid(format!(
                    "collection uses vector space {actual:?}, expected {model_id:?}"
                )));
            }
        }
        Ok(Self {
            collection,
            embedder,
            model_id,
        })
    }

    /// Embeds and upserts documents, retaining their text in each payload.
    pub fn upsert_documents(
        &self,
        documents: impl IntoIterator<Item = Document>,
    ) -> Result<WriteResult> {
        let points = self.embed_documents(documents)?;
        self.collection
            .upsert_with_vector_space(points, Some(&self.model_id))
    }

    /// Atomically replaces documents matching `filter` with a new document set.
    ///
    /// The collection must already exist. Use [`TextCollection::upsert_documents`]
    /// for the first write, then this method for repeatable source synchronization.
    pub fn replace_documents(
        &self,
        filter: Filter,
        documents: impl IntoIterator<Item = Document>,
    ) -> Result<MutationResult> {
        let points = self.embed_documents(documents)?;
        let mut mutations = Vec::with_capacity(points.len() + 1);
        mutations.push(SnapshotMutation::delete_filter(filter));
        mutations.extend(points.into_iter().map(SnapshotMutation::upsert));
        self.collection.apply(mutations)
    }

    /// Deletes text documents selected by IDs, metadata, or document filters.
    pub fn delete(&self, selector: DeleteSelector) -> Result<WriteResult> {
        self.collection.delete(selector)
    }

    /// Embeds one text query and returns nearest documents with payloads.
    pub fn search_text(&self, text: impl Into<String>, limit: usize) -> Result<Vec<ScoredPoint>> {
        let vectors = self.embedder.embed(&[text.into()])?;
        let mut vectors = vectors.into_iter();
        let vector = vectors
            .next()
            .ok_or_else(|| Error::Invalid("embedder returned no query vector".into()))?;
        if vectors.next().is_some() {
            return Err(Error::Invalid(
                "embedder returned multiple vectors for one query".into(),
            ));
        }
        self.collection.search(vector, limit)
    }

    /// Embeds and executes one filtered text query, returning typed documents.
    pub fn query(&self, query: TextQuery) -> Result<Vec<DocumentHit>> {
        self.query_batch([query])?
            .pop()
            .ok_or_else(|| Error::Invalid("text query batch returned no result".into()))
    }

    /// Embeds and executes text queries in input order as one embedding batch.
    pub fn query_batch(
        &self,
        queries: impl IntoIterator<Item = TextQuery>,
    ) -> Result<Vec<Vec<DocumentHit>>> {
        let queries: Vec<TextQuery> = queries.into_iter().collect();
        if queries.is_empty() {
            return Ok(Vec::new());
        }
        let input: Vec<String> = queries.iter().map(|query| query.text.clone()).collect();
        let vectors = self.embedder.embed(&input)?;
        if vectors.len() != queries.len() {
            return Err(Error::Invalid(format!(
                "embedder returned {} vectors for {} text queries",
                vectors.len(),
                queries.len()
            )));
        }
        let vector_queries = queries
            .into_iter()
            .zip(vectors)
            .map(|(text, vector)| Query {
                vector,
                limit: text.limit,
                filter: text.filter,
                with_payload: true,
                expected_vector_space: Some(self.model_id.clone()),
                params: text.params,
                ..Query::default()
            });
        self.collection
            .query_batch(vector_queries)?
            .into_iter()
            .map(|result| result.points.into_iter().map(document_hit).collect())
            .collect()
    }

    /// Returns the underlying vector collection handle.
    pub fn vectors(&self) -> &CollectionHandle {
        &self.collection
    }

    fn embed_documents(&self, documents: impl IntoIterator<Item = Document>) -> Result<Vec<Point>> {
        let documents: Vec<Document> = documents.into_iter().collect();
        if documents.is_empty() {
            return Err(Error::Invalid("document batch must not be empty".into()));
        }
        let input: Vec<String> = documents
            .iter()
            .map(|document| document.text.clone())
            .collect();
        let vectors = self.embedder.embed(&input)?;
        if vectors.len() != documents.len() {
            return Err(Error::Invalid(format!(
                "embedder returned {} vectors for {} documents",
                vectors.len(),
                documents.len()
            )));
        }
        documents
            .into_iter()
            .zip(vectors)
            .map(|(document, vector)| {
                let mut payload = document.metadata;
                if payload
                    .insert("document".into(), Value::String(document.text))
                    .is_some()
                {
                    return Err(Error::Invalid(
                        "document metadata reserves the key \"document\"".into(),
                    ));
                }
                Ok(Point {
                    id: document.id,
                    vector,
                    payload,
                })
            })
            .collect()
    }
}

fn document_hit(point: ScoredPoint) -> Result<DocumentHit> {
    let mut metadata = point
        .payload
        .ok_or_else(|| Error::Corrupt("text query result omitted its payload".into()))?;
    let document = metadata
        .remove("document")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| Error::Corrupt("text point has no string document field".into()))?;
    Ok(DocumentHit {
        id: point.id,
        document,
        metadata,
        score: point.score,
    })
}
