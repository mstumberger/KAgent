//! Frontmatter of a `SKILL.md` (skills.md section 1): YAML between `---` lines. Writing
//! one is `file.rs`.
//!
//! Supported: `key: value`, quoted scalars, block scalars (`>`, `>-`, `|`, `|-`) with
//! indented continuation lines, and nested mappings (kept as raw text, never
//! interpreted). Comment lines and lines that are not `key: value` are ignored: a line
//! never invalidates a skill. Only a frontmatter `name` that is not a valid id does.
//! A file without frontmatter is valid too; the caller supplies the directory name.

/// Longest description after unfolding (characters).
pub const MAX_DESCRIPTION_CHARS: usize = 1024;
/// A skill without frontmatter takes its description from its first body line.
const LINE_DESCRIPTION_CHARS: usize = 140;

/// One `SKILL.md`, split into its frontmatter fields and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDoc {
    /// The frontmatter `name`; empty when absent or blank, and the caller uses the
    /// directory name.
    pub name: String,
    /// Unfolded, at most [`MAX_DESCRIPTION_CHARS`] characters. Empty when absent.
    pub description: String,
    /// `when_to_use`; empty when absent.
    pub when_to_use: String,
    /// Everything after the frontmatter, without blank lines at either end.
    pub body: String,
}

/// `^[a-z0-9][a-z0-9-]{0,63}$`
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    matches!(bytes.first(), Some(b) if b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes.len() <= 64
        && bytes[1..].iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Parse one `SKILL.md`. `None` only for a frontmatter `name` that is not a valid id;
/// a name that falls back to the directory name is checked by the caller with
/// [`valid_name`].
pub fn parse(raw: &str) -> Option<SkillDoc> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut lines = raw.lines();
    if lines.next().is_some_and(|first| first.trim_end() != "---") {
        // No frontmatter: the description is the first non-empty line of the body.
        let body = raw.trim().to_string();
        let description: String = body.lines().next().unwrap_or("").chars().take(LINE_DESCRIPTION_CHARS).collect();
        return Some(SkillDoc { name: String::new(), description, when_to_use: String::new(), body });
    }
    let rest: Vec<&str> = lines.collect();
    // An unterminated block runs to the end of the file and leaves no body.
    let end = rest.iter().position(|l| l.trim_end() == "---").unwrap_or(rest.len());
    let mut doc = parse_frontmatter(&rest[..end]);
    doc.body = rest.get(end + 1..).unwrap_or(&[]).join("\n").trim().to_string();
    if !doc.name.is_empty() && !valid_name(&doc.name) {
        return None;
    }
    Some(doc)
}

fn parse_frontmatter(lines: &[&str]) -> SkillDoc {
    let mut doc = SkillDoc { name: String::new(), description: String::new(), when_to_use: String::new(), body: String::new() };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        // The blank and indented lines after a key belong to it.
        let block_end = lines[i..]
            .iter()
            .position(|l| !(l.trim().is_empty() || l.starts_with([' ', '\t'])))
            .unwrap_or(lines.len() - i);
        let block = &lines[i..i + block_end];
        i += block_end;
        let value = value.trim();
        let text = match Block::of(value) {
            Some(kind) => kind.unfold(block),
            None if value.is_empty() => raw_block(block),
            None => unquote(value),
        };
        let field = match key {
            "name" => &mut doc.name,
            "description" => &mut doc.description,
            "when_to_use" => &mut doc.when_to_use,
            _ => continue,
        };
        if field.is_empty() {
            *field = text;
        }
    }
    doc.description = doc.description.trim().chars().take(MAX_DESCRIPTION_CHARS).collect();
    doc.when_to_use = doc.when_to_use.trim().to_string();
    doc
}

/// A block scalar indicator: `>` folded, `|` literal, each with `-` to strip the final
/// newline.
enum Block {
    Folded(bool),
    Literal(bool),
}

impl Block {
    /// `value` when it is an indicator, alone or followed by a comment.
    fn of(value: &str) -> Option<Self> {
        let mut it = value.chars();
        let base = match it.next()? {
            '>' => Self::Folded(false),
            '|' => Self::Literal(false),
            _ => return None,
        };
        let rest = it.as_str();
        let (strip, rest) = match rest.chars().next() {
            Some('-') => (true, &rest[1..]),
            Some('+') => (false, &rest[1..]),
            _ => (false, rest),
        };
        let rest = rest.trim_start();
        if !(rest.is_empty() || rest.starts_with('#')) {
            return None;
        }
        Some(match base {
            Self::Folded(_) => Self::Folded(strip),
            Self::Literal(_) => Self::Literal(strip),
        })
    }

    fn unfold(self, lines: &[&str]) -> String {
        match self {
            Self::Folded(strip) => unfold_scalar(true, strip, lines),
            Self::Literal(strip) => unfold_scalar(false, strip, lines),
        }
    }
}

