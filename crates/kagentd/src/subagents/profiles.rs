//! The profile files of `<config>/agents` (agents.md section 1): read one, check one, write
//! one. The frontmatter is that of a skill: `name` and `description` are read by the skills
//! parser itself (`skills::frontmatter`); `model`, `toolGroups`, `maxIterations` and `prompt`
//! are read here. A file that is not a valid profile is not used: the caller lists it with
//! its reason (agents.md: skipped with a warning, shown in the UI).

use kagent_core::agent::children::{Profile, DEFAULT_MAX_ITERATIONS};
use kagent_core::skills::frontmatter;

use crate::sections::closest;

/// At most this many characters in the description of a profile.
pub const MAX_DESCRIPTION_CHARS: usize = 500;
/// At most this many in its `prompt`.
pub const MAX_PROMPT_CHARS: usize = 8_000;
/// `maxIterations`: 1..=80.
pub const MAX_ITERATIONS: usize = 80;
/// The keys of the frontmatter.
const KEYS: [&str; 6] = ["name", "description", "model", "toolGroups", "maxIterations", "prompt"];

/// `^[a-z0-9][a-z0-9-]{0,63}$`, the rule of a skill's name.
pub fn valid_name(name: &str) -> bool {
    frontmatter::valid_name(name)
}

/// Why `profile` is not valid: its fields against agents.md section 1. `known_groups` are
/// the tool groups a profile may name.
pub fn check(profile: &Profile, known_groups: &[&str]) -> Result<(), String> {
    if !valid_name(&profile.name) {
        return Err(format!("the name '{}' is not valid: lowercase letters, digits and hyphens, starting with a letter or a digit", profile.name));
    }
    let description = profile.description.trim();
    if description.is_empty() {
        return Err("description is required: say in a few words what the profile is for".into());
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(format!("the description has {} characters; at most {MAX_DESCRIPTION_CHARS}", description.chars().count()));
    }
    for group in &profile.tool_groups {
        if !known_groups.contains(&group.as_str()) {
            return Err(format!("toolGroups: unknown tool group '{group}' ({}), or leave the key out for every group", known_groups.join(", ")));
        }
    }
    if !(1..=MAX_ITERATIONS).contains(&profile.max_iterations) {
        return Err(format!("maxIterations must be between 1 and {MAX_ITERATIONS}, not {}", profile.max_iterations));
    }
    if profile.prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!("the prompt has {} characters; at most {MAX_PROMPT_CHARS}", profile.prompt.chars().count()));
    }
    Ok(())
}

/// The profile in the file `file` (`<name>.md`), or why there is none. The name is the
/// file's: a frontmatter `name` that differs from it is an error, not a second name for
/// the file. A key that is not one of the six is an error too (a misspelt `toolGroups`
/// would otherwise widen the profile to every group without a word).
pub fn parse(file: &str, raw: &str, known_groups: &[&str]) -> Result<Profile, String> {
    let stem = file.strip_suffix(".md").unwrap_or(file);
    let doc = frontmatter::parse(raw).ok_or_else(|| "its frontmatter name is not a valid id".to_string())?;
    let block = frontmatter_block(raw).ok_or_else(|| "no frontmatter: a `---` line, the fields, a `---` line".to_string())?;
    if !doc.name.is_empty() && doc.name != stem {
        return Err(format!("its name '{}' does not match its file name '{stem}.md'", doc.name));
    }
    let mut profile = Profile {
        name: stem.to_string(),
        description: doc.description.trim().to_string(),
        model: String::new(),
        tool_groups: Vec::new(),
        max_iterations: DEFAULT_MAX_ITERATIONS,
        prompt: String::new(),
        read_only: false,
    };
    for (key, value) in fields(&block) {
        match key.as_str() {
            "name" | "description" => {}
            "model" => profile.model = value.trim().to_string(),
            "toolGroups" => profile.tool_groups = groups(&value),
            "maxIterations" => profile.max_iterations = iterations(&value)?,
            "prompt" => profile.prompt = value.trim().to_string(),
            other => {
                let keys: Vec<String> = KEYS.iter().map(|k| k.to_string()).collect();
                return Err(match closest(other, &keys) {
                    Some(known) => format!("unknown key '{other}' (did you mean \"{known}\"?)"),
                    None => format!("unknown key '{other}': the keys are {}", KEYS.join(", ")),
                });
            }
        }
    }
    check(&profile, known_groups)?;
    Ok(profile)
}

/// The file of `profile`: what `parse` reads back.
pub fn render(profile: &Profile) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("name: {}\n", profile.name));
    out.push_str(&scalar("description", &profile.description));
    out.push_str(&if profile.model.is_empty() { "model:\n".to_string() } else { format!("model: {}\n", profile.model) });
    out.push_str(&if profile.tool_groups.is_empty() { "toolGroups:\n".to_string() } else { format!("toolGroups: [{}]\n", profile.tool_groups.join(", ")) });
    out.push_str(&format!("maxIterations: {}\n", profile.max_iterations));
    out.push_str(&literal("prompt", &profile.prompt));
    out.push_str("---\n");
    out
}

