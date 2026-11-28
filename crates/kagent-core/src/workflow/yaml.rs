//! The subset of YAML the workflow files use (workflows.md section 2): block maps and lists, flow maps and lists,
//! plain, single and double quoted scalars, comments. No anchors, tags, multi-line scalars or documents. The result is a
//! `serde_json::Value`, so the definitions are read with serde and `deny_unknown_fields` like every config type.

use serde_json::{Map, Value};

struct Line {
    no: usize,
    indent: usize,
    text: String,
}

/// Parse `src`; an error names the line.
pub fn parse(src: &str) -> Result<Value, String> {
    let mut lines = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let no = i + 1;
        if raw.contains('\t') && raw.trim_start_matches(' ').starts_with('\t') {
            return Err(format!("line {no}: a tab is not allowed in the indentation"));
        }
        let text = strip_comment(raw);
        let trimmed = text.trim_end();
        if trimmed.trim().is_empty() || trimmed.trim() == "---" {
            continue;
        }
        let indent = trimmed.len() - trimmed.trim_start().len();
        lines.push(Line { no, indent, text: trimmed.trim_start().to_string() });
    }
    if lines.is_empty() {
        return Err("the file is empty".into());
    }
    let mut p = Parser { lines, at: 0 };
    let indent = p.lines[0].indent;
    let value = p.node(indent)?;
    if let Some(line) = p.lines.get(p.at) {
        return Err(format!("line {}: unexpected indentation", line.no));
    }
    Ok(value)
}

/// Cut a comment: a `#` at the start or after whitespace, outside quotes.
fn strip_comment(raw: &str) -> String {
    let mut out = String::new();
    let (mut single, mut double, mut prev_space) = (false, false, true);
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if double => {
                out.push(c);
                if let Some(n) = chars.next() {
                    out.push(n);
                }
                prev_space = false;
                continue;
            }
            '"' if !single => double = !double,
            '\'' if !double => single = !single,
            '#' if !single && !double && prev_space => break,
            _ => {}
        }
        prev_space = c == ' ';
        out.push(c);
    }
    out
}

struct Parser {
    lines: Vec<Line>,
    at: usize,
}

