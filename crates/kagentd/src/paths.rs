//! KAgent paths: `~/.config/kagent/` + `~/.local/share/kagent/`, with `KAGENT_HOME`
//! overriding both (tests use `KAGENT_HOME=$(mktemp -d)`).

use std::path::PathBuf;

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
    /// Bundled skills: repo `skills/` in dev, next to the binary when installed.
    pub fn skills_dir(&self) -> PathBuf {
        let user = self.config.join("skills");
        if user.is_dir() {
            return user;
        }
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
        for candidate in [
            exe.parent().map(|p| p.join("../../skills")).map(canonical_opt).unwrap_or(None),
            canonical_opt(PathBuf::from("skills")),
            canonical_opt(PathBuf::from("/usr/share/kagent/skills")),
        ]
        .into_iter()
        .flatten()
        {
            return candidate;
        }
        PathBuf::from("skills")
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

fn canonical_opt(p: PathBuf) -> Option<PathBuf> {
    std::fs::canonicalize(&p).ok().filter(|c| c.is_dir())
}
