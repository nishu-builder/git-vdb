//! Public request, response, configuration, filtering, and mutation types.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;
use std::str::FromStr;

/// A JSON object used for point payloads.
pub type JsonObject = Map<String, Value>;

/// A typed point identifier.
///
/// String and unsigned-integer IDs occupy distinct namespaces and have a stable
/// canonical ordering used to break equal-score query ties.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PointId {
    /// A UTF-8 string identifier.
    String(String),
    /// An unsigned 64-bit integer identifier.
    UInt(u64),
}

impl PointId {
    pub(crate) fn canonical_bytes(&self) -> Vec<u8> {
        match self {
            Self::String(value) => {
                let mut bytes = vec![b's', 0];
                bytes.extend_from_slice(value.as_bytes());
                bytes
            }
            Self::UInt(value) => {
                let mut bytes = vec![b'u', 0];
                bytes.extend_from_slice(&value.to_be_bytes());
                bytes
            }
        }
    }
}

impl From<&str> for PointId {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for PointId {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<u64> for PointId {
    fn from(value: u64) -> Self {
        Self::UInt(value)
    }
}

impl Ord for PointId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.canonical_bytes().cmp(&other.canonical_bytes())
    }
}

impl PartialOrd for PointId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for PointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => write!(f, "{value}"),
            Self::UInt(value) => write!(f, "{value}"),
        }
    }
}

/// A hexadecimal Git object ID that identifies a tree or commit.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectId(pub String);

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ObjectId {
    type Err = git2::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        git2::Oid::from_str(value)?;
        Ok(Self(value.to_owned()))
    }
}

impl From<git2::Oid> for ObjectId {
    fn from(value: git2::Oid) -> Self {
        Self(value.to_string())
    }
}

impl AsRef<str> for ObjectId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A vector and its optional JSON payload, identified by a typed ID.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Point {
    /// The stable typed identifier for this point.
    pub id: PointId,
    /// The vector components, whose length must match the collection dimension.
    pub vector: Vec<f32>,
    /// Application-defined metadata used by filters and optional result output.
    #[serde(default)]
    pub payload: JsonObject,
}

impl Point {
    /// Creates a point with an empty payload.
    pub fn new(id: impl Into<PointId>, vector: impl IntoIterator<Item = f32>) -> Self {
        Self {
            id: id.into(),
            vector: vector.into_iter().collect(),
            payload: JsonObject::new(),
        }
    }

    /// Replaces the point payload and returns the updated point.
    #[must_use]
    pub fn with_payload(mut self, payload: JsonObject) -> Self {
        self.payload = payload;
        self
    }
}

/// The vector distance used for ranking.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Distance {
    /// Cosine similarity, returned as a descending score.
    #[default]
    Cosine,
}

/// Deterministic LSH index construction and query defaults.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexConfig {
    /// Number of independent LSH tables stored per point.
    pub tables: usize,
    /// Number of projection bits in each table signature.
    pub signature_bits: usize,
    /// Seed used to deterministically derive projection vectors.
    pub projection_seed: u64,
    /// Point-count threshold at or below which queries default to exact search.
    pub full_scan_threshold: usize,
    /// Default number of approximate buckets to probe.
    pub default_probes: usize,
    /// Default maximum number of approximate candidates to discover.
    pub default_candidate_limit: usize,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            tables: 12,
            signature_bits: 12,
            projection_seed: 0x6769_742d_7664_6231,
            full_scan_threshold: 1_000,
            default_probes: 96,
            default_candidate_limit: 10_000,
        }
    }
}

/// Collection-wide vector and index configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CollectionConfig {
    /// Required number of components in every stored and query vector.
    pub dimension: usize,
    /// Distance used to score vectors.
    pub distance: Distance,
    /// Optional application-defined vector-space identity checked by queries.
    pub vector_space: Option<String>,
    /// Deterministic approximate-index configuration.
    pub index: IndexConfig,
}

impl CollectionConfig {
    /// Creates a cosine collection configuration for the given dimension.
    pub fn new(dimension: usize) -> Self {
        Self {
            dimension,
            ..Self::default()
        }
    }

