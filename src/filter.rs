use crate::{Condition, Error, Filter, JsonObject, PointId, Range, Result};
use serde_json::Value;

pub(crate) fn matches_filter(filter: &Filter, id: &PointId, payload: &JsonObject) -> bool {
    filter
        .must
        .iter()
        .all(|condition| matches_condition(condition, id, payload))
        && filter
            .must_not
            .iter()
            .all(|condition| !matches_condition(condition, id, payload))
        && (filter.should.is_empty()
            || filter
                .should
                .iter()
                .any(|condition| matches_condition(condition, id, payload)))
}

pub(crate) fn validate_filter(filter: &Filter) -> Result<()> {
    for condition in filter
        .must
        .iter()
        .chain(&filter.should)
        .chain(&filter.must_not)
    {
        match condition {
            Condition::HasField { has_field } => validate_path(has_field)?,
            Condition::FieldIn { key, any } => {
                validate_path(key)?;
                validate_values("any", any)?;
            }
            Condition::FieldNotIn { key, none } => {
                validate_path(key)?;
                validate_values("none", none)?;
            }
            Condition::FieldContains { key, contains } => {
                validate_path(key)?;
                validate_scalar("contains", contains)?;
            }
            Condition::DocumentContains { document_contains } if document_contains.is_empty() => {
                return Err(Error::Invalid(
                    "document substring must not be empty".into(),
                ));
            }
            Condition::DocumentRegex { document_regex } => {
                regex::Regex::new(document_regex).map_err(|error| {
                    Error::Invalid(format!("invalid document regular expression: {error}"))
                })?;
            }
            Condition::Field {
                key,
                matches,
                range,
            } => {
                validate_path(key)?;
                if matches.is_none() && range.is_none() {
                    return Err(Error::Invalid(
                        "field condition requires match or range".into(),
                    ));
                }
                if let Some(matches) = matches {
                    validate_scalar("match", &matches.value)?;
                }
            }
            Condition::HasId { has_id } if has_id.is_empty() => {
                return Err(Error::Invalid("has_id must not be empty".into()));
            }
            Condition::Nested(nested) => validate_filter(nested)?,
            _ => {}
        }
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<()> {
    if path.is_empty() || path.split('.').any(str::is_empty) {
        return Err(Error::Invalid(format!(
            "invalid empty payload path {path:?}"
        )));
    }
    Ok(())
}

fn validate_values(name: &str, values: &[Value]) -> Result<()> {
    if values.is_empty() {
        return Err(Error::Invalid(format!("{name} values must not be empty")));
    }
    for value in values {
        validate_scalar(name, value)?;
    }
    Ok(())
}

fn validate_scalar(name: &str, value: &Value) -> Result<()> {
    if value.is_array() || value.is_object() {
        return Err(Error::Invalid(format!(
            "{name} value must be a JSON scalar"
        )));
    }
    Ok(())
}

fn matches_condition(condition: &Condition, id: &PointId, payload: &JsonObject) -> bool {
    match condition {
        Condition::HasField { has_field } => dot_path(payload, has_field).is_some(),
        Condition::FieldIn { key, any } => dot_path(payload, key)
            .is_some_and(|value| any.iter().any(|expected| scalar_equal(value, expected))),
        Condition::FieldNotIn { key, none } => dot_path(payload, key)
            .is_some_and(|value| none.iter().all(|expected| !scalar_equal(value, expected))),
        Condition::FieldContains { key, contains } => dot_path(payload, key)
            .and_then(Value::as_array)
            .is_some_and(|values| values.iter().any(|value| scalar_equal(value, contains))),
        Condition::DocumentContains { document_contains } => payload
            .get("document")
            .and_then(Value::as_str)
            .is_some_and(|document| document.contains(document_contains)),
        Condition::DocumentRegex { document_regex } => payload
            .get("document")
            .and_then(Value::as_str)
            .is_some_and(|document| {
                regex::Regex::new(document_regex).is_ok_and(|pattern| pattern.is_match(document))
            }),
        Condition::Field {
            key,
            matches,
            range,
        } => {
            let Some(value) = dot_path(payload, key) else {
                return false;
            };
            matches
                .as_ref()
                .is_none_or(|expected| scalar_equal(value, &expected.value))
                && range.as_ref().is_none_or(|range| in_range(value, range))
                && (matches.is_some() || range.is_some())
        }
        Condition::HasId { has_id } => has_id.contains(id),
        Condition::Nested(filter) => matches_filter(filter, id, payload),
    }
}

fn dot_path<'a>(payload: &'a JsonObject, path: &str) -> Option<&'a Value> {
    let mut segments = path.split('.');
    let first = segments.next()?;
    if first.is_empty() {
        return None;
    }
    let mut value = payload.get(first)?;
    for segment in segments {
        if segment.is_empty() {
            return None;
        }
        value = value.as_object()?.get(segment)?;
    }
    Some(value)
}

fn scalar_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::String(_), Value::String(_))
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Null, Value::Null) => left == right,
        _ => false,
    }
}

fn in_range(value: &Value, range: &Range) -> bool {
    let Some(number) = value.as_f64() else {
        return false;
    };
    range.gt.is_none_or(|bound| number > bound)
        && range.gte.is_none_or(|bound| number >= bound)
        && range.lt.is_none_or(|bound| number < bound)
        && range.lte.is_none_or(|bound| number <= bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Condition, Filter, MatchValue};
    use serde_json::json;

    #[test]
    fn nested_qdrant_style_filter() {
        let payload = json!({"meta": {"topic": "rust"}, "year": 2026})
            .as_object()
            .unwrap()
            .clone();
        let filter = Filter {
            must: vec![Condition::Field {
                key: "meta.topic".into(),
                matches: Some(MatchValue {
                    value: json!("rust"),
                }),
                range: None,
            }],
            must_not: vec![Condition::matches("year", 2025)],
            should: vec![],
        };
        assert!(matches_filter(&filter, &PointId::from("a"), &payload));
    }
}
