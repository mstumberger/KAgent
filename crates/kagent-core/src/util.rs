//! Small helpers: UTF-8 safe truncation, output budgets, minimal JSON-schema checks.

use std::sync::OnceLock;

use regex::Regex;
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
/// The line that tells a result was cut (agent-loop.md B.2, question 4): how many of `total` items
/// are not shown and how to get them; empty when everything is shown.
pub fn more_note(total: usize, shown: usize, how: &str) -> String {
    if total <= shown {
        return String::new();
    }
    format!("... {} more not shown ({how})\n", total - shown)
}

/// `text` cut to `max` bytes with a closing note of how much was left out and how to get it.
pub fn cut_with_note(text: &str, max: usize, how: &str) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let kept = truncate_str(text, max);
    format!("{kept}\n[cut: {} of {} characters not shown ({how})]", text.chars().count() - kept.chars().count(), text.chars().count())
}

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

/// The cap of `speakable` unless a caller names one (voice.md section 7): characters.
pub const SPEAK_CHARS: usize = 800;
/// Said in place of a code block, and appended when something was cut.
const CODE_NOTE: &str = "The code is on screen.";
const MORE: &str = "More is on screen.";
/// A reply as a voice reads it (voice.md section 7): no directive lines (`::name{...}`),
/// code blocks (replaced by "The code is on screen."), link syntax (the text stays),
/// bare addresses, heading and list markers, emphasis markers; table cells are joined
/// with commas; whitespace is collapsed. Over `max_chars` characters it is cut on a
/// sentence boundary and "More is on screen." is appended, within the cap.
pub fn speakable(text: &str, max_chars: usize) -> String {
    cut_on_sentence(&plain(text), max_chars)
}

fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut code, mut directive, mut items) = (false, false, false);
    for line in text.lines() {
        let t = line.trim_start();
        if directive {
            // A directive runs to its closing brace, which may be on a later line.
            if let Some(i) = t.find('}') {
                directive = false;
                piece(&mut out, &t[i + 1..], &mut items);
            }
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            if !code {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(CODE_NOTE);
                items = false;
            }
            code = !code;
            continue;
        }
        if code {
            continue;
        }
        // `::name{...}`: a widget directive, not text.
        if let Some(rest) = t.strip_prefix("::").filter(|r| r.split_once('{').is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'))) {
            match rest.find('}') {
                Some(i) => piece(&mut out, &rest[i + 1..], &mut items),
                None => directive = true,
            }
            continue;
        }
        piece(&mut out, line, &mut items);
    }
    out
}

/// One line of prose, cleaned, after what came before: list items and table rows are
/// joined with a semicolon, the rest with a space. `items` says whether the last line
/// was one.
fn piece(out: &mut String, line: &str, items: &mut bool) {
    let t = line.trim();
    if t.is_empty() || (t.len() >= 3 && t.chars().all(|c| matches!(c, '-' | '*' | '_' | ' '))) {
        *items = false;
        return;
    }
    let (text, item) = match table(t) {
        Some(cells) => (cells, true),
        None => { let (rest, item) = markers(t); (rest.to_string(), item) }
    };
    let text = inline(&text);
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !out.is_empty() {
        if item && *items {
            out.push(';');
        }
        out.push(' ');
    }
    out.push_str(text);
    *items = item;
}

/// A table row as "cell, cell", "" for its rule row; `None` for any other line.
fn table(t: &str) -> Option<String> {
    if !t.starts_with('|') || !t[1..].contains('|') {
        return None;
    }
    let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
    let rule = |c: &str| c.is_empty() || (c.contains('-') && c.chars().all(|x| matches!(x, '-' | ':')));
    Some(if cells.iter().any(|c| rule(c)) {
        String::new()
    } else {
        cells.iter().filter(|c| !c.is_empty()).copied().collect::<Vec<_>>().join(", ")
    })
}

/// Heading, list and quote markers off a line; true when the line is a list item.
fn markers(t: &str) -> (&str, bool) {
    let mut item = false;
    let mut s = t;
    loop {
        let rest = s.trim_start();
        let bytes = rest.as_bytes();
        let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
        let after = match bytes.first() {
            Some(b'#') => Some(&rest[rest.bytes().take_while(|b| *b == b'#').count()..]),
            Some(b'>') => Some(&rest[1..]),
            Some(b'-' | b'*' | b'+') if bytes.get(1) == Some(&b' ') => Some(&rest[1..]),
            // `1. ` or `1) `
            Some(b) if b.is_ascii_digit() && matches!(bytes.get(digits), Some(b'.' | b')')) && bytes.get(digits + 1) == Some(&b' ') => Some(&rest[digits + 1..]),
            _ => None,
        };
        match after {
            Some(a) => {
                item |= !matches!(bytes.first(), Some(b'#' | b'>'));
                s = a;
            }
            None => break,
        }
    }
    (s.trim_start(), item)
}

