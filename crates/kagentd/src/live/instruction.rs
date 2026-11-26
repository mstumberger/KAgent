//! The system instruction of the live model, one per brain mode (voice.md sections
//! 1 to 3). The approval-card and the destructive sentence are in all three, word for
//! word: they are a second line of defence next to `approve.rs`.

use crate::sections::VoiceBrain;
use crate::settings::Persona;
use crate::AppState;

/// A sentence of the voice rules since 2026-11-24; keep it word for word.
const CARD: &str = concat!(
    "Actions that change the system show an approval card on the screen: say what you asked for ",
    "and that it waits for approval there; the user can answer it by voice ('yes' / 'no'; root ",
    "actions need 'approve'), so ask a short question like 'Shall I send it?' and wait; never ",
    "claim it is done before the result arrives."
);

/// A sentence of the voice rules since 2026-11-24; keep it word for word.
const DESTRUCTIVE: &str = concat!(
    "Destructive actions (reboot, power off, kill, closing windows, deleting) ALWAYS need the ",
    "user's explicit spoken confirmation, even with full control: first warn about unsaved work ",
    "and ask 'Shall I reboot now? Say yes, reboot.' A question from the user ('should we ",
    "reboot?') is never a request; one-word or garbled fragments are never a confirmation."
);

const LISTENING: &str = concat!(
    "Listening: the user speaks English or Slovenian. Transcripts are noisy: a fragment in ",
    "another language (German, Spanish, Italian, Japanese...) or one that makes no sense in ",
    "context is a mis-hearing or background noise: ignore it or ask the user to repeat; never ",
    "type, send or act on it."
);

const DICTATION: &str = concat!(
    "Dictation: when the user says what to write to someone, the message is only their words for ",
    "the other person, never the instruction around it ('send this', 'yes', 'tell him', 'okay'). ",
    "Use the `message` tool: it types without sending; read the exact text back and send only ",
    "after a yes. If the user says 'what I told you', use their earlier exact words; if you are ",
    "not sure, ask."
);

const RETRIES: &str = concat!(
    "Retries: when something did not work, check first (screen with window, focus_context) and ",
    "fix the cause; do not repeat the same action blindly, and never type the same text twice."
);

/// The rules of a live model that acts itself (`live` and `auto`): the text of S10
/// without the "Recent conversation" block, which the history frames replace. What
/// follows is `CARD`, then `DESTRUCTIVE`, `LISTENING`, `DICTATION` and `RETRIES`.
const VOICE_MODE: &str = concat!(
    "VOICE MODE: you are speaking out loud. Answer in one to three short spoken sentences, no ",
    "markdown, no lists, no code. Do not narrate tool calls or repeat yourself; say a few words ",
    "first only when an action will take a while. Answer with the result. ",
);
const ROUTED: &str = concat!(
    "You are the voice interface. For everything the user says that asks for anything, call ",
    "`think` with the user's exact words in their language and say nothing else. When the ",
    "result arrives, read `reply` aloud word for word in its language, adding nothing. If ",
    "`status` is `handled` or `cancelled`, say nothing. If `failed`, say you could not reach ",
    "your brain and why. You may say one short phrase before `think` (\"Let me check\"). You ",
    "have no tools of your own: never say that something was done, looked up or sent; the brain ",
    "acts and its reply says what happened."
);

/// What the `think` result is and when to use it (`auto`).
const THINK: &str = concat!(
    "Thinking: you have one function, `think`, that hands the user's words to your brain (the ",
    "main agent with memory, skills and every tool). For anything it knows better (the state of ",
    "this machine, the owner's memory and skills, earlier conversations, a tool you do not ",
    "have, anything that takes several steps) call `think` with the user's exact words in their ",
    "language and say nothing else; when the result arrives, read `reply` aloud word for word ",
    "in its language, adding nothing. If `status` is `handled` or `cancelled`, say nothing. If ",
    "`failed`, say you could not reach your brain and why. Small talk and a command for one of ",
    "your own tools you answer yourself."
);

/// A reply that could not be delivered as the result of `think` (voice.md section 2).
const SAY: &str = "A message that starts with [SAY] is your own answer: read what follows word for word, adding nothing.";