fn is_seq_marker(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// Where the `key:` ends in `text`: the first `:` outside quotes and brackets that is followed by a space or the end.
fn colon_at(text: &str) -> Option<usize> {
    let (mut single, mut double, mut depth) = (false, false, 0i32);
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (k, &(i, c)) in chars.iter().enumerate() {
        match c {
            '"' if !single => double = !double,
            '\'' if !double => single = !single,
            '{' | '[' if !single && !double => depth += 1,
            '}' | ']' if !single && !double => depth -= 1,
            ':' if !single && !double && depth == 0 => {
                if chars.get(k + 1).is_none_or(|&(_, n)| n == ' ') {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

impl Parser {
    fn node(&mut self, indent: usize) -> Result<Value, String> {
        let line = &self.lines[self.at];
        if is_seq_marker(&line.text) {
            self.seq(indent)
        } else if colon_at(&line.text).is_some() {
            self.map(indent)
        } else {
            let (no, text) = (line.no, line.text.clone());
            self.at += 1;
            scalar(&text).map_err(|e| format!("line {no}: {e}"))
        }
    }

    fn seq(&mut self, indent: usize) -> Result<Value, String> {
        let mut items = Vec::new();
        while let Some(line) = self.lines.get(self.at) {
            if line.indent != indent || !is_seq_marker(&line.text) {
                break;
            }
            let (no, rest) = (line.no, line.text[1..].trim_start().to_string());
            if rest.is_empty() {
                self.at += 1;
                match self.lines.get(self.at) {
                    Some(next) if next.indent > indent => {
                        let i = next.indent;
                        items.push(self.node(i)?);
                    }
                    _ => items.push(Value::Null),
                }
            } else if !rest.starts_with(['{', '[', '"', '\'']) && colon_at(&rest).is_some() {
                // `- key: value` opens a map whose keys line up with `key`.
                let inner = indent + (self.lines[self.at].text.len() - rest.len());
                self.lines[self.at] = Line { no, indent: inner, text: rest };
                items.push(self.map(inner)?);
            } else {
                self.at += 1;
                items.push(scalar(&rest).map_err(|e| format!("line {no}: {e}"))?);
            }
        }
        Ok(Value::Array(items))
    }

    fn map(&mut self, indent: usize) -> Result<Value, String> {
        let mut map = Map::new();
        while let Some(line) = self.lines.get(self.at) {
            if line.indent < indent {
                break;
            }
            if line.indent > indent {
                return Err(format!("line {}: unexpected indentation", line.no));
            }
            if is_seq_marker(&line.text) {
                break;
            }
            let no = line.no;
            let Some(c) = colon_at(&line.text) else { return Err(format!("line {no}: expected `key: value`")) };
            let key = unquote_key(line.text[..c].trim()).map_err(|e| format!("line {no}: {e}"))?;
            let rest = line.text[c + 1..].trim().to_string();
            if map.contains_key(&key) {
                return Err(format!("line {no}: the key `{key}` is given twice"));
            }
            self.at += 1;
            let value = if rest.is_empty() {
                match self.lines.get(self.at) {
                    Some(next) if next.indent > indent || (next.indent == indent && is_seq_marker(&next.text)) => {
                        let i = next.indent;
                        self.node(i)?
                    }
                    _ => Value::Null,
                }
            } else if rest.starts_with(['|', '>']) {
                return Err(format!("line {no}: multi-line text (| and >) is not supported: write the text on one line"));
            } else {
                scalar(&rest).map_err(|e| format!("line {no}: {e}"))?
            };
            map.insert(key, value);
        }
        Ok(Value::Object(map))
    }
}

fn unquote_key(key: &str) -> Result<String, String> {
    if key.starts_with(['"', '\'']) {
        match scalar(key)? {
            Value::String(s) => Ok(s),
            _ => Err("a key must be text".into()),
        }
    } else if key.is_empty() {
        Err("a key is empty".into())
    } else {
        Ok(key.to_string())
    }
}

/// A value in block position: a flow collection, a quoted text, or a plain scalar to the end of the line.
fn scalar(text: &str) -> Result<Value, String> {
    let text = text.trim();
    if text.starts_with(['{', '[', '"', '\'']) {
        let mut f = Flow { chars: text.chars().collect(), at: 0 };
        let v = f.value()?;
        f.skip_ws();
        if f.at < f.chars.len() {
            return Err(format!("unexpected text after the value: `{}`", f.chars[f.at..].iter().collect::<String>()));
        }
        return Ok(v);
    }
    if text.starts_with(['&', '*', '!']) {
        return Err("anchors, aliases and tags are not supported".into());
    }
    Ok(plain(text))
}

fn plain(text: &str) -> Value {
    match text {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" | "~" => Value::Null,
        t => match t.parse::<i64>() {
            Ok(n) if !t.starts_with('+') && !(t.len() > 1 && t.starts_with('0')) => Value::from(n),
            _ => Value::String(t.to_string()),
        },
    }
}

struct Flow {
    chars: Vec<char>,
    at: usize,
}

impl Flow {
    fn skip_ws(&mut self) {
        while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
            self.at += 1;
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.skip_ws();
        match self.chars.get(self.at) {
            None => Err("a value is missing".into()),
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some(&q @ ('"' | '\'')) => self.quoted(q).map(Value::String),
            Some(_) => {
                let start = self.at;
                while self.chars.get(self.at).is_some_and(|c| !matches!(c, ',' | '}' | ']')) {
                    self.at += 1;
                }
                Ok(plain(self.chars[start..self.at].iter().collect::<String>().trim()))
            }
        }
    }

    fn quoted(&mut self, q: char) -> Result<String, String> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let Some(&c) = self.chars.get(self.at) else { return Err("a quoted text is not closed".into()) };
            self.at += 1;
            match c {
                c if c == q => {
                    // '' inside single quotes is one quote.
                    if q == '\'' && self.chars.get(self.at) == Some(&'\'') {
                        out.push('\'');
                        self.at += 1;
                    } else {
                        return Ok(out);
                    }
                }
                '\\' if q == '"' => {
                    let Some(&n) = self.chars.get(self.at) else { return Err("a quoted text is not closed".into()) };
                    self.at += 1;
                    out.push(match n {
                        'n' => '\n',
                        't' => '\t',
                        '"' => '"',
                        '\\' => '\\',
                        '/' => '/',
                        other => return Err(format!("the escape \\{other} is not supported")),
                    });
                }
                c => out.push(c),
            }
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.chars.get(self.at) {
                Some(']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                None => return Err("a list is not closed".into()),
                _ => {}
            }
            items.push(self.value()?);
            self.skip_ws();
            match self.chars.get(self.at) {
                Some(',') => self.at += 1,
                Some(']') => {}
                _ => return Err("a list is not closed".into()),
            }
        }
    }

    fn object(&mut self) -> Result<Value, String> {
        self.at += 1;
        let mut map = Map::new();
        loop {
            self.skip_ws();
            match self.chars.get(self.at) {
                Some('}') => {
                    self.at += 1;
                    return Ok(Value::Object(map));
                }
                None => return Err("a map is not closed".into()),
                _ => {}
            }
            self.skip_ws();
            let key = match self.chars.get(self.at) {
                Some(&q @ ('"' | '\'')) => self.quoted(q)?,
                _ => {
                    let start = self.at;
                    while self.chars.get(self.at).is_some_and(|c| !matches!(c, ':' | ',' | '}')) {
                        self.at += 1;
                    }
                    self.chars[start..self.at].iter().collect::<String>().trim().to_string()
                }
            };
            self.skip_ws();
            if self.chars.get(self.at) != Some(&':') {
                return Err(format!("expected `:` after `{key}` in a map"));
            }
            self.at += 1;
            let value = self.value()?;
            if map.insert(key.clone(), value).is_some() {
                return Err(format!("the key `{key}` is given twice"));
            }
            self.skip_ws();
            match self.chars.get(self.at) {
                Some(',') => self.at += 1,
                Some('}') => {}
                _ => return Err("a map is not closed".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_lists_flow_and_comments() {
        let v = parse("name: x # a comment\nsteps:\n  - id: a\n    worker: {kind: subagent, profile: p}\n    n: 3\n  - id: b\n    on: {back_to: a, max_rounds: 3}\nlist: [a, \"b, c\", 'd']\nempty:\nflag: true\n").unwrap();
        assert_eq!(
            v,
            json!({"name": "x", "steps": [{"id": "a", "worker": {"kind": "subagent", "profile": "p"}, "n": 3}, {"id": "b", "on": {"back_to": "a", "max_rounds": 3}}], "list": ["a", "b, c", "d"], "empty": null, "flag": true})
        );
    }

    #[test]
    fn a_plain_value_keeps_commas_and_double_colons_and_a_quoted_hash() {
        let v = parse("description: A strong model plans, a worker implements.\nmodel: anthropic::claude-x\nq: \"a # not a comment\"\n").unwrap();
        assert_eq!(v["description"], "A strong model plans, a worker implements.");
        assert_eq!(v["model"], "anthropic::claude-x");
        assert_eq!(v["q"], "a # not a comment");
    }

    #[test]
    fn a_list_may_sit_at_the_indent_of_its_key() {
        assert_eq!(parse("a:\n- 1\n- 2\nb: x\n").unwrap(), json!({"a": [1, 2], "b": "x"}));
    }

    #[test]
    fn errors_name_the_line() {
        assert!(parse("a: 1\na: 2\n").unwrap_err().starts_with("line 2: the key `a` is given twice"));
        assert!(parse("a: |\n  x\n").unwrap_err().starts_with("line 1: multi-line text"));
        assert!(parse("a: {b: 1\n").unwrap_err().starts_with("line 1: a map is not closed"));
        assert!(parse("a: &x 1\n").unwrap_err().contains("anchors"));
        assert!(parse("a:\n  b: 1\n    c: 2\n").unwrap_err().starts_with("line 3: unexpected indentation"));
        assert!(parse("a:\n\tb: 1\n").unwrap_err().contains("tab"));
        assert!(parse("# only a comment\n").unwrap_err().contains("empty"));
        assert!(parse("a: \"x\n").is_err());
    }
}
