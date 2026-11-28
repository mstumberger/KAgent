//! The greeting said out loud at the start of the session started at login (voice.md section 14.5).

use crate::AppState;

/// The greeting as the notification has it (`startup::greeting_text`), for the hour it is said.
pub fn greeting_now(st: &AppState) -> String {
    let s = st.settings.read().unwrap();
    let hour = chrono::Timelike::hour(&chrono::Local::now());
    crate::startup::greeting_text(&s.startup.greeting, &s.persona, hour)
}

/// What the `[SAY]` turn holds: the greeting, then the morning report in the same turn when one waits. Neither: no turn.
pub fn spoken_turn(greeting: Option<&str>, report: Option<&str>) -> Option<String> {
    match (greeting.map(str::trim).filter(|g| !g.is_empty()), report.map(str::trim).filter(|r| !r.is_empty())) {
        (Some(g), Some(r)) => Some(format!("{g}\n\n{r}")),
        (Some(g), None) => Some(g.to_string()),
        (None, Some(r)) => Some(r.to_string()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::{greeting_text, Greeting};

    #[test]
    fn the_report_follows_the_greeting_in_one_turn_and_neither_says_nothing() {
        assert_eq!(spoken_turn(Some("Good morning, Marko."), Some("Today: nothing.")).as_deref(), Some("Good morning, Marko.\n\nToday: nothing."));
        assert_eq!(spoken_turn(Some("Good morning, Marko."), None).as_deref(), Some("Good morning, Marko."), "the report is off or failed");
        assert_eq!(spoken_turn(Some("Good morning, Marko."), Some("  ")).as_deref(), Some("Good morning, Marko."));
        assert_eq!(spoken_turn(None, Some("Today: nothing.")).as_deref(), Some("Today: nothing."), "no greeting: the report as it always was");
        assert_eq!(spoken_turn(None, None), None);
        assert_eq!(spoken_turn(Some(""), None), None);
    }

    #[test]
    fn what_is_said_is_the_text_of_the_notification() {
        let persona = crate::settings::Persona { name: "KAgent".into(), address: "Marko".into(), language: "auto".into() };
        let nameless = crate::settings::Persona { address: String::new(), ..persona.clone() };
        let auto = Greeting::default();
        let custom = Greeting { text: "Pozdravljen!".into(), ..Greeting::default() };
        let said = |g: &Greeting, p: &crate::settings::Persona| spoken_turn(Some(&greeting_text(g, p, 9)), None).unwrap();
        assert_eq!(said(&auto, &persona), "Good morning, Marko. KAgent is up and watching.");
        assert_eq!(said(&custom, &persona), "Pozdravljen!");
        assert!(said(&auto, &nameless).ends_with(crate::startup::ASK_NAME), "with no name it asks for one");
    }
}
