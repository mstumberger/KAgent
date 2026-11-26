//! Writing a `SKILL.md` (skills.md section 5): a new one from its parts, or the three
//! fields of one that exists, with every other line of its frontmatter kept.

/// A `SKILL.md` that `frontmatter::parse` reads back with the same fields: `name` and
/// `description` in the frontmatter, `when_to_use` only when it has text. A value that
/// would not read back as one plain line (line breaks, a leading quote or indicator,
/// `": "`) is written as a literal block, which strict YAML readers accept too.
pub fn render(name: &str, description: &str, when_to_use: &str, body: &str) -> String {
    head(name, description, when_to_use, "", body)
}

/// Change the description, `when_to_use` and body of `existing` and keep `name` and every
/// other line of its frontmatter (other keys, comments): a form that shows three fields
/// must not drop what it does not show. A file without frontmatter is written as new.
pub fn replace(existing: &str, name: &str, description: &str, when_to_use: &str, body: &str) -> String {
    let existing = existing.strip_prefix('\u{feff}').unwrap_or(existing);
    if !existing.lines().next().is_some_and(|first| first.trim_end() == "---") {
        return render(name, description, when_to_use, body);
    }
    let rest: Vec<&str> = existing.lines().skip(1).collect();
    let end = rest.iter().position(|l| l.trim_end() == "---").unwrap_or(rest.len());
    let (mut kept, mut i) = (Vec::new(), 0);
    while i < end {
        let key = if rest[i].starts_with([' ', '\t']) { None } else { rest[i].split_once(':').map(|(k, _)| k.trim()) };
        if key.is_some_and(|k| matches!(k, "name" | "description" | "when_to_use")) {
            // The field goes, with the blank and indented lines after it (its block).
            i += 1;
            while i < end && (rest[i].trim().is_empty() || rest[i].starts_with([' ', '\t'])) {
                i += 1;
            }
            continue;
        }
        kept.push(rest[i]);
        i += 1;
    }
    let more = if kept.is_empty() { String::new() } else { kept.join("\n") + "\n" };
    head(name, description, when_to_use, &more, body)
}

/// The whole file: the frontmatter (`name`, `description`, `when_to_use`, then `more`,
/// the lines kept of an older frontmatter), its closing fence and the body.
fn head(name: &str, description: &str, when_to_use: &str, more: &str, body: &str) -> String {
    let mut out = String::from("---\n");
    out.push_str(&field("name", name));
    out.push_str(&field("description", description));
    if !when_to_use.trim().is_empty() {
        out.push_str(&field("when_to_use", when_to_use));
    }
    out.push_str(more);
    out.push_str("---\n");
    out.push_str(&with_body(body));
    out
}

fn with_body(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        String::new()
    } else {
        format!("\n{body}\n")
    }
}

fn field(key: &str, value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return format!("{key}:\n");
    }
    if plain(value) {
        return format!("{key}: {value}\n");
    }
    let lines: String = value.lines().map(|l| format!("  {l}\n")).collect();
    format!("{key}: |-\n{lines}")
}

/// Safe after `key: ` for every YAML reader, not only `frontmatter::parse`.
fn plain(value: &str) -> bool {
    !value.contains(['\n', '\r'])
        && !value.starts_with(['"', '\'', '>', '|', '#', '&', '*', '!', '%', '@', '`', '[', '{'])
        && !value.starts_with("- ")
        && !value.starts_with("? ")
        && !value.contains(": ")
        && !value.contains(" #")
        && !value.ends_with(':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::frontmatter;

    const HAND_MADE: &str = "---\n# written by hand\nname: hand-made\ndescription: >-\n  the old one\n  in two lines\nlicense: MIT\nmetadata:\n  author: someone\nwhen_to_use: old case\n---\n\nOld body.\n";

    #[test]
    fn replace_keeps_what_the_form_does_not_show() {
        let out = replace(HAND_MADE, "hand-made", "the new one", "", "New body.\n");
        assert_eq!(out, "---\nname: hand-made\ndescription: the new one\n# written by hand\nlicense: MIT\nmetadata:\n  author: someone\n---\n\nNew body.\n", "{out}");
        let doc = frontmatter::parse(&out).unwrap();
        assert_eq!((doc.name.as_str(), doc.description.as_str(), doc.when_to_use.as_str(), doc.body.as_str()), ("hand-made", "the new one", "", "New body."));

        // `when_to_use` joins the frontmatter; a multi-line description becomes a literal block.
        let out = replace(HAND_MADE, "hand-made", "line one\nline two: kept", "when it rains", "Body.");
        assert!(out.starts_with("---\nname: hand-made\ndescription: |-\n  line one\n  line two: kept\nwhen_to_use: when it rains\n# written by hand\nlicense: MIT\n"), "{out}");
        let doc = frontmatter::parse(&out).unwrap();
        assert_eq!((doc.description.as_str(), doc.when_to_use.as_str()), ("line one\nline two: kept", "when it rains"));
    }

    #[test]
    fn replace_writes_a_file_without_frontmatter_as_new() {
        let out = replace("# A heading\njust text", "x", "d", "", "B");
        assert_eq!(out, "---\nname: x\ndescription: d\n---\n\nB\n");
        // An unterminated frontmatter leaves no body to keep.
        assert_eq!(replace("---\nname: x\ndescription: d\n", "x", "e", "", "B"), "---\nname: x\ndescription: e\n---\n\nB\n");
        // Every field dropped: nothing but the closing fence follows.
        assert_eq!(replace("---\nname: x\ndescription: d\n---\nb", "x", "", "", ""), "---\nname: x\ndescription:\n---\n");
    }

    #[test]
    fn render_reads_back() {
        let out = render("a", "one line", "case", "Body.");
        assert_eq!(out, "---\nname: a\ndescription: one line\nwhen_to_use: case\n---\n\nBody.\n");
        let doc = frontmatter::parse(&out).unwrap();
        assert_eq!((doc.name.as_str(), doc.description.as_str(), doc.when_to_use.as_str(), doc.body.as_str()), ("a", "one line", "case", "Body."));
        // Plain only when every YAML reader sees the same text.
        assert_eq!(field("description", "- item"), "description: |-\n  - item\n");
        assert_eq!(field("description", "Step: one"), "description: |-\n  Step: one\n");
        assert_eq!(field("description", "Step one"), "description: Step one\n");
        assert_eq!(field("description", "\"quoted\""), "description: |-\n  \"quoted\"\n");
    }
}
