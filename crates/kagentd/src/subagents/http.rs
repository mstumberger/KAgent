//! The routes of the profiles (api.md section 6): list, create or change, delete; and
//! Stop for a subagent, the button of an Agents card. The work is in functions that take
//! the hub, so the validation is tested without a server (a second daemon cannot run
//! next to the owner's, S17); the handlers wrap them.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kagent_core::agent::children::{Profile, DEFAULT_MAX_ITERATIONS};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::ApiResult;
use crate::subagents::{check, render, valid_name, view, AgentProfileView, ProfileHub};
use crate::AppState;

/// Why the API answers with an error: the status, the code, the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub StatusCode, pub &'static str, pub String);

const INVALID: &str = "invalid_input";
const CONFLICT: &str = "conflict";

fn invalid(reason: String) -> Refusal {
    Refusal(StatusCode::BAD_REQUEST, INVALID, reason)
}

fn conflict(reason: String) -> Refusal {
    Refusal(StatusCode::CONFLICT, CONFLICT, reason)
}

fn unknown(name: &str) -> Refusal {
    Refusal(StatusCode::NOT_FOUND, "not_found", format!("no profile '{name}'"))
}

fn built_in(name: &str) -> Refusal {
    conflict(format!("'{name}' is a built-in profile: it cannot be changed or deleted"))
}

impl From<Refusal> for crate::server::HttpError {
    fn from(r: Refusal) -> Self {
        crate::server::HttpError(r.0, r.1, r.2)
    }
}

/// The body of PUT `/api/agents/:name`: camelCase, like the rest of the API.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct ProfileBody {
    description: Option<String>,
    model: Option<String>,
    tool_groups: Option<Vec<String>>,
    max_iterations: Option<usize>,
    prompt: Option<String>,
}

impl ProfileHub {
    /// What `GET /api/agents` lists.
    pub fn views(&self) -> Vec<AgentProfileView> {
        self.profiles().iter().map(view).collect()
    }

    /// Write the file of `profile` and read the folder again.
    async fn write(&self, profile: &Profile) -> Result<(), Refusal> {
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            return Err(Refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("cannot create the folder of the profiles: {e}")));
        }
        let path = self.dir.join(format!("{}.md", profile.name));
        if let Err(e) = kagent_core::session::write_atomic(&path, render(profile).as_bytes()).await {
            return Err(Refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("cannot write the profile: {e}")));
        }
        self.reload();
        Ok(())
    }
}

/// PUT `/api/agents/:name`: a new profile, or a change of one of the owner's (a field left
/// out keeps its value). `model_known` says whether a model ref is one the providers
/// know. The answer and whether it was created.
pub async fn put(hub: &ProfileHub, name: &str, body: &Value, model_known: impl Fn(&str) -> bool) -> Result<(AgentProfileView, bool), Refusal> {
    let req: ProfileBody = serde_json::from_value(body.clone()).map_err(|e| invalid(format!("expected an object with description, model, toolGroups, maxIterations and prompt: {e}")))?;
    if Profile::builtin(name).is_some() {
        return Err(built_in(name));
    }
    let current = hub.find(name);
    let was = |get: fn(&Profile) -> String| current.as_ref().map(get).unwrap_or_default();
    let mut profile = Profile {
        name: name.to_string(),
        description: req.description.clone().unwrap_or_else(|| was(|p| p.description.clone())),
        model: req.model.clone().unwrap_or_else(|| was(|p| p.model.clone())),
        tool_groups: req.tool_groups.clone().unwrap_or_else(|| current.as_ref().map_or_else(Vec::new, |p| p.tool_groups.clone())),
        max_iterations: req.max_iterations.unwrap_or_else(|| current.as_ref().map_or(DEFAULT_MAX_ITERATIONS, |p| p.max_iterations)),
        prompt: req.prompt.clone().unwrap_or_else(|| was(|p| p.prompt.clone())),
        read_only: false,
    };
    for text in [&mut profile.description, &mut profile.model, &mut profile.prompt] {
        *text = text.trim().to_string();
    }
    for group in &mut profile.tool_groups {
        *group = group.trim().to_string();
    }
    check(&profile, &crate::settings::known_groups()).map_err(invalid)?;
    if !profile.model.is_empty() && !model_known(&profile.model) {
        return Err(invalid(format!("model: '{}' is not a model the providers know (a ref is <providerId>::<modelId>); leave it empty for the chat model", profile.model)));
    }
    hub.write(&profile).await?;
    let created = current.is_none();
    Ok((view(&profile), created))
}

