//! Tool implementations, grouped as in `docs/specs/tools.md`.

pub mod apps;
pub mod change;
pub mod desktop;
pub mod display;
pub mod editor;
pub mod exec;
pub mod files;
pub mod focus;
pub mod hardware;
pub mod kernel;
pub mod media;
pub mod message;
pub mod power_actions;
pub mod privop;
pub mod security;
pub mod services;
pub mod sessions;
pub mod shell;
pub mod terminal;
pub mod sys;
pub mod timeline;
pub mod trace;
pub mod undo;
pub mod xdg;

use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::Path;

/// Parse a tool input `Value` into a typed struct, treating `null`/missing as the
/// type's default so models that send `null` instead of `{}` still work.
pub fn parse<T: Default + DeserializeOwned>(value: Value) -> anyhow::Result<T> {
    match value {
        Value::Null => Ok(T::default()),
        v => Ok(serde_json::from_value(v)?),
    }
}

/// Read a file to a trimmed string, `None` when missing or unreadable.
pub async fn read_trim(path: impl AsRef<Path>) -> Option<String> {
    tokio::fs::read_to_string(path.as_ref()).await.ok().map(|s| s.trim().to_string())
}

/// Tool group per tool name (tools.md sections 1-12). Used by settings.toolGroups
/// to decide which tools are advertised to the model.
pub fn group_of(name: &str) -> &'static str {
    match name {
        "system_info" | "process_list" | "process_signal" | "disk_info" | "network_info" | "what_changed" | "doctor" => "system",
        "kernel_info" | "kernel_module" | "sysctl" => "kernel",
        "systemd" | "journal" | "boot_analysis" => "services",
        "read_file" | "list_dir" | "search_files" | "write_file" | "edit_file" => "files",
        "shell" => "shell",
        "hardware" | "power" | "power_profile" | "audio" | "bluetooth" | "crashes" | "drivers"
        | "disk_health" => "hardware",
        "load_skill" => "skills",
        "tasks" => "tasks",
        "package" | "measure" | "undo_change" | "list_changes" => "change",
        "audio_control" | "bluetooth_control" | "wifi" | "display" | "windows" | "notify" | "open" => "desktop",
        "media" | "open_in_editor" | "projects" | "focus_context" | "clipboard" | "input" | "browser" | "ui" | "terminal" | "system_power" | "sessions" | "screen" | "message" => "desktop",
        "security_report" | "list_alerts" => "security",
        "router_status" | "lan_devices" | "router_log" | "router_lists" | "traffic_top"
        | "router_block_ip" | "router_unblock_ip" => "router",
        "trace" => "trace",
        // Unmapped tools are hidden when groups are filtered: a new tool must be added
        // here explicitly instead of silently landing in an enabled group.
        _ => "other",
    }
}

/// All builtin tools. Grows with each stage.
pub fn builtin_registry() -> crate::tool::ToolRegistry {
    let mut r = crate::tool::ToolRegistry::new();
    r.register(std::sync::Arc::new(sys::SystemInfo::new()));
    r.register(std::sync::Arc::new(sys::ProcessList::new()));
    r.register(std::sync::Arc::new(sys::ProcessSignal::new()));
    r.register(std::sync::Arc::new(sys::CommandReport::disk()));
    r.register(std::sync::Arc::new(sys::CommandReport::network()));
    r.register(std::sync::Arc::new(kernel::KernelInfo::new()));
    r.register(std::sync::Arc::new(kernel::KernelModule::new()));
    r.register(std::sync::Arc::new(kernel::Sysctl::new()));
    r.register(std::sync::Arc::new(services::Systemd::new()));
    r.register(std::sync::Arc::new(services::Journal::new()));
    r.register(std::sync::Arc::new(services::BootAnalysis::new()));
    r.register(std::sync::Arc::new(files::ReadFile::new()));
    r.register(std::sync::Arc::new(files::ListDir::new()));
    r.register(std::sync::Arc::new(files::SearchFiles::new()));
    r.register(std::sync::Arc::new(shell::Shell::new()));
    r.register(std::sync::Arc::new(hardware::Hardware::new()));
    r.register(std::sync::Arc::new(hardware::Power::new()));
    r.register(std::sync::Arc::new(hardware::Audio::new()));
    r.register(std::sync::Arc::new(hardware::Bluetooth::new()));
    r.register(std::sync::Arc::new(hardware::Crashes::new()));
    r.register(std::sync::Arc::new(hardware::Drivers::new()));
    r.register(std::sync::Arc::new(hardware::DiskHealth::new()));
    r.register(std::sync::Arc::new(change::Package::new()));
    r.register(std::sync::Arc::new(change::PowerProfile::new()));
    r.register(std::sync::Arc::new(change::WriteFile::new()));
    r.register(std::sync::Arc::new(change::EditFile::new()));
    r.register(std::sync::Arc::new(undo::Measure::new()));
    r.register(std::sync::Arc::new(undo::UndoChange::new()));
    r.register(std::sync::Arc::new(undo::ListChanges::new()));
    r.register(std::sync::Arc::new(desktop::AudioControl::new()));
    r.register(std::sync::Arc::new(desktop::BluetoothControl::new()));
    r.register(std::sync::Arc::new(desktop::Wifi::new()));
    r.register(std::sync::Arc::new(display::Display::new()));
    r.register(std::sync::Arc::new(apps::Windows::new()));
    r.register(std::sync::Arc::new(apps::Notify::new()));
    r.register(std::sync::Arc::new(apps::Open::new()));
    r.register(std::sync::Arc::new(media::Media::new()));
    r.register(std::sync::Arc::new(editor::OpenInEditor::new()));
    r.register(std::sync::Arc::new(editor::Projects::new()));
    r.register(std::sync::Arc::new(focus::FocusContext::new()));
    r.register(std::sync::Arc::new(focus::Clipboard::new()));
    r.register(std::sync::Arc::new(focus::Input::new()));
    r.register(std::sync::Arc::new(terminal::Terminal::new()));
    r.register(std::sync::Arc::new(message::MessageTool::new()));
    r.register(std::sync::Arc::new(power_actions::SystemPower::new()));
    r.register(std::sync::Arc::new(security::SecurityReport::new()));
    r.register(std::sync::Arc::new(trace::Trace::new()));
    r.register(std::sync::Arc::new(timeline::WhatChanged::new()));
    r.register(std::sync::Arc::new(timeline::Doctor::new()));
    r
}

#[cfg(test)]
mod group_tests {
    #[test]
    fn every_builtin_tool_has_a_group() {
        for c in super::builtin_registry().contracts() {
            assert_ne!(super::group_of(&c.name), "other", "tool {} has no group in group_of()", c.name);
        }
    }
}
