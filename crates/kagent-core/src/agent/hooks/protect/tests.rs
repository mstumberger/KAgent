//! The protection table of hooks.md section 5 (scenario S32 item 3) and of token-guard.md.

use std::collections::HashMap;

use serde_json::json;

use super::*;
use crate::permission::{AllowRule, Decision, PermissionEngine, PermissionMode, ToolOverride};
use crate::tool::{CallAssessment, Tool};
use crate::tools::{change::EditFile, change::WriteFile, shell::Shell};

/// The protected files where this process finds them.
struct Files {
    settings: String,
    config: String,
    data: String,
    /// The config folder under `~` (without the `~`), when it is there: else the environment names it.
    home_form: Option<String>,
}

fn files() -> Files {
    let (config, data) = dirs();
    Files {
        settings: config.join("settings.json").display().to_string(),
        config: config.display().to_string(),
        data: data.display().to_string(),
        home_form: config.starts_with(home()).then(|| config.strip_prefix(home()).unwrap().display().to_string()),
    }
}

#[test]
fn credential_files_in_every_written_form() {
    let f = files();
    let token = format!("{}/token", f.config);
    for path in [token.clone(), format!("{}/router.secret", f.config), format!("{}/telegram.secret", f.config), format!("{}/../kagent/token", f.config), format!("{}/./token", f.config)] {
        assert!(is_credential(&path), "{path}");
    }
    for path in [f.settings.clone(), format!("{}/token.bak", f.config), format!("{}/skills/token/SKILL.md", f.config), "/tmp/token".to_string(), format!("{}/sessions/token.jsonl", f.data), f.config.clone()] {
        assert!(!is_credential(&path), "{path}");
    }
    if let Some(home_form) = &f.home_form {
        assert!(is_credential(&format!("~/{home_form}/token")));
        assert!(is_credential(&format!("$HOME/{home_form}/token")));
    }
}

#[test]
fn shell_commands_that_read_a_credential_or_call_the_api() {
    let f = files();
    let token = format!("{}/token", f.config);
    let tilde = f.home_form.as_ref().map(|h| format!("~/{h}/token")).unwrap_or_else(|| token.clone());
    for cmd in [
        format!("cat {token}"),
        format!("cat {tilde}"),
        format!("cat \"{tilde}\""),
        format!("curl -s -H \"Authorization: Bearer $(cat {tilde})\" -X POST http://127.0.0.1:7878/api/approvals/1"),
        format!("echo `cat {tilde}`"),
        format!("cat {}/*", f.config),
        format!("cd {} && cat token", f.config),
        "journalctl --user -u kagentd.service -n 50".to_string(),
        "curl http://localhost:7878/api/state".to_string(),
        "curl http://127.0.0.1:7878/api/settings -X PUT -d '{}'".to_string(),
    ] {
        assert!(credential_command(&cmd, None) || own_api(&cmd), "must ask: {cmd}");
    }
    for cmd in [format!("cat {}/settings.json", f.config), format!("ls {}", f.config), "cat /proc/cpuinfo".to_string(), "journalctl -u ssh -n 20".to_string(), "cat /tmp/token".to_string(), format!("cat {token}2")] {
        assert!(!credential_command(&cmd, None) && !own_api(&cmd), "must not ask: {cmd}");
    }
}

#[test]
fn the_file_tools_see_a_protected_file_in_every_written_form() {
    let f = files();
    let agents = format!("{}/agents", f.config);
    for path in [f.settings.clone(), format!("{}/telegram.json", f.config), format!("{}/audit.jsonl", f.data), format!("{}/tasks.json", f.data), agents.clone(), format!("{agents}/general.md")] {
        assert!(is_protected(&path), "{path}");
    }
    assert!(is_protected(&format!("{}/agents/../settings.json", f.config)), "`..` is resolved");
    assert!(is_protected(&format!("{}/./settings.json", f.config)));
    for path in [format!("{}/settings.json.bak", f.config), format!("{}/skills/x/SKILL.md", f.config), format!("{}/alerts.json", f.data), format!("{}/sessions/x.jsonl", f.data)] {
        assert!(!is_protected(&path), "{path}");
    }
    assert!(!is_protected("/tmp/settings.json"), "another program's settings file");
    if let Some(suffix) = &f.home_form {
        assert!(is_protected(&format!("~/{suffix}/settings.json")), "`~` names the home folder");
        assert!(is_protected(&format!("~/{suffix}/agents/new.md")));
    }
    assert!(!is_protected("~/kagent-s32-other.txt"));
    // A folder linked to the config folder is the config folder: the tool follows it when it writes.
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(&f.config, dir.path().join("link")).unwrap();
    assert!(is_protected(dir.path().join("link").join("settings.json").to_str().unwrap()));
}