/// DELETE `/api/agents/:name`: the file of a profile of the owner's.
pub async fn remove(hub: &ProfileHub, name: &str) -> Result<(), Refusal> {
    if Profile::builtin(name).is_some() {
        return Err(built_in(name));
    }
    let path = hub.dir().join(format!("{name}.md"));
    if !valid_name(name) || tokio::fs::symlink_metadata(&path).await.is_err() {
        return Err(unknown(name));
    }
    if let Err(e) = tokio::fs::remove_file(&path).await {
        return Err(Refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("cannot delete the profile: {e}")));
    }
    hub.reload();
    Ok(())
}

// ------------------------------------------------------------------ handlers

async fn ok(v: impl IntoResponse) -> ApiResult<Response> {
    Ok(v.into_response())
}

/// GET `/api/agents`: the folder is read again, so a file edited by hand is listed as it is.
pub async fn list(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    st.subagents.profiles.reload();
    ok(Json(json!({ "agents": st.subagents.profiles.views(), "invalid": st.subagents.profiles.invalid() }))).await
}

pub async fn put_profile(State(st): State<Arc<AppState>>, Path(name): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    let (profile, created) = put(&st.subagents.profiles, &name, &body, |r| st.providers.resolve(r).is_ok()).await?;
    Ok((if created { StatusCode::CREATED } else { StatusCode::OK }, Json(profile)).into_response())
}

