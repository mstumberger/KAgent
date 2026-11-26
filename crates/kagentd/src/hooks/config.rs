//! `settings.hooks` (config.md section 11): the events and the `Hook` records, read
//! strictly. An unknown key, an event nobody knows, a record under another event than
//! its own, a `matcher` on another event than the two tool events, or a value out of its
//! range makes the settings file invalid, on load and on a patch.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::sections::SectionError;
use crate::settings::Settings;

/// The events (hooks.md section 1), in the order the UI lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Event {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    AlertRaised,
}

pub const EVENTS: [Event; 6] = [Event::SessionStart, Event::UserPromptSubmit, Event::PreToolUse, Event::PostToolUse, Event::Stop, Event::AlertRaised];

/// The names of the events, as `GET /api/hooks` lists them.
pub const EVENT_NAMES: [&str; 6] = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "AlertRaised"];

impl Event {
    pub fn name(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::Stop => "Stop",
            Self::AlertRaised => "AlertRaised",
        }
    }

    /// `matcher` applies to the two tool events only.
    pub fn takes_matcher(self) -> bool {
        matches!(self, Self::PreToolUse | Self::PostToolUse)
    }

    pub fn parse(name: &str) -> Option<Self> {
        EVENTS.into_iter().find(|e| e.name() == name)
    }
}

/// The default deadline of a hook, in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;

/// One hook (config.md section 11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hook {
    /// Assigned by the daemon: `hook_` and 8 hex characters.
    #[serde(default)]
    pub id: String,
    /// The event it runs at. The record sits under the same name in `events`.
    pub event: Event,
    /// Tool names; empty = every tool (the two tool events only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matcher: Vec<String>,
    /// An executable path or name found through `PATH` when `shell` is empty, else one
    /// line for that shell.
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Empty: no shell.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shell: String,
    /// 100..=60000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Started and not awaited; only its context is used, for the next turn.
    #[serde(default, skip_serializing_if = "is_false")]
    pub r#async: bool,
    /// On unless the record says `false` (a record that leaves it out is on).
    #[serde(default = "default_enabled", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// The fields of a record, as they are written.
const FIELDS: [&str; 9] = ["id", "event", "matcher", "command", "args", "shell", "timeoutMs", "async", "enabled"];

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_true(v: &bool) -> bool {
    *v
}

impl Hook {
    /// A hook that is on, with the default deadline.
    pub fn new(event: Event, command: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            event,
            matcher: Vec::new(),
            command: command.into(),
            args: Vec::new(),
            shell: String::new(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            r#async: false,
            enabled: true,
        }
    }

    /// The id the daemon assigns.
    pub fn fresh_id() -> String {
        format!("hook_{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
    }

    /// Whether this hook runs for `tool` at its event: it is on, and it names `tool` or no tool.
    pub fn matches(&self, tool: &str) -> bool {
        self.enabled && (self.matcher.is_empty() || self.matcher.iter().any(|m| m == tool))
    }

    /// The command as it runs, for the audit record and the list.
    pub fn line(&self) -> String {
        if self.shell.is_empty() {
            std::iter::once(self.command.as_str()).chain(self.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
        } else {
            format!("{} -c {}", self.shell, self.command)
        }
    }

    /// The rules of config.md section 11 and hooks.md section 1, for a record the daemon keeps.
    pub fn check(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("id is required (the daemon assigns one: add the hook in Settings -> Hooks)".into());
        }
        if self.command.trim().is_empty() {
            return Err("command is required".into());
        }
        if self.command.contains('\n') {
            return Err("command must be one line".into());
        }
        if self.shell.chars().any(char::is_whitespace) {
            return Err("shell must be one word: the program that runs the line of command".into());
        }
        if !self.shell.is_empty() && !self.args.is_empty() {
            return Err("args cannot be used with shell: put the whole line in command".into());
        }
        if !(100..=60_000).contains(&self.timeout_ms) {
            return Err(format!("timeoutMs must be between 100 and 60000, not {}", self.timeout_ms));
        }
        if !self.event.takes_matcher() && !self.matcher.is_empty() {
            return Err(format!("matcher applies to PreToolUse and PostToolUse only, not to {}", self.event.name()));
        }
        if self.matcher.iter().any(|m| m.trim().is_empty()) {
            return Err("matcher holds tool names; none of them is empty".into());
        }
        if self.command.contains('\0') || self.args.iter().any(|a| a.contains('\0')) {
            return Err("a NUL character is not allowed".into());
        }
        Ok(())
    }
}

/// `settings.hooks`: the hooks of each event, by the event's name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HooksSection {
    /// `BTreeMap` keeps the file in a stable order.
    pub events: BTreeMap<String, Vec<Hook>>,
}

