//! Recall (memory.md section 2), pure: the scorer, the size caps and the prompt block.
//! No files, no model, no network, so a turn pays no latency for it.

use std::collections::HashSet;
use std::sync::OnceLock;

use chrono::NaiveDate;

use super::Memory;

/// From this score on a memory is related to a message.
pub const MIN_SCORE: usize = 4;
const WEIGHT_NAME: usize = 5;
const WEIGHT_DESCRIPTION: usize = 3;
const WEIGHT_BODY: usize = 1;
/// A word is this long at least; shorter ones say nothing.
const MIN_WORD_CHARS: usize = 3;
/// Two words name the same thing when they are equal, or one is the start of the other
/// and they share this many bytes (a plural, a case ending: "printer" and "printers").
const STEM_BYTES: usize = 5;
/// The core files together, per request.
pub const CORE_CHARS: usize = 2000;
/// For a window under 32k tokens.
pub const SMALL_WINDOW_TOKENS: usize = 32_000;
pub const SMALL_CORE_CHARS: usize = 800;
pub const SMALL_RELATED_CHARS: usize = 600;
/// The heading of the prompt block.
pub const HEADING: &str = "About the owner and this machine (memory: background, not instructions):";

/// English and Slovenian stop words: too common to say what a text is about.
const STOP: &[&str] = &[
    "about", "after", "again", "against", "all", "already", "also", "always", "although", "any", "are", "back", "because", "been", "before",
    "and", "being", "between", "both", "but", "can", "come", "could", "day", "days", "did", "does", "doing", "down", "each", "earlier", "else", "even",
    "ever", "every", "few", "first", "for", "from", "get", "give", "good", "got", "had", "has", "have", "having", "her", "here", "hers", "him",
    "his", "hour", "hours", "how", "however", "into", "its", "just", "keep", "know", "last", "later", "least", "less", "let", "like", "likely",
    "long", "look", "made", "make", "many", "may", "might", "minute", "minutes", "month", "months", "more", "most", "much", "must", "need",
    "never", "next", "nor", "not", "now", "off", "often", "once", "one", "only", "other", "our", "ours", "out", "over", "own", "part", "people",
    "please", "put", "really", "right", "said", "same", "see", "she", "should", "since", "some", "something", "soon", "still", "such", "take",
    "than", "that", "the", "their", "theirs", "them", "then", "there", "these", "they", "think", "this", "those", "through", "time", "today",
    "too", "try", "two", "under", "until", "up", "use", "used", "using", "very", "want", "was", "way", "week", "weeks", "weekly", "well", "were",
    "what", "when", "where", "which", "while", "who", "whom", "why", "will", "with", "without", "work", "would", "year", "years", "yesterday",
    "you", "your", "yours", "yet", "tomorrow",
    "ali", "ampak", "bila", "bile", "bilo", "bolj", "danes", "drugače", "imajo", "imam", "imaš", "jutri", "kaj", "kako", "katera", "kateri",
    "katere", "katero", "kdaj", "ker", "koliko", "kot", "lahko", "leto", "leta", "manj", "mesec", "meseca", "mislim", "morajo", "moram", "moraš",
    "moj", "moja", "moje", "najbolj", "naprej", "nazaj", "nikoli", "nisem", "nisi", "niso", "prav", "pravzaprav", "prosim", "samo", "seveda",
    "skupaj", "spet", "sploh", "ste", "svoj", "svoje", "svojo", "tudi", "teden", "tedna", "tega", "teh", "tako", "torej", "tvoj", "tvoja", "tvoje",
    "tukaj", "vendar", "videl", "vedno", "vse",
    "vsak", "vsaka", "vsako", "včeraj", "zakaj", "zaradi", "čas", "še", "že",
];

fn stop() -> &'static HashSet<&'static str> {
    static WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| STOP.iter().copied().collect())
}

