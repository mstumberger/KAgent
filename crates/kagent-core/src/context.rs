//! Building the provider-visible history: repair (every tool call has exactly one
//! result) and trimming to the model's context window.

use std::collections::HashMap;

use crate::protocol::{Message, Role};

/// Rough chars-per-token used for budgeting. Deliberately low (real text is ~4) so the
/// estimate errs on the safe side for code, JSON and non-English text.
pub(crate) const CHARS_PER_TOKEN: usize = 3;

/// Repair history before sending it: drop tool results whose call is missing and add
/// synthetic results for calls without one (e.g. the daemon died mid-turn).
pub fn sanitize_history(messages: &[Message]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let mut i = 0;
    while i < messages.len() {
        let m = &messages[i];
        match m.role {
            Role::Tool => {} // orphan; results are emitted with their assistant message
            Role::User => out.push(m.clone()),
            Role::Assistant => {
                out.push(m.clone());
                let mut j = i + 1;
                let mut seen = HashMap::new();
                while j < messages.len() && messages[j].role == Role::Tool {
                    if let Some(id) = &messages[j].tool_call_id {
                        seen.insert(id.clone(), messages[j].clone());
                    }
                    j += 1;
                }
                for call in &m.tool_calls {
                    out.push(seen.remove(&call.id).unwrap_or_else(|| {
                        Message::tool_result(call, "no result recorded (interrupted)", true)
                    }));
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

pub(crate) fn message_chars(m: &Message) -> usize {
    m.content.len()
        + m.tool_calls.iter().map(|c| c.name.len() + c.input.to_string().len() + 16).sum::<usize>()
        + 16
        + m.images.iter().map(image_chars).sum::<usize>()
}

/// What an image costs in the estimate: about one token per 750 pixels (at most 1600), and a
/// token is `CHARS_PER_TOKEN` characters. An image already described is counted as its note.
fn image_chars(image: &crate::protocol::ImageRef) -> usize {
    let tokens = ((u64::from(image.width) * u64::from(image.height)) / 750).min(1_600) as usize;
    tokens * CHARS_PER_TOKEN
}

/// Token budget left for history once the system prompt, tool specs and the reply are
/// reserved. `context_tokens == 0` means unknown: no trimming.
pub fn history_budget_tokens(context_tokens: usize, max_output: usize, system_chars: usize, tools_chars: usize) -> usize {
    if context_tokens == 0 {
        return usize::MAX;
    }
    let fixed = max_output + (system_chars + tools_chars) / CHARS_PER_TOKEN;
    // Keep 10% headroom for estimation error and provider framing.
    (context_tokens * 9 / 10).saturating_sub(fixed)
}

/// Drop the oldest messages until the rest fits the budget. Cuts only at a user
/// message, so an assistant tool call is never separated from its results. The most
/// recent user message is always kept. Returns the kept messages and how many were
/// dropped; when something was dropped the first kept user message is prefixed with a
/// short note so the model knows the history is partial. `request_history` starts a
/// compacted request with its summary message.
pub fn fit_to_budget(messages: Vec<Message>, budget_tokens: usize) -> (Vec<Message>, usize) {
    fit_keeping(messages, budget_tokens, 0)
}

/// `fit_to_budget` with the first `head` messages kept whatever their size (the
/// summary message a compacted request starts with): the cut is taken among the user
/// messages after them. `dropped` counts what was dropped, the head not included.
pub(crate) fn fit_keeping(messages: Vec<Message>, budget_tokens: usize, head: usize) -> (Vec<Message>, usize) {
    if budget_tokens == usize::MAX {
        return (messages, 0);
    }
    let budget_chars = budget_tokens.saturating_mul(CHARS_PER_TOKEN);
    let total: usize = messages.iter().map(message_chars).sum();
    if total <= budget_chars {
        return (messages, 0);
    }
    let Some(mut last_user) = messages.iter().rposition(|m| m.role == Role::User) else {
        return (messages, 0);
    };
    if last_user < head {
        return (messages, 0); // only the head holds a user message: nothing to cut
    }
    last_user = last_user.max(head);
    // Earliest user message whose suffix (it and everything after, the head in front) fits; else the last user message, even if that alone is over budget.
    let head_chars: usize = messages[..head].iter().map(message_chars).sum();
    let mut suffix = total;
    let mut cut = last_user;
    for (i, m) in messages.iter().enumerate().take(last_user + 1) {
        if i >= head && m.role == Role::User && head_chars + suffix <= budget_chars {
            cut = i;
            break;
        }
        suffix -= message_chars(m);
    }
    let dropped = cut - head;
    let mut rest = messages.into_iter();
    let mut kept: Vec<Message> = rest.by_ref().take(head).collect();
    kept.extend(rest.skip(dropped));
    if dropped > 0 {
        if let Some(first) = kept.get_mut(head) {
            first.content = format!(
                "[Note: {dropped} earlier messages were omitted to fit the context window.]\n\n{}",
                first.content
            );
        }
    }
    (kept, dropped)
}

/// The summary a compacted request starts with (compaction.md section 4): the
/// synthetic user message and the number of stored messages it covers.
#[derive(Debug, Clone, Copy)]
pub struct HistorySummary<'a> {
    pub message: &'a Message,
    pub upto: usize,
}

/// The history of a model request (compaction.md section 4): with a summary, that one
/// synthetic user message, then the stored messages from `upto` on; without one, every
/// stored message, as before. Repaired, then trimmed by `fit_to_budget` when still too
/// long (the summary counts, and stays while a user turn after it can go). The
/// synthetic message is never written to the session.
pub fn request_history(stored: &[Message], summary: Option<HistorySummary<'_>>, budget_tokens: usize) -> (Vec<Message>, usize) {
    match summary {
        None => fit_to_budget(sanitize_history(stored), budget_tokens),
        Some(s) => {
            let upto = s.upto.min(stored.len());
            let mut history = Vec::with_capacity(1 + stored.len() - upto);
            history.push(s.message.clone());
            history.extend(sanitize_history(&stored[upto..]));
            fit_keeping(history, budget_tokens, 1)
        }
    }
}

/// Tool groups in the order they are kept when the window is small: diagnosis first,
/// then changes, then the rest.
pub const GROUP_PRIORITY: [&str; 18] = ["system", "kernel", "services", "hardware", "skills", "tasks", "agent", "memory", "files", "web", "calendar", "security", "change", "desktop", "shell", "trace", "router", "other"];

pub fn spec_chars(t: &crate::tool::ToolSpec) -> usize {
    t.name.len() + t.description.len() + t.input_schema.to_string().len() + 40
}

/// Words that make a tool group relevant to the request (English and Slovenian stems).
const GROUP_HINTS: [(&str, &[&str]); 9] = [
    // The agent's own todo list, plan mode and subagents (a "todo" of the owner's is a task too).
    ("agent", &["todo", "to-do", "checklist", " plan", "subagent", "sub-agent", "in parallel", "delegate", "načrt", "seznam opravil", "podagent", "vzporedno"]),
    ("tasks", &["remind", "schedule", "every day", "every morning", "every week", "tomorrow", "later", "in an hour", "in 10", "in 20", "in 30", "task", "to-do", "todo", "opomni", "jutri", "vsak dan", "naloga", "kasneje"]),
    ("desktop", &["open", "launch", "start ", "play", "pause", "stop", "music", "song", "video", "movie", "film", "vlc", "spotify", "youtube", "browser", "tab", "website", "web page", "http", "url", "click", "type ", "window", "editor", "vs code", "vscode", "kate", "intellij", "pycharm", "project", "line ", "clipboard", "copy", "paste", "volume", "louder", "quieter", "mute", "bluetooth", "wifi", "wi-fi", "vpn", "dark mode", "light mode", "notif", "screen", "this file", "terminal", "console", "konsole", "claude code", "signal", "slack", "write to", "send to", "type into", "message", "screen", "can you see", "look at", "reboot", "restart the computer", "suspend", "lock the", "lock my", "transcript", "export", "konzol", "napiši", "pošlji", "sporočil", "zaslon", "poglej", "odpri", "zaženi", "predvajaj", "ustavi", "pavz", "glasb", "pesem", "brskal", "zavih", "klikni", "okn", "projekt", "vrstic", "odložišč", "glasn", "utišaj", "temn"]),
    ("calendar", &["calendar", "event", "meeting", "schedule", "koledar", "dogodek", "sestanek"]),
    ("security", &["intrud", "hack", "honeytoken", "attack", "breach", "malware", "rootkit", "suspicious", "vdor", "napad", "sumljiv"]),
    ("trace", &["trace", "latency", "ebpf", "syscall", "who is writing", "what is writing", "retransmit", "stutter", " lag", "zatika"]),
    ("router", &["router", "mikrotik", " lan ", "traffic", "block ip", "devices on my network", "usmerjev", "promet", "omrež"]),
    ("change", &["install", "uninstall", "package", "undo", "revert", "roll back", "namesti", "odstrani", "razveljav"]),
    ("shell", &["shell", "command", "bash", "script", "ukaz"]),
];

/// Groups the request mentions, in GROUP_PRIORITY order.
pub fn hinted_groups(text: &str) -> Vec<&'static str> {
    let t = format!(" {} ", text.to_lowercase());
    GROUP_PRIORITY.iter().copied().filter(|g| GROUP_HINTS.iter().any(|(hg, words)| hg == g && words.iter().any(|w| t.contains(w)))).collect()
}

/// Keep whole tool groups until the specs would exceed `share` of the context (small
/// local models drown in 50 tool schemas). Groups the request mentions (`hint`, the
/// latest user message) go first, then GROUP_PRIORITY order. Unknown window: all.
pub fn budget_tools(specs: Vec<crate::tool::ToolSpec>, context_tokens: usize, share: f64, hint: &str) -> Vec<crate::tool::ToolSpec> {
    budget_tools_with(specs, context_tokens, share, hint, crate::tools::group_of)
}

/// `budget_tools` with the group lookup as a parameter, so a test can name a group
/// the registry does not have.
fn budget_tools_with(
    specs: Vec<crate::tool::ToolSpec>,
    context_tokens: usize,
    share: f64,
    hint: &str,
    group_of: impl Fn(&str) -> &'static str,
) -> Vec<crate::tool::ToolSpec> {
    if context_tokens == 0 {
        return specs;
    }
    let total: usize = specs.iter().map(spec_chars).sum();
    let budget = (context_tokens as f64 * share) as usize * CHARS_PER_TOKEN;
    if total <= budget {
        return specs;
    }
    let boosted = hinted_groups(hint);
    let mut order: Vec<&str> = boosted.iter().copied().chain(GROUP_PRIORITY.iter().copied().filter(|g| !boosted.contains(g))).collect();
    // A group missing from GROUP_PRIORITY (a later stage forgot the list) is still a
    // candidate: after the known ones, in registry order.
    let groups: Vec<String> = specs.iter().map(|t| group_of(&t.name).to_string()).collect();
    for g in &groups {
        if !order.contains(&g.as_str()) {
            order.push(g);
        }
    }
    let mut kept_groups: Vec<&str> = Vec::new();
    let mut used = 0;
    for g in order {
        let size: usize = specs.iter().filter(|t| group_of(&t.name) == g).map(spec_chars).sum();
        if used + size > budget && !kept_groups.is_empty() {
            continue; // a later, smaller group may still fit
        }
        used += size;
        kept_groups.push(g);
    }
    // Keep the registry order so the prompt is stable between turns.
    specs.into_iter().filter(|t| kept_groups.contains(&group_of(&t.name))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ToolCall;
    use serde_json::json;

    fn convo(turns: usize, size: usize) -> Vec<Message> {
        let mut v = Vec::new();
        for t in 0..turns {
            v.push(Message::user(format!("q{t} {}", "x".repeat(size))));
            let call = ToolCall { id: format!("c{t}"), name: "system_info".into(), input: json!({}) };
            v.push(Message::assistant("", vec![call.clone()]));
            v.push(Message::tool_result(&call, "y".repeat(size), false));
            v.push(Message::assistant(format!("a{t}"), vec![]));
        }
        v
    }

    #[test]
    fn no_trim_when_it_fits_or_budget_unknown() {
        let (kept, dropped) = fit_to_budget(convo(3, 10), 10_000);
        assert_eq!((kept.len(), dropped), (12, 0));
        let (kept, dropped) = fit_to_budget(convo(3, 10_000), history_budget_tokens(0, 0, 0, 0));
        assert_eq!((kept.len(), dropped), (12, 0));
    }

    #[test]
    fn trims_at_user_boundary_and_keeps_pairs() {
        // Each turn is ~2 KB; budget for ~2 turns.
        let (kept, dropped) = fit_to_budget(convo(6, 1000), 1500);
        assert!(dropped > 0);
        assert_eq!(kept[0].role, Role::User);
        assert!(kept[0].content.starts_with("[Note:"));
        // Every assistant tool call is still followed by its result.
        let fixed = sanitize_history(&kept);
        assert_eq!(fixed.len(), kept.len());
        assert!(kept.last().unwrap().content.starts_with("a5"));
    }

    #[test]
    fn a_summary_starts_the_request_and_only_the_messages_after_it_follow() {
        let summary = Message::user("Conversation so far (summary of the first 8 messages):\nEarlier: q0 and q1 were answered.");
        // Without a summary the request is what it was.
        let (plain, dropped) = request_history(&convo(3, 10), None, usize::MAX);
        assert_eq!((dropped, plain.len()), (0, 12));

        let (messages, dropped) = request_history(&convo(3, 10), Some(HistorySummary { message: &summary, upto: 8 }), usize::MAX);
        assert_eq!((dropped, messages.len()), (0, 5), "the summary message, then the stored messages from 8 on");
        assert_eq!(messages[0].content, summary.content, "the summary message comes first");
        assert!(messages[1].content.starts_with("q2"), "{:?}", messages[1].content);
        assert!(messages.iter().skip(1).all(|m| !m.content.starts_with("[Note:")), "nothing was dropped, so there is no note");

        // Still too long: the summary stays and the oldest turns after it go, with the note.
        let (small, dropped) = request_history(&convo(5, 10), Some(HistorySummary { message: &summary, upto: 4 }), 60);
        assert_eq!((dropped, small.len()), (12, 5), "three whole turns after the summary, the last one kept");
        assert_eq!(small[0].content, summary.content, "the summary is never what is dropped");
        assert!(small[1].content.starts_with("[Note: 12 earlier messages were omitted to fit the context window.]\n\nq4"), "the note is on the first kept user message: {}", small[1].content);
    }

    #[test]
    fn always_keeps_the_last_user_turn() {
        let (kept, _) = fit_to_budget(convo(3, 50_000), 10);
        assert_eq!(kept.len(), 4);
        assert!(kept[0].content.contains("q2"));
    }

    #[test]
    fn tool_budget_keeps_priority_groups() {
        use crate::tool::ToolSpec;
        let mk = |n: &str| ToolSpec { name: n.into(), description: "x".repeat(300), input_schema: json!({"type":"object"}) };
        let specs = vec![mk("shell"), mk("system_info"), mk("kernel_info"), mk("trace"), mk("journal")];
        assert_eq!(budget_tools(specs.clone(), 0, 0.35, "").len(), 5);
        assert_eq!(budget_tools(specs.clone(), 100_000, 0.35, "").len(), 5);
        // ~360 chars per tool, 3 chars/token: 1000 tokens * 0.35 = 1050 chars -> 2 tools.
        let kept: Vec<String> = budget_tools(specs.clone(), 1000, 0.35, "").into_iter().map(|t| t.name).collect();
        assert_eq!(kept, vec!["system_info", "kernel_info"]);
        // The request names a group: it is kept first.
        let kept: Vec<String> = budget_tools(specs, 1000, 0.35, "Run this bash command").into_iter().map(|t| t.name).collect();
        assert_eq!(kept, vec!["shell", "system_info"]);
    }

    #[test]
    fn a_group_missing_from_the_priority_list_is_still_kept() {
        use crate::tool::ToolSpec;
        let mk = |name: &str, description: usize| {
            ToolSpec { name: name.into(), description: "x".repeat(description), input_schema: json!({"type":"object"}) }
        };
        // "newgroup" is in no list, so it is considered last, in registry order.
        let specs = vec![mk("known", 300), mk("big", 700), mk("fresh", 300)];
        let groups = |name: &str| match name {
            "known" => "system",
            "big" => "shell",
            _ => "newgroup",
        };
        let names = |context: usize| {
            budget_tools_with(specs.clone(), context, 0.35, "", groups).into_iter().map(|t| t.name).collect::<Vec<_>>()
        };
        // ~1480 chars of specs in a 1050-char budget: system and newgroup fit, shell does not.
        assert_eq!(names(1000), vec!["known", "fresh"]);
        assert_eq!(names(400), vec!["known"], "when it does not fit, it yields like any other group");
    }

    #[test]
    fn hints_in_both_languages() {
        assert_eq!(hinted_groups("pause the music"), vec!["desktop"]);
        assert_eq!(hinted_groups("Predvajaj ta film v VLC"), vec!["desktop"]);
        assert_eq!(hinted_groups("is someone hacking me? block ip on the router"), vec!["security", "router"]);
        assert!(hinted_groups("why is my machine slow").is_empty());
        assert_eq!(hinted_groups("Write to the Claude Code console in the KAgent folder"), vec!["desktop"]);
        assert_eq!(hinted_groups("can you see my screen?"), vec!["desktop"]);
        // The agent group (todo list, plan mode, subagents): a "todo" can also be a task.
        assert_eq!(hinted_groups("make a todo list for this"), vec!["tasks", "agent"]);
        assert_eq!(hinted_groups("plan the cleanup first"), vec!["agent"]);
        assert_eq!(hinted_groups("use a subagent for each disk"), vec!["agent"]);
        assert_eq!(hinted_groups("Naredi načrt"), vec!["agent"]);
        assert!(hinted_groups("explain this").is_empty(), "no plan in explain");
    }

    #[test]
    fn budget_reserves_prompt_tools_and_output() {
        assert_eq!(history_budget_tokens(32_000, 4_000, 3_000, 9_000), 28_800 - 4_000 - 4_000);
        assert_eq!(history_budget_tokens(1_000, 4_000, 0, 0), 0);
    }
}