pub async fn delete(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> ApiResult<Response> {
    remove(&st.subagents.profiles, &name).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// POST `/api/sessions/:id/subagents/:child/stop`: a running subagent is cancelled and a
/// queued one never starts (`agent_stop` for the Stop button of an Agents card).
pub async fn stop(State(st): State<Arc<AppState>>, Path((id, child)): Path<(String, String)>) -> ApiResult<Response> {
    match st.agent.cancel_child(&id, &child).await {
        Ok(status) => ok(Json(json!({ "status": status }))).await,
        Err(reason) if reason.starts_with("the subagent already ended") => Err(Refusal(StatusCode::CONFLICT, CONFLICT, reason).into()),
        Err(reason) => Err(Refusal(StatusCode::NOT_FOUND, "not_found", reason).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub() -> (tempfile::TempDir, Arc<ProfileHub>) {
        let dir = tempfile::tempdir().unwrap();
        let hub = ProfileHub::new(dir.path().join("agents"));
        hub.reload();
        (dir, hub)
    }

    #[tokio::test]
    async fn a_profile_is_created_changed_and_deleted() {
        let (dir, hub) = hub();
        let known = |_: &str| true;
        assert_eq!(hub.views().len(), 2, "the built-ins");
        let (made, created) = put(&hub, "disk-audit", &json!({ "description": "Disk usage", "toolGroups": ["files", "system"], "maxIterations": 12, "prompt": "Be brief." }), known).await.unwrap();
        assert!(created);
        assert_eq!((made.name.as_str(), made.source, made.max_iterations, made.tool_groups.clone()), ("disk-audit", "user", 12, vec!["files".to_string(), "system".into()]));
        let file = dir.path().join("agents/disk-audit.md");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "---\nname: disk-audit\ndescription: Disk usage\nmodel:\ntoolGroups: [files, system]\nmaxIterations: 12\nprompt: |\n  Be brief.\n---\n");
        assert_eq!(hub.find("disk-audit").unwrap().prompt, "Be brief.", "the folder was read again");

        // A change: a field left out keeps its value.
        let (changed, created) = put(&hub, "disk-audit", &json!({ "description": "Disk and inode usage" }), known).await.unwrap();
        assert!(!created);
        assert_eq!((changed.description.as_str(), changed.max_iterations, changed.tool_groups.len(), changed.prompt.as_str()), ("Disk and inode usage", 12, 2, "Be brief."));
        let (cleared, _) = put(&hub, "disk-audit", &json!({ "toolGroups": [], "model": "ollama::qwen2.5:7b", "maxIterations": 80 }), known).await.unwrap();
        assert_eq!((cleared.tool_groups.is_empty(), cleared.model.as_str(), cleared.max_iterations), (true, "ollama::qwen2.5:7b", 80));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "---\nname: disk-audit\ndescription: Disk and inode usage\nmodel: ollama::qwen2.5:7b\ntoolGroups:\nmaxIterations: 80\nprompt: |\n  Be brief.\n---\n");

        remove(&hub, "disk-audit").await.unwrap();
        assert!(!file.exists());
        assert_eq!(hub.views().len(), 2);
        assert_eq!(remove(&hub, "disk-audit").await.unwrap_err(), unknown("disk-audit"));
        assert!(!dir.path().join("agents").exists() || std::fs::read_dir(dir.path().join("agents")).unwrap().next().is_none(), "nothing left behind (no .tmp)");
    }

    #[tokio::test]
    async fn what_the_api_refuses() {
        let (_dir, hub) = hub();
        // The one model the providers know.
        async fn create(hub: &ProfileHub, name: &str, value: &Value) -> Result<(AgentProfileView, bool), Refusal> {
            put(hub, name, value, |r: &str| r == "ollama::qwen2.5:7b").await
        }

        // Built-in names: 409, and no file.
        for name in ["general", "explore"] {
            assert_eq!(create(&hub, name, &json!({ "description": "mine" })).await.unwrap_err(), built_in(name));
            assert_eq!(remove(&hub, name).await.unwrap_err(), built_in(name));
        }
        // The name, the description, the groups, the iterations, the model, unknown keys.
        assert_eq!(create(&hub, "Bad Name", &json!({ "description": "d" })).await.unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(create(&hub, "disk", &json!({})).await.unwrap_err().1, "invalid_input", "the description is required for a new profile");
        let group = create(&hub, "disk", &json!({ "description": "d", "toolGroups": ["filles"] })).await.unwrap_err().2;
        assert!(group.starts_with("toolGroups: unknown tool group 'filles' (system,") && group.contains("), or leave the key out for every group"), "{group}");
        assert_eq!(create(&hub, "disk", &json!({ "description": "d", "maxIterations": 0 })).await.unwrap_err().2, "maxIterations must be between 1 and 80, not 0");
        assert_eq!(create(&hub, "disk", &json!({ "description": "d", "model": "nope::x" })).await.unwrap_err().2, "model: 'nope::x' is not a model the providers know (a ref is <providerId>::<modelId>); leave it empty for the chat model");
        assert!(create(&hub, "disk", &json!({ "description": "d", "toolgroup": [] })).await.unwrap_err().2.contains("unknown field `toolgroup`"), "a key that is not a field is refused");
        assert!(create(&hub, "disk", &json!("text")).await.unwrap_err().0 == StatusCode::BAD_REQUEST);
        assert!(hub.find("disk").is_none() && hub.views().len() == 2, "nothing was written");
        // An unknown profile.
        assert_eq!(remove(&hub, "nope").await.unwrap_err(), unknown("nope"));
        assert_eq!(remove(&hub, "../settings").await.unwrap_err(), unknown("../settings"), "the name is checked before it becomes a path");
    }

    #[tokio::test]
    async fn a_file_edited_by_hand_counts() {
        let (dir, hub) = hub();
        std::fs::create_dir_all(dir.path().join("agents")).unwrap();
        std::fs::write(dir.path().join("agents/old.md"), "---\ndescription: before\n---\n").unwrap();
        assert_eq!(hub.reload(), 1);
        let (changed, created) = put(&hub, "old", &json!({ "prompt": "after" }), |_| true).await.unwrap();
        assert!(!created, "the file was there");
        assert_eq!((changed.description.as_str(), changed.prompt.as_str()), ("before", "after"));
        // A file that is not valid is replaced by a valid one.
        std::fs::write(dir.path().join("agents/bad.md"), "no frontmatter").unwrap();
        assert_eq!(hub.reload(), 1);
        assert_eq!(hub.invalid()[0].file, "bad.md");
        let (fixed, created) = put(&hub, "bad", &json!({ "description": "fixed" }), |_| true).await.unwrap();
        assert!(created);
        assert_eq!((fixed.description.as_str(), hub.invalid().len()), ("fixed", 0));
    }
}
