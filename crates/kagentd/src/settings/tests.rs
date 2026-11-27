//! The tests of `settings.rs`.

use super::*;

#[test]
fn new_default_groups_are_enabled_once() {
    let mut s = Settings::defaults();
    s.tool_groups = vec!["system".into(), "desktop".into()];
    s.known_tool_groups = vec![];
    assert!(s.migrate_tool_groups());
    assert!(s.tool_groups.contains(&"security".to_string()) && s.tool_groups.contains(&"trace".to_string()));
    assert!(!s.tool_groups.contains(&"kernel".to_string()), "a group the user turned off stays off");
    // The user now turns security off again: it stays off.
    s.tool_groups.retain(|g| g != "security");
    assert!(!s.migrate_tool_groups());
    assert!(!s.tool_groups.contains(&"security".to_string()));
}

// `desktop_prefs`, `browser_config` and `voice_pauses_media` are tested in sections.rs.

#[test]
fn nested_patch_keeps_its_siblings() {
    let mut s = Settings::defaults();
    s.startup.greeting.text = "Pozdravljen!".into();
    // Scenario S17 item 4.
    s.patch(serde_json::json!({"startup": {"greeting": {"enabled": false}}})).unwrap();
    assert!(s.startup.autostart, "autostart is not named by the patch");
    assert!(!s.startup.greeting.enabled);
    assert_eq!(s.startup.greeting.text, "Pozdravljen!", "the greeting text is not named by the patch");
    // Arrays and scalars are replaced, deeper objects merged.
    s.patch(serde_json::json!({"toolGroups": ["system"], "persona": {"language": "sl"}})).unwrap();
    assert_eq!(s.tool_groups, vec!["system"]);
    assert_eq!((s.persona.language.as_str(), s.persona.name.as_str()), ("sl", "KAgent"));
    assert!(s.patch(serde_json::json!(["not", "an", "object"])).is_err());
}

#[test]
fn merge_patch_merges_objects_and_replaces_the_rest() {
    let mut base = serde_json::json!({"a": {"x": 1, "y": {"deep": 1}}, "list": [1, 2], "n": 1});
    merge_patch(&mut base, &serde_json::json!({"a": {"y": {"deeper": 2}}, "list": [], "n": null, "new": {"b": 1}}));
    assert_eq!(
        base,
        serde_json::json!({"a": {"x": 1, "y": {"deep": 1, "deeper": 2}}, "list": [], "n": null, "new": {"b": 1}})
    );
}

#[test]
fn invalid_tool_groups_and_overrides_are_rejected() {
    let mut s = Settings::defaults();
    let before = s.clone();
    assert!(s.patch(serde_json::json!({"toolGroups": ["system", "bogus"]})).is_err());
    assert!(s.patch(serde_json::json!({"toolGroups": ["shell"], "toolOverrides": {"shell": "maybe"}})).is_err());
    assert_eq!(s, before, "a rejected patch changes nothing, not even its valid part");
    s.patch(serde_json::json!({"toolGroups": ["shell", "router"], "toolOverrides": {"shell": "ask", "journal": "allow"}})).unwrap();
    assert_eq!(s.tool_groups, vec!["shell", "router"]);
    assert_eq!(s.tool_overrides["shell"], "ask");
    // `null` removes one override (the Tools section's "follows the mode"); the rest stays.
    s.patch(serde_json::json!({"toolOverrides": {"shell": null, "systemd": null}})).unwrap();
    assert_eq!(s.tool_overrides.len(), 1);
    assert_eq!(s.tool_overrides["journal"], "allow");
    // The whole object is still a map: `null` there would wipe every override.
    assert!(s.patch(serde_json::json!({"toolOverrides": null})).is_err());

    // An entry already in the file (hand-edited) is tolerated at load and does not
    // block a patch that does not name it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, serde_json::json!({"toolGroups": ["system", "bogus"], "toolOverrides": {"shell": "maybe"}}).to_string()).unwrap();
    let mut s = Settings::load(&path).unwrap();
    assert!(s.tool_groups.contains(&"bogus".to_string()));
    s.patch(serde_json::json!({"maxIterations": 12})).unwrap();
    assert_eq!(s.max_iterations, 12);
}

#[test]
fn defaults_roundtrip_and_patch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let s = Settings::load(&path).unwrap();
    assert_eq!(s.permission_mode, PermissionMode::Ask);
    assert_eq!(s.tool_groups, DEFAULT_TOOL_GROUPS.iter().map(|g| g.to_string()).collect::<Vec<_>>());
    assert!(s.tool_groups.iter().any(|g| g == "security"));
    assert!(s.startup.autostart && s.startup.greeting.enabled, "startup defaults on");
    assert!(path.exists());

    let mut s2 = Settings::load(&path).unwrap();
    s2.patch(serde_json::json!({"permissionMode": "trusted", "futureKey": {"a": 1}})).unwrap();
    // Invalid values are rejected as a whole and leave settings untouched.
    assert!(s2.patch(serde_json::json!({"permissionMode": "yolo", "maxIterations": 1})).is_err());
    assert_eq!(s2.permission_mode, PermissionMode::Trusted);
    assert_ne!(s2.max_iterations, 1);
    assert_eq!(s2.permission_mode, PermissionMode::Trusted);
    assert_eq!(s2.extra.get("futureKey").and_then(|v| v.get("a")), Some(&serde_json::json!(1)));
    s2.save(&path).unwrap();
    let s3 = Settings::load(&path).unwrap();
    assert_eq!(s3.permission_mode, PermissionMode::Trusted);
    assert!(s3.extra.contains_key("futureKey"));

    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn an_old_max_output_tokens_stays_in_the_file_and_is_read_by_nothing() {
    // models.md section 6: the model's own output limit is the only source from S40 on.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{"chatModel": "a::x", "options": {"maxOutputTokens": 4321, "temperature": 0.5}}"#).unwrap();
    let mut s = Settings::load(&path).unwrap();
    assert_eq!(s.options.max_output_tokens, Some(4321));
    s.patch(serde_json::json!({"maxIterations": 12})).unwrap();
    s.save(&path).unwrap();
    assert_eq!(Settings::load(&path).unwrap().options, ModelOptionsSerde { reasoning_level: None, max_output_tokens: Some(4321), temperature: Some(0.5) }, "kept through a patch and a save");
}

#[test]
fn an_old_reasoning_level_stays_in_the_file_and_is_read_by_nothing() {
    // picker.md section 2: a conversation chooses its effort from S41 on.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{"chatModel": "a::x", "options": {"reasoningLevel": "high"}}"#).unwrap();
    let mut s = Settings::load(&path).unwrap();
    assert_eq!(s.options.reasoning_level.as_deref(), Some("high"));
    s.patch(serde_json::json!({"maxIterations": 12})).unwrap();
    s.save(&path).unwrap();
    assert_eq!(Settings::load(&path).unwrap().options, ModelOptionsSerde { reasoning_level: Some("high".into()), max_output_tokens: None, temperature: None }, "kept through a patch and a save");
}