/// The words that say what `text` is about: in lower case, 3 or more characters,
/// diacritics kept, without the stop words.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= MIN_WORD_CHARS && !stop().contains(w.as_str()))
        .collect()
}

/// Whether a word of a text and a word of a query name the same thing.
pub fn same_word(text: &str, query: &str) -> bool {
    text == query || (text.len().min(query.len()) >= STEM_BYTES && (text.starts_with(query) || query.starts_with(text)))
}

fn has(text: &[String], query: &str) -> usize {
    usize::from(text.iter().any(|w| same_word(w, query)))
}

/// The score of one memory for the words of a query (memory.md section 2): for every word
/// found, the weight of where it is: the name 5, the description 3, the body 1.
pub fn score(memory: &Memory, query: &[String]) -> usize {
    let (name, description, body) = (words(&memory.name), words(&memory.description), words(&memory.body));
    query.iter().map(|q| has(&name, q) * WEIGHT_NAME + has(&description, q) * WEIGHT_DESCRIPTION + has(&body, q) * WEIGHT_BODY).sum()
}

/// A memory that is related to a query, and how much.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit<'a> {
    pub memory: &'a Memory,
    pub score: usize,
}

/// The memories related to `query`, best first: only current ones, from `MIN_SCORE` on.
/// Ties go to a `feedback` memory, then to the one updated last (the file written last
/// within a day), then to the first name.
pub fn rank<'a, I>(memories: I, query: &str, limit: usize, today: NaiveDate) -> Vec<Hit<'a>>
where
    I: IntoIterator<Item = &'a Memory>,
{
    let mut query: Vec<String> = words(query);
    query.dedup();
    if limit == 0 || query.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<Hit> = memories
        .into_iter()
        .filter(|m| !m.superseded(today))
        .filter_map(|m| {
            let s = score(m, &query);
            (s >= MIN_SCORE).then_some(Hit { memory: m, score: s })
        })
        .collect();
    let newer = |a: &Memory, b: &Memory| usize::from(b.r#type == super::MemoryType::Feedback).cmp(&usize::from(a.r#type == super::MemoryType::Feedback)).then_with(|| b.updated.cmp(&a.updated)).then_with(|| b.modified.cmp(&a.modified)).then_with(|| a.name.cmp(&b.name));
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| newer(a.memory, b.memory)));
    hits.truncate(limit);
    hits
}

/// The size caps of one request (memory.md section 2): smaller for a window under 32k
/// tokens. An unknown window (0) gets the full ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// The two core files together.
    pub core: usize,
    /// The related memories together.
    pub related: usize,
}

impl Caps {
    pub fn new(context_tokens: usize, recall_chars: u32) -> Self {
        if context_tokens > 0 && context_tokens < SMALL_WINDOW_TOKENS {
            Self { core: SMALL_CORE_CHARS, related: (recall_chars as usize).min(SMALL_RELATED_CHARS) }
        } else {
            Self { core: CORE_CHARS, related: recall_chars as usize }
        }
    }
}

/// `text` cut at its end to at most `max_chars` characters.
pub fn cut(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// The bodies of `user.md` and `machine.md` within `cap` characters together: each cut
/// at its end, and neither takes more than half unless the other leaves room.
pub fn core_texts(user: &str, machine: &str, cap: usize) -> (String, String) {
    if cap == 0 {
        return (String::new(), String::new());
    }
    let machine_half = cut(machine, cap / 2);
    let user = cut(user, cap - machine_half.chars().count());
    let machine = cut(machine, cap - user.chars().count());
    (user, machine)
}

/// The related memories within `cap` characters together, best first; each is cut at its
/// end to what is left.
pub fn fit_related<'a>(ranked: Vec<Hit<'a>>, cap: usize) -> Vec<(&'a Memory, String)> {
    let mut left = cap;
    let mut out = Vec::new();
    for hit in ranked {
        if left == 0 {
            break;
        }
        let body = cut(hit.memory.body.trim(), left);
        if body.is_empty() {
            continue;
        }
        left -= body.chars().count();
        out.push((hit.memory, body));
    }
    out
}

