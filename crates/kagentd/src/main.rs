//! kagentd: the daemon. `kagentd serve` (default) starts the API; the other
//! subcommands arrive with their stages (S6 audit, S8 sudoers, S12 mcp).

mod api;
mod rounded;
mod tasks;
mod screen;
mod appwin;
mod tray;
mod ui;
mod browser;
mod krunner;
mod live;
mod mcp;
mod net;
mod paths;
mod prompt;
mod secrets;
mod sentinel;
mod server;
mod settings;
mod startup;
mod voice_levels;
mod watch;
mod ws;

use std::sync::Arc;

use tokio::sync::broadcast;

use kagent_core::agent::{Agent, AgentConfig, AgentDeps};
use kagent_core::permission::{PermissionBroker, PermissionEngine};
use kagent_core::protocol::AgentEvent;
use kagent_core::session::SessionStore;
use kagent_core::tools::builtin_registry;

/// Shared daemon state for all handlers.
pub struct AppState {
    pub agent: Arc<Agent>,
    pub providers: Arc<kagent_providers::ProviderStore>,
    pub settings: Arc<std::sync::RwLock<settings::Settings>>,
    pub events: broadcast::Sender<AgentEvent>,
    pub token: String,
    pub machine: serde_json::Value,
    pub version: &'static str,
    pub levels: Arc<voice_levels::LevelHub>,
    pub services: Arc<kagent_core::tool::ToolServices>,
    /// Sessions with an open live voice socket (one per session).
    pub live_sessions: std::sync::Mutex<std::collections::HashSet<String>>,
    pub alerts: Arc<watch::AlertManager>,
    pub sentinel: Arc<sentinel::Sentinel>,
    pub net: Arc<net::Net>,
    pub ui: Arc<ui::UiHub>,
    /// KAgent's own app windows (main UI, orb widget).
    pub windows: Arc<appwin::AppWindows>,
    pub tasks: Arc<tasks::TaskStore>,
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    match arg.as_str() {
        "serve" => {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            let r = rt.block_on(run());
            // Do not wait for slow blocking work (SUID scan, journalctl) on shutdown.
            rt.shutdown_timeout(std::time::Duration::from_secs(2));
            if let Err(e) = r {
                eprintln!("kagentd: {e:#}");
                std::process::exit(1);
            }
        }
        "audit" => {
            if std::env::args().nth(2).as_deref() == Some("verify") {
                let path = paths::Paths::resolve().data.join("audit.jsonl");
                match kagent_core::audit::verify(&path) {
                    Ok(n) => println!("OK: {n} records, chain intact ({})", path.display()),
                    Err((line, reason)) => {
                        println!("BROKEN at line {line}: {reason} ({})", path.display());
                        std::process::exit(1);
                    }
                }
            } else {
                usage();
            }
        }
        "sudoers" => print_sudoers(),
        "mikrotik-setup" => net::print_setup(&std::env::args().nth(2).unwrap_or_else(|| "<this-PC-IP>".into())),
        "router-unblock" => {
            // Undo path for router_block_ip (recorded in the change journal).
            let addr = std::env::args().nth(2).unwrap_or_default();
            let token = std::fs::read_to_string(paths::Paths::resolve().token()).unwrap_or_default();
            let out = std::process::Command::new("curl")
                .args(["-fsS", "-X", "POST", "-H", &format!("Authorization: Bearer {}", token.trim()), "-H", "Content-Type: application/json", "-d", &serde_json::json!({"address": addr}).to_string(), "http://127.0.0.1:7878/api/net/unblock"])
                .status();
            std::process::exit(if out.is_ok_and(|s| s.success()) { 0 } else { 1 });
        }
        "mcp" => {
            // stdout is the protocol channel: logs must not go there.
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            if let Err(e) = rt.block_on(mcp::serve_stdio(&paths::Paths::resolve())) {
                eprintln!("kagentd mcp: {e:#}");
                std::process::exit(1);
            }
        }
        _ => usage(),
    }
}

