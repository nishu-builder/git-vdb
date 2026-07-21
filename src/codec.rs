use crate::{Error, JsonObject, PointId, Result};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const VECTOR_MAGIC: &[u8; 8] = b"GTVDBV01";

pub(crate) fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value)?;
    validate_json(&value)?;
    Ok(serde_json::to_vec(&value)?)
}

fn validate_json(value: &Value) -> Result<()> {
    match value {
        Value::Array(items) => {
            for item in items {
                validate_json(item)?;
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                validate_json(value)?;
            }
        }
        Value::Number(number) => {
            if number.as_f64().is_some_and(|n| !n.is_finite()) {
                return Err(Error::Invalid("JSON numbers must be finite".into()));
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn encode_id(id: &PointId) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    #[serde(tag = "type", content = "value", rename_all = "snake_case")]
    enum StoredId<'a> {
        String(&'a str),
        #[serde(rename = "uint")]
        UInt(u64),
    }
    canonical_json(&match id {
        PointId::String(value) => StoredId::String(value),
        PointId::UInt(value) => StoredId::UInt(*value),
    })
}

pub(crate) fn decode_id(bytes: &[u8]) -> Result<PointId> {
    let value: Value = serde_json::from_slice(bytes)?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::Corrupt("id.json is not an object".into()))?;
    match object.get("type").and_then(Value::as_str) {
        Some("string") => Ok(PointId::String(
            object
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Corrupt("invalid string point ID".into()))?
                .to_owned(),
        )),
        Some("uint") => Ok(PointId::UInt(
            object
                .get("value")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::Corrupt("invalid uint point ID".into()))?,
        )),
        _ => Err(Error::Corrupt("unknown point ID type".into())),
    }
}

pub(crate) fn id_hash(id: &PointId) -> String {
    hex::encode(Sha256::digest(id.canonical_bytes()))
}

pub(crate) fn encode_vector(vector: &[f32]) -> Result<Vec<u8>> {
    validate_vector_components(vector)?;
    let dimension = u32::try_from(vector.len())
        .map_err(|_| Error::Invalid("vector dimension exceeds u32".into()))?;
    let mut bytes = Vec::with_capacity(12 + vector.len() * 4);
    bytes.extend_from_slice(VECTOR_MAGIC);
    bytes.extend_from_slice(&dimension.to_le_bytes());
    for component in vector {
        bytes.extend_from_slice(&component.to_bits().to_le_bytes());
    }
    Ok(bytes)
}

pub(crate) fn decode_vector(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() < 12 || &bytes[..8] != VECTOR_MAGIC {
        return Err(Error::Corrupt("invalid vector magic or header".into()));
    }
    let dimension = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if bytes.len() != 12 + dimension * 4 {
        return Err(Error::Corrupt(
            "vector byte length does not match header".into(),
        ));
    }
    let mut vector = Vec::with_capacity(dimension);
    for chunk in bytes[12..].chunks_exact(4) {
        vector.push(f32::from_bits(u32::from_le_bytes(
            chunk.try_into().unwrap(),
        )));
    }
    validate_vector_components(&vector).map_err(|error| Error::Corrupt(error.to_string()))?;
    Ok(vector)
}

pub(crate) fn validate_vector_components(vector: &[f32]) -> Result<()> {
    if vector.iter().any(|component| !component.is_finite()) {
        return Err(Error::Invalid(
            "vector components must be finite f32 values".into(),
        ));
    }
    Ok(())
}

pub(crate) fn decode_payload(bytes: &[u8]) -> Result<JsonObject> {
    let payload: JsonObject = serde_json::from_slice(bytes)?;
    if canonical_json(&payload)? != bytes {
        return Err(Error::Corrupt("payload.json is not canonical JSON".into()));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_fixture_is_stable() {
        let bytes = encode_vector(&[1.0, -2.5]).unwrap();
        assert_eq!(
            hex::encode(bytes),
            "4754564442563031020000000000803f000020c0"
        );
    }

    #[test]
    fn typed_ids_do_not_collide() {
        assert_ne!(id_hash(&PointId::from("1")), id_hash(&PointId::from(1_u64)));
    }
}