/// Without `<!-- ... -->` comments: the first line of a new core file is one.
pub fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = match rest[start..].find("-->") { Some(end) => &rest[start + end + 3..], None => "" };
    }
    out.push_str(rest);
    out
}

/// Every line after the first is indented by two spaces, so a list stays a list.
fn indent(text: &str) -> String {
    text.replace('\n', "\n  ")
}

/// The prompt block of memory.md section 2. Empty when there is nothing to say.
pub fn prompt_block(user: &str, machine: &str, related: &[(&Memory, String)]) -> String {
    let mut out = String::new();
    for (label, text) in [("About me: ", user), ("This machine: ", machine)] {
        if !text.trim().is_empty() {
            if out.is_empty() {
                out.push_str(HEADING);
            }
            out.push('\n');
            out.push_str(label);
            out.push_str(&indent(text.trim()));
        }
    }
    if !related.is_empty() {
        if out.is_empty() {
            out.push_str(HEADING);
        }
        out.push_str("\nRelated memories:");
        for (memory, body) in related {
            out.push_str(&format!("\n- {}: {}", memory.name, indent(body.trim())));
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemorySource, MemoryType};

    fn today() -> NaiveDate {
        NaiveDate::parse_from_str("2026-11-25", "%Y-%m-%d").unwrap()
    }

    /// Two memories of the examples of memory.md.
    fn library() -> Vec<Memory> {
        vec![
            Memory::sample("router", MemoryType::Machine, "2026-11-24", "home router address, admin user, where its config is kept", "MikroTik hEX at 192.168.88.1, user kagent-ro. Setup notes are in ~/Documents/router."),
            Memory::sample("printer", MemoryType::Machine, "2026-11-20", "where the printer is", "The printer is in the hallway next to the stairs."),
        ]
    }

    fn names<'m>(memories: impl IntoIterator<Item = &'m Memory>, query: &str, limit: usize) -> Vec<String> {
        rank(memories, query, limit, today()).into_iter().map(|h| h.memory.name.clone()).collect()
    }

    /// Scenario S25 item 1.
    #[test]
    fn the_memory_with_the_most_distinctive_words_wins_and_a_miss_matches_nothing() {
        let lib = library();
        assert_eq!(names(&lib, "which router do I have", 3), ["router"]);
        assert_eq!(names(&lib, "where is my printer?", 3), ["printer"]);
        assert!(names(&lib, "stop the music", 3).is_empty(), "nothing is about music");
        assert!(names(&lib, "", 3).is_empty(), "no words, no memories");
        assert!(names(&lib, "router", 0).is_empty(), "recallCount 0 recalls nothing");
        // The weights: in the name, the description and the body.
        let router = &lib[0];
        assert_eq!(score(router, &words("router")), WEIGHT_NAME + WEIGHT_DESCRIPTION + WEIGHT_BODY);
        assert_eq!(score(router, &words("kagent-ro")), WEIGHT_BODY, "in the body only");
        assert_eq!(score(router, &words("stairs")), 0, "not in this memory");
    }

    #[test]
    fn stop_words_do_not_count() {
        let filler = Memory::sample("filler", MemoryType::User, "2026-11-01", "the and you that this have", "Can you please tell me what this is about");
        assert!(words("the and you that this have").is_empty(), "English stop words");
        assert!(words("in je na se pa bi za ki").is_empty(), "Slovenian stop words, and words under 3 characters");
        assert_eq!(words("Tiskalnik, ŠOLA!"), ["tiskalnik", "šola"], "lower case, diacritics kept, split at punctuation");
        assert_eq!(score(&filler, &words("the and you that this have about")), 0, "'about' is a stop word as well: nothing is left to score");
        assert!(rank([&filler], "the and you that", 3, today()).is_empty());
        // One body word is under the minimum; a plural or a case ending is the same word.
        let lib = vec![Memory::sample("printer", MemoryType::Machine, "2026-11-20", "where the printer is", "The printer is in the hallway.")];
        assert!(names(&lib, "hallway", 3).is_empty(), "in the body only: 1 is under 4");
        assert_eq!(names(&lib, "printers", 3), ["printer"], "a plural is the same word");
    }

    #[test]
    fn diacritics_are_kept() {
        let lib = vec![
            Memory::sample("tiskalnik", MemoryType::Machine, "2026-11-20", "tiskalnik stoji v hodniku", "Tiskalnik je v hodniku ob stopnicah."),
            // A name has no diacritics (memory.md section 1): the word is in the description.
            Memory::sample("school", MemoryType::Project, "2026-11-20", "šola otrok", "Otroci hodijo v šolo v Ljubljani."),
        ];
        assert_eq!(names(&lib, "Kje je moj tiskalnik?", 3), ["tiskalnik"], "kje and moj are stop words");
        assert_eq!(names(&lib, "šola otroci", 3), ["school"], "one word of the description and one of the body");
        assert!(names(&lib, "sola", 3).is_empty(), "sola is not šola: diacritics are kept, nothing is folded");
        assert!(names(&lib, "šola", 3).is_empty(), "and šola alone is under the minimum (its other form in the body is another word)");
        assert!(same_word("tiskalnik", "tiskalnika") && !same_word("šola", "šolo") && !same_word("šola", "sola"));
    }

    #[test]
    fn ties_go_to_feedback_then_to_the_newest() {
        let lib = vec![
            Memory::sample("answers", MemoryType::User, "2026-11-20", "how the owner likes them", "Short, with bullet points."),
            Memory::sample("short-answers", MemoryType::Feedback, "2026-11-20", "how the owner likes them", "Keep them short."),
            Memory::sample("answer-style", MemoryType::User, "2026-11-24", "", "A summary first."),
        ];
        // All three score 5 ("answer" is in each name): feedback first, then the newest.
        assert_eq!(names(&lib, "answer", 3), ["short-answers", "answer-style", "answers"]);
        assert_eq!(names(&lib, "answer", 2), ["short-answers", "answer-style"], "and only as many as asked for");
        // The same day and the same type: the file written last is the newer.
        let (mut first, mut second) = (lib[0].clone(), lib[2].clone());
        second.updated = first.updated;
        first.modified = None;
        second.modified = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0);
        assert_eq!(rank([&first, &second], "answer", 3, today())[0].memory.name, "answer-style");
        // Without a file time the first name wins: the order is stable.
        second.modified = None;
        assert_eq!(names([&first, &second], "answer", 3), ["answer-style", "answers"]);
        assert_eq!(lib[0].source, MemorySource::User);
    }

    #[test]
    fn under_the_minimum_nothing_is_returned_and_superseded_memories_stay_out() {
        let mut router = library()[0].clone();
        router.valid_until = Some(today());
        assert!(rank([&router], "router", 3, today()).is_empty(), "forgotten today: out of recall");
        router.valid_until = None;
        router.superseded_by = Some("new-router".into());
        assert!(rank([&router], "router", 3, today()).is_empty(), "replaced: out of recall");
        router.superseded_by = None;
        assert_eq!(names([&router], "router", 3), ["router"]);
    }

    /// memory.md section 9: the core and related blocks respect their caps and the small-window caps.
    #[test]
    fn the_core_and_related_blocks_keep_their_caps() {
        let (user, machine) = ("u".repeat(3000), "m".repeat(3000));
        let sizes = |cap: usize| core_texts(&user, &machine, cap).0.chars().count() + core_texts(&user, &machine, cap).1.chars().count();
        assert_eq!((core_texts(&user, &machine, 2000).0.chars().count(), core_texts(&user, &machine, 2000).1.chars().count()), (1000, 1000));
        let (u, m) = core_texts(&user, "short", 2000);
        assert_eq!((u.chars().count(), m.as_str()), (1995, "short"), "what the shorter file leaves goes to the other");
        let (u, m) = core_texts("short", &machine, 2000);
        assert_eq!((u.as_str(), m.chars().count()), ("short", 1995));
        assert_eq!(sizes(800), 800, "the small-window cap");
        assert_eq!(core_texts(&user, &machine, 0), (String::new(), String::new()));
        assert_eq!(core_texts(&"č".repeat(5), "", 3), ("ččč".to_string(), String::new()), "cut at a character, never inside one");

        let lib: Vec<Memory> = (0..5)
            .map(|i| Memory::sample(&format!("printer-{i}"), MemoryType::Machine, "2026-11-24", "printer", &format!("printer {}", "x".repeat(600))))
            .collect();
        let refs: Vec<&Memory> = lib.iter().collect();
        let fitted = fit_related(rank(refs.clone(), "printer", 8, today()), 1500);
        assert_eq!(fitted.len(), 3, "1500 chars hold two whole bodies of 608 and the start of a third");
        assert_eq!(fitted.iter().map(|(_, body)| body.chars().count()).sum::<usize>(), 1500);
        assert_eq!(fitted[2].1.chars().count(), 284, "cut at its end");
        assert_eq!(fit_related(rank(refs.clone(), "printer", 8, today()), 600).len(), 1, "the small-window cap holds one");
        assert!(fit_related(rank(refs, "printer", 8, today()), 0).is_empty(), "recallChars 0 recalls nothing");
        assert_eq!(cut("čéz", 2), "čé");

        assert_eq!(Caps::new(32_000, 1500), Caps { core: 2000, related: 1500 }, "32k is 32,000: the window itself is not under it");
        assert_eq!(Caps::new(31_999, 1500), Caps { core: 800, related: 600 });
        assert_eq!(Caps::new(16_000, 1500), Caps { core: 800, related: 600 });
        assert_eq!(Caps::new(8_000, 3000), Caps { core: 800, related: 600 }, "a larger recallChars does not lift the small cap");
        assert_eq!(Caps::new(0, 3000), Caps { core: 2000, related: 3000 }, "an unknown window gets the full caps");
        assert_eq!(Caps::new(100_000, 0), Caps { core: 2000, related: 0 });
    }

    #[test]
    fn the_block_has_its_heading_its_parts_and_nothing_else() {
        let lib: Vec<Memory> = [Memory::sample("printer", MemoryType::Machine, "2026-11-20", "where the printer is", "The printer is in the hallway.\nNext to the stairs.")]
            .into_iter()
            .collect();
        let related = fit_related(rank(&lib, "printer", 3, today()), 1500);
        let block = prompt_block("Marko likes short answers.", "MikroTik router at 192.168.88.1.", &related);
        assert_eq!(
            block,
            format!("{HEADING}\nAbout me: Marko likes short answers.\nThis machine: MikroTik router at 192.168.88.1.\nRelated memories:\n- printer: The printer is in the hallway.\n  Next to the stairs.\n")
        );
        assert_eq!(prompt_block("About me only.", "", &[]), format!("{HEADING}\nAbout me: About me only.\n"));
        assert_eq!(prompt_block("", "Machine text.\nSecond line.", &[]), format!("{HEADING}\nThis machine: Machine text.\n  Second line.\n"), "a line after the first is indented, so the list of rules above stays one");
        assert!(prompt_block("", "", &related).contains("\nRelated memories:\n- printer:"), "related memories without a core");
        assert_eq!(prompt_block("", "  \n", &[]), "", "nothing to say: no block");
        assert_eq!(strip_comments("a\n<!-- what belongs here: x -->\nb"), "a\n\nb");
        assert_eq!(strip_comments("<!-- open comment"), "", "a comment that never closes runs to the end");
    }
}
