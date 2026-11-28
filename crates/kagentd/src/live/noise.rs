//! A call that follows only a noise fragment (voice.md section 13, agent-loop.md A.2). The live
//! transcript of 2026-11-27 turned background noise into "Sie sollen das Fruehstueck selbst
//! bezahlen.", "Oh, que. Oh, que. Oh, que.", "Ya es tarde." and a Devanagari fragment, and the
//! model acted on them. The instruction says to ignore such a fragment; this is the check.

use std::sync::Mutex;

use kagent_core::tool::CallAssessment;

/// The owner's words of the turn in progress (the input transcript), cleared when the turn completes.
#[derive(Default)]
pub struct Heard(Mutex<String>);

impl Heard {
    pub fn push(&self, chunk: &str) {
        self.0.lock().unwrap().push_str(chunk);
    }
    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
    pub fn text(&self) -> String {
        self.0.lock().unwrap().clone()
    }
}

/// Words of German, Spanish, French, Italian and Portuguese that are not Slovenian or English words.
const FOREIGN: &[&str] = &[
    "sie", "sollen", "das", "und", "ist", "ich", "nicht", "wir", "ein", "eine", "der", "den", "dem", "mit", "für", "auch", "aber", "wie", "wo",
    "bitte", "danke", "nein", "selbst", "bezahlen", "können", "müssen", "haben", "sind", "von", "bei", "nach", "auf", "dass", "wenn", "noch",
    "schon", "sehr", "heute", "guten", "tag", "es", "ya", "que", "el", "los", "las", "una", "por", "para", "muy", "bien", "gracias", "hoy",
    "tarde", "pero", "como", "está", "esto", "eso", "hola", "est", "les", "des", "il", "elle", "nous", "vous", "suis", "pour", "dans", "avec",
    "bonjour", "merci", "oui", "che", "sono", "noi", "ciao", "grazie", "prego", "è", "não", "obrigado", "uma", "mais", "très", "señor",
];

/// Common words of Slovenian and English: a text that has more of them than foreign words is not noise.
const KNOWN: &[&str] = &[
    "the", "is", "and", "you", "to", "a", "of", "in", "it", "that", "i", "we", "this", "for", "on", "with", "are", "was", "can", "do", "not", "what",
    "how", "please", "yes", "no", "ok", "okay", "my", "me", "open", "close", "je", "da", "se", "na", "ne", "ki", "za", "tudi", "ali", "sem", "si",
    "so", "bi", "pa", "to", "ni", "ste", "smo", "kaj", "kako", "prosim", "ja", "jaz", "mi", "ti", "ga", "jo", "od", "do", "po", "pri", "kot", "ker",
];

/// Letters that are in neither Slovenian (which has č š ž) nor English.
const FOREIGN_LETTERS: &str = "ñßäöüøåãõçàèìòùâêîôûœæéíóúáë¿¡";

fn is_latin(c: char) -> bool {
    c <= '\u{024F}' || ('\u{1E00}'..='\u{1EFF}').contains(&c)
}

/// True only when `text` is surely neither Slovenian nor English. Conservative: when unsure it
/// is false, so normal speech is never held. A word list never decides alone: foreign words must
/// be at least half of the words and more than the common Slovenian and English ones.
pub fn not_sl_en(text: &str) -> bool {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() < 2 {
        return false;
    }
    let outside = letters.iter().filter(|c| !is_latin(**c)).count();
    if outside * 2 > letters.len() {
        return true;
    }
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !(c.is_alphabetic() || c == '\'')).filter(|w| !w.is_empty()).collect();
    if words.len() < 2 {
        return false;
    }
    let foreign = words.iter().filter(|w| FOREIGN.contains(w) || w.chars().any(|c| FOREIGN_LETTERS.contains(c))).count();
    let known = words.iter().filter(|w| KNOWN.contains(w)).count();
    foreign * 2 >= words.len() && known < foreign
}

