//! The `load_skill` tool (tools.md section 17) over the merged library, and the
//! usage counter it writes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tool::{Tool, ToolContext, ToolContract, ToolOutput};
use crate::tools::parse;

use super::library::{SkillLibrary, SkillSource};

/// The directory variables a skill body may contain.
const DIR_VARS: [&str; 3] = ["${CLAUDE_SKILL_DIR}", "${ZCODE_SKILL_DIR}", "${KAGENT_SKILL_DIR}"];

/// How often one skill was loaded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SkillUse {
    pub uses: u32,
    /// RFC 3339.
    pub last_used: String,
}

/// `<data>/skill-usage.json`: `{"<name>": {"uses": 3, "lastUsed": "<rfc3339>"}}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillUsage(BTreeMap<String, SkillUse>);

impl SkillUsage {
    /// The file; an empty counter when it is missing or damaged.
    pub fn load(path: &Path) -> Self {
        let Ok(bytes) = std::fs::read(path) else { return Self::default() };
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!("{} not readable, the usage count starts empty: {e}", path.display());
            Self::default()
        })
    }

    pub fn get(&self, name: &str) -> Option<SkillUse> {
        self.0.get(name).cloned()
    }

    /// Count one use of `name`, at `now`.
    pub fn record(&mut self, name: &str, now: DateTime<Utc>) {
        let uses = self.0.entry(name.to_string()).or_default();
        uses.uses = uses.uses.saturating_add(1);
        uses.last_used = now.to_rfc3339();
    }

    /// Write the file atomically, readable by the owner only.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        let mut file = std::fs::File::create(&tmp)?;
        let json = serde_json::to_vec_pretty(self).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        file.write_all(&json)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        drop(file);
        std::fs::rename(tmp, path)
    }
}

/// How often every skill was loaded: in memory, and in the file after each use. One
/// copy is shared by `load_skill`, a message that names a skill (`/name`, S20) and the
/// skills API, so a count is never lost between them.
#[derive(Debug)]
pub struct SkillCounter {
    usage: Mutex<SkillUsage>,
    path: PathBuf,
}

impl SkillCounter {
    /// The file; an empty counter when it is missing or damaged.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self { usage: Mutex::new(SkillUsage::load(&path)), path }
    }

    pub fn get(&self, name: &str) -> Option<SkillUse> {
        self.usage.lock().unwrap().get(name)
    }

    /// One more use of `name`. A file that cannot be written is logged, not an error
    /// of the caller.
    pub fn record(&self, name: &str) {
        let mut usage = self.usage.lock().unwrap();
        usage.record(name, Utc::now());
        if let Err(e) = usage.save(&self.path) {
            tracing::warn!("skill usage not saved: {e}");
        }
    }
}

/// The body with every directory variable replaced by the skill's directory.
pub fn expand_dirs(body: &str, dir: &Path) -> String {
    let dir = dir.to_string_lossy();
    DIR_VARS.iter().fold(body.to_string(), |text, var| text.replace(var, &dir))
}

/// `load_skill`: the body of an enabled skill of the merged library.
pub struct LoadSkill {
    contract: ToolContract,
    library: Arc<RwLock<SkillLibrary>>,
    counter: Arc<SkillCounter>,
}