    /// Assigns an application-defined vector-space identity.
    #[must_use]
    pub fn with_vector_space(mut self, vector_space: impl Into<String>) -> Self {
        self.vector_space = Some(vector_space.into());
        self
    }

    /// Replaces the deterministic LSH configuration.
    #[must_use]
    pub fn with_index(mut self, index: IndexConfig) -> Self {
        self.index = index;
        self
    }
}

impl Default for CollectionConfig {
    fn default() -> Self {
        Self {
            dimension: 0,
            distance: Distance::Cosine,
            vector_space: None,
            index: IndexConfig::default(),
        }
    }
}

/// A JSON scalar value required by a field-match condition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchValue {
    /// The scalar value that must equal the payload field.
    pub value: Value,
}

/// Inclusive or exclusive numeric bounds for a payload field.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Range {
    /// Exclusive lower bound.
    pub gt: Option<f64>,
    /// Inclusive lower bound.
    pub gte: Option<f64>,
    /// Exclusive upper bound.
    pub lt: Option<f64>,
    /// Inclusive upper bound.
    pub lte: Option<f64>,
}

/// A predicate used inside a [`Filter`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum Condition {
    /// Matches or range-checks a dot-separated payload field.
    Field {
        /// Dot-separated payload path.
        key: String,
        /// Optional equality match.
        #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
        matches: Option<MatchValue>,
        /// Optional numeric range.
        #[serde(skip_serializing_if = "Option::is_none")]
        range: Option<Range>,
    },
    /// Matches points whose typed ID is in the supplied set.
    HasId {
        /// Accepted typed IDs.
        has_id: Vec<PointId>,
    },
    /// Evaluates a nested Boolean filter.
    Nested(Filter),
}

impl Condition {
    /// Creates an equality condition for a dot-separated payload path.
    pub fn matches(key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::Field {
            key: key.into(),
            matches: Some(MatchValue {
                value: value.into(),
            }),
            range: None,
        }
    }

    /// Creates a numeric-range condition for a dot-separated payload path.
    pub fn range(key: impl Into<String>, range: Range) -> Self {
        Self::Field {
            key: key.into(),
            matches: None,
            range: Some(range),
        }
    }

    /// Creates a condition that accepts the supplied typed point IDs.
    pub fn has_id(ids: impl IntoIterator<Item = PointId>) -> Self {
        Self::HasId {
            has_id: ids.into_iter().collect(),
        }
    }
}

/// A Boolean point filter.
///
/// Every `must` condition and no `must_not` condition must match. When `should`
/// is non-empty, at least one `should` condition must also match.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// Conditions that must all match.
    pub must: Vec<Condition>,
    /// Conditions of which at least one must match when the list is non-empty.
    pub should: Vec<Condition>,
    /// Conditions that must not match.
    pub must_not: Vec<Condition>,
}

impl Filter {
    /// Creates a filter containing only required conditions.
    pub fn must(conditions: impl IntoIterator<Item = Condition>) -> Self {
        Self {
            must: conditions.into_iter().collect(),
            ..Self::default()
        }
    }
}

/// Exact or approximate query execution parameters.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QueryParams {
    /// Explicit mode override; `None` selects exact search for small collections.
    pub exact: Option<bool>,
    /// Approximate buckets to probe, or zero to use the collection default.
    pub probes: usize,
    /// Approximate candidate limit, or zero to use the collection default.
    pub candidate_limit: usize,
}

/// A vector similarity query.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Query {
    /// Query vector, which must match the collection dimension.
    pub vector: Vec<f32>,
    /// Maximum number of scored points to return.
    pub limit: usize,
    /// Optional point filter applied before final ranking.
    pub filter: Option<Filter>,
    /// Whether returned winners include their JSON payloads.
    pub with_payload: bool,
    /// Whether returned winners include their stored vectors.
    pub with_vector: bool,
    /// Optional vector-space identity that must match collection metadata.
    pub expected_vector_space: Option<String>,
    /// Exact or approximate execution controls.
    pub params: QueryParams,
}