/// Links, addresses, emphasis and code spans out of prose; whitespace collapsed. (`\>`
/// is a word boundary in this crate, so the closing bracket of an address is a class.)
fn inline(text: &str) -> String {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            (r"!\[([^\]]*)\]\([^)]*\)", "$1"),
            (r"\[([^\]]*)\]\([^)]*\)", "$1"),
            (r"\[([^\]]+)\]", "$1"),
            (r"<?https?://[^\s<>)]+[>]?", ""),
            (r"\*\*|__|~~|==", ""),
            // `_word_` is emphasis; the underscore of a name such as `max_output_tokens` stays.
            (r"(^|[^A-Za-z0-9])_([^_\s][^_]*?)_($|[^A-Za-z0-9])", "${1}${2}${3}"),
            (r"\*", ""),
            (r"`+", ""),
        ]
        .iter()
        .map(|(p, to)| (Regex::new(p).expect("a fixed expression"), *to))
        .collect()
    });
    let mut s = text.to_string();
    for (re, to) in rules {
        s = re.replace_all(&s, *to).into_owned();
    }
    for (from, to) in [("&amp;", "&"), ("&lt;", "<"), ("&gt;", ">"), ("&quot;", "\""), ("&#39;", "'"), ("&nbsp;", " ")] {
        s = s.replace(from, to);
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whole when it fits in `max_chars` characters; else up to the last sentence end (a word end when there is none), and "More is on screen.".
fn cut_on_sentence(text: &str, max_chars: usize) -> String {
    let all: Vec<char> = text.chars().collect();
    if all.len() <= max_chars {
        return text.to_string();
    }
    let room = max_chars.saturating_sub(MORE.chars().count() + 1).min(all.len());
    let head = &all[..room];
    let cut = sentence_end(head).or_else(|| word_end(head)).unwrap_or(room);
    format!("{} {MORE}", all[..cut].iter().collect::<String>().trim_end())
}

/// Where the last sentence in `chars` ends (closing quotes included), if any does.
fn sentence_end(chars: &[char]) -> Option<usize> {
    for i in (0..chars.len()).rev() {
        if matches!(chars[i], '.' | '!' | '?') {
            let mut j = i + 1;
            while j < chars.len() && matches!(chars[j], '"' | '\'' | ')' | ']' | '}' | '»') {
                j += 1;
            }
            if j == chars.len() || chars[j].is_whitespace() {
                return Some(j);
            }
        }
    }
    None
}

fn word_end(chars: &[char]) -> Option<usize> {
    (0..chars.len()).rev().find(|&i| chars[i].is_whitespace())
}

#[cfg(test)]
mod cut_tests {
    use super::*;

    #[test]
    fn a_cut_result_says_what_was_cut_and_how_to_get_the_rest() {
        assert_eq!(more_note(10, 10, "x"), "");
        assert_eq!(more_note(3, 10, "x"), "");
        assert_eq!(more_note(12, 10, "raise limit"), "... 2 more not shown (raise limit)\n");
        assert_eq!(cut_with_note("abc", 10, "x"), "abc");
        assert_eq!(cut_with_note("abcdefghij", 4, "export it"), "abcd\n[cut: 6 of 10 characters not shown (export it)]");
        // A cut in the middle of a character keeps the characters whole.
        assert!(cut_with_note("ččččč", 5, "x").starts_with("čč\n[cut: 3 of 5 characters"), "{}", cut_with_note("ččččč", 5, "x"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn speakable_reads_markdown_as_sentences() {
        assert_eq!(speakable("You are running kernel version 7.0.0-44-generic. This version was compiled on a build host named `lcy02-amd64-068`.", 800), "You are running kernel version 7.0.0-44-generic. This version was compiled on a build host named lcy02-amd64-068.");
        let md = "## Result\n\n- **CPU**: *idle* at `42` C\n- _RAM_ used: 3.1 GB\n\nSee [the docs](https://docs.example.com/x).\n> quoted ~~old~~ note\n1. first\n2) second\n";
        assert_eq!(speakable(md, 800), "Result CPU: idle at 42 C; RAM used: 3.1 GB See the docs. quoted old note first; second");
        assert_eq!(speakable("Set `max_output_tokens` to 2_048 and keep snake_case.", 800), "Set max_output_tokens to 2_048 and keep snake_case.");
        assert_eq!(speakable("Go to <https://example.com/a> or write to <a@b.cc>.", 800), "Go to or write to <a@b.cc>.");
    }

    #[test]
    fn speakable_joins_a_table_with_commas() {
        let table = "Status:\n\n| Service | State |\n|---------|-------|\n| docker | running |\n| podman | stopped |\n\nDone.";
        assert_eq!(speakable(table, 800), "Status: Service, State; docker, running; podman, stopped Done.");
    }

    #[test]
    fn speakable_replaces_code_and_directives() {
        assert_eq!(speakable("Run this:\n```bash\nsudo apt update\n```\nThen reboot.", 800), "Run this: The code is on screen. Then reboot.");
        assert_eq!(speakable("Before.\n::widget{id=\"s/1\"}\nAfter anything else?", 800), "Before. After anything else?");
        assert_eq!(speakable("::widget{id=\"s/2\"\n  title=\"x\"}\nDone.", 800), "Done.");
        assert_eq!(speakable("::not a directive", 800), "::not a directive");
    }

    #[test]
    fn speakable_cuts_a_long_text_on_a_sentence_boundary() {
        let sentence = "The quick brown fox jumps over the lazy dog again and again. ";
        let long = sentence.repeat(20);
        let out = speakable(&long, 200);
        assert!(out.ends_with(" More is on screen."), "{out}");
        assert!(out.chars().count() <= 200, "{} chars", out.chars().count());
        assert_eq!(out.trim_end_matches(" More is on screen."), sentence.repeat(2).trim_end());
        // No sentence end in the window: the cut is a word end.
        let words = "abc ".repeat(100);
        let out = speakable(&words, 50);
        assert!(out.chars().count() <= 50 && out.ends_with("abc More is on screen."), "{out}");
        assert_eq!(speakable("3.5 GB free.", 100), "3.5 GB free.");
    }

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