impl LoadSkill {
    pub fn new(library: Arc<RwLock<SkillLibrary>>, counter: Arc<SkillCounter>) -> Self {
        Self {
            contract: ToolContract::read_only(
                "load_skill",
                "Load a skill by name: its instructions, with the frontmatter removed. The names \
                 are listed under \"Skills\" in the system prompt; load a skill before using it.",
                json!({"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}),
            ),
            library,
            counter,
        }
    }
}

#[derive(Deserialize, Default)]
struct NameArgs {
    name: Option<String>,
}

/// The enabled skills by source, for an error message.
fn available_list(library: &SkillLibrary) -> String {
    let mut out = String::from("Available skills:");
    for source in SkillSource::ALL {
        let names: Vec<&str> = library.available().filter(|e| e.source == source).map(|e| e.name.as_str()).collect();
        let names = if names.is_empty() { "(none)".to_string() } else { names.join(", ") };
        out.push_str(&format!("\n- {}: {}", source.label(), names));
    }
    out
}

#[async_trait]
impl Tool for LoadSkill {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: NameArgs = parse(input)?;
        let name = args.name.unwrap_or_default();
        let name = name.trim();
        if name.is_empty() {
            return Ok(ToolOutput::error("name is required"));
        }
        let library = self.library.read().unwrap();
        match library.get(name) {
            Some(entry) if entry.available() => {
                let body = expand_dirs(&entry.body, &entry.dir);
                drop(library);
                self.counter.record(name);
                Ok(ToolOutput::ok(body))
            }
            Some(_) => Ok(ToolOutput::error(format!("skill '{name}' is disabled in settings (skills.disabled). {}", available_list(&library)))),
            None => Ok(ToolOutput::error(format!("unknown skill '{name}'. {}", available_list(&library)))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use super::super::SkillRoot;

    fn skill(root: &Path, name: &str, description: &str, body: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\n{body}")).unwrap();
    }

    /// A bundled `alpha` with the directory variables and a bundled `hidden` (disabled
    /// by name), plus a user skill `zeta`.
    fn rig() -> (tempfile::TempDir, LoadSkill, Arc<SkillCounter>, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let (bundled, user) = (dir.path().join("bundled"), dir.path().join("user"));
        skill(&bundled, "alpha", "first", "Run ${CLAUDE_SKILL_DIR}/a.sh, ${ZCODE_SKILL_DIR}/b.sh and ${KAGENT_SKILL_DIR}/c.sh.\n");
        skill(&bundled, "hidden", "not for the model", "Hidden body");
        skill(&user, "zeta", "written by the owner", "Zeta body");
        let roots = [SkillRoot { path: bundled, source: SkillSource::Bundled }, SkillRoot { path: user, source: SkillSource::User }];
        let library = Arc::new(RwLock::new(SkillLibrary::scan(&roots, &["hidden".to_string()])));
        let usage = dir.path().join("data/skill-usage.json");
        let counter = Arc::new(SkillCounter::load(&usage));
        (dir, LoadSkill::new(library, counter.clone()), counter, usage)
    }

    #[tokio::test]
    async fn returns_the_body_with_the_directory_expanded_and_counts_a_use() {
        // Scenario S18 item 5.
        let (dir, tool, counter, usage_path) = rig();
        let ctx = ToolContext::standalone(Duration::from_secs(5), 1 << 20);
        let out = tool.call(json!({"name": "alpha"}), ctx.clone()).await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        let skill_dir = dir.path().join("bundled/alpha").display().to_string();
        assert_eq!(out.content, format!("Run {skill_dir}/a.sh, {skill_dir}/b.sh and {skill_dir}/c.sh."), "frontmatter removed");
        assert_eq!(counter.get("alpha").map(|u| u.uses), Some(1), "counted in memory");
        let uses = SkillUsage::load(&usage_path);
        assert_eq!(uses.get("alpha").map(|u| u.uses), Some(1), "and in the file");
        let rfc3339 = uses.get("alpha").map(|u| DateTime::parse_from_rfc3339(&u.last_used).is_ok());
        assert_eq!(rfc3339, Some(true), "lastUsed is RFC 3339");
        tool.call(json!({"name": "alpha"}), ctx).await.unwrap();
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&usage_path).unwrap()).unwrap();
        assert_eq!(file["alpha"]["uses"], 2);
        assert!(file["alpha"]["lastUsed"].is_string() && file.as_object().unwrap().len() == 1, "{file}");
    }

    #[tokio::test]
    async fn unknown_and_disabled_names_list_the_enabled_skills_by_source() {
        let (_dir, tool, _counter, _usage) = rig();
        let ctx = ToolContext::standalone(Duration::from_secs(5), 1 << 20);
        let out = tool.call(json!({"name": "nope"}), ctx.clone()).await.unwrap();
        assert!(out.is_error && out.content.contains("unknown skill 'nope'"), "{}", out.content);
        assert!(
            out.content.contains("\n- user: zeta") && out.content.contains("\n- imported: (none)") && out.content.contains("\n- bundled: alpha"),
            "{}",
            out.content
        );
        assert!(!out.content.contains("hidden"), "a disabled skill is not offered: {}", out.content);
        // A disabled skill is refused too, with the same list.
        let out = tool.call(json!({"name": "hidden"}), ctx.clone()).await.unwrap();
        assert!(out.is_error && out.content.contains("skill 'hidden' is disabled") && out.content.contains("- bundled: alpha"), "{}", out.content);
        let out = tool.call(json!({"name": "zeta"}), ctx).await.unwrap();
        assert_eq!(out.content, "Zeta body");
    }

    #[test]
    fn usage_file_round_trips_and_a_damaged_one_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skill-usage.json");
        let mut usage = SkillUsage::load(&path);
        assert_eq!(usage, SkillUsage::default(), "no file yet");
        let at = "2026-11-25T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        usage.record("slow-machine", at);
        usage.record("slow-machine", at);
        usage.save(&path).unwrap();
        assert_eq!(SkillUsage::load(&path).get("slow-machine"), Some(SkillUse { uses: 2, last_used: "2026-11-25T10:00:00+00:00".into() }));
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(SkillUsage::load(&path), SkillUsage::default());
    }
}
