//! Provider-independent document embedding and text search.

use crate::{
    CollectionHandle, Error, JsonObject, Point, PointId, Result, ScoredPoint, Store, WriteResult,
};
use serde::Serialize;
use serde_json::Value;
#[cfg(feature = "fastembed")]
use std::sync::Mutex;

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
        Self::try_from_options(FastEmbedInitOptions::new(model))
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
        let points = documents
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
            .collect::<Result<Vec<_>>>()?;
        self.collection
            .upsert_with_vector_space(points, Some(&self.model_id))
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

    /// Returns the underlying vector collection handle.
    pub fn vectors(&self) -> &CollectionHandle {
        &self.collection
    }
}
