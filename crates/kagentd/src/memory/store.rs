//! The memory store (memory.md section 1): the folder `<config>/memory/`, its files, the
//! two core files, the limits and the generated index `MEMORY.md`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use kagent_core::skills::frontmatter;

use super::file;
use super::guard::{self, REFUSED};
use super::{is_core, today, Memory, MemoryError, MemorySource, SaveMode, INDEX, INDEX_NAME, MACHINE, MACHINE_SEED, MAX_BODY_CHARS, MAX_DESCRIPTION_CHARS, MAX_FILES, USER};

/// What a save was checked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// The memory to write.
    pub memory: Memory,
    /// A memory of that name exists already and is replaced.
    pub existed: bool,
    /// The memory named with `replaces`.
    pub old: Option<Memory>,
}

/// The folder `<config>/memory/`.
pub struct MemoryStore {
    dir: PathBuf,
    /// What a new `user.md` says: the address and the language of the persona.
    seed: Mutex<String>,
    /// Held during every change, so that two of them cannot interleave.
    lock: Mutex<()>,
}

impl MemoryStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            seed: Mutex::new(String::from("The owner writes Slovenian or English: answer in that language.")),
            lock: Mutex::new(()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file of the memory `name`.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.md"))
    }

    /// What a new `user.md` says (the persona).
    pub fn set_seed(&self, text: impl Into<String>) {
        *self.seed.lock().unwrap_or_else(|e| e.into_inner()) = text.into();
    }

    /// The folder exists and is 0700, and every file in it is 0600 (memory.md section 8).
    pub fn secure(&self) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&self.dir)?;
        std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o600))?;
            }
        }
        Ok(())
    }

    /// The two core files, where one is missing: `user.md` with what the persona says
    /// about the owner, `machine.md` with a comment that says what belongs there.
    /// Nothing else is filled in (memory.md section 1).
    pub fn ensure_core(&self) -> std::io::Result<()> {
        let seed = self.seed.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for (name, text) in [(USER, seed), (MACHINE, MACHINE_SEED.to_string())] {
            if !self.path(name).exists() {
                file::write_private(&self.path(name), &format!("{text}\n"))?;
            }
        }
        Ok(())
    }

    /// Every memory file by name, the superseded ones included. Skipped: the index, a
    /// name that is not valid, a link, anything that is not a regular file, and a file
    /// that cannot be read or parsed.
    pub fn load(&self) -> Vec<Memory> {
        let today = today();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut files: Vec<(String, PathBuf)> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?.strip_suffix(".md")?.to_string();
                (name != INDEX_NAME && frontmatter::valid_name(&name)).then(|| (name, path))
            })
            .collect();
        files.sort();
        files.into_iter().filter_map(|(name, path)| self.read(&name, &path, today)).collect()
    }

    /// Every memory file, after the two core files exist: a change to the store is a
    /// first use too, so the index it rewrites names them.
    fn files(&self) -> Result<Vec<Memory>, MemoryError> {
        self.ensure_core().map_err(|e| MemoryError::Io(format!("cannot create the core files in {}: {e}", self.dir.display())))?;
        Ok(self.load())
    }

    /// One memory by name, a superseded one too.
    pub fn get(&self, name: &str) -> Option<Memory> {
        if name == INDEX_NAME || !frontmatter::valid_name(name) {
            return None;
        }
        let path = self.path(name);
        if !std::fs::symlink_metadata(&path).ok()?.file_type().is_file() {
            return None;
        }
        self.read(name, &path, today())
    }

    fn read(&self, name: &str, path: &Path, today: chrono::NaiveDate) -> Option<Memory> {
        let raw = std::fs::read_to_string(path).ok()?;
        let mut m = file::parse(name, &raw, today)?;
        m.bytes = raw.len() as u64;
        m.modified = std::fs::metadata(path).ok().and_then(|md| md.modified().ok()).map(DateTime::<Utc>::from);
        Some(m)
    }

    /// Check a save against the store, without writing: what the card of `memory_save`
    /// shows. A save of the agent's may take the name of a memory that exists.
    pub fn plan(&self, new: &super::NewMemory, replaces: Option<&str>) -> Result<Planned, MemoryError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        plan_new(&self.load(), new, replaces, SaveMode::Overwrite, today())
    }

    /// Write a memory. With `replaces`, that memory gets `validUntil: today` and
    /// `supersededBy`, and is returned. The index is rewritten.
    pub fn save(&self, new: &super::NewMemory, replaces: Option<&str>, mode: SaveMode) -> Result<(Memory, Option<Memory>), MemoryError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let today = today();
        let mut all = self.files()?;
        let plan = plan_new(&all, new, replaces, mode, today)?;
        let created = all.iter().find(|m| m.name == plan.memory.name).map_or(today, |m| m.created);
        let memory = self.write(&mut all, Memory { created, ..plan.memory })?;
        let replaced = match plan.old {
            Some(old) => {
                let old = Memory { valid_until: Some(today), superseded_by: Some(memory.name.clone()), updated: today, ..old };
                Some(self.write(&mut all, old)?)
            }
            None => None,
        };
        self.index(&all)?;
        Ok((memory, replaced))
    }

    /// Change one memory; `edit` gets its current state. What it leaves is checked like a
    /// new memory. `updated` becomes today.
    pub fn update(&self, name: &str, mut edit: impl FnMut(&mut Memory) -> Result<(), MemoryError>) -> Result<Memory, MemoryError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.files()?;
        let mut memory = all.iter().find(|m| m.name == name).cloned().ok_or_else(|| MemoryError::Unknown(unknown(name)))?;
        edit(&mut memory)?;
        check_stored(&memory)?;
        memory.updated = today();
        let memory = self.write(&mut all, memory)?;
        self.index(&all)?;
        Ok(memory)
    }

    /// Mark a topic memory `validUntil: today`: it stays on disk and is no longer
    /// recalled. One that is out of recall already is left as it is.
    pub fn forget(&self, name: &str) -> Result<Memory, MemoryError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.files()?;
        let mut memory = all.iter().find(|m| m.name == name).cloned().ok_or_else(|| MemoryError::Unknown(unknown(name)))?;
        if memory.is_core() {
            return Err(MemoryError::Core(format!("the core files '{USER}' and '{MACHINE}' are not forgotten: edit them in Settings -> Memory")));
        }
        if memory.valid_until.is_none() {
            memory.valid_until = Some(today());
            memory = self.write(&mut all, memory)?;
            self.index(&all)?;
        }
        Ok(memory)
    }

    /// Delete the file of a topic memory.
    pub fn remove(&self, name: &str) -> Result<(), MemoryError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if is_core(name) {
            return Err(MemoryError::Core(format!("'{name}' is a core file and is not deleted: clear its text in Settings -> Memory instead")));
        }
        let path = self.path(name);
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            return Err(MemoryError::Unknown(unknown(name)));
        }
        std::fs::remove_file(&path).map_err(|e| MemoryError::Io(format!("cannot remove {}: {e}", path.display())))?;
        self.index(&self.files()?)?;
        Ok(())
    }

    /// `MEMORY.md`; a failed write names the file.
    fn index(&self, all: &[Memory]) -> Result<(), MemoryError> {
        let path = self.dir.join(INDEX);
        self.write_index(all).map_err(|e| MemoryError::Io(format!("cannot write {}: {e}", path.display())))
    }

    /// Write the file of `m` and keep `all` in step with it.
    fn write(&self, all: &mut Vec<Memory>, mut m: Memory) -> Result<Memory, MemoryError> {
        m.description = file::one_line(&m.description);
        let text = file::render(&m);
        let path = self.path(&m.name);
        file::write_private(&path, &text).map_err(|e| MemoryError::Io(format!("cannot write {}: {e}", path.display())))?;
        m.bytes = text.len() as u64;
        m.modified = Some(Utc::now());
        match all.iter_mut().find(|x| x.name == m.name) {
            Some(slot) => *slot = m.clone(),
            None => all.push(m.clone()),
        }
        Ok(m)
    }

    /// `MEMORY.md` (memory.md section 1): one line per current memory, rewritten on
    /// every change. Editing it has no effect.
    pub fn write_index(&self, all: &[Memory]) -> std::io::Result<()> {
        let today = today();
        let mut out = String::from("# Memory\n\nGenerated by KAgent from the files in this folder. Editing it has no effect: it is rewritten on every change.\n\n");
        let mut current: Vec<&Memory> = all.iter().filter(|m| !m.superseded(today)).collect();
        current.sort_by(|a, b| a.name.cmp(&b.name));
        for m in current {
            out.push_str(&format!("- [{}]({}.md) - {}\n", m.name, m.name, file::one_line(&m.description)));
        }
        file::write_private(&self.dir.join(INDEX), &out)
    }
}