impl<'de> Deserialize<'de> for HooksSection {
    /// As the derived one, strictly, and with the suggestion a typo of the settings gets
    /// (`Settings::section` gives it for the keys of the section; this gives it for the
    /// keys of a record).
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            #[serde(default)]
            events: BTreeMap<String, Vec<serde_json::Value>>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let known: Vec<String> = FIELDS.iter().map(|f| f.to_string()).collect();
        let mut events = BTreeMap::new();
        for (event, records) in raw.events {
            let mut hooks: Vec<Hook> = Vec::with_capacity(records.len());
            for record in records {
                if let serde_json::Value::Object(fields) = &record {
                    for key in fields.keys() {
                        if !FIELDS.contains(&key.as_str()) {
                            let hint = crate::sections::closest(key, &known).map_or_else(String::new, |field| format!(" (did you mean \"{field}\"?)"));
                            return Err(serde::de::Error::custom(format!("unknown field `{key}`{hint}")));
                        }
                    }
                }
                hooks.push(Hook::deserialize(record).map_err(serde::de::Error::custom)?);
            }
            events.insert(event, hooks);
        }
        Ok(Self { events })
    }
}

impl HooksSection {
    /// Every hook, by event, then as stored.
    pub fn all(&self) -> impl Iterator<Item = &Hook> {
        EVENTS.iter().filter_map(|e| self.events.get(e.name())).flatten()
    }

    pub fn find(&self, id: &str) -> Option<&Hook> {
        self.all().find(|h| h.id == id)
    }

    /// The hooks that run for `tool` at `event` (every hook, for an event that is not about a tool call).
    pub fn matching(&self, event: Event, tool: &str) -> Vec<Hook> {
        self.events.get(event.name()).into_iter().flatten().filter(|h| h.matches(tool)).cloned().collect()
    }

    /// Whether the section is one the daemon can run: every name is an event, and every record is valid and sits under its own.
    pub fn validate(&self) -> Result<(), SectionError> {
        for (name, hooks) in &self.events {
            let Some(event) = Event::parse(name) else {
                return Err(SectionError::Invalid {
                    key: "hooks".into(),
                    message: format!("hooks.events: unknown event '{name}' (one of {})", EVENT_NAMES.join(", ")),
                });
            };
            for hook in hooks {
                if hook.event != event {
                    return Err(SectionError::Invalid { key: "hooks".into(), message: format!("hooks.events.{name}: the record '{}' names the event {}", hook.id, hook.event.name()) });
                }
                hook.check().map_err(|message| SectionError::Invalid { key: "hooks".into(), message: format!("hooks.events.{name}: {message}") })?;
            }
        }
        Ok(())
    }

    /// Put `hook` where it belongs: in place of the one with its id, else at the end of its event.
    pub fn upsert(&mut self, hook: Hook) {
        self.remove(&hook.id);
        self.events.entry(hook.event.name().to_string()).or_default().push(hook);
    }

    pub fn remove(&mut self, id: &str) -> Option<Hook> {
        let hook = self.find(id).cloned()?;
        for hooks in self.events.values_mut() {
            hooks.retain(|h| h.id != id);
        }
        self.events.retain(|_, hooks| !hooks.is_empty());
        Some(hook)
    }
}

impl Settings {
    /// settings.hooks: the hooks of the owner, by event (hooks.md).
    pub fn hooks_config(&self) -> HooksSection {
        self.section::<HooksSection>("hooks").unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
