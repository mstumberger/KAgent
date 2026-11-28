//! The system instruction of the live model, one per brain mode (voice.md sections
//! 1 to 3). The approval-card and the destructive sentence are in all three, word for
//! word: they are a second line of defence next to `approve.rs`.

use crate::sections::VoiceBrain;
use crate::prompt;
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
    "context is a mis-hearing or background noise: ignore it or ask the user to repeat; an action ",
    "that follows only such a fragment is asked out loud first."
);

const DICTATION: &str = concat!(
    "Dictation: when the user says what to write to someone, the message is only their words for ",
    "the other person, never the instruction around it ('send this', 'yes', 'tell him', 'okay'). ",
    "Use the `message` tool: it types without sending; read the exact text back; `send` is ",
    "refused until the owner has answered. If the user says 'what I told you', use their earlier exact words; if you are ",
    "not sure, ask."
);

/// Spoken confirmation (voice.md section 12): the daemon holds back a call that needs
/// approval until the owner's own voice confirms it.
const CONFIRM: &str = concat!(
    "Confirmation: a tool result with status `needs_confirmation` means nothing was done yet. ",
    "Say its `say` text word for word and wait for the owner's answer; never claim it was done. ",
    "When they confirm, call the same tool again with exactly the same arguments; when they say ",
    "no, drop it. Never call an action that changes the system because of a question ('how can ",
    "we fix it?', 'should we reboot?'): answer the question and offer the action instead."
);

/// Looking at the screen (vision.md section 3): `screen` puts a screenshot in the live model's view.
const LOOK: &str = "To see the screen call `screen`; never describe the screen without having looked in this turn.";

const RETRIES: &str = concat!(
    "Retries: when something did not work, check first (look again with screen, focus_context) and ",
    "fix the cause; do not repeat the same action blindly: the same changing call twice within 20 seconds is refused."
);

/// The rules of a live model that acts itself (`live` and `auto`): the text of S10
/// without the "Recent conversation" block, which the history frames replace. What
/// follows is `CARD`, then `DESTRUCTIVE`, `CONFIRM`, `LISTENING`, `DICTATION` and `RETRIES`.
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

/// A reply that could not be delivered as the result of `think` (voice.md section 2), and the
/// morning report (calendar.md section 6): what follows is read aloud as it stands.
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
    let address = if persona.address.trim().is_empty() { "the owner" } else { persona.address.as_str() };
    format!(
        "You are {name}, the voice interface of {address}'s Linux machine. {lang}\n{ROUTED}\n{SAY}\n{LISTENING}\n{OWN_WORDS}\n{CARD}\n{DESTRUCTIVE}",
        name = persona.name,
        address = address,
        lang = language_rule(&persona.language)
    )
}

/// `live` (voice.md section 3): the chat system prompt and the voice rules.
pub fn live(chat_prompt: &str, address: &str) -> String {
    // The Brain, `settings` and name lines (voice-control.md): this model has the tool whatever the chat
    // prompt was built with, and a line the chat prompt already holds is not said twice.
    let rules: String = prompt::owner_rules(true, address).lines().filter(|l| !chat_prompt.contains(l)).map(|l| format!("{l}\n")).collect();
    format!("{chat_prompt}\n\n{rules}{VOICE_MODE}{CARD}\n{DESTRUCTIVE}\n{CONFIRM}\n{LOOK}\n{SAY}\n{LISTENING}\n{DICTATION}\n{RETRIES}")
}

/// `auto` (voice.md section 1): the instruction of `live`, and when to call `think`.
pub fn auto(chat_prompt: &str, address: &str) -> String {
    format!("{}\n{THINK}", live(chat_prompt, address))
}

/// The instruction of a session in `mode`, read once when it connects.
pub fn of(st: &AppState, mode: VoiceBrain) -> String {
    match mode {
        VoiceBrain::Routed => routed(&st.settings.read().unwrap().persona),
        VoiceBrain::Live => live(&st.agent.config().system_prompt, &st.settings.read().unwrap().persona.address),
        VoiceBrain::Auto => auto(&st.agent.config().system_prompt, &st.settings.read().unwrap().persona.address),
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
        let (routed, live, auto) = (routed(&persona()), live("PROMPT", "Marko"), auto("PROMPT", "Marko"));
        for text in [&routed, &live, &auto] {
            assert!(text.contains(CARD_TEXT) && text.contains(DESTRUCTIVE_TEXT), "the two safety sentences are word for word in every mode:\n{text}");
        }
        assert!(routed.starts_with("You are KAgent, the voice interface of Marko's Linux machine. Always answer in Slovenian.\n"), "{routed}");
        assert!(routed.contains("call `think` with the user's exact words in their language and say nothing else") && routed.contains("[SAY] is your own answer"), "{routed}");
        assert!(!routed.contains("Recent conversation"), "the history frames carry the conversation");
        assert!(live.starts_with("PROMPT\n\n- The owner calls the chat model 'the Brain'") && live.contains("VOICE MODE: you are speaking out loud."), "{live}");
        assert!(live.contains("VOICE MODE: you are speaking out loud.") && !live.contains("Recent conversation") && !live.contains("`think`,"), "live is today's text without the Recent conversation block:\n{live}");
        assert!(live.contains("Retries: when something did not work, check first (look again with screen, focus_context) and fix the cause; do not repeat the same action blindly: the same changing call twice within 20 seconds is refused."));
        assert!(auto.starts_with(&live) && auto.contains("one function, `think`"), "auto is live plus the think rule");
        // voice.md section 12: the model that acts itself knows what `needs_confirmation` is.
        for text in [&live, &auto] {
            assert!(text.contains("status `needs_confirmation` means nothing was done yet") && text.contains("call the same tool again with exactly the same arguments"), "{text}");
        }
        assert!(!routed.contains("needs_confirmation"), "routed has no tools of its own");
        // vision.md section 3: the model that acts itself looks at the screen with `screen`, once per sentence.
        for text in [&live, &auto] {
            assert_eq!(text.matches("To see the screen call `screen`; never describe the screen without having looked in this turn.").count(), 1, "{text}");
        }
        assert!(!routed.contains("call `screen`"), "routed has no tools of its own: the Brain looks");
        // calendar.md section 6: the morning report reaches a `live` session as a [SAY] text turn.
        for text in [&routed, &live, &auto] {
            assert_eq!(text.matches("A message that starts with [SAY] is your own answer: read what follows word for word, adding nothing.").count(), 1, "{text}");
        }
    }

    /// voice-control.md sections 3 and 5 in the live instructions.
    #[test]
    fn live_and_auto_carry_the_brain_settings_and_name_lines_once() {
        for text in [live("PROMPT", "Marko"), auto("PROMPT", "Marko")] {
            assert_eq!(text.matches(prompt::BRAIN_RULE.trim_end()).count(), 1, "{text}");
            assert!(text.contains(prompt::SETTINGS_RULE.trim_end()) && !text.contains("do not know the owner's name"));
        }
        let nameless = live("PROMPT", "");
        assert!(nameless.contains(prompt::NAME_RULE.trim_end()), "{nameless}");
        // A chat prompt that has the lines already is not repeated.
        let chat = format!("PROMPT\n{}{}", prompt::BRAIN_RULE, prompt::SETTINGS_RULE);
        let text = live(&chat, "Marko");
        assert_eq!(text.matches("the Brain'").count(), 1, "{text}");
        assert!(!routed(&persona()).contains("`settings`"), "routed has no tools of its own");
        let nameless_persona = Persona { address: String::new(), ..persona() };
        assert!(routed(&nameless_persona).contains("of the owner's Linux machine"));
    }
}
