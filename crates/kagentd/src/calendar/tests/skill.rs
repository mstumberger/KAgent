//! The shipped skill `calendar-event` (calendar.md section 4): it loads, names the triggers,
//! and keeps the interview's rules. The rules themselves are enforced by the tools (the
//! refusals of `tests/change.rs`); the skill only tells the model how to ask.

use std::path::Path;

fn skill() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/calendar-event/SKILL.md")).unwrap()
}

#[test]
fn the_skill_loads_and_is_loaded_on_the_words_of_the_spec() {
    let raw = skill();
    let doc = kagent_core::skills::frontmatter::parse(&raw).expect("a valid skill");
    assert_eq!(doc.name, "calendar-event");
    for trigger in ["add an event", "schedule", "dodaj dogodek"] {
        assert!(doc.description.contains(trigger), "'{trigger}' loads it: {}", doc.description);
    }
    assert!(doc.when_to_use.contains("calendar") && !doc.body.is_empty());
    assert!(!raw.contains('\u{2014}'), "no em dashes");
    assert!(raw.lines().count() <= 60, "{} lines", raw.lines().count());
}

#[test]
fn the_interview_has_its_six_steps_in_order_and_names_the_tools() {
    let raw = skill();
    let at = |needle: &str| raw.find(needle).unwrap_or_else(|| panic!("the skill says '{needle}'"));
    let steps = ["1. **Title, day and time.**", "2. **How long.**", "3. **Where.**", "4. **Calendar and reminder.**", "5. **Notes, call and guests.**", "6. **Call `calendar_change` with `add`.**"];
    assert!(steps.windows(2).all(|w| at(w[0]) < at(w[1])), "the steps come in the order of the spec");
    for said in ["one question at a time", "place_search", "mapsUrl", "I could not check this on the map", "Just add it", "never guess", "meet: true", "calendar_events", "is refused"] {
        assert!(raw.to_lowercase().contains(&said.to_lowercase()), "the skill says '{said}'");
    }
}