#[test]
fn the_shell_sees_a_redirect_or_an_in_place_edit_of_a_protected_file() {
    let f = files();
    let (settings, config, agents) = (&f.settings, &f.config, &format!("{}/agents", f.config));
    let changes = [
        format!("echo {{}} > {settings}"),
        format!("echo x >> {settings}"),
        format!("cat a | tee {settings}"),
        format!("echo x | sudo tee {settings}"),
        format!("sed -i s/a/b/ {settings}"),
        format!("sed -i.bak 's/a/b/' {settings}"),
        format!("rm {settings}"),
        format!("rm -rf -- {config}"),
        format!("rm {config}/settings*"),
        format!("mv /tmp/new.json {settings}"),
        format!("mv {settings} /tmp/gone.json"),
        format!("cp /tmp/new.json {settings}"),
        format!("cp -t {agents} /tmp/new.md"),
        format!("ln -s {settings} /tmp/door"),
        format!("echo x > {agents}/general.md"),
        format!("echo x 2>&1 > {settings}"),
        format!("cd {config} && echo x > settings.json"),
        format!("cd {config}; rm telegram.json"),
        format!("(cd {config}; echo x > settings.json)"),
        format!("FOO=1 rm {settings}"),
        format!("sh -c 'echo x > {settings}'"),
        format!("echo x > \"{settings}\""),
    ];
    for command in &changes {
        assert!(protected_command(command, None), "must ask: {command}");
        let a = Shell::new().assess(&json!({ "command": command }));
        assert!(a.destructive && !a.read_only && a.needs_approval, "the gate that asks in every mode: {command}");
    }
    let reads = [
        format!("cat {settings}"),
        format!("grep x {settings} | head -3"),
        format!("cp {settings} /tmp/backup.json"),
        format!("diff {settings} /tmp/new.json"),
        "echo x > other.json".to_string(),
        format!("ls {config}"),
        format!("echo \"a > b\" {settings}"),
        format!("sed s/a/b/ {settings}"),
        "cd /tmp && echo x > settings.json".to_string(),
        format!("echo x > {settings}.bak"),
        format!("echo x 2> {settings}.err"),
    ];
    for command in &reads {
        assert!(!protected_command(command, None), "need not ask: {command}");
        // (A redirect to an absolute path is "destructive" for a reason of its own, `safety.rs`.)
        assert!(!Shell::new().assess(&json!({ "command": command })).destructive || crate::safety::looks_destructive(command), "{command}");
    }
    // A relative name is judged from the folder the call starts in, and from the one a `cd` names.
    assert!(!protected_command("echo x > settings.json", Some("/tmp")));
    assert!(protected_command("echo x > settings.json", Some(&f.config)));
    assert!(!protected_command("cat x", Some("~")));
    if let Some(suffix) = &f.home_form {
        let tail = suffix.strip_prefix('/').unwrap_or(suffix);
        for written in [format!("$HOME/{tail}/settings.json"), format!("${{HOME}}/{tail}/telegram.json"), format!("~/{tail}/settings.json")] {
            assert!(protected_command(&format!("echo x > {written}"), None), "{written}");
        }
        assert!(protected_command("echo x > settings.json", Some(&format!("~/{tail}"))), "the folder of the call, written with ~");
        assert!(protected_command("cd ~ && echo x > .config/kagent/settings.json", None), "a `cd` with ~");
    }
}

#[test]
fn the_protection_asks_in_every_mode_even_over_a_rule_and_an_override() {
    let f = files();
    let write = WriteFile::new().assess(&json!({ "path": f.settings, "content": "{}" }));
    let edit = EditFile::new().assess(&json!({ "path": f.settings, "old": "a", "new": "b" }));
    let shell = Shell::new().assess(&json!({ "command": format!("echo {{}} > {}", f.settings) }));
    let sed = Shell::new().assess(&json!({ "command": format!("sed -i s/a/b/ {}", f.settings) }));
    let reads = Shell::new().assess(&json!({ "command": format!("cat {}", f.settings) }));
    assert!(reads.read_only, "reading the file stays read-only");
    for mode in [PermissionMode::Ask, PermissionMode::Trusted, PermissionMode::Full] {
        let engine = PermissionEngine::new(mode, ["write_file", "edit_file", "shell"].map(|tool| AllowRule { tool: tool.into(), prefix: String::new() }).to_vec());
        engine.set_overrides(HashMap::from([("write_file", ToolOverride::Allow), ("edit_file", ToolOverride::Allow), ("shell", ToolOverride::Allow)].map(|(t, o)| (t.to_string(), o))));
        for (tool, call) in [("write_file", &write), ("edit_file", &edit), ("shell", &shell), ("shell", &sed)] {
            let decision = engine.evaluate("s", tool, call);
            assert_eq!(decision, Decision::Ask { reason: "potentially destructive: needs your explicit confirmation".into() }, "{mode:?} {tool}");
        }
        assert!(matches!(engine.evaluate("s", "shell", &reads), Decision::Allow { .. }), "{mode:?}: reading needs no card");
        let other: CallAssessment = WriteFile::new().assess(&json!({ "path": "~/kagent-s32-other.txt", "content": "x" }));
        assert!(matches!(engine.evaluate("s", "write_file", &other), Decision::Allow { .. }), "{mode:?}: any other file follows the rules");
    }
}
