use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;
use std::str::FromStr;

pub type JsonObject = Map<String, Value>;

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PointId {
    String(String),
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub id: PointId,
    pub vector: Vec<f32>,
    #[serde(default)]
    pub payload: JsonObject,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Distance {
    #[default]
    Cosine,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexConfig {
    pub tables: usize,
    pub signature_bits: usize,
    pub projection_seed: u64,
    pub full_scan_threshold: usize,
    pub default_probes: usize,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CollectionConfig {
    pub dimension: usize,
    pub distance: Distance,
    pub vector_space: Option<String>,
    pub index: IndexConfig,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchValue {
    pub value: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Range {
    pub gt: Option<f64>,
    pub gte: Option<f64>,
    pub lt: Option<f64>,
    pub lte: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Condition {
    Field {
        key: String,
        #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
        matches: Option<MatchValue>,
        #[serde(skip_serializing_if = "Option::is_none")]
        range: Option<Range>,
    },
    HasId {
        has_id: Vec<PointId>,
    },
    Nested(Filter),
}

impl Condition {
    pub fn matches(key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::Field {
            key: key.into(),
            matches: Some(MatchValue {
                value: value.into(),
            }),
            range: None,
        }
    }

    pub fn range(key: impl Into<String>, range: Range) -> Self {
        Self::Field {
            key: key.into(),
            matches: None,
            range: Some(range),
        }
    }

    pub fn has_id(ids: impl IntoIterator<Item = PointId>) -> Self {
        Self::HasId {
            has_id: ids.into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    pub must: Vec<Condition>,
    pub should: Vec<Condition>,
    pub must_not: Vec<Condition>,
}

impl Filter {
    pub fn must(conditions: impl IntoIterator<Item = Condition>) -> Self {
        Self {
            must: conditions.into_iter().collect(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QueryParams {
    pub exact: Option<bool>,
    pub probes: usize,
    pub candidate_limit: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Query {
    pub vector: Vec<f32>,
    pub limit: usize,
    pub filter: Option<Filter>,
    pub with_payload: bool,
    pub with_vector: bool,
    pub expected_vector_space: Option<String>,
    pub params: QueryParams,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoredPoint {
    pub id: PointId,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryMode {
    Exact,
    Approximate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryStats {
    pub mode: QueryMode,
    pub collection_points: usize,
    pub buckets_probed: usize,
    pub candidates_discovered: usize,
    pub vectors_scored: usize,
    pub probe_limit_exhausted: bool,
    pub candidate_limit_exhausted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryResult {
    pub root: ObjectId,
    pub points: Vec<ScoredPoint>,
    pub stats: QueryStats,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GetRequest {
    pub ids: Vec<PointId>,
    pub filter: Option<Filter>,
    pub offset: usize,
    pub limit: Option<usize>,
    pub with_payload: bool,
    pub with_vector: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub id: PointId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetResult {
    pub root: ObjectId,
    pub points: Vec<Record>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeleteSelector {
    pub ids: Vec<PointId>,
    pub filter: Option<Filter>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WriteResult {
    pub root: ObjectId,
    pub affected_points: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CountResult {
    pub root: ObjectId,
    pub count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionInfo {
    pub root: ObjectId,
    pub name: String,
    pub point_count: usize,
    pub config: CollectionConfig,
    pub read_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub root: ObjectId,
    pub point_count: usize,
    pub config: CollectionConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum SnapshotMutation {
    Upsert { point: Point },
    DeleteIds { ids: Vec<PointId> },
    DeleteFilter { filter: Filter },
}

impl SnapshotMutation {
    pub fn upsert(point: Point) -> Self {
        Self::Upsert { point }
    }

    pub fn delete_ids(ids: impl IntoIterator<Item = PointId>) -> Self {
        Self::DeleteIds {
            ids: ids.into_iter().collect(),
        }
    }

    pub fn delete_filter(filter: Filter) -> Self {
        Self::DeleteFilter { filter }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub commit: ObjectId,
    pub root: ObjectId,
    pub parent: Option<ObjectId>,
    pub message: String,
    pub time_seconds: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ObjectStats {
    pub objects: usize,
    pub bytes: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffResult {
    pub left_root: ObjectId,
    pub right_root: ObjectId,
    pub added: Vec<PointId>,
    pub removed: Vec<PointId>,
    pub changed: Vec<PointId>,
    pub configuration_changed: bool,
    pub buckets_changed: bool,
    pub shared: ObjectStats,
    pub left_unique: ObjectStats,
    pub right_unique: ObjectStats,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationReport {
    pub root: ObjectId,
    pub full: bool,
    pub point_count: usize,
    pub checked_buckets: usize,
    pub valid: bool,
}
