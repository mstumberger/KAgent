//! The memory file format (memory.md section 1): frontmatter and body. Name, description
//! and body are read with the skills parser (`frontmatter::parse`), so quoted scalars and
//! block scalars read back; the other keys are plain `key: value` lines, read here.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use chrono::NaiveDate;
use kagent_core::skills::frontmatter;

use super::{Memory, MemorySource, MemoryType, MACHINE, USER};

/// The type of a core file; `None` for a topic memory.
pub(super) fn core_type(name: &str) -> Option<MemoryType> {
    match name {
        USER => Some(MemoryType::User),
        MACHINE => Some(MemoryType::Machine),
        _ => None,
    }
}

/// The memory in the file `name` (`raw` is the file). `None` when its frontmatter names
/// a memory that is not a valid id. A key that is missing, or not understood, takes its
/// default: a file without `source` and `reviewed` is the owner's. A file without
/// frontmatter is all body (the core files are written so).
pub(super) fn parse(name: &str, raw: &str, today: NaiveDate) -> Option<Memory> {
    let doc = frontmatter::parse(raw)?;
    let core = core_type(name);
    let mut m = Memory {
        name: name.to_string(),
        description: if has_frontmatter(raw) { doc.description.trim().to_string() } else { String::new() },
        body: doc.body,
        r#type: core.unwrap_or_default(),
        created: today,
        updated: today,
        source: MemorySource::User,
        reviewed: true,
        valid_until: None,
        superseded_by: None,
        modified: None,
        bytes: raw.len() as u64,
    };
    let (mut created, mut updated) = (None, None);
    for (key, value) in scalars(raw) {
        let value = unquote(value);
        match key {
            "type" if core.is_none() => m.r#type = MemoryType::parse(value).unwrap_or_default(),
            "created" => created = date(value),
            "updated" => updated = date(value),
            "source" => m.source = MemorySource::parse(value).unwrap_or_default(),
            "reviewed" => m.reviewed = value.eq_ignore_ascii_case("true"),
            "validUntil" if core.is_none() => m.valid_until = date(value),
            "supersededBy" if core.is_none() => m.superseded_by = (!value.is_empty()).then(|| value.to_string()),
            _ => {}
        }
    }
    m.created = created.unwrap_or(today);
    m.updated = updated.or(created).unwrap_or(today);
    Some(m)
}

/// The file of a memory: its frontmatter (a core file has none), then its body.
pub(super) fn render(m: &Memory) -> String {
    let mut out = String::new();
    if core_type(&m.name).is_none() {
        out.push_str("---\n");
        out.push_str(&format!("name: {}\n", m.name));
        out.push_str(&format!("description: {}\n", description_line(&m.description)));
        out.push_str(&format!("type: {}\n", m.r#type.as_str()));
        out.push_str(&format!("created: {}\n", m.created));
        out.push_str(&format!("updated: {}\n", m.updated));
        out.push_str(&format!("source: {}\n", m.source.as_str()));
        out.push_str(&format!("reviewed: {}\n", m.reviewed));
        if let Some(day) = m.valid_until {
            out.push_str(&format!("validUntil: {day}\n"));
        }
        if let Some(by) = &m.superseded_by {
            out.push_str(&format!("supersededBy: {by}\n"));
        }
        out.push_str("---\n");
    }
    let body = m.body.trim();
    out.push_str(body);
    if !body.is_empty() {
        out.push('\n');
    }
    out
}

/// The description as written after `description:`. Plain, unless a plain scalar would
/// not read back (one that starts with a quote, a comment mark or a block mark): then a
/// quoted string.
fn description_line(description: &str) -> String {
    if description.is_empty() {
        return String::new();
    }
    if description.starts_with(['"', '\'', '#', '>', '|']) { serde_json::to_string(description).expect("a string") } else { description.to_string() }
}

/// The text on one line, without control characters: what a description is stored as.
pub(super) fn one_line(text: &str) -> String {
    text.chars().filter(|c| c.is_whitespace() || !c.is_control()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first line with text, cut to `max` characters: what stands in for a missing
/// description.
pub(super) fn first_line(text: &str, max: usize) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).map_or_else(String::new, |l| l.chars().take(max).collect())
}

fn has_frontmatter(raw: &str) -> bool {
    raw.strip_prefix('\u{feff}').unwrap_or(raw).lines().next().is_some_and(|first| first.trim_end() == "---")
}

/// The `key: value` lines of the frontmatter, in the order of the file.
fn scalars(raw: &str) -> Vec<(&str, &str)> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut lines = raw.lines();
    if !lines.next().is_some_and(|first| first.trim_end() == "---") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        // A comment, or a line inside a block scalar, is no key.
        if key.starts_with([' ', '\t', '#']) {
            continue;
        }
        out.push((key.trim(), value.trim()));
    }
    out
}

/// `YYYY-MM-DD`, also at the start of a longer timestamp; `None` in any other form.
fn date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.get(..10).unwrap_or(value), "%Y-%m-%d").ok()
}

/// A value without the quotes a hand-written file may have around it.
fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && matches!(bytes[0], b'"' | b'\'') && bytes[bytes.len() - 1] == bytes[0] { &value[1..value.len() - 1] } else { value }
}

