//! Answering approval cards by voice. Only the user's own speech transcript (Gemini's
//! input transcription) is matched here, never the model's output, so the agent cannot
//! approve its own actions. Root actions need an explicit "approve"/"confirm".

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).map(String::from).collect()
}

const NO: [&str; 12] = ["no", "nope", "don't", "dont", "stop", "cancel", "deny", "wait", "ne", "nikar", "počakaj", "prekliči"];
const YES: [&str; 17] = ["yes", "yeah", "yep", "ok", "okay", "sure", "approve", "approved", "allow", "confirm", "proceed", "ja", "lahko", "potrdi", "dovoli", "seveda", "izvedi"];
const YES_PHRASES: [&str; 6] = ["do it", "go ahead", "send it", "run it", "go on", "daj ga"];
const STRONG: [&str; 6] = ["approve", "approved", "confirm", "potrdi", "odobri", "dovoli"];

/// The user's answer in this utterance, if any. "No" wins over "yes" ("no, don't send it").
/// `privileged`: only explicit approve/confirm words count as yes.
#[cfg(test)]
pub fn classify(text: &str, privileged: bool) -> Option<Answer> {
    classify_for(text, privileged, "")
}

/// Strong confirmation (root or destructive actions): an explicit approve/confirm word,
/// or a yes together with the action's own word from the card ("yes, reboot").
/// `action` = the card summary, e.g. "reboot now".
pub fn classify_for(text: &str, strong: bool, action: &str) -> Option<Answer> {
    let privileged = strong;
    let w = words(text);
    if w.is_empty() {
        return None;
    }
    if w.iter().any(|x| NO.contains(&x.as_str())) {
        return Some(Answer::No);
    }
    let lower = format!(" {} ", w.join(" "));
    let verb = action.split_whitespace().next().unwrap_or("").to_lowercase();
    let said_yes = w.iter().any(|x| YES.contains(&x.as_str())) || YES_PHRASES.iter().any(|p| lower.contains(&format!(" {p} ")));
    let yes = if privileged {
        w.iter().any(|x| STRONG.contains(&x.as_str())) || (said_yes && verb.len() >= 3 && w.iter().any(|x| *x == verb))
    } else {
        w.iter().any(|x| YES.contains(&x.as_str())) || YES_PHRASES.iter().any(|p| lower.contains(&format!(" {p} ")))
    };
    // Long utterances are new requests, not answers ("ok, now write to Miha that...").
    (yes && w.len() <= 8).then_some(Answer::Yes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers() {
        assert_eq!(classify("Yes.", false), Some(Answer::Yes));
        assert_eq!(classify("go ahead", false), Some(Answer::Yes));
        assert_eq!(classify("Ja, lahko.", false), Some(Answer::Yes));
        assert_eq!(classify("No, don't send it", false), Some(Answer::No));
        assert_eq!(classify("wait", false), Some(Answer::No));
        assert_eq!(classify("prosecco", false), None);
        assert_eq!(classify("ok now write to Miha that I will be late for the meeting today", false), None, "a new request, not an answer");
        assert_eq!(classify("yes", true), None, "root needs an explicit approve");
        assert_eq!(classify("I approve", true), Some(Answer::Yes));
        // Destructive: the transcript that rebooted the PC must not confirm anything.
        assert_eq!(classify_for("Should we reboot?", true, "reboot now"), None);
        assert_eq!(classify_for("Ezel", true, "reboot now"), None);
        assert_eq!(classify_for("b", true, "reboot now"), None);
        assert_eq!(classify_for("yes", true, "reboot now"), None, "a bare yes is not enough");
        assert_eq!(classify_for("Yes, reboot.", true, "reboot now"), Some(Answer::Yes));
        assert_eq!(classify_for("confirm", true, "reboot now"), Some(Answer::Yes));
        assert_eq!(classify_for("no, don't reboot", true, "reboot now"), Some(Answer::No));
    }
}
