use crate::{Condition, Filter, JsonObject, PointId, Range};
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

fn matches_condition(condition: &Condition, id: &PointId, payload: &JsonObject) -> bool {
    match condition {
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