/// The lines between the two `---` lines (`None`: the file has no frontmatter).
fn frontmatter_block(raw: &str) -> Option<String> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut lines = raw.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let rest: Vec<&str> = lines.collect();
    let end = rest.iter().position(|l| l.trim_end() == "---").unwrap_or(rest.len());
    Some(rest[..end].join("\n"))
}

/// The top-level fields of a frontmatter block: each key once (its first line wins), with
/// the text of its value: a block scalar (`|`, `>`) unfolded, a quoted scalar unquoted,
/// the lines of an empty value joined. Comment lines and indented lines belong to the key
/// above them.
fn fields(block: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = block.lines().collect();
    let mut out: Vec<(String, String)> = Vec::new();
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
        if key.is_empty() || out.iter().any(|(seen, _)| seen == key) {
            continue;
        }
        let block_end = lines[i..].iter().position(|l| !(l.trim().is_empty() || l.starts_with([' ', '\t']))).unwrap_or(lines.len() - i);
        let body = &lines[i..i + block_end];
        i += block_end;
        out.push((key.to_string(), value_of(value.trim(), body)));
    }
    out
}

fn value_of(value: &str, body: &[&str]) -> String {
    match indicator(value) {
        Some((folded, strip)) => frontmatter::unfold_scalar(folded, strip, body),
        None if value.is_empty() => body.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n").trim().to_string(),
        None => unquote(value),
    }
}

/// `(folded, strip)` when `value` is a block scalar indicator (`|`, `>-`, `|+`, with a comment).
fn indicator(value: &str) -> Option<(bool, bool)> {
    let mut chars = value.chars();
    let folded = match chars.next()? {
        '>' => true,
        '|' => false,
        _ => return None,
    };
    let rest = chars.as_str();
    let (strip, rest) = match rest.chars().next() {
        Some('-') => (true, &rest[1..]),
        Some('+') => (false, &rest[1..]),
        _ => (false, rest),
    };
    let rest = rest.trim_start();
    (rest.is_empty() || rest.starts_with('#')).then_some((folded, strip))
}

/// A scalar without a pair of quotes around it.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"') || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')) {
        value[1..value.len() - 1].replace("\\n", "\n").replace("\\t", "\t")
    } else {
        value.to_string()
    }
}

/// `toolGroups` as a flow list (`[files, system]`), a comma-separated line, or a block
/// list (`- files`), with or without quotes.
fn groups(text: &str) -> Vec<String> {
    text.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .lines()
        .flat_map(|line| line.split(','))
        .map(|g| g.trim().trim_start_matches('-').trim().trim_matches(['"', '\'']).trim())
        .filter(|g| !g.is_empty())
        .map(str::to_string)
        .collect()
}

/// `maxIterations`: absent or blank is the default; a trailing comment is allowed.
fn iterations(text: &str) -> Result<usize, String> {
    let text = text.split('#').next().unwrap_or_default().trim();
    if text.is_empty() {
        return Ok(DEFAULT_MAX_ITERATIONS);
    }
    text.parse::<usize>().map_err(|_| format!("maxIterations must be a whole number, not '{text}'"))
}

/// One value on its own line, or as a literal block when it has more than one line or
/// does not start like plain text.
fn scalar(key: &str, text: &str) -> String {
    let plain = !text.contains('\n') && text.chars().next().is_none_or(char::is_alphanumeric);
    if plain { format!("{key}: {text}\n") } else { literal(key, text) }
}

