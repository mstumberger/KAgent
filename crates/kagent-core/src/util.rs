//! Small helpers: UTF-8 safe truncation, output budgets, minimal JSON-schema checks.

use serde_json::Value;

/// Truncate to at most `max` bytes on a char boundary.
pub fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn tail_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// Keep head and tail of oversized tool output so the model sees both the command
/// banner and the final error lines (ZCode result budget, strategy "truncate").
pub fn budget_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head = truncate_str(s, max * 2 / 3);
    let tail = tail_str(s, max / 3);
    let omitted = s.len() - head.len() - tail.len();
    format!("{head}\n\n[... {omitted} bytes omitted ...]\n\n{tail}")
}

/// Validate the subset of JSON Schema our tool contracts use: object/required,
/// primitive types, enums and arrays. Returns human readable issues for the model.
pub fn validate_schema(schema: &Value, value: &Value) -> Vec<String> {
    let mut issues = Vec::new();
    check(schema, value, "input", &mut issues);
    issues
}

fn check(schema: &Value, value: &Value, path: &str, issues: &mut Vec<String>) {
    if let Some(ty) = schema.get("type").and_then(Value::as_str) {
        let ok = match ty {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            _ => true,
        };
        if !ok {
            issues.push(format!("{path}: expected {ty}, got {}", type_name(value)));
            return;
        }
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            issues.push(format!("{path}: must be one of {}", Value::Array(options.clone())));
        }
    }
    if let Some(obj) = value.as_object() {
        if let Some(req) = schema.get("required").and_then(Value::as_array) {
            for key in req.iter().filter_map(Value::as_str) {
                if !obj.contains_key(key) || obj[key].is_null() {
                    issues.push(format!("{path}.{key}: required"));
                }
            }
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (key, sub) in props {
                if let Some(v) = obj.get(key) {
                    if !v.is_null() {
                        check(sub, v, &format!("{path}.{key}"), issues);
                    }
                }
            }
        }
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), value.as_array()) {
        for (i, v) in arr.iter().enumerate() {
            check(items, v, &format!("{path}[{i}]"), issues);
        }
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Local models often send numbers as strings and objects as JSON strings. Coerce the
/// obvious cases before validation (ZCode `input-normalization`, simplified).
pub fn normalize_input(schema: &Value, value: Value) -> Value {
    let value = match value {
        Value::String(s) if schema.get("type").and_then(Value::as_str) == Some("object") => {
            serde_json::from_str(&s).unwrap_or(Value::String(s))
        }
        Value::Null if schema.get("type").and_then(Value::as_str) == Some("object") => {
            Value::Object(Default::default())
        }
        v => v,
    };
    let Value::Object(mut obj) = value else { return value };
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (key, sub) in props {
            let Some(v) = obj.remove(key) else { continue };
            let ty = sub.get("type").and_then(Value::as_str);
            let coerced = match (ty, v) {
                (Some("integer"), Value::String(s)) => {
                    s.trim().parse::<i64>().map(Value::from).unwrap_or(Value::String(s))
                }
                (Some("number"), Value::String(s)) => {
                    s.trim().parse::<f64>().map(Value::from).unwrap_or(Value::String(s))
                }
                (Some("boolean"), Value::String(s)) => match s.trim() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    _ => Value::String(s),
                },
                (Some("array"), Value::String(s)) if s.trim_start().starts_with('[') => {
                    serde_json::from_str(&s).unwrap_or(Value::String(s))
                }
                (_, v) => v,
            };
            obj.insert(key.clone(), coerced);
        }
    }
    Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncation_respects_char_boundaries() {
        assert_eq!(truncate_str("čžš", 3), "č");
        let out = budget_output(&"a".repeat(1000), 300);
        assert!(out.contains("bytes omitted"));
        assert!(out.len() < 400);
    }

    #[test]
    fn schema_checks_required_and_types() {
        let schema = json!({"type":"object","required":["pid"],
            "properties":{"pid":{"type":"integer"},"signal":{"type":"string","enum":["TERM","KILL"]}}});
        assert!(validate_schema(&schema, &json!({"pid": 1})).is_empty());
        assert_eq!(validate_schema(&schema, &json!({})).len(), 1);
        assert_eq!(validate_schema(&schema, &json!({"pid": 1, "signal": "X"})).len(), 1);
    }

    #[test]
    fn normalization_coerces_strings() {
        let schema = json!({"type":"object","properties":{"pid":{"type":"integer"},"all":{"type":"boolean"}}});
        let v = normalize_input(&schema, json!("{\"pid\":\"42\",\"all\":\"true\"}"));
        assert_eq!(v, json!({"pid":42,"all":true}));
    }
}