/// How the routed model speaks for itself: a greeting, or the phrase before `think`.
const OWN_WORDS: &str = concat!(
    "VOICE MODE: you are speaking out loud. Your own words are one or two short spoken ",
    "sentences, no markdown, no lists, no code; do not narrate tool calls or repeat yourself."
);

fn language_rule(language: &str) -> &'static str {
    match language {
        "sl" => "Always answer in Slovenian.",
        "en" => "Always answer in English.",
        _ => "Answer in the language the user writes in (Slovenian or English).",
    }
}

/// `routed` (voice.md section 2): the chat model thinks; the live model listens and
/// reads its reply.
pub fn routed(persona: &Persona) -> String {
    format!(
        "You are {name}, the voice interface of {address}'s Linux machine. {lang}\n{ROUTED}\n{SAY}\n{LISTENING}\n{OWN_WORDS}\n{CARD}\n{DESTRUCTIVE}",
        name = persona.name,
        address = persona.address,
        lang = language_rule(&persona.language)
    )
}

/// `live` (voice.md section 3): the chat system prompt and the voice rules.
pub fn live(chat_prompt: &str) -> String {
    format!("{chat_prompt}\n\n{VOICE_MODE}{CARD}\n{DESTRUCTIVE}\n{LISTENING}\n{DICTATION}\n{RETRIES}")
}

/// `auto` (voice.md section 1): the instruction of `live`, and when to call `think`.
pub fn auto(chat_prompt: &str) -> String {
    format!("{}\n{THINK}\n{SAY}", live(chat_prompt))
}

/// The instruction of a session in `mode`, read once when it connects.
pub fn of(st: &AppState, mode: VoiceBrain) -> String {
    match mode {
        VoiceBrain::Routed => routed(&st.settings.read().unwrap().persona),
        VoiceBrain::Live => live(&st.agent.config().system_prompt),
        VoiceBrain::Auto => auto(&st.agent.config().system_prompt),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persona() -> Persona {
        Persona { name: "KAgent".into(), address: "Marko".into(), language: "sl".into() }
    }

    /// voice.md section 10: the routed instruction contains the card and destructive sentences.
    #[test]
    fn every_mode_keeps_the_card_and_destructive_sentences() {
        const CARD_TEXT: &str = "Actions that change the system show an approval card on the screen: say what you asked for and that it waits for approval there; the user can answer it by voice ('yes' / 'no'; root actions need 'approve'), so ask a short question like 'Shall I send it?' and wait; never claim it is done before the result arrives.";
        const DESTRUCTIVE_TEXT: &str = "Destructive actions (reboot, power off, kill, closing windows, deleting) ALWAYS need the user's explicit spoken confirmation, even with full control: first warn about unsaved work and ask 'Shall I reboot now? Say yes, reboot.' A question from the user ('should we reboot?') is never a request; one-word or garbled fragments are never a confirmation.";
        let (routed, live, auto) = (routed(&persona()), live("PROMPT"), auto("PROMPT"));
        for text in [&routed, &live, &auto] {
            assert!(text.contains(CARD_TEXT) && text.contains(DESTRUCTIVE_TEXT), "the two safety sentences are word for word in every mode:\n{text}");
        }
        assert!(routed.starts_with("You are KAgent, the voice interface of Marko's Linux machine. Always answer in Slovenian.\n"), "{routed}");
        assert!(routed.contains("call `think` with the user's exact words in their language and say nothing else") && routed.contains("[SAY] is your own answer"), "{routed}");
        assert!(!routed.contains("Recent conversation"), "the history frames carry the conversation");
        assert!(live.starts_with("PROMPT\n\nVOICE MODE: you are speaking out loud.") && !live.contains("Recent conversation") && !live.contains("`think`,"), "live is today's text without the Recent conversation block:\n{live}");
        assert!(live.contains("Retries: when something did not work, check first (screen with window, focus_context) and fix the cause; do not repeat the same action blindly, and never type the same text twice."));
        assert!(auto.starts_with(&live) && auto.contains("one function, `think`"), "auto is live plus the think rule");
    }
}
