//! The config tests of hooks.md section 8: parsing, and what is rejected.

use serde_json::json;

use super::*;

fn ids(hooks: &[Hook]) -> Vec<&str> {
    hooks.iter().map(|h| h.id.as_str()).collect()
}

fn record(event: Event, command: &str) -> serde_json::Value {
    json!({ "id": "hook_1a2b3c4d", "event": event.name(), "command": command, "timeoutMs": 5000, "async": false, "enabled": true })
}

fn with_hooks(hooks: serde_json::Value) -> serde_json::Value {
    json!({ "hooks": { "events": hooks } })
}

#[test]
fn the_documented_record_parses_and_round_trips() {
    let value = json!({
        "id": "hook_1a2b3c4d", "event": "PreToolUse", "matcher": ["shell", "write_file"],
        "command": "/home/me/bin/notify-hook", "args": ["--quiet"], "shell": "",
        "timeoutMs": 5000, "async": false, "enabled": true
    });
    let hook: Hook = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        hook,
        Hook {
            id: "hook_1a2b3c4d".into(),
            event: Event::PreToolUse,
            matcher: vec!["shell".into(), "write_file".into()],
            command: "/home/me/bin/notify-hook".into(),
            args: vec!["--quiet".into()],
            shell: String::new(),
            timeout_ms: 5000,
            r#async: false,
            enabled: true
        }
    );
    assert_eq!(
        serde_json::to_value(&hook).unwrap(),
        json!({ "id": "hook_1a2b3c4d", "event": "PreToolUse", "matcher": ["shell", "write_file"], "command": "/home/me/bin/notify-hook", "args": ["--quiet"], "timeoutMs": 5000 }),
        "an empty or default field is left out"
    );
    let mut keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut fields = FIELDS;
    fields.sort_unstable();
    assert_eq!(keys, fields, "the fields of a record, as `Deserialize` of the section knows them");
    assert_eq!(hook.line(), "/home/me/bin/notify-hook --quiet");
    hook.check().unwrap();

    // What a record may leave out.
    let hook: Hook = serde_json::from_value(json!({ "event": "Stop", "command": "/bin/true" })).unwrap();
    assert_eq!((hook.timeout_ms, hook.r#async, hook.enabled, hook.matcher.len(), hook.args.len(), hook.shell.len()), (DEFAULT_TIMEOUT_MS, false, true, 0, 0, 0));
    assert_eq!(hook.line(), "/bin/true");
    assert_eq!(hook.check().unwrap_err(), "id is required (the daemon assigns one: add the hook in Settings -> Hooks)");
    let sh = Hook { id: "hook_a1b2c3d4".into(), shell: "/bin/bash".into(), ..Hook::new(Event::Stop, "echo done") };
    assert_eq!(sh.line(), "/bin/bash -c echo done");
    assert_eq!(Event::parse("AlertRaised"), Some(Event::AlertRaised));
    assert_eq!((Event::PreToolUse.takes_matcher(), Event::PostToolUse.takes_matcher(), Event::Stop.takes_matcher()), (true, true, false));
}

#[test]
fn an_unknown_key_is_rejected_and_a_typo_gets_its_suggestion() {
    let mut s = Settings::defaults();
    assert_eq!(s.hooks_config(), HooksSection::default(), "no hooks until the owner adds one");
    let err = s.patch(with_hooks(json!({ "PreToolUse": [{ "id": "hook_1a2b3c4d", "event": "PreToolUse", "comand": "/bin/x" }] }))).unwrap_err();
    assert_eq!(err, "invalid settings.hooks: unknown field `comand` (did you mean \"command\"?)", "a typo gets its suggestion");
    let err = s.patch(with_hooks(json!({ "PreToolUse": [{ "id": "hook_1a2b3c4d", "event": "PreToolUse", "command": "/bin/x", "timeoutms": 5 }] }))).unwrap_err();
    assert_eq!(err, "invalid settings.hooks: unknown field `timeoutms` (did you mean \"timeoutMs\"?)", "case does not hide a typo");
    let err = s.patch(with_hooks(json!({ "PreToolUse": [{ "id": "hook_1a2b3c4d", "event": "PreToolUse", "command": "/bin/x", "something": 5 }] }))).unwrap_err();
    assert_eq!(err, "invalid settings.hooks: unknown field `something`", "a key that is no typo is simply unknown");
    let err = s.patch(json!({ "hooks": { "event": {} } })).unwrap_err();
    assert_eq!(err, "unknown key \"hooks.event\" (did you mean \"events\"?)");
    let err = s.patch(json!({ "hooks": { "other": 1 } })).unwrap_err();
    assert!(err.starts_with("invalid settings.hooks: unknown field `other`"), "{err}");
    let err = s.patch(json!({ "hooks": 4 })).unwrap_err();
    assert_eq!(err, "settings.hooks must be an object");
    assert!(s.extra.get("hooks").is_none(), "a rejected patch leaves nothing behind");
}

#[test]
fn an_unknown_event_and_a_record_under_another_event_are_rejected() {
    let mut s = Settings::defaults();
    let err = s.patch(with_hooks(json!({ "OnStop": [] }))).unwrap_err();
    assert_eq!(err, "invalid settings.hooks: hooks.events: unknown event 'OnStop' (one of SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, AlertRaised)");
    let err = s.patch(with_hooks(json!({ "Stop": [record(Event::PreToolUse, "/bin/true")] }))).unwrap_err();
    assert_eq!(err, "invalid settings.hooks: hooks.events.Stop: the record 'hook_1a2b3c4d' names the event PreToolUse");
    let err = s.patch(with_hooks(json!({ "Stop": [{ "id": "hook_1a2b3c4d", "event": "onc", "command": "/bin/true" }] }))).unwrap_err();
    assert!(err.starts_with("invalid settings.hooks: unknown variant `onc`"), "{err}");

    // Refused on load as well, like every typed section: the daemon does not start with it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, with_hooks(json!({ "PreToolUse": [record(Event::PreToolUse, "/bin/true")], "OnStop": [] })).to_string()).unwrap();
    let err = Settings::load(&path).unwrap_err().to_string();
    assert!(err.contains("unknown event 'OnStop'"), "{err}");
    std::fs::write(&path, with_hooks(json!({ "PreToolUse": [record(Event::PreToolUse, "/bin/true")] })).to_string()).unwrap();
    assert_eq!(Settings::load(&path).unwrap().hooks_config().matching(Event::PreToolUse, "shell").len(), 1);
}

#[test]
fn a_matcher_on_another_event_than_the_tool_events_is_rejected() {
    let mut s = Settings::defaults();
    for event in [Event::SessionStart, Event::UserPromptSubmit, Event::Stop, Event::AlertRaised] {
        let mut value = record(event, "/bin/true");
        value["matcher"] = json!(["shell"]);
        let err = s.patch(with_hooks(json!({ event.name(): [value] }))).unwrap_err();
        let name = event.name();
        assert_eq!(err, format!("invalid settings.hooks: hooks.events.{name}: matcher applies to PreToolUse and PostToolUse only, not to {name}"));
    }
    for event in [Event::PreToolUse, Event::PostToolUse] {
        let mut value = record(event, "/bin/true");
        value["matcher"] = json!(["shell"]);
        s.patch(with_hooks(json!({ event.name(): [value] }))).unwrap();
    }
}

#[test]
fn values_out_of_range_and_empty_forms_are_rejected() {
    let hook = |event: Event| Hook { id: "hook_1a2b3c4d".into(), ..Hook::new(event, "/bin/true") };
    assert_eq!(hook(Event::Stop).check(), Ok(()));
    for timeout in [99u64, 60_001, 0] {
        assert_eq!(Hook { timeout_ms: timeout, ..hook(Event::Stop) }.check().unwrap_err(), format!("timeoutMs must be between 100 and 60000, not {timeout}"));
    }
    for timeout in [100u64, 60_000] {
        Hook { timeout_ms: timeout, ..hook(Event::Stop) }.check().unwrap();
    }
    assert_eq!(Hook { command: "  ".into(), ..hook(Event::Stop) }.check().unwrap_err(), "command is required");
    assert_eq!(Hook { command: "a\nb".into(), ..hook(Event::Stop) }.check().unwrap_err(), "command must be one line");
    assert_eq!(Hook { shell: "/bin/bash -c".into(), ..hook(Event::Stop) }.check().unwrap_err(), "shell must be one word: the program that runs the line of command");
    assert_eq!(Hook { args: vec!["-q".into()], shell: "/bin/sh".into(), ..hook(Event::Stop) }.check().unwrap_err(), "args cannot be used with shell: put the whole line in command");
    assert_eq!(Hook { matcher: vec![" ".into()], ..hook(Event::PreToolUse) }.check().unwrap_err(), "matcher holds tool names; none of them is empty");
    assert_eq!(Hook { args: vec!["a\0b".into()], ..hook(Event::Stop) }.check().unwrap_err(), "a NUL character is not allowed");

    // The whole section is among those load and a patch check.
    let mut s = Settings::defaults();
    s.patch(with_hooks(json!({ "Stop": [record(Event::Stop, "/bin/true")] }))).unwrap();
    assert!(s.validate_sections().is_ok());
    let mut bad = record(Event::Stop, "/bin/true");
    bad["timeoutMs"] = json!(1);
    assert_eq!(s.patch(with_hooks(json!({ "Stop": [bad] }))).unwrap_err(), "invalid settings.hooks: hooks.events.Stop: timeoutMs must be between 100 and 60000, not 1");
}

#[test]
fn matching_upsert_and_remove() {
    let mut section = HooksSection::default();
    let on = Hook { id: "hook_aaaaaaaa".into(), ..Hook::new(Event::PreToolUse, "/bin/a") };
    let mut off = Hook { id: "hook_bbbbbbbb".into(), ..Hook::new(Event::PreToolUse, "/bin/b") };
    off.enabled = false;
    let mut only = Hook { id: "hook_cccccccc".into(), ..Hook::new(Event::PreToolUse, "/bin/c") };
    only.matcher = vec!["shell".into()];
    let stop = Hook { id: "hook_dddddddd".into(), ..Hook::new(Event::Stop, "/bin/d") };
    for hook in [&on, &off, &only, &stop] {
        section.upsert(hook.clone());
    }
    assert_eq!(ids(&section.matching(Event::PreToolUse, "shell")), ["hook_aaaaaaaa", "hook_cccccccc"], "a hook that is off never runs; a matcher names the tool");
    assert_eq!(ids(&section.matching(Event::PreToolUse, "write_file")), ["hook_aaaaaaaa"]);
    assert!(section.matching(Event::UserPromptSubmit, "").is_empty(), "nothing is spawned where no hook matches");
    assert_eq!(ids(&section.matching(Event::Stop, "")), ["hook_dddddddd"]);
    assert_eq!(section.all().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["hook_aaaaaaaa", "hook_bbbbbbbb", "hook_cccccccc", "hook_dddddddd"], "by event, then as stored");
    assert_eq!(section.find("hook_cccccccc"), Some(&only));

    // A change keeps the place; a change of event moves it.
    section.upsert(Hook { event: Event::PostToolUse, command: "/bin/a2".into(), ..on.clone() });
    assert_eq!(section.find("hook_aaaaaaaa").map(|h| (h.event, h.command.as_str())), Some((Event::PostToolUse, "/bin/a2")));
    assert_eq!(section.events.keys().collect::<Vec<_>>(), ["PostToolUse", "PreToolUse", "Stop"], "a sorted file");
    assert_eq!(section.remove("hook_dddddddd"), Some(stop));
    assert_eq!(section.remove("hook_dddddddd"), None);
    assert_eq!(section.events.keys().collect::<Vec<_>>(), ["PostToolUse", "PreToolUse"], "an event without hooks is gone");
    section.validate().unwrap();
    assert!(Hook::fresh_id().starts_with("hook_") && Hook::fresh_id().len() == 13);
}