/// Owner action (never executed by KAgent): install the root helper + sudoers rule.
fn print_sudoers() {
    let user = std::env::var("USER").unwrap_or_else(|_| "<your-user>".into());
    let repo = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| ".".into());
    if let Some(line) = kagent_core::tools::privop::setup_oneliner() {
        println!("# KAgent root helper setup: paste this ONE line into a terminal (asks for your password once):");
        println!("{line}");
        println!();
        println!("# Or step by step from the source tree:");
    } else {
        println!("# KAgent root helper setup. Review, then run these yourself (KAgent never runs sudo).");
    }
    println!("cd {repo}");
    println!("cargo build --release -p kagent-priv");
    println!("sudo install -o root -g root -m 0755 target/release/kagent-priv {}", kagent_core::tools::privop::DEFAULT_HELPER);
    println!("echo '{user} ALL=(root) NOPASSWD: {}' | sudo tee /etc/sudoers.d/kagent", kagent_core::tools::privop::DEFAULT_HELPER);
    println!("sudo chmod 0440 /etc/sudoers.d/kagent && sudo visudo -c");
    println!();
    println!("# Optional: allow free-form root shell commands (each one still asks in KAgent):");
    println!("# sudo mkdir -p /etc/kagent && sudo touch /etc/kagent/allow-root-shell");
    println!("# Remove everything: sudo rm /etc/sudoers.d/kagent {}", kagent_core::tools::privop::DEFAULT_HELPER);
}

fn usage() {
    eprintln!("usage: kagentd [serve | audit verify | sudoers | mcp]");
    std::process::exit(2);
}

