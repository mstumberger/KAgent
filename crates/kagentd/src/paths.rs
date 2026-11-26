//! KAgent paths: `~/.config/kagent/` + `~/.local/share/kagent/`, with `KAGENT_HOME`
//! overriding both (tests use `KAGENT_HOME=$(mktemp -d)`).

use std::path::PathBuf;

use kagent_core::skills::{SkillRoot, SkillSource};

#[derive(Clone, Debug)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Paths {
    pub fn resolve() -> Self {
        if let Ok(home) = std::env::var("KAGENT_HOME") {
            let home = PathBuf::from(home);
            return Self { config: home.join("config"), data: home.join("data") };
        }
        let config = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("kagent");
        let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("kagent");
        Self { config, data }
    }

    pub fn settings(&self) -> PathBuf {
        self.config.join("settings.json")
    }
    pub fn providers(&self) -> PathBuf {
        self.config.join("providers.json")
    }
    pub fn token(&self) -> PathBuf {
        self.config.join("token")
    }
    pub fn sessions(&self) -> PathBuf {
        self.data.join("sessions")
    }
    /// Skill roots, lowest precedence first (skills.md section 2): the bundled
    /// playbooks, the imported skills and the owner's own.
    pub fn skills_roots(&self) -> Vec<SkillRoot> {
        vec![
            SkillRoot { source: SkillSource::Bundled, path: bundled_skills() },
            SkillRoot { source: SkillSource::Imported, path: self.config.join("skills-imported") },
            SkillRoot { source: SkillSource::User, path: self.config.join("skills") },
        ]
    }
    /// `<data>/skill-usage.json`: how often each skill was loaded.
    pub fn skill_usage(&self) -> PathBuf {
        self.data.join("skill-usage.json")
    }
    pub fn web_dist(&self) -> PathBuf {
        // Next to the binary, or in the repo when run from target/.
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
        let candidates = [
            exe.parent().map(|p| p.join("../../web/dist")).map(canonical_opt).unwrap_or(None),
            canonical_opt(PathBuf::from("web/dist")),
        ];
        candidates.into_iter().flatten().next().unwrap_or_else(|| PathBuf::from("web/dist"))
    }
}

/// The bundled skills: repo `skills/` in dev (next to the binary, or the working
/// directory), `/usr/share/kagent/skills` when installed. The first that exists.
fn bundled_skills() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    [
        exe.parent().map(|p| p.join("../../skills")).map(canonical_opt).unwrap_or(None),
        canonical_opt(PathBuf::from("skills")),
        canonical_opt(PathBuf::from("/usr/share/kagent/skills")),
    ]
    .into_iter()
    .flatten()
    .next()
    .unwrap_or_else(|| PathBuf::from("skills"))
}

fn canonical_opt(p: PathBuf) -> Option<PathBuf> {
    std::fs::canonicalize(&p).ok().filter(|c| c.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_roots_are_bundled_imported_and_user_and_usage_is_in_data() {
        let paths = Paths { config: PathBuf::from("/tmp/kagent-paths-test/config"), data: PathBuf::from("/tmp/kagent-paths-test/data") };
        let roots = paths.skills_roots();
        let sources: Vec<SkillSource> = roots.iter().map(|r| r.source).collect();
        assert_eq!(sources, [SkillSource::Bundled, SkillSource::Imported, SkillSource::User], "lowest precedence first");
        assert_eq!(roots[1].path, PathBuf::from("/tmp/kagent-paths-test/config/skills-imported"));
        assert_eq!(roots[2].path, PathBuf::from("/tmp/kagent-paths-test/config/skills"));
        assert_eq!(paths.skill_usage(), PathBuf::from("/tmp/kagent-paths-test/data/skill-usage.json"));
    }
}