impl Query {
    /// Creates a query using collection defaults for exact or approximate mode.
    pub fn new(vector: impl IntoIterator<Item = f32>, limit: usize) -> Self {
        Self {
            vector: vector.into_iter().collect(),
            limit,
            ..Self::default()
        }
    }

    /// Creates a query that scores every eligible point.
    pub fn exact(vector: impl IntoIterator<Item = f32>, limit: usize) -> Self {
        let mut query = Self::new(vector, limit);
        query.params.exact = Some(true);
        query
    }

    /// Creates a query that uses deterministic LSH candidate discovery.
    pub fn approximate(vector: impl IntoIterator<Item = f32>, limit: usize) -> Self {
        let mut query = Self::new(vector, limit);
        query.params.exact = Some(false);
        query
    }

    /// Adds a point filter.
    #[must_use]
    pub fn with_filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Requests payloads for returned winners.
    #[must_use]
    pub fn with_payload(mut self) -> Self {
        self.with_payload = true;
        self
    }

    /// Requests stored vectors for returned winners.
    #[must_use]
    pub fn with_vector(mut self) -> Self {
        self.with_vector = true;
        self
    }

    /// Requires the collection to use the supplied vector-space identity.
    #[must_use]
    pub fn in_vector_space(mut self, vector_space: impl Into<String>) -> Self {
        self.expected_vector_space = Some(vector_space.into());
        self
    }

    /// Replaces the exact or approximate execution parameters.
    #[must_use]
    pub fn with_params(mut self, params: QueryParams) -> Self {
        self.params = params;
        self
    }
}

impl Default for Query {
    fn default() -> Self {
        Self {
            vector: Vec::new(),
            limit: 10,
            filter: None,
            with_payload: false,
            with_vector: false,
            expected_vector_space: None,
            params: QueryParams::default(),
        }
    }
}

/// A scored query winner.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoredPoint {
    /// Typed point identifier.
    pub id: PointId,
    /// Descending cosine similarity score.
    pub score: f32,
    /// Payload when requested by [`Query::with_payload`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<JsonObject>,
    /// Stored vector when requested by [`Query::with_vector`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
}

/// Query algorithm selected after applying collection defaults.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QueryMode {
    /// Every filter-eligible point was scored.
    Exact,
    /// Candidates were discovered through deterministic LSH buckets.
    Approximate,
}

/// Work counters for a completed query.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryStats {
    /// Algorithm selected for the query.
    pub mode: QueryMode,
    /// Total points in the resolved collection root.
    pub collection_points: usize,
    /// LSH buckets visited; zero for exact search.
    pub buckets_probed: usize,
    /// Distinct approximate candidates discovered.
    pub candidates_discovered: usize,
    /// Point vectors actually scored.
    pub vectors_scored: usize,
    /// Whether approximate discovery consumed its probe budget.
    pub probe_limit_exhausted: bool,
    /// Whether approximate discovery consumed its candidate budget.
    pub candidate_limit_exhausted: bool,
}

/// Ordered similarity-search results and execution statistics.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryResult {
    /// Immutable root that was queried.
    pub root: ObjectId,
    /// Winners ordered by descending score and canonical typed ID.
    pub points: Vec<ScoredPoint>,
    /// Algorithm and work counters.
    pub stats: QueryStats,
}

/// A deterministic point-retrieval request.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GetRequest {
    /// Optional typed IDs; when combined with a filter, both must match.
    pub ids: Vec<PointId>,
    /// Optional payload or ID filter.
    pub filter: Option<Filter>,
    /// Number of canonically ordered matches to skip.
    pub offset: usize,
    /// Maximum matches to return, or all remaining matches when `None`.
    pub limit: Option<usize>,
    /// Whether results include payloads.
    pub with_payload: bool,
    /// Whether results include vectors.
    pub with_vector: bool,
}

/// A point returned without similarity scoring.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    /// Typed point identifier.
    pub id: PointId,
    /// Payload when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<JsonObject>,
    /// Stored vector when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
}

/// Canonically ordered point-retrieval results.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetResult {
    /// Immutable root that was read.
    pub root: ObjectId,
    /// Matching records after offset and limit are applied.
    pub points: Vec<Record>,
}