fn unknown(name: &str) -> String {
    format!("there is no memory named '{name}'")
}

/// Check a new memory against what the store holds.
fn plan_new(all: &[Memory], new: &super::NewMemory, replaces: Option<&str>, mode: SaveMode, today: chrono::NaiveDate) -> Result<Planned, MemoryError> {
    use MemoryError as E;
    let name = new.name.trim();
    if !frontmatter::valid_name(name) {
        return Err(E::Invalid(format!("the name '{}' is not valid: lowercase letters, digits and hyphens, a letter or a digit first, at most 64 characters", new.name.trim())));
    }
    if is_core(name) {
        return Err(E::Core(format!("'{name}' is a core file: change its text in Settings -> Memory, or choose another name")));
    }
    if name == INDEX_NAME {
        return Err(E::Invalid(format!("'{INDEX_NAME}' would overwrite the index {INDEX}: choose another name")));
    }
    let body = new.body.trim();
    if body.is_empty() {
        return Err(E::Invalid("the content is empty: write the fact to remember".into()));
    }
    let description = file::one_line(&new.description);
    check_text(name, &description, body)?;
    let existing = all.iter().find(|m| m.name == name);
    if mode == SaveMode::Create && existing.is_some() {
        return Err(E::Exists(format!("a memory named '{name}' exists already; change it instead")));
    }
    let old = match replaces.map(str::trim).filter(|r| !r.is_empty()) {
        Some(old) if old == name => return Err(E::Invalid("a memory cannot replace itself: save it under its own name, without replaces".into())),
        Some(old) if is_core(old) => return Err(E::Core(format!("'{old}' is a core file and is not replaced: change its text in Settings -> Memory"))),
        Some(old) => Some(all.iter().find(|m| m.name == old).cloned().ok_or_else(|| E::Unknown(format!("there is no memory named '{old}' to replace")))?),
        None => None,
    };
    if existing.is_none() && all.len() >= MAX_FILES {
        return Err(E::Full);
    }
    let description = if description.is_empty() { file::first_line(body, super::LINE_DESCRIPTION_CHARS) } else { description };
    Ok(Planned {
        memory: Memory {
            name: name.to_string(),
            description,
            body: body.to_string(),
            r#type: new.r#type,
            created: today,
            updated: today,
            source: new.source,
            reviewed: new.source == MemorySource::User,
            valid_until: None,
            superseded_by: None,
            modified: None,
            bytes: 0,
        },
        existed: existing.is_some(),
        old,
    })
}

/// What a stored memory must keep to: not empty (a core file may be cleared), within
/// the limits, and no secret (memory.md section 4).
fn check_stored(m: &Memory) -> Result<(), MemoryError> {
    if !m.is_core() && m.body.trim().is_empty() {
        return Err(MemoryError::Invalid("the content is empty: write the fact to remember".into()));
    }
    check_text(&m.name, &m.description, &m.body.trim())
}

fn check_text(name: &str, description: &str, body: &str) -> Result<(), MemoryError> {
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(MemoryError::Invalid(format!("the content has {} characters; at most {MAX_BODY_CHARS} are kept", body.chars().count())));
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(MemoryError::Invalid(format!("the description has {} characters; at most {MAX_DESCRIPTION_CHARS} are kept", description.chars().count())));
    }
    if let Some(looks) = guard::find_secret(&format!("{description}\n{body}")) {
        return Err(MemoryError::Invalid(format!("memory '{name}': {REFUSED} (it looks like {looks})")));
    }
    Ok(())
}