/// What the live session says when a changing call follows only a noise fragment; `None` when
/// the call may go on (a read-only call, nothing heard, or speech in Slovenian or English).
pub fn question(heard: &str, a: &CallAssessment) -> Option<String> {
    let heard = heard.trim();
    if a.read_only || !not_sl_en(heard) {
        return None;
    }
    let what = a.summary.trim().trim_end_matches('.');
    let heard = kagent_core::util::truncate_str(heard, 80);
    Some(format!("I heard something that may not be you: \"{heard}\". Do you want me to do this: {what}? Say yes to go ahead, or no."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kagent_core::tool::ToolContract;
    use serde_json::json;

    #[test]
    fn noise_fragments_are_not_slovenian_or_english() {
        // The fragments recorded on 2026-11-27, and others of the same kind.
        for noise in [
            "Sie sollen das Fruehstueck selbst bezahlen.",
            "Oh, que. Oh, que. Oh, que.",
            "Ya es tarde.",
            "यह एक परीक्षण है",
            "今日は天気がいいですね",
            "Привет, как дела",
            "Das ist nicht gut, ich bin müde",
            "Bonjour, merci beaucoup",
            "Ciao, grazie mille",
        ] {
            assert!(not_sl_en(noise), "{noise}");
        }
    }

    #[test]
    fn normal_speech_is_never_noise() {
        for speech in [
            "",
            "ok",
            "Ya",
            "da",
            "Zapri vsa okna.",
            "Kako si danes? Kaj je novega?",
            "Prosim, odpri brskalnik in poišči vreme za jutri.",
            "Restart the bluetooth service and tell me what the log says.",
            "Should we reboot? I think the update needs it.",
            "Open the café menu on the screen",
            "Send Müller a message that I am late",
            "Pošlji Marku sporočilo, da bom zamudil",
            "What does the word que mean in Spanish?",
            "Set the Brain to Gemma and read it to me",
            "Zdravo, KAgent!",
            "yes, reboot",
        ] {
            assert!(!not_sl_en(speech), "{speech}");
        }
    }

    fn call(read_only: bool) -> CallAssessment {
        let contract = ToolContract::read_only("terminal", "", json!({"type": "object"}));
        let mut a = CallAssessment::from_contract(&contract, "terminal send ls".into(), "terminal".into());
        a.read_only = read_only;
        a
    }

    /// A changing call that follows only a fragment is asked out loud; every other call runs on.
    #[test]
    fn only_a_changing_call_after_noise_is_held() {
        let say = question("Ya es tarde.", &call(false)).expect("held");
        assert_eq!(say, "I heard something that may not be you: \"Ya es tarde.\". Do you want me to do this: terminal send ls? Say yes to go ahead, or no.");
        assert_eq!(question("Ya es tarde.", &call(true)), None, "a look is never held");
        assert_eq!(question("Zapri okno", &call(false)), None, "speech goes on");
        assert_eq!(question("", &call(false)), None, "a turn with no transcript goes on");
        let heard = Heard::default();
        heard.push("Oh, que.");
        heard.push(" Oh, que.");
        assert!(question(&heard.text(), &call(false)).is_some());
        heard.clear();
        assert_eq!(question(&heard.text(), &call(false)), None, "a new turn starts empty");
    }

    /// The held call is asked with the noise sentence; the owner's yes runs it, a no refuses it, in any mode.
    #[test]
    fn a_held_call_runs_after_a_yes_and_not_after_a_no() {
        use super::super::approve::Answer;
        use super::super::confirm::{Step, VoiceConfirm};
        let (a, args) = (call(false), json!({"action": "send", "text": "ls"}));
        let say = question("Ya es tarde.", &a);
        let c = VoiceConfirm::default();
        let t0 = std::time::Instant::now();
        let Step::Ask(v) = c.step_saying("terminal", &args, &a, t0, say.as_deref()) else { panic!("asked") };
        assert_eq!((v["status"].as_str(), v["say"].as_str()), (Some("needs_confirmation"), say.as_deref()));
        assert_eq!(c.hear("yes"), Some(Answer::Yes));
        // The yes may be in the same transcript buffer as the fragment: the answered call still runs.
        assert_eq!(c.step_saying("terminal", &args, &a, t0, question("Ya es tarde. Yes", &a).as_deref()), Step::Run);
        let Step::Ask(_) = c.step_saying("terminal", &args, &a, t0, say.as_deref()) else { panic!("one yes runs one call") };
        assert_eq!(c.hear("no"), Some(Answer::No));
        assert!(matches!(c.step_saying("terminal", &args, &a, t0, say.as_deref()), Step::Refused(_)));
    }
}