/// Selects the union of typed IDs and filter matches for deletion.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeleteSelector {
    /// Typed IDs to delete; missing IDs are ignored.
    pub ids: Vec<PointId>,
    /// Optional filter whose matches are also deleted.
    pub filter: Option<Filter>,
}

/// The outcome of a named collection write.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WriteResult {
    /// New deterministic collection root.
    pub root: ObjectId,
    /// Number of submitted upserts or points actually removed.
    pub affected_points: usize,
}

/// A count and the immutable root from which it was read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CountResult {
    /// Immutable root that was counted.
    pub root: ObjectId,
    /// Number of matching points.
    pub count: usize,
}

/// Metadata for a named or historical collection view.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionInfo {
    /// Resolved deterministic root.
    pub root: ObjectId,
    /// Named collection.
    pub name: String,
    /// Number of points at the resolved root.
    pub point_count: usize,
    /// Collection configuration stored in the root.
    pub config: CollectionConfig,
    /// Whether the view is historical and rejects writes.
    pub read_only: bool,
}

/// Metadata for an immutable root snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Deterministic root tree ID.
    pub root: ObjectId,
    /// Number of points in the root.
    pub point_count: usize,
    /// Collection configuration stored in the root.
    pub config: CollectionConfig,
}

/// One operation in an ordered immutable-root mutation batch.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SnapshotMutation {
    /// Adds a new point or replaces an existing point with the same typed ID.
    Upsert {
        /// Complete replacement point.
        point: Point,
    },
    /// Deletes the supplied typed IDs.
    DeleteIds {
        /// Typed IDs to delete; missing IDs are ignored.
        ids: Vec<PointId>,
    },
    /// Deletes every point matching a filter.
    DeleteFilter {
        /// Filter evaluated against the preceding mutation state.
        filter: Filter,
    },
}

impl SnapshotMutation {
    /// Creates an upsert mutation.
    pub fn upsert(point: Point) -> Self {
        Self::Upsert { point }
    }

    /// Creates a typed-ID deletion mutation.
    pub fn delete_ids(ids: impl IntoIterator<Item = PointId>) -> Self {
        Self::DeleteIds {
            ids: ids.into_iter().collect(),
        }
    }

    /// Creates a filter deletion mutation.
    pub fn delete_filter(filter: Filter) -> Self {
        Self::DeleteFilter { filter }
    }
}

/// One named-collection commit in newest-first history order.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Commit object ID.
    pub commit: ObjectId,
    /// Canonical root tree recorded by the commit.
    pub root: ObjectId,
    /// First parent commit, when present.
    pub parent: Option<ObjectId>,
    /// Commit message generated for the collection operation.
    pub message: String,
    /// Commit time in Unix seconds.
    pub time_seconds: i64,
}

/// Count and logical size of a set of reachable Git objects.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ObjectStats {
    /// Number of Git objects.
    pub objects: usize,
    /// Sum of logical object bytes.
    pub bytes: usize,
}

/// Logical point and structural-sharing differences between two roots.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffResult {
    /// Resolved left root.
    pub left_root: ObjectId,
    /// Resolved right root.
    pub right_root: ObjectId,
    /// IDs present only on the right.
    pub added: Vec<PointId>,
    /// IDs present only on the left.
    pub removed: Vec<PointId>,
    /// IDs present in both roots with changed point content.
    pub changed: Vec<PointId>,
    /// Whether collection metadata differs.
    pub configuration_changed: bool,
    /// Whether any approximate-index buckets differ.
    pub buckets_changed: bool,
    /// Objects reachable from both roots.
    pub shared: ObjectStats,
    /// Objects reachable only from the left root.
    pub left_unique: ObjectStats,
    /// Objects reachable only from the right root.
    pub right_unique: ObjectStats,
}

/// Result of basic or full canonical-root validation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationReport {
    /// Root that was validated.
    pub root: ObjectId,
    /// Whether expensive index recomputation was requested.
    pub full: bool,
    /// Validated point count.
    pub point_count: usize,
    /// Approximate-index buckets checked during full validation.
    pub checked_buckets: usize,
    /// `true` when validation completed without finding corruption.
    pub valid: bool,
}