/// Write `text` to `path` in one step, readable by the owner only (memory.md section 8:
/// the files are 0600, the folder 0700, because a folder of 0600 could not be entered).
pub(super) fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(text.as_bytes())
    };
    match write() {
        Ok(()) => std::fs::rename(&tmp, path),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::parse_from_str("2026-11-25", "%Y-%m-%d").unwrap()
    }

    fn day(d: &str) -> NaiveDate {
        NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn a_memory_round_trips_through_its_file() {
        // The example of memory.md section 1.
        let mut router = Memory::sample(
            "router",
            MemoryType::Machine,
            "2026-11-24",
            "home router address, admin user, where its config is kept",
            "MikroTik hEX at 192.168.88.1, user kagent-ro. Setup notes are in ~/Documents/router.",
        );
        router.source = MemorySource::Agent;
        router.reviewed = false;
        let text = render(&router);
        assert!(
            text.starts_with("---\nname: router\ndescription: home router address, admin user, where its config is kept\ntype: machine\ncreated: 2026-01-01\nupdated: 2026-11-24\nsource: agent\nreviewed: false\n---\nMikroTik"),
            "{text}"
        );
        let mut back = parse("router", &text, today()).unwrap();
        back.bytes = router.bytes;
        assert_eq!(back, router, "what is written reads back as it was");

        // Paragraphs and an indented line, a description with a colon and one that starts
        // with a quote, diacritics, an empty description, both optional keys.
        for (description, body) in [
            ("router: admin user kagent-ro", "First paragraph.\n\nSecond one, with Š and ž.\n  indented line"),
            ("\"quoted\" is how it starts", "Češnja in šola."),
            ("", "Only a body."),
        ] {
            let mut m = Memory::sample("note", MemoryType::Feedback, "2026-11-20", description, body);
            m.valid_until = Some(today());
            m.superseded_by = Some("new-note".into());
            let text = render(&m);
            let mut back = parse("note", &text, today()).unwrap();
            back.bytes = m.bytes;
            assert_eq!(back, m, "{text}");
        }
    }

    #[test]
    fn a_file_without_source_and_reviewed_is_the_owners() {
        // Written by hand: a plain description, no source, no reviewed.
        let hand = "---\nname: router\ndescription: home router address\ntype: machine\ncreated: 2026-11-24\nupdated: 2026-11-24\n---\nMikroTik hEX.\n";
        let m = parse("router", hand, today()).unwrap();
        assert_eq!((m.source, m.reviewed), (MemorySource::User, true), "a file without source and reviewed is the owner's");
        assert_eq!((m.created, m.updated), (day("2026-11-24"), day("2026-11-24")));
        assert_eq!(m.body, "MikroTik hEX.");

        // No frontmatter (how the core files are written): the whole text is the body, the
        // first line is no description, and the dates are today's.
        let plain = parse("machine", "MikroTik hEX at 192.168.88.1.\n<!-- a comment -->\n", today()).unwrap();
        assert_eq!((plain.body.as_str(), plain.r#type, plain.created, plain.description.as_str()), ("MikroTik hEX at 192.168.88.1.\n<!-- a comment -->", MemoryType::Machine, today(), ""));

        // Quoted values, Windows line endings, a date with a time, an unknown type.
        let quoted = "---\r\nname: r\r\ntype: \"fact\"\r\nsource: 'agent'\r\nreviewed: true\r\ncreated: 2026-11-01T10:00:00Z\r\nsupersededBy: 'r2'\r\n---\r\nbody\r\n";
        let m = parse("r", quoted, today()).unwrap();
        assert_eq!((m.r#type, m.source, m.reviewed), (MemoryType::User, MemorySource::Agent, true), "an unknown type is a user memory");
        assert_eq!((m.created, m.superseded_by.as_deref()), (day("2026-11-01"), Some("r2")));

        // The agent's own files say reviewed: false until the owner saw them.
        let unreviewed = parse("x", "---\ntype: user\nsource: agent\nreviewed: false\n---\nbody\n", today()).unwrap();
        assert!(unreviewed.unreviewed());

        // A frontmatter name that is not a valid id is skipped; one that differs from the
        // file name loses: the file name is the memory.
        assert!(parse("router", "---\nname: Bad Name\n---\nbody\n", today()).is_none());
        assert_eq!(parse("router", "---\nname: other\n---\nbody\n", today()).unwrap().name, "router");
    }

    #[test]
    fn superseded_is_a_past_valid_until_or_a_replacing_memory() {
        let mut m = Memory::sample("router", MemoryType::Machine, "2026-11-24", "", "body");
        assert!(!m.superseded(today()));
        m.valid_until = Some(day("2026-11-24"));
        assert!(m.superseded(today()), "validUntil before today");
        m.valid_until = Some(today());
        assert!(m.superseded(today()), "forgotten today: out of recall at once");
        m.valid_until = Some(day("2026-11-26"));
        assert!(!m.superseded(today()), "still valid");
        m.valid_until = None;
        m.superseded_by = Some("new-router".into());
        assert!(m.superseded(today()), "replaced");
        m.superseded_by = None;
        assert!(!m.superseded(today()));
        // A core file is never superseded, whatever a hand edit says.
        let core = parse("user", "---\nsupersededBy: x\nvalidUntil: 2020-01-01\n---\nbody\n", today()).unwrap();
        assert!(!core.superseded(today()));
    }

    #[test]
    fn one_line_and_first_line() {
        assert_eq!(one_line("  two   lines\n\rand\ttabs "), "two lines and tabs");
        assert_eq!(first_line("\n\n  The printer is in the hallway.  \nmore", 140), "The printer is in the hallway.");
        assert_eq!(first_line("", 140), "");
        assert_eq!(first_line(&"x".repeat(200), 140).chars().count(), 140);
        assert_eq!(description_line("home router address, admin user"), "home router address, admin user");
        assert_eq!(description_line("# headed"), "\"# headed\"", "a plain scalar starting with a comment mark would be dropped");
        assert_eq!(description_line(""), "");
    }
}
