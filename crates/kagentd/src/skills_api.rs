//! The skills routes of api.md section 6: list, read, create, change, delete, switch
//! on and off, rescan (S20). Import and sources are `skills_sources_api` (S21). The work
//! is in functions that take the hub, so the validation is tested without a server; the
//! handlers wrap them.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kagent_core::skills::file;
use kagent_core::skills::{plan_delete, plan_save, remove_skill, write_skill, SavePlan, SkillSource};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::skills::{SkillHub, SkillView};
use crate::AppState;

/// Why the API answers with an error instead of a skill: the status, the code, the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub StatusCode, pub &'static str, pub String);

const INVALID: &str = "invalid_input";
const CONFLICT: &str = "conflict";

pub(crate) fn invalid(reason: String) -> Refusal {
    Refusal(StatusCode::BAD_REQUEST, INVALID, reason)
}

pub(crate) fn conflict(reason: String) -> Refusal {
    Refusal(StatusCode::CONFLICT, CONFLICT, reason)
}

fn unknown(name: &str) -> Refusal {
    Refusal(StatusCode::NOT_FOUND, "not_found", format!("unknown skill '{name}'"))
}

pub(crate) fn broken(reason: String) -> Refusal {
    Refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal", reason)
}

impl From<Refusal> for HttpError {
    fn from(r: Refusal) -> Self {
        HttpError(r.0, r.1, r.2)
    }
}

impl From<HttpError> for Refusal {
    fn from(e: HttpError) -> Self {
        Refusal(e.0, e.1, e.2)
    }
}

/// The body of POST and PUT `/api/skills`: camelCase, like the rest of the API.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SkillBody {
    name: Option<String>,
    description: Option<String>,
    when_to_use: Option<String>,
    body: Option<String>,
}

fn skill_body(value: &Value) -> Result<SkillBody, Refusal> {
    serde_json::from_value(value.clone()).map_err(|e| invalid(format!("expected an object with name, description, whenToUse and body as text: {e}")))
}

/// A field of the request; a missing one is an error, an empty text is not.
fn text<'a>(field: &str, value: &'a Option<String>) -> Result<&'a str, Refusal> {
    value.as_deref().ok_or_else(|| invalid(format!("{field} is required")))
}

/// The save checked against the library and the owner's root; a refusal is a 400.
fn plan(hub: &SkillHub, name: &str, description: &str, body: &str) -> Result<SavePlan, Refusal> {
    let library = hub.library().read().map_err(|_| broken("skills library lock poisoned".into()))?;
    plan_save(&library, &hub.root(SkillSource::User), name, description, body).map_err(invalid)
}

/// Write the skill. With `existing` (a change) the frontmatter the form does not show
/// stays: other keys, comments (`file::replace`).
async fn write(hub: &SkillHub, plan: &SavePlan, req: &SkillBody, existing: Option<&str>) -> Result<(), Refusal> {
    let (description, when_to_use, body) =
        (req.description.as_deref().unwrap_or_default(), req.when_to_use.as_deref().unwrap_or_default(), req.body.as_deref().unwrap_or_default());
    let text = match existing {
        Some(raw) => file::replace(raw, &plan.name, description, when_to_use, body),
        None => file::render(&plan.name, description, when_to_use, body),
    };
    write_skill(plan, &text).await.map_err(|e: std::io::Error| broken(format!("cannot write the skill: {e}")))?;
    hub.reload();
    Ok(())
}

/// POST `/api/skills`: a new skill of the owner (skills.md sections 5 and 11).
pub async fn create(hub: &SkillHub, value: &Value) -> Result<SkillView, Refusal> {
    let req = skill_body(value)?;
    let (name, description, body) = (text("name", &req.name)?, text("description", &req.description)?, text("body", &req.body)?);
    let plan = plan(hub, name, description, body)?;
    if plan.overwrite {
        return Err(conflict(format!("a skill named '{name}' exists already; change it instead")));
    }
    write(hub, &plan, &req, None).await?;
    Ok(hub.view(name, true).ok_or_else(|| unknown(name))?)
}

/// PUT `/api/skills/:name`: change the description, `whenToUse` or body of a skill of
/// the owner; a field left out keeps its value, and so does every other line of the
/// file's frontmatter (other keys, comments), which the form does not show.
pub async fn update(hub: &SkillHub, name: &str, value: &Value) -> Result<SkillView, Refusal> {
    let req = skill_body(value)?;
    let current = hub.view(name, true).ok_or_else(|| unknown(name))?;
    if current.source != SkillSource::User {
        return Err(conflict(format!("'{name}' is a {} skill; only your own skills can be changed", current.source.label())));
    }
    let keep = |given: &Option<String>, was: String| Some(given.clone().unwrap_or(was));
    let req = SkillBody {
        name: None,
        description: keep(&req.description, current.description),
        when_to_use: keep(&req.when_to_use, current.when_to_use),
        body: keep(&req.body, current.body.unwrap_or_default()),
    };
    let plan = plan(hub, name, text("description", &req.description)?, text("body", &req.body)?)?;
    let existing = tokio::fs::read_to_string(plan.dir.join("SKILL.md")).await.ok();
    write(hub, &plan, &req, existing.as_deref()).await?;
    Ok(hub.view(name, true).ok_or_else(|| unknown(name))?)
}