/// `key: |` and the lines of `text`, indented by two spaces; `key:` alone when it is empty.
fn literal(key: &str, text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return format!("{key}:\n");
    }
    let mut out = format!("{key}: |\n");
    for line in text.lines() {
        let line = line.trim_end();
        out.push_str(if line.is_empty() { "\n" } else { "  " });
        if !line.is_empty() {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: [&str; 3] = ["files", "system", "hardware"];
    const FILE: &str = "disk-audit.md";

    fn profile() -> Profile {
        Profile {
            name: "disk-audit".into(),
            description: "use for disk usage investigations; read-only".into(),
            model: "ollama::qwen2.5:7b-instruct".into(),
            tool_groups: vec!["files".into(), "system".into(), "hardware".into()],
            max_iterations: 25,
            prompt: "Extra system prompt text for the subagent.\n\n- step one\n- step two".into(),
            read_only: false,
        }
    }

    #[test]
    fn the_profile_of_the_spec_is_read() {
        // The file of agents.md section 1.
        let raw = "---\nname: disk-audit\ndescription: use for disk usage investigations; read-only\nmodel:\ntoolGroups: [files, system, hardware]\nmaxIterations: 25\nprompt: |\n  Extra system prompt text for the subagent.\n---\n";
        let mut want = profile();
        (want.model, want.prompt, want.max_iterations) = (String::new(), "Extra system prompt text for the subagent.".into(), 25);
        assert_eq!(parse(FILE, raw, &KNOWN).unwrap(), want);
        // And what is written is read back as it was.
        assert_eq!(parse(FILE, &render(&profile()), &KNOWN).unwrap(), profile());
        let text = render(&profile());
        assert!(text.starts_with("---\nname: disk-audit\ndescription: use for disk usage investigations; read-only\nmodel: ollama::qwen2.5:7b-instruct\ntoolGroups: [files, system, hardware]\nmaxIterations: 25\nprompt: |\n  Extra system prompt text for the subagent.\n\n  - step one\n  - step two\n---\n"), "{text}");
    }

    #[test]
    fn the_other_ways_of_writing_a_value_are_read() {
        let raw = "---\ndescription: \"Quoted: with a colon\"\nmodel: 'provider::model'\ntoolGroups:\n  - \"files\"\n  - 'system'\nmaxIterations: 3 # short\nprompt: >-\n  folded\n  text\n\n  second paragraph\n---\nA body, which a profile does not use.\n";
        let p = parse("x.md", raw, &KNOWN).unwrap();
        assert_eq!(p.name, "x", "without a frontmatter name the file's is taken");
        assert_eq!((p.description.as_str(), p.model.as_str()), ("Quoted: with a colon", "provider::model"));
        assert_eq!(p.tool_groups, ["files", "system"]);
        assert_eq!(p.max_iterations, 3);
        assert_eq!(p.prompt, "folded text\nsecond paragraph");
        // A flow list with quotes, a comma-separated line, and an empty one.
        let groups_of = |line: &str| parse("x.md", &format!("---\ndescription: d\ntoolGroups: {line}\n---\n"), &KNOWN).unwrap().tool_groups;
        assert_eq!(groups_of("[\"files\", 'system']"), ["files", "system"]);
        assert_eq!(groups_of("files, hardware"), ["files", "hardware"]);
        assert_eq!(groups_of(""), Vec::<String>::new(), "empty: every group");
        // Without a model and with no frontmatter name at all.
        assert_eq!(parse("x.md", "---\ndescription: d\n---\n", &KNOWN).unwrap(), Profile { name: "x".into(), description: "d".into(), ..Profile::general() });
    }

    #[test]
    fn an_invalid_file_says_why() {
        // agents.md section 7: an invalid name or a missing description is skipped.
        let why = |file: &str, raw: &str| parse(file, raw, &KNOWN).unwrap_err();
        assert_eq!(why("Disk Audit.md", "---\ndescription: d\n---\n"), "the name 'Disk Audit' is not valid: lowercase letters, digits and hyphens, starting with a letter or a digit");
        assert_eq!(why("x.md", "---\nname: not valid\ndescription: d\n---\n"), "its frontmatter name is not a valid id");
        assert_eq!(why("x.md", "---\nname: other\ndescription: d\n---\n"), "its name 'other' does not match its file name 'x.md'");
        assert_eq!(why("x.md", "---\nmodel: m\n---\n"), "description is required: say in a few words what the profile is for");
        assert_eq!(why("x.md", "---\ndescription:   \n---\n"), "description is required: say in a few words what the profile is for");
        assert_eq!(why("x.md", "no frontmatter at all"), "no frontmatter: a `---` line, the fields, a `---` line");
        assert_eq!(why("x.md", &format!("---\ndescription: {}\n---\n", "d".repeat(501))), "the description has 501 characters; at most 500");
        assert_eq!(why("x.md", "---\ndescription: d\nmaxIterations: 0\n---\n"), "maxIterations must be between 1 and 80, not 0");
        assert_eq!(why("x.md", "---\ndescription: d\nmaxIterations: 81\n---\n"), "maxIterations must be between 1 and 80, not 81");
        assert_eq!(why("x.md", "---\ndescription: d\nmaxIterations: many\n---\n"), "maxIterations must be a whole number, not 'many'");
        let long = format!("---\ndescription: d\nprompt: |\n  {}\n---\n", "p".repeat(MAX_PROMPT_CHARS + 1));
        assert_eq!(why("x.md", &long), format!("the prompt has {} characters; at most {MAX_PROMPT_CHARS}", MAX_PROMPT_CHARS + 1));
        assert_eq!(why("x.md", "---\ndescription: d\ntoolGroups: [file]\n---\n"), "toolGroups: unknown tool group 'file' (files, system, hardware), or leave the key out for every group");
        // A misspelt key is an error, with the key it is closest to.
        assert_eq!(why("x.md", "---\ndescription: d\ntoolGroup: [files]\n---\n"), "unknown key 'toolGroup' (did you mean \"toolGroups\"?)");
        assert_eq!(why("x.md", "---\ndescription: d\npromptt: x\n---\n"), "unknown key 'promptt' (did you mean \"prompt\"?)");
        assert_eq!(why("x.md", "---\ndescription: d\nlicence: MIT\n---\n"), "unknown key 'licence': the keys are name, description, model, toolGroups, maxIterations, prompt");
        // Comment lines are not keys.
        assert!(parse("x.md", "---\n# a comment\ndescription: d\n---\n", &KNOWN).is_ok());
        assert!(check(&Profile { prompt: "é".repeat(MAX_PROMPT_CHARS + 1), ..Profile::general() }, &KNOWN).is_err());
        assert!(check(&Profile { prompt: "é".repeat(MAX_PROMPT_CHARS), ..Profile::general() }, &KNOWN).is_ok());
    }
}
