//! The note of a setting the owner changed on the screen (agent-loop.md C.2, kind `setting`): which
//! values differ between the settings before and after, in words, never a secret.

use serde_json::Value;

use crate::settings_tool::keys::is_secret;

/// At most this many settings in one note, then "and N more".
const MAX_PATHS: usize = 3;
/// A value longer than this is cut.
const MAX_VALUE: usize = 80;

/// The leaves that differ between `before` and `after`, as `(dotted path, old, new)`. An array is one leaf.
fn differences(before: &Value, after: &Value, path: &str, out: &mut Vec<(String, Value, Value)>) {
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let sub = if path.is_empty() { k.to_string() } else { format!("{path}.{k}") };
                differences(a.get(k).unwrap_or(&Value::Null), b.get(k).unwrap_or(&Value::Null), &sub, out);
            }
        }
        _ if before != after => out.push((path.to_string(), before.clone(), after.clone())),
        _ => {}
    }
}

fn show(v: &Value) -> String {
    let text = match v {
        Value::String(s) => format!("\"{s}\""),
        Value::Null => "nothing".to_string(),
        other => other.to_string(),
    };
    if text.chars().count() > MAX_VALUE {
        format!("{}...", text.chars().take(MAX_VALUE).collect::<String>())
    } else {
        text
    }
}

/// The note text of a change from `before` to `after`, or `None` when nothing differs.
pub fn note_text(before: &Value, after: &Value) -> Option<String> {
    let mut diff = Vec::new();
    differences(before, after, "", &mut diff);
    if diff.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = diff
        .iter()
        .take(MAX_PATHS)
        .map(|(path, old, new)| {
            if is_secret(path) {
                format!("`{path}` (value not shown)")
            } else {
                format!("`{path}` from {} to {}", show(old), show(new))
            }
        })
        .collect();
    if diff.len() > MAX_PATHS {
        parts.push(format!("and {} more", diff.len() - MAX_PATHS));
    }
    Some(format!("The owner changed {}.", parts.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_change_names_the_setting_and_both_values() {
        let before = json!({"persona": {"address": "Marko", "name": "KAgent"}, "chatModel": "a::b", "voice": {"pauseMedia": true}});
        let after = json!({"persona": {"address": "Mark", "name": "KAgent"}, "chatModel": "a::b", "voice": {"pauseMedia": true}});
        assert_eq!(note_text(&before, &after).unwrap(), "The owner changed `persona.address` from \"Marko\" to \"Mark\".");
        assert_eq!(note_text(&before, &before), None);
    }

    #[test]
    fn a_secret_shows_no_value_and_many_changes_are_counted() {
        let before = json!({"calendar": {"clientId": "old-id", "clientSecret": "old-secret"}, "telegram": {"botToken": "111"}, "a": 1, "b": 2, "c": 3});
        let after = json!({"calendar": {"clientId": "new-id", "clientSecret": "new-secret"}, "telegram": {"botToken": "222"}, "a": 9, "b": 8, "c": 7});
        let text = note_text(&before, &after).unwrap();
        assert_eq!(text, "The owner changed `a` from 1 to 9; `b` from 2 to 8; `c` from 3 to 7; and 3 more.");
        let only = note_text(&json!({"calendar": {"clientId": "old-id", "clientSecret": "old-secret"}, "telegram": {"botToken": "111"}}), &json!({"calendar": {"clientId": "new-id", "clientSecret": "new-secret"}, "telegram": {"botToken": "222"}})).unwrap();
        assert_eq!(only, "The owner changed `calendar.clientId` (value not shown); `calendar.clientSecret` (value not shown); `telegram.botToken` (value not shown).");
        for secret in ["old-id", "new-id", "old-secret", "new-secret", "111", "222"] {
            assert!(!only.contains(secret), "{only}");
        }
    }

    #[test]
    fn a_long_value_is_cut_and_a_list_is_one_value() {
        let long = "x".repeat(200);
        let text = note_text(&json!({"toolGroups": ["a"], "persona": {"name": "n"}}), &json!({"toolGroups": ["a", "b"], "persona": {"name": long}})).unwrap();
        assert!(text.contains("`toolGroups` from [\"a\"] to [\"a\",\"b\"]") && text.contains(&format!("`persona.name` from \"n\" to \"{}...", "x".repeat(79))), "{text}");
    }
}