/// DELETE `/api/skills/:name`: a skill of the owner with its folder. An imported one
/// goes too, a link as a link (skills.md section 7); the skills KAgent ships with stay.
pub async fn delete(hub: &SkillHub, name: &str) -> Result<(), Refusal> {
    let view = hub.view(name, false).ok_or_else(|| unknown(name))?;
    let (root, link_ok) = match view.source {
        SkillSource::Bundled => return Err(conflict(format!("'{name}' is a skill KAgent ships with: switch it off instead of deleting it"))),
        SkillSource::User => (hub.root(SkillSource::User), false),
        SkillSource::Imported => (hub.root(SkillSource::Imported), true),
    };
    let plan = {
        let library = hub.library().read().map_err(|_| broken("skills library lock poisoned".into()))?;
        plan_delete(&library, &root, name, link_ok).map_err(conflict)?
    };
    remove_skill(&plan).await.map_err(|e: std::io::Error| broken(format!("cannot remove the skill: {e}")))?;
    hub.reload();
    Ok(())
}

/// `skills.disabled` after `name` is switched: out of the list when on, in it when off.
pub fn disabled_after(disabled: &[String], name: &str, enabled: bool) -> Vec<String> {
    let mut next: Vec<String> = disabled.iter().filter(|n| n.as_str() != name).cloned().collect();
    if !enabled {
        next.push(name.to_string());
    }
    next
}

/// `skills.disabled` after `names` are switched off: each once, the rest as it was.
/// An import or a sync calls this before it writes a file (skills.md sections 7 and 8).
pub fn quarantine_all(disabled: &[String], names: &[String]) -> Vec<String> {
    let mut next = disabled.to_vec();
    for name in names {
        if !next.iter().any(|d| d == name) {
            next.push(name.clone());
        }
    }
    next
}

// ------------------------------------------------------------------ handlers

fn ok(v: impl IntoResponse) -> ApiResult<Response> {
    Ok(v.into_response())
}

pub async fn list(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    ok(Json(json!({ "skills": st.skills.views() })))
}

