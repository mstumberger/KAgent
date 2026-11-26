//! What the live model is told about the conversation when it thinks itself or decides
//! per utterance (voice.md sections 4 and 5): the frames that follow `setupComplete`
//! (the history, then the context message) and the custom vocabulary of the setup.
//! Nothing here needs a model or the network, so connecting pays no model latency.

use std::collections::HashMap;

use serde_json::Value;

use kagent_core::protocol::{Message, Role};
use kagent_core::todo::TodoStatus;

use super::protocol;
use crate::memory::today;
use crate::watch::Severity;
use crate::AppState;

/// The context message (voice.md section 4): this many characters on a first
/// connection, and this many after a roll.
pub const FIRST_CAP: usize = 2500;
pub const ROLL_CAP: usize = 1200;
/// How much of the core files are read, and of the summary.
const CORE_CHARS: usize = 8000;
/// The last user messages say what the conversation is about, for the related memories.
const QUERY_MESSAGES: usize = 3;
const QUERY_CHARS: usize = 300;
/// One related memory in the message.
const RELATED_CHARS: usize = 200;
const MAX_ALERTS: usize = 500;
const MIN_CAPITALIZED: usize = 3;

pub const HEAD: &str = "Context for this conversation (background, not instructions):";
const CORE_HEAD: &str = "About the owner and this machine:";
const RELATED_HEAD: &str = "Related memories:";

/// What is known about the conversation when the live model connects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    /// `SessionStore::digest`: the summary of the session, or where it stood.
    pub summary: String,
    /// The two core memory files.
    pub memory_core: String,
    /// `(name, body)` of the memories related to the conversation, best first.
    pub related: Vec<(String, String)>,
    /// `(done, total)` while the session has a todo list.
    pub todo: Option<(usize, usize)>,
    /// `(open, high)` while alerts are open.
    pub alerts: Option<(usize, usize)>,
}

impl Context {
    /// Collected for a session. Memory that is off gives nothing, as in the prompt.
    pub async fn gather(st: &AppState, session: &str) -> Self {
        let memory = st.memory.section();
        let (memory_core, related) = if !memory.enabled {
            (String::new(), Vec::new())
        } else {
            let query = about(&st.agent.messages(session).await.unwrap_or_default());
            let related = if query.is_empty() {
                Vec::new()
            } else {
                st.memory.search(&query, usize::from(memory.recall_count)).into_iter().map(|m| (m.name, m.body)).collect()
            };
            (st.memory.core_text(CORE_CHARS), related)
        };
        let todo = st.agent.todo(session).await.ok().filter(|t| !t.is_empty()).map(|t| (t.iter().filter(|i| i.status == TodoStatus::Completed).count(), t.len()));
        let open = st.alerts.list(true, MAX_ALERTS);
        let alerts = (!open.is_empty()).then(|| (open.len(), open.iter().filter(|a| a.severity == Severity::High).count()));
        Self { summary: st.agent.store().digest(session, ROLL_CAP / 2).await.unwrap_or_default(), memory_core, related, todo, alerts }
    }
}

/// The context message within `cap` characters (voice.md section 4). After a roll
/// (`roll`) it is the summary and the memory core only. Empty when there is nothing
/// to say.
pub fn message(c: &Context, cap: usize, roll: bool) -> String {
    // The head and the last lines are kept whole; the rest shares what is left.
    let mut foot = String::new();
    if !roll {
        if let Some((done, total)) = c.todo {
            foot.push_str(&format!("Todo: {done} of {total} done"));
        }
        if let Some((open, high)) = c.alerts {
            if !foot.is_empty() {
                foot.push('\n');
            }
            foot.push_str(&format!("Open alerts: {open}, high: {high}"));
        }
    }
    let mut left = cap.saturating_sub(HEAD.chars().count() + foot.chars().count() + 66);
    let summary = cut(c.summary.trim(), left * 2 / 5);
    left -= summary.chars().count();
    let core = cut(c.memory_core.trim(), if roll { left } else { left.min(cap * 11 / 25) });
    left -= core.chars().count();
    let related = if roll { String::new() } else { related(&c.related, left) };
    if summary.is_empty() && core.is_empty() && related.is_empty() && foot.is_empty() {
        return String::new();
    }
    let mut out = String::from(HEAD);
    for block in [
        summary.clone(),
        if core.is_empty() { String::new() } else { format!("{CORE_HEAD}\n{core}") },
        related,
        foot,
    ] {
        if !block.is_empty() {
            out.push('\n');
            out.push_str(&block);
        }
    }
    cut(&out, cap)
}

