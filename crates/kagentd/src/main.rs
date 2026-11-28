//! kagentd: the daemon. `kagentd serve` (default) starts the API; the other
//! subcommands arrive with their stages (S6 audit, S8 sudoers, S12 mcp).

mod api;
mod cli;
mod bots;
mod calendar;
mod local_models;
mod usage;
mod diff;
mod gitref;
mod tickets;
mod background_api;
mod compact_api;
mod rounded;
mod tasks;
mod todo_api;
mod screen;
mod screen_watch;
mod appwin;
mod tray;
mod turn_config;
mod ui;
mod browser;
mod hooks;
mod image_readers;
mod krunner;
mod listen_start;
mod live;
mod machine;
mod mcp;
mod memory;
mod net;
mod notify;
mod paths;
mod model_api;
mod plan_api;
mod project_api;
mod prompt;
mod seams;
mod secrets;
mod sections;
mod sentinel;
mod server;
mod settings;
mod settings_apply;
mod settings_tool;
mod skills;
mod skills_api;
mod skills_import;
mod skills_sources;
mod skills_sources_api;
mod startup;
mod runs;
mod subagents;
mod voice_levels;
mod watch;
mod web;
mod widgets;
mod ws;

use std::sync::Arc;

use tokio::sync::broadcast;

use kagent_core::agent::{Agent, AgentDeps, TurnContext};
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
    /// The one-time marker and the session started at login (voice.md section 14).
    pub listen_start: Arc<listen_start::ListenStart>,
    pub alerts: Arc<watch::AlertManager>,
    pub sentinel: Arc<sentinel::Sentinel>,
    pub net: Arc<net::Net>,
    pub ui: Arc<ui::UiHub>,
    /// KAgent's own app windows (main UI, orb widget).
    pub windows: Arc<appwin::AppWindows>,
    pub tasks: Arc<tasks::TaskStore>,
    /// Desktop (and later Telegram) notifications, one path for every caller.
    pub notifier: Arc<notify::Notifier>,
    /// The Telegram bot (bots.md): its routes, its polling task and the Telegram channel.
    pub bots: Arc<bots::Bots>,
    /// Google Calendar (calendar.md): the connection, the tools, the morning report.
    pub calendar: Arc<calendar::Calendar>,
    /// Recommended local models and their downloads (local-models.md).
    pub local_models: Arc<local_models::LocalModels>,
    /// The counters and limits of the providers (usage.md): behind `/api/usage` and the `usage` tool.
    pub usage: Arc<usage::UsageHub>,
    /// Workflow runs (workflows.md): the engine, the definitions, behind `/api/runs` and the `workflow_*` tools.
    pub runs: Arc<runs::Runs>,
    /// The tickets of the projects (tickets.md): behind `/api/tickets` and the `tickets` tool.
    pub tickets: Arc<tickets::Tickets>,
    /// The screen watch (screen-watch.md): the one watch, its tool and its routes.
    pub screen_watch: Arc<screen_watch::ScreenWatch>,
    pub skills: Arc<skills::SkillHub>,
    /// The memory store, behind the routes and the memory tools (memory.md).
    pub memory: Arc<memory::MemoryHub>,
    /// Git sources of skills; its lock serializes every change to imported skills.
    pub skill_sources: Arc<skills_sources::SourceStore>,
    /// The `browser` tool: also the routes of the attached browser (browser.md).
    pub browser: Arc<browser::Browser>,
    /// The `web_fetch` and `web_search` tools: also the Brave key behind its route (web.md).
    pub web: Arc<web::Web>,
    /// The subagent profiles behind `/api/agents` (agents.md).
    pub subagents: Arc<subagents::Subagents>,
    /// The owner's hooks (hooks.md): behind `/api/hooks`, and plugged into the agent and the alerts.
    pub hooks: Arc<hooks::HookHub>,
    /// The inline widgets of the chat (widgets.md): the `widget_show` tool and the route that serves a fragment.
    pub widgets: Arc<widgets::Widgets>,
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
        other => cli::run(other),
    }
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
    // level only; enabling never starts a second copy of this daemon). A daemon
    // started under KAGENT_HOME or from target/ leaves the owner's unit alone.
    startup::sync_at_start(settings.startup.autostart);
    let catalog = kagent_providers::ProviderStore::builtin_catalog()?;
    // A model the settings name keeps its roles when the rows of an old fetch are corrected (models.md section 3).
    let named: Vec<String> = api::named_models(&settings).into_iter().map(|(_, r)| r).collect();
    let providers = Arc::new(kagent_providers::ProviderStore::load(paths.providers(), catalog, &named)?);
    let keyring = secrets::attach(providers.clone()).await;
    tracing::info!("provider keys: {}", if keyring { "Secret Service (desktop keyring)" } else { "providers.json (0600)" });

    let snapshot = machine::MachineSnapshot::system().await;
    let machine = snapshot.current();
    let (events, _) = broadcast::channel(1024);
    let audit = std::sync::Arc::new(kagent_core::audit::AuditLog::open(&paths.data.join("audit.jsonl"))?);
    audit.append_start(VERSION).ok();
    // One skills library for the prompt, `load_skill` and the skills API, rescanned between turns.
    let skill_sources = Arc::new(skills_sources::SourceStore::load(paths.config.join("skill-sources.json"), paths.data.join("skill-sources")).map_err(anyhow::Error::msg)?);
    let skills = Arc::new(skills::SkillHub::new(&paths.skills_roots(), &settings.skills_config().disabled, events.clone(), paths.skill_usage()).with_sources(skill_sources.clone()));
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
    registry.register(Arc::new(skills.tool()));
    for tool in skills.author_tools() {
        registry.register(tool);
    }
    let task_store = tasks::TaskStore::load(&paths.data, events.clone(), settings.tasks_config());
    registry.register(Arc::new(tasks::TasksTool::new(task_store.clone())));
    // The `settings` tool (voice-control.md) changes settings through the route's function; the
    // daemon's state exists only later, so the tool gets it through this cell.
    let settings_host: settings_tool::HostCell = Default::default();
    registry.register(Arc::new(settings_tool::SettingsTool::new(settings_host.clone())));
    let ui_hub = ui::UiHub::new(events.clone());
    registry.register(Arc::new(ui::UiTool::new(ui_hub.clone())));
    registry.register(Arc::new(kagent_core::tools::sessions::Sessions::new(paths.sessions())?));
    // The todo lists of the sessions (plan-todo.md section 1): the tools reach the agent's sessions through this handle.
    let todo_access = kagent_core::todo::TodoAccess::default();
    registry.register(Arc::new(kagent_core::todo::TodoWrite::new(todo_access.clone())));
    registry.register(Arc::new(kagent_core::todo::TodoRead::new(todo_access.clone())));
    // Plan mode (plan-todo.md section 2): the same kind of handle, and the folder of the approved plans.
    let plan_access = kagent_core::plan::PlanAccess::default();
    registry.register(Arc::new(kagent_core::plan::PlanEnter::new(plan_access.clone())));
    registry.register(Arc::new(kagent_core::plan::PlanExit::new(plan_access.clone(), paths.data.join("plans"))));
    // Subagents and background tasks (agents.md): the four subagent tools reach the agent
    // and its profiles through this handle; the tasks use the daemon's one registry.
    let agent_access = kagent_core::tools::agents::AgentAccess::default();
    agent_access.register(&mut registry);
    // Workflow runs (workflows.md): the tools reach the engine through this handle.
    let workflow_access = kagent_core::tools::workflow::WorkflowAccess::default();
    workflow_access.register(&mut registry);
    let background = kagent_core::tools::background::Background::shared();
    registry.register(Arc::new(kagent_core::tools::tasktools::TaskOutput::new(background.clone())));
    registry.register(Arc::new(kagent_core::tools::tasktools::TaskStop::new(background)));
    let browser = Arc::new(browser::Browser::from_settings(&paths.data, &settings));
    registry.register(browser.clone());
    let settings_cell = Arc::new(std::sync::OnceLock::new());
    registry.register(Arc::new(screen::Screen::new(providers.clone(), settings_cell.clone(), &paths.data)));
    // The web tools (web.md): they read settings.search from the same cell.
    let web = Arc::new(web::Web::new(settings_cell.clone(), &paths.config, &paths.data, events.clone()));
    registry.register(web.fetch_tool());
    registry.register(web.search_tool());
    // Google Calendar (calendar.md): the tools are registered now and find the module once it exists.
    let calendar_cell: calendar::tools::CalendarCell = Default::default();
    for tool in calendar::tools::all(&calendar_cell) {
        registry.register(tool);
    }
    // Recommended local models (local-models.md): the tool finds the module once it exists.
    let local_models_cell: local_models::tool::LocalModelsCell = Default::default();
    registry.register(Arc::new(local_models::tool::LocalModelsTool::new(local_models_cell.clone())));
    // Usage and limits (usage.md): the tool finds the hub once it exists.
    let usage_cell: usage::tool::UsageCell = Default::default();
    registry.register(Arc::new(usage::tool::UsageTool::new(usage_cell.clone())));
    // Tickets (tickets.md): the tool finds the store once it exists.
    let tickets_cell: tickets::tool::TicketsCell = Default::default();
    registry.register(Arc::new(tickets::tool::TicketsTool::new(tickets_cell.clone())));
    // The screen watch (screen-watch.md): the tool finds the watch once it exists.
    let screen_watch_cell: screen_watch::tool::WatchCell = Default::default();
    registry.register(Arc::new(screen_watch::tool::ScreenWatchTool::new(screen_watch_cell.clone())));
    // Inline widgets (widgets.md): the fragments live under <data>/widgets.
    let widgets = Arc::new(widgets::Widgets::new(paths.data.clone()));
    registry.register(Arc::new(widgets::WidgetShow::new(widgets.clone())));
    // Memory (memory.md): one store for the prompt, the tools and the API. The tools read the
    // settings from the cell the screen tool uses, set once the settings are loaded.
    let memory = Arc::new(memory::MemoryHub::new(memory::MemoryStore::new(paths.config.join("memory")), paths.sessions(), settings_cell.clone(), events.clone()));
    memory.register(&mut registry);
    // (name, group) of every registered tool, for the per-turn prompt.
    let tool_index: Vec<(String, &'static str)> = registry
        .contracts()
        .iter()
        .map(|c| (c.name.clone(), kagent_core::tools::group_of(&c.name)))
        .collect();

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
    // Usage and limits (usage.md): the providers client reports every answer to the hub.
    let usage = usage::UsageHub::open(&paths.data, providers.clone(), settings_arc.clone(), events.clone());
    let _ = usage_cell.set(usage.clone());
    let tickets = tickets::Tickets::new(paths.data.join("tickets"), events.clone());
    let _ = tickets_cell.set(tickets.clone());
    // Hooks (hooks.md): the owner's own commands at the points of the agent's life. The agent
    // cannot write them (hooks.md section 5); a change applies from the next event.
    let hooks = Arc::new(hooks::HookHub::new(settings_arc.clone(), paths.settings(), audit.clone(), events.clone()));
    *services.hooks.write().unwrap() = Some(hooks.clone());
    // Token guard (token-guard.md): the daemon's API token never reaches the model, whatever a
    // tool read it from (a file, the journal, a browser profile on disk).
    {
        let token = token.clone();
        *services.redact.write().unwrap() = Some(Arc::new(move |text: &str| {
            if token.is_empty() {
                text.to_string()
            } else {
                text.replace(token.as_str(), "<redacted: KAgent API token>")
            }
        }));
    }
    alerts.set_alert_hooks(hooks.clone());
    // Built for every turn (turn_config.rs).
    let turn_config = turn_config::TurnConfig {
        settings: settings_arc.clone(),
        providers: providers.clone(),
        machine: snapshot,
        skills: skills.clone(),
        memory: memory.clone(),
        tool_index,
    };
    let cfg = move |turn: TurnContext| turn_config.build(turn);

    let agent = Agent::new(AgentDeps {
        services: Some(services.clone()),
        hooks: Some(hooks.clone()),
        audit: Some(audit.clone()),
        model: Arc::new(kagent_providers::ProvidersClient::new(providers.clone()).with_usage(usage.clone())),
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
    todo_access.attach(&agent);
    tickets.attach(&agent);
    plan_access.attach(&agent);
    // Subagents (agents.md): the profiles, the model windows, the background tasks and the limit.
    let subagents = subagents::Subagents::start(
        agent.clone(),
        agent_access,
        subagents::ProfileHub::new(paths.config.join("agents")),
        providers.clone(),
        paths.data.join("task-out"),
        settings_arc.read().unwrap().subagents_config().max_concurrent,
        events.clone(),
    )
    .await;
    // Workflow runs (workflows.md): the engine over <data>/runs, the definitions of <config>/workflows.
    let project_check: kagent_core::tools::workflow::ProjectCheck = {
        let settings = settings_arc.clone();
        Arc::new(move |path: &str| {
            let roots = settings.read().unwrap().desktop_prefs().project_roots;
            let wanted = std::fs::canonicalize(path).ok()?;
            kagent_core::tools::xdg::projects(&roots).into_iter().filter_map(|p| std::fs::canonicalize(p).ok()).find(|p| *p == wanted).map(|p| p.to_string_lossy().to_string())
        })
    };
    let runs = runs::Runs::start(agent.clone(), workflow_access, subagents.profiles.clone(), paths.data.join("runs"), paths.config.join("workflows"), settings_arc.read().unwrap().workflows_config().max_runs, project_check);
    // A run's tasks are tickets (workflows.md section 7).
    runs::tickets::TicketSync::new(tickets.clone(), runs.engine.clone()).spawn(events.subscribe());
    // Compaction (compaction.md section 1) reads its settings before every model request.
    let (settings_for_compaction, providers_for_compaction) = (settings_arc.clone(), providers.clone());
    agent.set_compaction_source(Arc::new(move || {
        let s = settings_for_compaction.read().unwrap();
        let section = s.compaction_config();
        kagent_core::compact::CompactionConfig {
            enabled: section.enabled,
            threshold_pct: section.threshold_pct,
            // A fast model that is not set, or cannot be used, is none.
            fast_model_ref: if providers_for_compaction.usable(&s.fast_model).is_some() { s.fast_model.clone() } else { String::new() },
        }
    }));

    // Who reads the images of a message (vision.md section 2).
    agent.set_vision_source(image_readers::source(settings_arc.clone(), providers.clone()));

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

    let notifier = notify::Notifier::new();
    let bots = bots::Bots::daemon(agent.clone(), alerts.clone(), providers.clone(), settings_arc.clone(), &paths, events.clone()).await;
    let calendar = calendar::Calendar::daemon(settings_arc.clone(), &paths, events.clone(), web.clone(), notifier.clone(), bots.clone(), task_store.clone()).await;
    let _ = calendar_cell.set(calendar.clone());
    calendar::change::register_undo(&calendar, &services);
    let local_models = local_models::LocalModels::new(providers.clone(), settings_arc.clone(), events.clone());
    let _ = local_models_cell.set(local_models.clone());
    let screen_watch = screen_watch::ScreenWatch::new(
        Arc::new(screen_watch::env::RealEnv { providers: providers.clone(), settings: settings_arc.clone(), local_models: local_models.clone(), agent: agent.clone(), notifier: notifier.clone() }),
        events.clone(),
        Some(&paths.data),
    );
    let _ = screen_watch_cell.set(screen_watch.clone());
    screen_watch.start_up().await;
    let state = Arc::new(AppState {
        net,
        ui: ui_hub,
        tasks: task_store,
        windows: Arc::new(appwin::AppWindows::new(&paths.data, &token, 7878)),
        sentinel,
        alerts,
        notifier,
        bots,
        calendar,
        local_models,
        usage,
        tickets,
        runs,
        screen_watch,
        skills: skills.clone(),
        memory,
        skill_sources,
        browser,
        web,
        subagents,
        hooks,
        widgets,
        live_sessions: Default::default(),
        listen_start: Default::default(),
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

    let _ = settings_host.set(Arc::new(settings_apply::Host(state.clone())));

    // KRunner (Alt+Space "ka ..."): keep the connection alive for the daemon's lifetime.
    // KRunner is Plasma's launcher; other desktops have no consumer for it.
    let _krunner = if kagent_core::platform::desktop() == kagent_core::platform::Desktop::Kde { krunner::serve(state.clone()).await } else { None };
    tracing::info!("platform: {}", kagent_core::platform::summary());
    tasks::start(state.clone());
    state.bots.start(state.notifier.clone(), state.alerts.clone());
    state.calendar.start();
    rounded::watch(state.windows.clone(), 14);
    {
        // Name old sessions that still have a placeholder title.
        let agent = state.agent.clone();
        tokio::spawn(async move { agent.backfill_titles().await });
    }
    let tray_ok = tray::start(state.clone()).await;
    // KAgent lives in the tray and starts as the small widget (settings.startWidget,
    // default on). A window left open by a previous daemon is reused. With Listen on start
    // (voice.md section 14) the window of a start after login is opened with the one-time marker instead.
    let listens = state.settings.read().unwrap().voice_config().listen_on_start;
    if listens || (tray_ok && state.settings.read().unwrap().start_widget()) {
        let st = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if listens && listen_start::start::at_daemon_start(st.clone()).await {
                return;
            }
            if !(tray_ok && st.settings.read().unwrap().start_widget()) {
                return;
            }
            // A window left open by the previous daemon keeps its layout (forcing the widget
            // here once squeezed the full window's content into widget mode at full size).
            if !st.windows.adopt_existing().await {
                tray::set_window(&st, appwin::Mode::Compact, false).await;
            }
        });
    }
    let audit_on_exit = audit.clone();
    let web_on_exit = state.web.clone();
    let r = server::serve(state).await;
    // The reader's browser is closed with the daemon (web.md section 8).
    web_on_exit.shutdown().await;
    // Every background task gets TERM, and KILL if it is still there 2 s later (agents.md section 4).
    kagent_core::tools::background::Background::shared().shutdown().await;
    watch_cancel.cancel();
    let _ = audit_on_exit.append(kagent_core::audit::AuditKind::Stop, "daemon", serde_json::json!({"version": VERSION}));
    tracing::info!("kagentd stopped");
    r
}