pub async fn get_one(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> ApiResult<Response> {
    ok(Json(st.skills.view(&name, true).ok_or_else(|| not_found(format!("unknown skill '{name}'")))?))
}

pub async fn create_skill(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let view = create(&st.skills, &body).await?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

pub async fn update_skill(State(st): State<Arc<AppState>>, Path(name): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    ok(Json(update(&st.skills, &name, &body).await?))
}

pub async fn delete_skill(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> ApiResult<Response> {
    delete(&st.skills, &name).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn set_enabled(State(st): State<Arc<AppState>>, Path(name): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    st.skills.view(&name, false).ok_or_else(|| not_found(format!("unknown skill '{name}'")))?;
    let enabled = body.get("enabled").and_then(Value::as_bool).ok_or_else(|| bad_request("enabled (true or false) is required"))?;
    let current = st.settings.read().unwrap().skills_config().disabled;
    let next = disabled_after(&current, &name, enabled);
    if next != current {
        // The road of a save from the settings page: checked, stored, and every UI is told.
        crate::api::put_settings(State(st.clone()), Json(json!({ "skills": { "disabled": next } }))).await?;
        st.skills.refresh(&st.settings.read().unwrap().skills_config());
    }
    ok(Json(st.skills.view(&name, false).ok_or_else(|| not_found(format!("unknown skill '{name}'")))?))
}

pub async fn rescan(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    ok(Json(json!({ "count": st.skills.reload() })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use kagent_core::skills::author::RESERVED;
    use kagent_core::skills::SkillRoot;
    use tokio::sync::broadcast;

    fn write_skill(root: &Path, name: &str, description: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\nBody of {name}.")).unwrap();
    }

    /// A hub over a bundled root, `<home>/config/skills-imported` and `<home>/config/skills`.
    fn hub_in(home: &Path) -> (SkillHub, PathBuf, PathBuf) {
        let (bundled, imported, user) = (home.join("bundled"), home.join("config/skills-imported"), home.join("config/skills"));
        write_skill(&bundled, "alpha", "first");
        write_skill(&bundled, "skill-creator", "how to write a skill");
        write_skill(&imported, "gamma", "from elsewhere");
        write_skill(&user, "zeta", "written by the owner");
        let roots = [
            SkillRoot { path: bundled.clone(), source: SkillSource::Bundled },
            SkillRoot { path: imported, source: SkillSource::Imported },
            SkillRoot { path: user, source: SkillSource::User },
        ];
        let (events, _rx) = broadcast::channel(16);
        let hub = SkillHub::new(&roots, &[], events, home.join("data/skill-usage.json"));
        (hub, bundled, home.join("config"))
    }

    fn folder_count(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    #[tokio::test]
    async fn create_refuses_an_invalid_reserved_and_oversized_skill_with_400() {
        // skills.md section 11: API validation.
        let home = tempfile::tempdir().unwrap();
        let (hub, _bundled, config) = hub_in(home.path());
        let valid = json!({"name": "bluetooth-fix", "description": "no sound after reconnect", "body": "1. `bluetooth` info."});
        for (patch, said) in [
            (json!({"name": "Slow_Machine"}), "not a valid skill name"),
            (json!({"name": "../escape"}), "not a valid skill name"),
            (json!({"name": "skill-creator"}), "KAgent ships with"),
            (json!({"body": "x".repeat(64 * 1024 + 1)}), "bytes, more than"),
            (json!({"body": ""}), "the body is empty"),
            (json!({"description": "d".repeat(1025)}), "characters, more than"),
            (json!({"name": null}), "name is required"),
            (json!({"body": 7}), "as text"),
        ] {
            let mut body = valid.clone();
            body.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
            let before = folder_count(&config.join("skills"));
            let refusal = create(&hub, &body).await.err().unwrap_or_else(|| panic!("{body} must be refused"));
            assert_eq!((refusal.0, refusal.1), (StatusCode::BAD_REQUEST, INVALID), "{body}: {:?}", refusal.2);
            assert!(refusal.2.contains(said), "{body}: {}", refusal.2);
            assert_eq!(folder_count(&config.join("skills")), before, "{body}: nothing is written");
        }
        // At the limit, and with the optional field: fine.
        let mut at_limit = valid.clone();
        at_limit["body"] = json!("x".repeat(64 * 1024));
        at_limit["whenToUse"] = json!("headphones pair but stay silent");
        let view = create(&hub, &at_limit).await.unwrap();
        assert_eq!((view.name.as_str(), view.when_to_use.as_str(), view.source), ("bluetooth-fix", "headphones pair but stay silent", SkillSource::User));
        assert_eq!(folder_count(&config.join("skills")), 2, "zeta and the new one");
        assert!(RESERVED.contains(&"skill-creator"));
    }

    #[tokio::test]
    async fn create_change_and_delete_keep_to_the_owners_skills() {
        let home = tempfile::tempdir().unwrap();
        let (hub, bundled, config) = hub_in(home.path());

        // Create: the file is where the agent and the library expect it.
        let view = create(&hub, &json!({"name": "bluetooth-fix", "description": "no sound", "body": "1. `bluetooth` info."})).await.unwrap();
        assert_eq!(view.path, config.join("skills/bluetooth-fix"));
        assert_eq!(view.body.as_deref(), Some("1. `bluetooth` info."));
        assert!(config.join("skills/bluetooth-fix/SKILL.md").is_file());
        let again = create(&hub, &json!({"name": "bluetooth-fix", "description": "again", "body": "other"})).await.err().unwrap();
        assert_eq!((again.0, again.1), (StatusCode::CONFLICT, CONFLICT), "a skill that exists is not overwritten by a create");
        assert_eq!(hub.view("bluetooth-fix", true).unwrap().body.as_deref(), Some("1. `bluetooth` info."));

        // Change: what is left out keeps its value.
        let view = update(&hub, "bluetooth-fix", &json!({"body": "2. Restart it."})).await.unwrap();
        assert_eq!((view.description.as_str(), view.body.as_deref()), ("no sound", Some("2. Restart it.")));
        let view = update(&hub, "bluetooth-fix", &json!({"whenToUse": "after a reconnect"})).await.unwrap();
        assert_eq!(view.when_to_use, "after a reconnect");
        let view = update(&hub, "bluetooth-fix", &json!({"description": "no sound after reconnect"})).await.unwrap();
        assert_eq!(view.description, "no sound after reconnect");
        let raw = std::fs::read_to_string(config.join("skills/bluetooth-fix/SKILL.md")).unwrap();
        assert!(raw.contains("description: no sound after reconnect\nwhen_to_use: after a reconnect\n---\n\n2. Restart it.\n"), "{raw}");

        // A skill KAgent ships with, or one from elsewhere, is not the owner's to change or delete.
        for name in ["alpha", "skill-creator", "gamma"] {
            let refusal = update(&hub, name, &json!({"body": "other"})).await.err().unwrap_or_else(|| panic!("{name} must be refused"));
            assert_eq!((refusal.0, refusal.1), (StatusCode::CONFLICT, CONFLICT), "{name}: {}", refusal.2);
        }
        let refusal = delete(&hub, "alpha").await.err().unwrap();
        assert_eq!((refusal.0, refusal.1), (StatusCode::CONFLICT, CONFLICT));
        assert!(std::fs::read_to_string(bundled.join("alpha/SKILL.md")).unwrap().contains("Body of alpha."));
        let refusal = delete(&hub, "nope").await.err().unwrap();
        assert_eq!((refusal.0, refusal.1), (StatusCode::NOT_FOUND, "not_found"));

        // Delete the owner's: the folder is gone, from the disk and from the library.
        std::fs::write(config.join("skills/bluetooth-fix/notes.txt"), "more").unwrap();
        delete(&hub, "bluetooth-fix").await.unwrap();
        assert!(!config.join("skills/bluetooth-fix").exists());
        assert!(hub.view("bluetooth-fix", false).is_none());
        assert_eq!(update(&hub, "bluetooth-fix", &json!({"body": "back"})).await.err().unwrap().0, StatusCode::NOT_FOUND);

        // A name a bundled skill has may be taken over by the owner's; that one can go again.
        create(&hub, &json!({"name": "alpha", "description": "mine", "body": "My alpha"})).await.unwrap();
        assert_eq!(hub.view("alpha", false).unwrap().source, SkillSource::User);
        assert!(hub.views().iter().any(|v| v.name == "alpha" && v.shadowed), "the bundled one is listed as shadowed");
        delete(&hub, "alpha").await.unwrap();
        assert_eq!(hub.view("alpha", false).unwrap().source, SkillSource::Bundled);
    }

    #[tokio::test]
    async fn a_change_keeps_the_frontmatter_the_form_does_not_show() {
        let home = tempfile::tempdir().unwrap();
        let (hub, _bundled, config) = hub_in(home.path());
        std::fs::create_dir_all(config.join("skills/hand-made")).unwrap();
        std::fs::write(
            config.join("skills/hand-made/SKILL.md"),
            "---\n# written by hand\nname: hand-made\ndescription: the old one\nlicense: MIT\nmetadata:\n  author: someone\n---\n\nOld body.\n",
        )
        .unwrap();
        hub.reload();

        // A field left out keeps its value.
        let view = update(&hub, "hand-made", &json!({"description": "the new one", "whenToUse": "when it rains"})).await.unwrap();
        assert_eq!((view.description.as_str(), view.when_to_use.as_str(), view.body.as_deref()), ("the new one", "when it rains", Some("Old body.")));
        // And the file keeps its comment, its other keys and their nested lines.
        let raw = std::fs::read_to_string(config.join("skills/hand-made/SKILL.md")).unwrap();
        assert_eq!(
            raw,
            "---\nname: hand-made\ndescription: the new one\nwhen_to_use: when it rains\n# written by hand\nlicense: MIT\nmetadata:\n  author: someone\n---\n\nOld body.\n",
            "{raw}"
        );
        // A new skill gets the plain form.
        create(&hub, &json!({"name": "plain", "description": "d", "body": "b"})).await.unwrap();
        assert_eq!(std::fs::read_to_string(config.join("skills/plain/SKILL.md")).unwrap(), "---\nname: plain\ndescription: d\n---\n\nb\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_of_an_imported_link_removes_the_link_only() {
        let home = tempfile::tempdir().unwrap();
        let (hub, _bundled, config) = hub_in(home.path());
        let target = home.path().join("elsewhere/gamma");
        write_skill(&home.path().join("elsewhere"), "gamma", "linked");
        std::fs::remove_dir_all(config.join("skills-imported/gamma")).unwrap();
        std::os::unix::fs::symlink(&target, config.join("skills-imported/gamma")).unwrap();
        hub.reload();
        assert_eq!(hub.views().len(), 4, "zeta, gamma, alpha, skill-creator");
        delete(&hub, "gamma").await.unwrap();
        assert!(!config.join("skills-imported/gamma").exists(), "the link is gone");
        assert!(target.join("SKILL.md").is_file(), "the target is not");
    }

    #[test]
    fn switching_a_skill_edits_the_disabled_list_by_name() {
        let disabled = vec!["slow-machine".to_string(), "alpha".to_string()];
        assert_eq!(disabled_after(&disabled, "alpha", true), ["slow-machine"]);
        assert_eq!(disabled_after(&disabled, "alpha", false), ["slow-machine", "alpha"], "no double entry");
        assert_eq!(disabled_after(&disabled, "beta", false), ["slow-machine", "alpha", "beta"]);
        assert_eq!(disabled_after(&[], "alpha", true), Vec::<String>::new());
    }
}