/// Join the lines of a block scalar. The indentation of its first non-blank line is
/// removed from every line and trailing blank lines are dropped. Folded (`>`) joins the
/// lines of a paragraph with a space and keeps a blank line as a line break; a line
/// indented deeper than the paragraph keeps its own line. Literal (`|`) keeps every
/// line. Without `strip` the text ends with one newline.
pub fn unfold_scalar(folded: bool, strip: bool, lines: &[&str]) -> String {
    let indent = lines.iter().find(|l| !l.trim().is_empty()).map_or(0, |l| l.len() - l.trim_start().len());
    let content: Vec<&str> = lines.iter().map(|l| dedent(l, indent).trim_end()).collect();
    let end = content.iter().rposition(|l| !l.is_empty()).map_or(0, |p| p + 1);
    let mut out = String::new();
    let (mut prev, mut blanks) = (None::<&str>, 0usize);
    for line in &content[..end] {
        if line.is_empty() {
            blanks += 1;
            continue;
        }
        if let Some(before) = prev {
            let deeper = indented(line) || indented(before);
            if !folded || blanks > 0 || deeper {
                out.push_str(&"\n".repeat(blanks + usize::from(!folded || deeper)));
            } else {
                out.push(' ');
            }
        }
        out.push_str(line);
        prev = Some(line);
        blanks = 0;
    }
    if !strip && !out.is_empty() {
        out.push('\n');
    }
    out
}

fn indented(line: &str) -> bool {
    line.starts_with([' ', '\t'])
}

/// Remove up to `indent` columns of leading whitespace.
fn dedent(line: &str, indent: usize) -> &str {
    let n = line.len() - line.trim_start().len();
    line.get(n.min(indent)..).unwrap_or(line)
}

