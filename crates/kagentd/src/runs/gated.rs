//! `plan-only` against the real local model (`qwen2.5:7b-instruct` in Ollama on this machine). Gated: it runs only with
//! `KAGENT_IT=1` (local Ollama only, like the other gated tests) and reports what the small model did. A 7B model may not
//! return a valid plan: then the run must fail cleanly and say why, and that is a pass.

use std::sync::Arc;
use std::time::{Duration, Instant};

use kagent_core::agent::{Agent, AgentConfig, AgentDeps};
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::session::SessionStore;
use kagent_core::tool::ToolRegistry;
use kagent_core::workflow::{roles, shipped, Engine, RunState, StartRequest};
use kagent_providers::client::ProvidersClient;
use kagent_providers::config::{BuiltinCatalog, ModelConfigPatch, ModelEntry};
use kagent_providers::store::ProviderStore;

#[tokio::test]
async fn plan_only_against_the_real_local_model() {
    if std::env::var("KAGENT_IT").as_deref() != Ok("1") {
        eprintln!("skipped: KAGENT_IT=1 runs it against the local Ollama");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let catalog: BuiltinCatalog = serde_json::from_str(include_str!("../../../../config/providers.builtin.json")).unwrap();
    let store = Arc::new(ProviderStore::load(dir.path().join("providers.json"), catalog, &[]).unwrap());
    if store.provider("ollama").is_none() {
        store.add_from_template("ollama", None, None, None, Default::default(), None).unwrap();
    }
    store
        .update("ollama", |p| {
            if !p.models.iter().any(|m| m.id == "qwen2.5:7b-instruct") {
                p.models.insert(0, ModelEntry { id: "qwen2.5:7b-instruct".into(), name: String::new(), enabled: true, roles: vec!["chat".into()], smart: true, config: ModelConfigPatch::default() });
            }
        })
        .unwrap();
    let sessions = SessionStore::new(dir.path().join("sessions")).unwrap();
    let (events, _rx) = tokio::sync::broadcast::channel(1024);
    let access = kagent_core::tools::workflow::WorkflowAccess::default();
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(kagent_core::tools::files::ReadFile::new()));
    tools.register(Arc::new(kagent_core::tools::files::ListDir::new()));
    access.register(&mut tools);
    let cfg = AgentConfig { model_ref: "ollama::qwen2.5:7b-instruct".into(), system_prompt: "You are a coding assistant.".into(), max_iterations: 12, options: Default::default(), privilege: Default::default(), context_tokens: 8192, fallback_model_ref: String::new(), fallback_context_tokens: 0, fallback_max_output: None };
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(ProvidersClient::new(store)),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Full, vec![])),
        broker: PermissionBroker::new(events.clone(), Duration::from_secs(60)),
        store: sessions,
        events,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    let known = |p: &str| roles::profile(p).is_some();
    let engine = Engine::new(
        agent,
        dir.path().join("runs"),
        Arc::new(|p: &str| roles::profile(p)),
        Arc::new(move |name: &str| shipped::all(&known).into_iter().find(|d| d.name == name).ok_or_else(|| format!("no workflow {name}"))),
    );
    access.attach(&engine);
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("math.py"), "def double(x):\n    return x * 2\n").unwrap();
    let run = engine
        .start_run(StartRequest { workflow: "plan-only".into(), project: project.to_string_lossy().to_string(), goal: "Add a function add(a, b) to math.py that returns the sum of two numbers.".into(), origin_session: None, ticket: None, models: Default::default() })
        .await
        .unwrap();
    let started = Instant::now();
    let mut approved = false;
    loop {
        let r = engine.get(&run.id).unwrap();
        if r.state == RunState::AwaitingApproval && !approved {
            approved = true;
            eprintln!("workflow gated: the plan after {:?}: {} task(s): {:?}", started.elapsed(), r.tasks.len(), r.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>());
            engine.approve_plan(&run.id, None).unwrap();
        }
        if r.state.is_final() {
            eprintln!("workflow gated: the run ended {:?} after {:?}: reason {:?}", r.state, started.elapsed(), r.reason);
            match r.state {
                RunState::Done => assert!(!r.tasks.is_empty() && approved, "a plan was approved"),
                RunState::Failed => assert!(!r.reason.is_empty(), "a failed run says why"),
                other => panic!("unexpected end {other:?}"),
            }
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(420), "the run did not end in 7 minutes: {:?}", r.state);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