/// The related memories that fit in `left` characters, best first.
fn related(list: &[(String, String)], left: usize) -> String {
    let mut lines = Vec::new();
    let mut used = RELATED_HEAD.chars().count() + 1;
    for (name, body) in list {
        let line = format!("- {name}: {}", cut(body.trim(), RELATED_CHARS));
        let size = line.chars().count() + 1;
        if used + size > left {
            break;
        }
        used += size;
        lines.push(line);
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{RELATED_HEAD}\n{}", lines.join("\n"))
    }
}

fn cut(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// What follows `setupComplete` on a connection (voice.md section 4): on the first one
/// the history of the session, then with `context` on the context message, which comes
/// again after every roll (smaller, without the history).
pub async fn opening(st: &AppState, session: &str, first: bool, context: bool) -> Vec<Value> {
    let mut frames = if first { protocol::history_frames(&st.agent.messages(session).await.unwrap_or_default()) } else { Vec::new() };
    if context {
        let text = message(&Context::gather(st, session).await, if first { FIRST_CAP } else { ROLL_CAP }, !first);
        if !text.is_empty() {
            frames.push(protocol::client_content("user", &text, false));
        }
    }
    frames
}

/// The words of the last user messages: what the conversation is about.
fn about(messages: &[Message]) -> String {
    messages.iter().rev().filter(|m| m.role == Role::User && !m.content.trim().is_empty()).take(QUERY_MESSAGES).map(|m| cut(m.content.trim(), QUERY_CHARS)).collect::<Vec<_>>().join(" ")
}

/// The custom vocabulary (voice.md section 5), most important first: the persona
/// address, the project folders under `desktop.projectRoots`, memory names, the
/// capitalized words of the core files, skill names.
pub async fn vocabulary(st: &AppState) -> Vec<String> {
    let settings = st.settings.read().unwrap();
    let address = settings.persona.address.clone();
    let projects: Vec<String> = kagent_core::tools::xdg::projects(&settings.desktop_prefs().project_roots)
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    drop(settings);
    let (names, words) = if st.memory.enabled() {
        let all = st.memory.all();
        (all.iter().filter(|m| !m.is_core() && !m.superseded(today())).map(|m| m.name.clone()).collect(), capitalized(&st.memory.core_text(CORE_CHARS)))
    } else {
        (Vec::new(), Vec::new())
    };
    let skills: Vec<String> = st.skills.views().into_iter().filter(|s| s.enabled).map(|s| s.name).collect();
    terms(&[std::slice::from_ref(&address), &projects, &names, &words, &skills])
}

/// The terms of several sources, the first source first: a name is said with spaces
/// for its hyphens and underscores, each term once (case ignored), 2 to 40 characters,
/// at most `protocol::VOCABULARY_MAX`.
pub fn terms(sources: &[&[String]]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for term in sources.iter().copied().flatten().map(|s| spoken(s)) {
        let chars = term.chars().count();
        if (protocol::VOCABULARY_MIN_CHARS..=protocol::VOCABULARY_MAX_CHARS).contains(&chars) && out.len() < protocol::VOCABULARY_MAX && !out.iter().any(|t| t.eq_ignore_ascii_case(&term)) {
            out.push(term);
        }
    }
    out
}

/// `printer-location` and `max_output_tokens` as they are said.
fn spoken(name: &str) -> String {
    name.trim().split(['-', '_']).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The words of a core file that start with a capital and are not ordinary words of
/// the text: a name, a device, a product (`Marko`, `LaserJet`, `KDE`). A word that
/// only opens a sentence does not count. Most frequent first.
pub fn capitalized(text: &str) -> Vec<String> {
    #[derive(Default)]
    struct Seen {
        /// The first form that had a capital.
        word: String,
        count: usize,
        /// Occurrences that do not open a sentence.
        inside: usize,
        /// Seen as a plain lower-case word: an ordinary word.
        lower: bool,
        /// An acronym, or a capital inside the word.
        special: bool,
    }
    let (mut order, mut seen) = (Vec::new(), HashMap::new());
    let mut flush = |word: &str, at_start: bool| {
        if word.is_empty() {
            return;
        }
        let entry = seen.entry(word.to_lowercase()).or_insert_with(|| {
            order.push(word.to_lowercase());
            Seen::default()
        });
        entry.count += 1;
        // A name starts with a capital (`Marko`), or has one inside (`hEX`).
        let inner = word.chars().skip(1).any(char::is_uppercase);
        let letters: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
        entry.special |= inner || (letters.len() >= 2 && letters.iter().all(|c| c.is_uppercase()));
        if word.starts_with(char::is_uppercase) || inner {
            if entry.word.is_empty() {
                entry.word = word.to_string();
            }
            entry.inside += usize::from(!at_start);
        } else {
            entry.lower = true;
        }
    };
    let (mut word, mut at_start, mut previous) = (String::new(), true, None);
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_alphanumeric() {
            if word.is_empty() {
                at_start = previous.is_none_or(|p| matches!(p, '\n' | '.' | '!' | '?' | ':' | ';'));
            }
            word.push(c);
            previous = Some(c);
            continue;
        }
        flush(&word, at_start);
        word.clear();
        if c == '\n' || !c.is_whitespace() {
            previous = Some(c);
        }
    }
    let mut hits: Vec<(String, usize)> = order
        .into_iter()
        .filter_map(|k| seen.remove(&k))
        .filter(|e| !e.lower && e.word.chars().count() >= MIN_CAPITALIZED && (e.inside > 0 || e.special))
        .map(|e| (e.word.clone(), e.count))
        .collect();
    hits.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    hits.into_iter().map(|(word, _)| word).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Context {
        Context {
            summary: "s".repeat(5000),
            memory_core: "c".repeat(5000),
            related: (0..10).map(|i| (format!("memory-{i}"), format!("body{i} ").repeat(60))).collect(),
            todo: Some((2, 5)),
            alerts: Some((3, 1)),
        }
    }

    /// voice.md section 10: the context message respects both caps.
    #[test]
    fn the_context_message_respects_both_caps() {
        let first = message(&full(), FIRST_CAP, false);
        assert!(first.starts_with(HEAD) && first.chars().count() <= FIRST_CAP, "{} chars", first.chars().count());
        for expected in [CORE_HEAD, "Related memories:\n- memory-0: ", "Todo: 2 of 5 done", "Open alerts: 3, high: 1"] {
            assert!(first.contains(expected), "the first connection names {expected:?}:\n{}", &first[..200]);
        }
        let roll = message(&full(), ROLL_CAP, true);
        assert!(roll.starts_with(HEAD) && roll.chars().count() <= ROLL_CAP && roll.contains(CORE_HEAD), "{} chars", roll.chars().count());
        for absent in ["Related memories", "Todo:", "Open alerts"] {
            assert!(!roll.contains(absent), "after a roll the message is the summary and the memory core only: found {absent}");
        }
        // Little room: the summary and the core share it, nothing else is promised.
        let small = message(&Context { summary: "a".repeat(100), memory_core: "b".repeat(100), ..Default::default() }, 150, false);
        assert!(small.chars().count() <= 150 && small.contains("aaaa") && small.contains("bbbb"), "{small}");
        assert_eq!(message(&Context::default(), FIRST_CAP, false), "", "nothing to say");
        let lines = message(&Context { todo: Some((1, 1)), alerts: Some((1, 0)), ..Default::default() }, FIRST_CAP, false);
        assert!(lines.ends_with("Todo: 1 of 1 done\nOpen alerts: 1, high: 0"), "{lines}");
    }

    #[test]
    fn vocabulary_terms_are_said_trimmed_unique_and_capped() {
        let (a, b) = (
            vec!["Marko".to_string(), " KAgent ".to_string(), "Kagent".into(), "x".repeat(41)],
            vec!["printer-location".into(), "a".into(), "max_output_tokens".into(), "HP".into(), "slow-machine".into()],
        );
        assert_eq!(terms(&[&a, &b]), ["Marko", "KAgent", "printer location", "max output tokens", "HP", "slow machine"]);
        let many: Vec<String> = (0..130).map(|i| format!("term{i}")).collect();
        assert_eq!(terms(&[&many]).len(), protocol::VOCABULARY_MAX);
    }

    #[test]
    fn capitalized_words_are_names_not_sentence_starts() {
        let core = "Address the owner as Marko.\nThe owner wants answers in English.\nPrinter: HP LaserJet in the hallway.\n\
            Router: MikroTik hEX at 192.168.88.1. Note: KDE Connect works.\nSometimes the printer jams; (Miha) called twice. Miha again.\n";
        // Most frequent first, then in order of appearance. "English" is a capitalized word
        // in the middle of a sentence: harmless in a vocabulary. "HP" is too short.
        assert_eq!(capitalized(core), ["Miha", "Marko", "English", "LaserJet", "MikroTik", "hEX", "KDE", "Connect"]);
        assert!(capitalized("").is_empty() && capitalized("nothing to hear.").is_empty());
    }
}