/// A nested mapping: its lines as written, not interpreted.
fn raw_block(lines: &[&str]) -> String {
    lines.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// A scalar without its quotes and escapes (`\"`, `\\`, `\n`, `\t`, `''`); any other
/// value as it is.
fn unquote(value: &str) -> String {
    let Some(&quote) = value.as_bytes().first() else { return String::new() };
    if quote != b'"' && quote != b'\'' {
        return value.to_string();
    }
    let mut out = String::new();
    let mut chars = value[1..].chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quote == b'"' => out.push(match chars.next() {
                Some('n') => '\n',
                Some('t') => '\t',
                Some(other) => other,
                None => '\\',
            }),
            '\'' if quote == b'\'' => match chars.peek() {
                Some('\'') => {
                    chars.next();
                    out.push('\'');
                }
                _ => return out, // closing quote
            },
            '"' => return out, // closing quote
            other => out.push(other),
        }
    }
    // No closing quote: the text as written.
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/skills/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path).expect("fixture")
    }

    /// (file, name, start of the description) of the real frontmatter blocks. Among
    /// them the three the S7 loader dropped: a `>` and a `|-` description.
    const REAL: [(&str, &str, &str); 13] = [
        ("academy-guide.md", "academy-guide", "Stop and check this skill before finishing any reply to a question about how to use Claude or a Claude product - it recommends matching courses, tutorials, and use cases from Claude Academy (academy.claude.com), Anthropic's learning hub."),
        ("claude-api.md", "claude-api", "Reference for the Claude API / Anthropic SDK - model ids, pricing, params, streaming, tool use, MCP, agents, caching, token counting, model migration.\nTRIGGER - read BEFORE opening the target file;"),
        ("claude-handoff.md", "claude-handoff", "Hand the current conversation off to a fresh background agent that picks up the work immediately."),
        ("code-review.md", "code-review", "Review the changes since a fixed point (commit, branch, tag, or merge-base) along two axes:"),
        ("discernment-nudge.md", "discernment-nudge", "After you give a substantive answer or draft that the user may act on - advice or recommendations, drafted artifacts such as goals, plans, pitches, proposals, or emails, estimates or projections, analysis or"),
        ("docx.md", "docx", "Use this skill whenever the user wants to create, read, edit, or manipulate Word documents (.docx files) or Word templates (.dotx files)."),
        ("frontend-crafting.md", "frontend-crafting", "Subject-driven design and implementation of user-facing interfaces. Use when building, implementing, redesigning, reviewing, or refining pages, layouts, forms, navigation, dashboards, or reusable UI components in HTML, CSS, Vue, Nuxt, React, or other frontend frameworks, including implementation-framed changes to responsive design, styling, accessibility, visual hierarchy, interaction states, or user experience. Use alongside web-debug for browser verification (root-cause investigation of unexpected behavior is debugging)."),
        ("graphify.md", "graphify", "any input (code, docs, papers, images) - knowledge graph - clustered communities - HTML + JSON + audit report"),
        ("imagegen.md", "imagegen", "Generate or edit raster images when the task benefits from AI-created bitmap visuals"),
        ("pr.md", "pr", "Use when writing a PR body."),
        ("skill-creator.md", "skill-creator", "Guide for creating effective skills. This skill should be used when users want to create a new skill (or update an existing skill) that extends Codex's capabilities with specialized knowledge, workflows, or tool integrations."),
        ("social-media-manager.md", "social-media-manager", "Plan social media: strategy, content calendar, community management, follower growth. Use whenever the user asks what to post and when, or how to grow followers. Single posts: social-content."),
        ("xlsx.md", "xlsx", "Use this skill any time a spreadsheet file is the primary input or output."),
    ];

    #[test]
    fn real_frontmatter_fixtures_parse() {
        // Scenario S18 item 1.
        for (file, name, start) in REAL {
            let doc = parse(&fixture(file)).unwrap_or_else(|| panic!("{file} must parse"));
            assert_eq!(doc.name, name, "{file}");
            assert!(doc.description.starts_with(start), "{file}: {:?}", doc.description);
            assert!(doc.body.starts_with("Body stub:"), "{file}: {:?}", doc.body);
            assert!(doc.description.chars().count() <= MAX_DESCRIPTION_CHARS, "{file}");
        }
        let api = parse(&fixture("claude-api.md")).unwrap();
        assert!(api.description.contains("\nTRIGGER - read BEFORE opening the target file"), "a literal block keeps its line breaks");
        assert!(api.description.contains("SKIP only when another provider is being worked on"), "{}", api.description);
        let nudge = parse(&fixture("discernment-nudge.md")).unwrap();
        assert!(nudge.description.ends_with("the skill file explains these boundaries and the exact output format."), "a folded block is one paragraph: {}", nudge.description);
        let review = parse(&fixture("code-review.md")).unwrap();
        assert!(review.description.ends_with("or asks to \"review since X\"."), "a quoted scalar loses its quotes and escapes: {}", review.description);
    }

    #[test]
    fn comments_nested_mappings_and_other_keys_are_ignored() {
        let doc = parse(&fixture("comments.md")).unwrap();
        assert_eq!(doc.name, "with-comments");
        assert_eq!(doc.description, "Check that comments and blank lines inside frontmatter are ignored.");
        assert_eq!(doc.when_to_use, "when a skill file is written or edited by hand.");
        assert_eq!(doc.body, "Body stub: synthetic fixture.");
    }

    #[test]
    fn no_frontmatter_is_valid_and_an_invalid_name_is_skipped() {
        let doc = parse(&fixture("no-frontmatter.md")).unwrap();
        assert_eq!(doc.name, "", "the directory name is used");
        assert_eq!(doc.description, "# Exploratory Test Skill");
        assert!(doc.body.starts_with("# Exploratory Test Skill\n") && doc.body.ends_with("part of the fixture."));
        assert!(parse(&fixture("invalid-name.md")).is_none(), "a name that is not a valid id");
        let blank = parse("---\nname: \ndescription: d\n---\nb").unwrap();
        assert_eq!((blank.name.as_str(), blank.body.as_str()), ("", "b"), "a blank name counts as absent");
    }

    #[test]
    fn names() {
        for good in ["a", "0", "slow-machine", "a1-b2", &"a".repeat(64)] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "-a", "A", "a_b", "a b", "a/b", "slow_machine", &"a".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn unfold_scalar_folds_and_keeps_lines() {
        assert_eq!(unfold_scalar(true, false, &["  one", "  two", "", "  three"]), "one two\nthree\n");
        assert_eq!(unfold_scalar(true, true, &["  one", "  two", "", ""]), "one two");
        assert_eq!(unfold_scalar(false, false, &["  one", "", "    deeper", "  two"]), "one\n\n  deeper\ntwo\n");
        assert_eq!(unfold_scalar(true, false, &["    one", "      code", "    two"]), "one\n  code\ntwo\n");
        assert_eq!(unfold_scalar(false, true, &[""]), "");
    }

    #[test]
    fn edges() {
        // No closing fence: the frontmatter runs to the end, the body is empty.
        assert_eq!(parse("---\nname: a\n").unwrap().body, "");
        // Blank lines at both ends of the body are removed.
        assert_eq!(parse("---\nname: a\n---\n\nbody\n\n").unwrap().body, "body");
        // A description is cut at the limit, characters not bytes.
        let long = "x".repeat(MAX_DESCRIPTION_CHARS) + &"é".repeat(50);
        assert_eq!(parse(&format!("---\ndescription: {long}\n---\n")).unwrap().description.chars().count(), MAX_DESCRIPTION_CHARS);
        // The first body line of a skill without frontmatter is cut at 140 characters.
        let line = "y".repeat(200);
        let doc = parse(&format!("{line}\nmore")).unwrap();
        assert_eq!(doc.description.chars().count(), LINE_DESCRIPTION_CHARS);
        assert_eq!(doc.body, format!("{line}\nmore"));
        // A nested mapping under a field is kept as raw text.
        let doc = parse("---\ndescription:\n  one: 1\n  two: 2\n---\n").unwrap();
        assert_eq!(doc.description, "one: 1\n  two: 2");
        // A block indicator followed by a comment, and Windows line endings.
        let doc = parse("---\r\nname: a\r\ndescription: >- # short\r\n  folded text\r\n---\r\nbody\r\n").unwrap();
        assert_eq!((doc.name.as_str(), doc.description.as_str(), doc.body.as_str()), ("a", "folded text", "body"));
    }
}