async fn run() -> anyhow::Result<()> {
    let paths = paths::Paths::resolve();
    std::fs::create_dir_all(&paths.config)?;
    std::fs::create_dir_all(&paths.data)?;
    std::fs::create_dir_all(paths.sessions())?;

    // Logs: stderr + <data>/logs/kagentd.log.YYYY-MM-DD (7 days kept).
    let file = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("kagentd.log")
        .max_log_files(7)
        .build(paths.data.join("logs"))?;
    let (file_writer, _log_guard) = tracing_appender::non_blocking(file);
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(file_writer))
            .init();
    }

    // Token: 32 random bytes hex, 0600, printed once per start.
    let token = match std::fs::read_to_string(paths.token()) {
        Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            use rand::RngCore;
            let mut bytes = [0u8; 32];
            rand::rng().fill_bytes(&mut bytes);
            let t: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            use std::io::Write;
            use std::os::unix::fs::PermissionsExt;
            let mut f = std::fs::File::create(paths.token())?;
            writeln!(f, "{t}")?;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            t
        }
    };

    let settings = settings::Settings::load(&paths.settings())?;
    // Keep the systemd user unit in sync with settings.startup.autostart (user
    // level only; enabling never starts a second copy of this daemon).
    startup::apply_autostart(settings.startup.autostart);
    let catalog = kagent_providers::ProviderStore::builtin_catalog()?;
    let providers = Arc::new(kagent_providers::ProviderStore::load(paths.providers(), catalog)?);
    let keyring = secrets::attach(providers.clone()).await;
    tracing::info!("provider keys: {}", if keyring { "Secret Service (desktop keyring)" } else { "providers.json (0600)" });

    let machine = kagent_core::tools::sys::system_snapshot().await;
    let skills_dir = paths.skills_dir();
    let skills = kagent_core::skills::load_skills(&skills_dir);
    tracing::info!("loaded {} skills from {}", skills.len(), skills_dir.display());
    let (events, _) = broadcast::channel(1024);
    let audit = std::sync::Arc::new(kagent_core::audit::AuditLog::open(&paths.data.join("audit.jsonl"))?);
    audit.append_start(VERSION).ok();
    let alerts = watch::AlertManager::new(paths.data.join("alerts.json"), events.clone(), Some(audit.clone()), std::env::var_os("KAGENT_NO_NOTIFY").is_none());
    if let Some(q) = settings.extra.get("watchers").and_then(|w| w.get("quietHours")).and_then(serde_json::Value::as_str) {
        alerts.set_quiet_hours(q);
    }
    let mut registry = builtin_registry();
    registry.register(Arc::new(watch::ListAlerts::new(alerts.clone())));
    // MikroTik router + traffic (S15): idle until configured in settings.mikrotik.
    let router_cfg: net::mikrotik::RouterConfig = settings
        .extra
        .get("mikrotik")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let net = net::Net::new(router_cfg, &paths.config, &paths.data, alerts.clone());
    for t in net::tools::all(net.clone()) {
        registry.register(t);
    }
    registry.register(Arc::new(kagent_core::skills::LoadSkill::new(skills_dir)));
    let task_store = tasks::TaskStore::load(&paths.data, events.clone());
    registry.register(Arc::new(tasks::TasksTool::new(task_store.clone())));
    let ui_hub = ui::UiHub::new(events.clone());
    registry.register(Arc::new(ui::UiTool::new(ui_hub.clone())));
    registry.register(Arc::new(kagent_core::tools::sessions::Sessions::new(paths.sessions())?));
    let (browser_port, browser_bin) = settings.browser_config();
    registry.register(Arc::new(browser::Browser::new(&paths.data, browser_port, &browser_bin)));
    let settings_cell = Arc::new(std::sync::OnceLock::new());
    registry.register(Arc::new(screen::Screen::new(providers.clone(), settings_cell.clone(), &paths.data)));
    // (name, group) of every registered tool, for the per-turn prompt.
    let tool_index: Vec<(String, &'static str)> = registry
        .contracts()
        .iter()
        .map(|c| (c.name.clone(), kagent_core::tools::group_of(&c.name)))
        .collect();
    let skills_index = kagent_core::skills::skills_index(&skills);


    // Change journal + root helper, shared by every mutating tool.
    let journal = Arc::new(kagent_core::journal::ChangeJournal::new(paths.data.join("changes"))?);
    {
        let (events, audit) = (events.clone(), audit.clone());
        journal.set_listener(move |c| {
            let v = serde_json::to_value(c).unwrap_or_default();
            let kind = if c.undone_at.is_some() { kagent_core::audit::AuditKind::Undo } else { kagent_core::audit::AuditKind::Change };
            let data = serde_json::json!({"id": c.id, "tool": c.tool, "summary": c.summary, "status": v["status"], "privileged": c.privileged});
            if let Err(e) = audit.append(kind, &c.session_id, data) {
                tracing::warn!("audit append failed: {e:#}");
            }
            let _ = events.send(AgentEvent::ChangeRecorded { change: v });
        });
    }
    let priv_client = Arc::new(kagent_core::tools::privop::PrivClient::new());
    tracing::info!("root helper {}: {}", priv_client.helper().display(), priv_client.status());
    let services = Arc::new(kagent_core::tool::ToolServices::new(journal.clone(), priv_client.clone()));
    *services.desktop.write().unwrap() = settings.desktop_prefs();
    let engine = Arc::new(PermissionEngine::new(settings.permission_mode, settings.rules.clone()));
    let broker = PermissionBroker::new(events.clone(), std::time::Duration::from_secs(settings.approval_timeout_sec));
    let store = SessionStore::new(paths.sessions())?;

    engine.set_overrides(settings::parse_overrides(&settings.tool_overrides));
    let settings_arc = Arc::new(std::sync::RwLock::new(settings));
    let _ = settings_cell.set(settings_arc.clone());
    let settings_for_config = settings_arc.clone();
    let providers_for_config = providers.clone();
    let machine_for_config = machine.clone();
    // Built for every turn: persona, language, enabled tools, clock and the chat
    // model's context window are always current.
    let cfg = move || {
        let s = settings_for_config.read().unwrap();
        let enabled: Vec<String> = tool_index
            .iter()
            .filter(|(_, g)| s.tool_groups.is_empty() || s.tool_groups.iter().any(|x| x == g))
            .map(|(n, _)| n.clone())
            .collect();
        let mode = serde_json::to_value(s.permission_mode).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let system_prompt = prompt::build(&prompt::PromptInput {
            persona_name: &s.persona.name,
            address: &s.persona.address,
            language: &s.persona.language,
            machine: &machine_for_config,
            tool_names: &enabled,
            skills_index: &skills_index,
            permission_mode: &mode,
            now: chrono::Local::now(),
        });
        let resolved = providers_for_config.resolve(&s.chat_model).ok();
        let context_tokens = resolved.as_ref().map(|r| r.config.context_window as usize).unwrap_or(0);
        let max_output = s.options.max_output_tokens.or(resolved.as_ref().map(|r| r.config.max_output_tokens));
        AgentConfig {
            model_ref: s.chat_model.clone(),
            system_prompt,
            max_iterations: s.max_iterations,
            options: kagent_core::model::ModelOptions {
                reasoning_level: s.options.reasoning_level.clone(),
                max_output_tokens: max_output,
                temperature: s.options.temperature,
            },
            privilege: s.privilege_method(),
            context_tokens,
            fallback_context_tokens: providers_for_config.resolve(&s.fallback_model).map(|r| r.config.context_window as usize).unwrap_or(0),
            fallback_max_output: providers_for_config.resolve(&s.fallback_model).ok().map(|r| r.config.max_output_tokens),
            fallback_model_ref: s.fallback_model.clone(),
        }
    };

    let agent = Agent::new(AgentDeps {
        services: Some(services.clone()),
        audit: Some(audit.clone()),
        model: Arc::new(kagent_providers::ProvidersClient::new(providers.clone())),
        tools: Arc::new(registry),
        permissions: engine,
        broker,
        store,
        events: events.clone(),
        config: Arc::new(cfg),
        on_rules_changed: {
            let settings = settings_arc.clone();
            let paths = paths.clone();
            Some(Arc::new(move |rules| {
                let mut s = settings.write().unwrap();
                s.rules = rules;
                let _ = s.save(&paths.settings());
            }))
        },
    });
    agent.set_tool_groups(Some(settings_arc.read().unwrap().tool_groups.clone()));

    // Voice levels: activity follows the agent's event stream.
    let levels = Arc::new(voice_levels::LevelHub::new());
    {
        let (levels, mut rx) = (levels.clone(), events.subscribe());
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => levels.on_event(&ev),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }

    // Watchers: journal stream + periodic checks (S13).
    let watch_cancel = tokio_util::sync::CancellationToken::new();
    if settings_arc.read().unwrap().extra.get("watchers").and_then(|w| w.get("enabled")).and_then(serde_json::Value::as_bool) != Some(false) {
        watch::sources::start(alerts.clone(), watch_cancel.clone(), priv_client.status() == "installed", paths.data.clone());
    }

    // Intrusion detection: persistence baseline, honeytokens (opt-in), heartbeat (S14).
    let sentinel = sentinel::Sentinel::new(paths.data.clone(), alerts.clone());
    sentinel.start(watch_cancel.clone(), true);

    net.start();

    let state = Arc::new(AppState {
        net,
        ui: ui_hub,
        tasks: task_store,
        windows: Arc::new(appwin::AppWindows::new(&paths.data, &token, 7878)),
        sentinel,
        alerts,
        live_sessions: Default::default(),
        services,
        levels,
        agent,
        providers,
        settings: settings_arc,
        events,
        token,
        machine,
        version: VERSION,
    });

    // KRunner (Alt+Space "ka ..."): keep the connection alive for the daemon's lifetime.
    // KRunner is Plasma's launcher; other desktops have no consumer for it.
    let _krunner = if kagent_core::platform::desktop() == kagent_core::platform::Desktop::Kde { krunner::serve(state.clone()).await } else { None };
    tracing::info!("platform: {}", kagent_core::platform::summary());
    tasks::start(state.clone());
    rounded::watch(state.windows.clone(), 14);
    {
        // Name old sessions that still have a placeholder title.
        let agent = state.agent.clone();
        tokio::spawn(async move { agent.backfill_titles().await });
    }
    let tray_ok = tray::start(state.clone()).await;
    // KAgent lives in the tray and starts as the small widget (settings.startWidget,
    // default on). A window left open by a previous daemon is reused.
    if tray_ok && state.settings.read().unwrap().start_widget() {
        let st = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            // A window left open by the previous daemon keeps its layout (forcing the widget
            // here once squeezed the full window's content into widget mode at full size).
            if !st.windows.adopt_existing().await {
                tray::set_window(&st, appwin::Mode::Compact, false).await;
            }
        });
    }
    let audit_on_exit = audit.clone();
    let r = server::serve(state).await;
    watch_cancel.cancel();
    let _ = audit_on_exit.append(kagent_core::audit::AuditKind::Stop, "daemon", serde_json::json!({"version": VERSION}));
    tracing::info!("kagentd stopped");
    r
}
