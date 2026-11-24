//! Code navigation (docs/specs/tools.md section 14): `projects` (known project folders)
//! and `open_in_editor` (open a project, a file, or the first match of a search, at a
//! line, in the preferred editor: VS Code, Kate, JetBrains, Zed, ...).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::xdg;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

/// Editors in auto-detection order: (id, spoken names, desktop ids, binaries).
const EDITORS: &[(&str, &[&str], &[&str], &[&str])] = &[
    ("vscode", &["vscode", "code", "vs code", "visual studio code"], &["code_code", "code", "visual-studio-code"], &["code", "codium"]),
    ("idea", &["idea", "intellij"], &["jetbrains-idea", "jetbrains-idea-ce"], &["idea", "idea.sh"]),
    ("pycharm", &["pycharm"], &["jetbrains-pycharm", "jetbrains-pycharm-ce"], &["pycharm", "pycharm.sh"]),
    ("webstorm", &["webstorm"], &["jetbrains-webstorm"], &["webstorm", "webstorm.sh"]),
    ("phpstorm", &["phpstorm"], &["jetbrains-phpstorm"], &["phpstorm", "phpstorm.sh"]),
    ("clion", &["clion"], &["jetbrains-clion"], &["clion", "clion.sh"]),
    ("studio", &["android studio", "studio"], &["jetbrains-studio"], &["studio", "studio.sh"]),
    ("zed", &["zed"], &["dev.zed.Zed"], &["zed", "zeditor"]),
    ("kate", &["kate"], &["org.kde.kate"], &["kate"]),
    ("kwrite", &["kwrite"], &["org.kde.kwrite"], &["kwrite"]),
    ("gedit", &["gedit"], &["org.gnome.gedit"], &["gedit"]),
    ("gnome-text-editor", &["gnome text editor", "text editor"], &["org.gnome.TextEditor"], &["gnome-text-editor"]),
];

fn is_jetbrains(id: &str) -> bool {
    matches!(id, "idea" | "pycharm" | "webstorm" | "phpstorm" | "clion" | "studio")
}

/// Editor id for a spoken name (`"VS Code"` -> `vscode`).
pub fn editor_id(name: &str) -> Option<&'static str> {
    let n = name.trim().to_lowercase();
    EDITORS.iter().find(|e| e.0 == n || e.1.contains(&n.as_str())).map(|e| e.0)
}

/// Program to run for an editor: a binary on PATH, else the desktop entry's Exec.
fn resolve_program(id: &str) -> Option<String> {
    let e = EDITORS.iter().find(|e| e.0 == id)?;
    if let Some(p) = e.3.iter().find_map(|b| xdg::which(b)) {
        return Some(p.display().to_string());
    }
    let apps = xdg::all_apps();
    e.2.iter().find_map(|d| apps.iter().find(|a| a.id == *d)).map(|a| a.program()).filter(|p| !p.is_empty() && xdg::which(p).is_some())
}

/// argv to open `file` (or the project when `file` is None) at line/column.
pub fn editor_argv(id: &str, program: &str, project: Option<&Path>, file: Option<&Path>, line: u32, col: u32) -> Vec<String> {
    let mut v = vec![program.to_string()];
    let f = file.map(|f| f.display().to_string());
    match id {
        "vscode" => {
            if let Some(p) = project {
                v.push(p.display().to_string());
            }
            if let Some(f) = f {
                if line > 0 {
                    v.push("-g".into());
                    v.push(format!("{f}:{line}:{}", col.max(1)));
                } else {
                    v.push(f);
                }
            }
        }
        "kate" | "kwrite" => {
            if line > 0 {
                v.extend(["-l".into(), line.to_string(), "-c".into(), col.saturating_sub(1).to_string()]);
            }
            v.push(f.unwrap_or_else(|| project.map(|p| p.display().to_string()).unwrap_or_default()));
        }
        "gedit" | "gnome-text-editor" => {
            // Both take `+LINE[:COL] file`.
            if line > 0 && f.is_some() {
                v.push(if col > 0 { format!("+{line}:{col}") } else { format!("+{line}") });
            }
            v.push(f.unwrap_or_else(|| project.map(|p| p.display().to_string()).unwrap_or_default()));
        }
        "zed" => match f {
            Some(f) if line > 0 => v.push(format!("{f}:{line}:{}", col.max(1))),
            Some(f) => v.push(f),
            None => v.push(project.map(|p| p.display().to_string()).unwrap_or_default()),
        },
        _ if is_jetbrains(id) => {
            if let Some(p) = project {
                v.push(p.display().to_string());
            }
            if let Some(f) = f {
                if line > 0 {
                    v.extend(["--line".into(), line.to_string()]);
                    if col > 0 {
                        v.extend(["--column".into(), col.to_string()]);
                    }
                }
                v.push(f);
            }
        }
        _ => v.extend(f),
    }
    v
}

/// Editor to use: explicit, then the project's own (.idea/.vscode), then the preferred
/// one from settings, then the first installed.
fn choose_editor(explicit: &str, pref: &str, project: Option<&Path>) -> Option<(&'static str, String)> {
    let mut order: Vec<&'static str> = Vec::new();
    order.extend(editor_id(explicit));
    if let Some(p) = project {
        if p.join(".idea").is_dir() {
            order.push("idea");
        }
        if p.join(".vscode").is_dir() {
            order.push("vscode");
        }
    }
    order.extend(editor_id(pref));
    order.extend(EDITORS.iter().map(|e| e.0));
    if !explicit.trim().is_empty() {
        // An explicitly named editor must not silently fall back to another one.
        order.truncate(1);
    }
    order.into_iter().find_map(|id| resolve_program(id).map(|p| (id, p)))
}

const SKIP_DIRS: [&str; 12] = [".git", "node_modules", "target", "dist", "build", ".venv", "venv", "__pycache__", ".next", ".idea", ".gradle", "vendor"];

/// Files under `root` (depth-limited, skipping build/vendor dirs), up to `max`.
pub fn walk(root: &Path, max: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < 10 && !SKIP_DIRS.contains(&name.as_str()) && !(name.starts_with('.') && name != ".github") {
                    stack.push((e.path(), depth + 1));
                }
            } else if ft.is_file() {
                out.push(e.path());
                if out.len() >= max {
                    return out;
                }
            }
        }
    }
    out
}

/// Resolve a file name or relative path inside `root`: exact relative path, else the
/// shortest path whose suffix matches. Returns all candidates (best first).
pub fn find_file(root: &Path, name: &str) -> Vec<PathBuf> {
    let direct = root.join(name);
    if direct.is_file() {
        return vec![direct];
    }
    let want = name.trim_start_matches("./").to_lowercase();
    let mut hits: Vec<PathBuf> = walk(root, 50_000)
        .into_iter()
        .filter(|p| {
            let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().to_lowercase();
            rel == want || rel.ends_with(&format!("/{want}")) || (!want.contains('/') && p.file_name().is_some_and(|f| f.to_string_lossy().to_lowercase() == want))
        })
        .collect();
    if hits.is_empty() && !want.contains('/') {
        // Fuzzy: file stem contains the query ("live client" -> live_client.ts).
        let q: String = want.chars().filter(|c| c.is_alphanumeric()).collect();
        hits = walk(root, 50_000)
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|f| f.to_string_lossy().to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>().contains(&q)))
            .collect();
    }
    hits.sort_by_key(|p| (p.components().count(), p.as_os_str().len()));
    hits
}

/// First lines matching `pattern` (regex, case-insensitive) in files: (file, line no, text).
pub fn search(files: &[PathBuf], pattern: &str, max: usize) -> anyhow::Result<Vec<(PathBuf, u32, String)>> {
    let re = regex::RegexBuilder::new(pattern).case_insensitive(true).size_limit(1 << 20).build()?;
    // Code before docs and notes, shallow before deep: "where is X used" means code.
    let is_doc = |p: &PathBuf| matches!(p.extension().and_then(|e| e.to_str()), Some("md" | "txt" | "rst" | "adoc" | "log" | "json" | "lock" | "csv"));
    let mut files: Vec<&PathBuf> = files.iter().collect();
    files.sort_by_key(|p| (is_doc(p), p.components().count(), p.as_os_str().len()));
    let mut out = Vec::new();
    for f in files {
        if std::fs::metadata(f).map(|m| m.len() > 1_000_000).unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else { continue }; // binary
        for (i, l) in text.lines().enumerate() {
            if re.is_match(l) {
                out.push((f.clone(), i as u32 + 1, l.trim().chars().take(160).collect()));
                if out.len() >= max {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

fn snippet(file: &Path, line: u32) -> String {
    let Ok(t) = std::fs::read_to_string(file) else { return String::new() };
    let start = line.saturating_sub(4) as usize;
    t.lines().enumerate().skip(start).take(7).map(|(i, l)| format!("{:>5}{} {}", i + 1, if i as u32 + 1 == line { ">" } else { "|" }, l)).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------- projects

pub struct Projects {
    contract: ToolContract,
}

impl Projects {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "projects",
                "List the user's code projects (folders under the configured project roots), find one by name, or search its code without opening anything (project + search: regex, returns file:line matches).",
                json!({"type":"object","properties":{
                    "project":{"type":"string","description":"optional: project name to look up"},
                    "search":{"type":"string","description":"optional regex to find in the project's files (needs project)"}
                }}),
            ),
        }
    }
}

#[async_trait]
impl Tool for Projects {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let all = xdg::projects(&ctx.desktop_prefs().project_roots);
        let name = input.get("project").or_else(|| input.get("name")).and_then(Value::as_str).unwrap_or("");
        let pattern = input.get("search").and_then(Value::as_str).unwrap_or("").trim();
        if !name.is_empty() {
            return Ok(match xdg::find_project(&all, name) {
                Some(p) if !pattern.is_empty() => {
                    let hits = search(&walk(&p, 20_000), pattern, 30).map_err(|e| anyhow::anyhow!("bad search regex: {e}"))?;
                    if hits.is_empty() {
                        ToolOutput::ok(format!("no matches for /{pattern}/ in {}", p.display()))
                    } else {
                        ToolOutput::ok(format!("{}:\n{}", p.display(), hits.iter().map(|(f, l, t)| format!("{}:{l}: {t}", f.strip_prefix(&p).unwrap_or(f).display())).collect::<Vec<_>>().join("\n")))
                    }
                }
                Some(p) => ToolOutput::ok(p.display().to_string()),
                None => ToolOutput::error(format!("no project named '{name}'; known: {}", all.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy()).collect::<Vec<_>>().join(", "))),
            });
        }
        Ok(ToolOutput::ok(all.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")))
    }
}

// ---------------------------------------------------------------- open_in_editor

pub struct OpenInEditor {
    contract: ToolContract,
}

impl OpenInEditor {
    pub fn new() -> Self {
        let mut contract = ToolContract::read_only(
            "open_in_editor",
            "Opens a window: show code to the user in their editor (only when they want to see it; to just find code use `projects` with search) (VS Code, Kate, IntelliJ/PyCharm/WebStorm, Zed). Give `project` (name, e.g. ReelGen) and/or `path` (absolute, or relative/file name inside the project), optional `line`/`column`, or `search` (regex) to jump to the first matching line. Returns the opened location and the surrounding lines.",
            json!({"type":"object","properties":{
                "project":{"type":"string"},
                "path":{"type":"string"},
                "line":{"type":"integer","minimum":1},
                "column":{"type":"integer","minimum":1},
                "search":{"type":"string","description":"regex; opens the first match (in `path` if given, else in the whole project)"},
                "editor":{"type":"string","description":"vscode, kate, idea, pycharm, webstorm, phpstorm, clion, zed; default: settings or the project's own"}
            }}),
        )
        .mutating(SideEffect::System, Risk::Low);
        contract.needs_approval = false; // only opens a window
        Self { contract }
    }
}

#[async_trait]
impl Tool for OpenInEditor {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let g = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let what = [g("project"), g("path"), g("search")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        CallAssessment::from_contract(&self.contract, format!("open in editor: {what}"), "open_in_editor".into())
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let g = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let prefs = ctx.desktop_prefs();
        let (project_name, path, pattern) = (g("project"), g("path"), g("search"));
        let mut line = input.get("line").and_then(Value::as_u64).unwrap_or(0) as u32;
        let col = input.get("column").and_then(Value::as_u64).unwrap_or(0) as u32;

        let mut project = if project_name.is_empty() {
            None
        } else {
            let all = xdg::projects(&prefs.project_roots);
            match xdg::find_project(&all, &project_name) {
                Some(p) => Some(p),
                None => return Ok(ToolOutput::error(format!("no project named '{project_name}' (see `projects`; roots are set in Settings -> Desktop)"))),
            }
        };
        let mut notes = Vec::new();
        let mut file: Option<PathBuf> = None;
        if !path.is_empty() {
            let p = xdg::expand(&path);
            if p.is_absolute() {
                if !p.exists() {
                    return Ok(ToolOutput::error(format!("{} does not exist", p.display())));
                }
                if p.is_dir() {
                    project = Some(p);
                } else {
                    file = Some(p);
                }
            } else if let Some(root) = &project {
                let hits = find_file(root, &path);
                if hits.is_empty() {
                    return Ok(ToolOutput::error(format!("no file matching '{path}' in {}", root.display())));
                }
                if hits.len() > 1 {
                    notes.push(format!("{} matches, opened the closest; others: {}", hits.len(), hits.iter().skip(1).take(5).map(|h| h.strip_prefix(root).unwrap_or(h).display().to_string()).collect::<Vec<_>>().join(", ")));
                }
                file = hits.into_iter().next();
            } else {
                return Ok(ToolOutput::error("relative path needs `project` (or give an absolute path)"));
            }
        }
        // A file outside a named project still opens its project window when it has one.
        if project.is_none() {
            if let Some(f) = &file {
                project = f.ancestors().skip(1).find(|a| a.join(".git").exists()).map(Path::to_path_buf);
            }
        }
        if !pattern.is_empty() {
            let files = match (&file, &project) {
                (Some(f), _) => vec![f.clone()],
                (None, Some(root)) => walk(root, 20_000),
                (None, None) => return Ok(ToolOutput::error("search needs `project` or `path`")),
            };
            let hits = search(&files, &pattern, 20).map_err(|e| anyhow::anyhow!("bad search regex: {e}"))?;
            let Some((f, l, _)) = hits.first().cloned() else {
                return Ok(ToolOutput::error(format!("no line matches /{pattern}/")));
            };
            if hits.len() > 1 {
                let base = project.clone().unwrap_or_default();
                notes.push(format!(
                    "other matches:\n{}",
                    hits.iter().skip(1).take(8).map(|(f, l, t)| format!("  {}:{l}: {t}", f.strip_prefix(&base).unwrap_or(f).display())).collect::<Vec<_>>().join("\n")
                ));
            }
            file = Some(f);
            line = l;
        }
        if file.is_none() && project.is_none() {
            return Ok(ToolOutput::error("give `project`, `path` or `search`"));
        }
        let Some((id, program)) = choose_editor(&g("editor"), &prefs.editor, project.as_deref()) else {
            let ed = g("editor");
            return Ok(ToolOutput::error(if ed.is_empty() { "no supported editor found (VS Code, Kate, JetBrains, Zed)".to_string() } else { format!("editor '{ed}' is not installed") }));
        };
        let argv = editor_argv(id, &program, project.as_deref(), file.as_deref(), line, col);
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        if let Err(e) = cmd.spawn() {
            return Ok(ToolOutput::error(format!("could not start {id}: {e}")));
        }
        let target = match &file {
            Some(f) if line > 0 => format!("{}:{line}", f.display()),
            Some(f) => f.display().to_string(),
            None => project.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        };
        let mut out = format!("opened {target} in {id}");
        if let (Some(f), true) = (&file, line > 0) {
            out.push_str(&format!("\n{}", snippet(f, line)));
        }
        for n in notes {
            out.push_str(&format!("\n{n}"));
        }
        Ok(ToolOutput::ok(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_per_editor() {
        let p = Path::new("/h/ReelGen");
        let f = Path::new("/h/ReelGen/src/live.ts");
        assert_eq!(editor_argv("vscode", "code", Some(p), Some(f), 42, 0), vec!["code", "/h/ReelGen", "-g", "/h/ReelGen/src/live.ts:42:1"]);
        assert_eq!(editor_argv("kate", "kate", Some(p), Some(f), 42, 5), vec!["kate", "-l", "42", "-c", "4", "/h/ReelGen/src/live.ts"]);
        assert_eq!(editor_argv("idea", "/x/idea.sh", Some(p), Some(f), 7, 0), vec!["/x/idea.sh", "/h/ReelGen", "--line", "7", "/h/ReelGen/src/live.ts"]);
        assert_eq!(editor_argv("zed", "zed", None, Some(f), 3, 2), vec!["zed", "/h/ReelGen/src/live.ts:3:2"]);
        assert_eq!(editor_argv("vscode", "code", Some(p), None, 0, 0), vec!["code", "/h/ReelGen"]);
        assert_eq!(editor_id("VS Code"), Some("vscode"));
        assert_eq!(editor_argv("gnome-text-editor", "gnome-text-editor", Some(p), Some(f), 9, 0), vec!["gnome-text-editor", "+9", "/h/ReelGen/src/live.ts"]);
        assert_eq!(editor_id("Text Editor"), Some("gnome-text-editor"));
        assert_eq!(editor_id("IntelliJ"), Some("idea"));
    }

    #[test]
    fn find_and_search_in_project() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src/voice")).unwrap();
        std::fs::create_dir_all(r.join("node_modules/x")).unwrap();
        std::fs::write(r.join("src/voice/live_client.ts"), "import x\nconst ws = new WebSocket(url)\nws.onclose = reconnect\n").unwrap();
        std::fs::write(r.join("node_modules/x/live_client.ts"), "new WebSocket(a)\n").unwrap();
        std::fs::write(r.join("README.md"), "hello\nWe use new WebSocket here\n").unwrap();
        assert_eq!(find_file(r, "live_client.ts"), vec![r.join("src/voice/live_client.ts")]);
        assert_eq!(find_file(r, "voice/live_client.ts"), vec![r.join("src/voice/live_client.ts")]);
        assert_eq!(find_file(r, "live client"), vec![r.join("src/voice/live_client.ts")]);
        let hits = search(&walk(r, 100), r"new websocket", 10).unwrap();
        assert_eq!(hits.len(), 2, "node_modules skipped");
        assert_eq!((hits[0].0.clone(), hits[0].1), (r.join("src/voice/live_client.ts"), 2), "code ranks before docs");
        assert!(snippet(&hits[0].0, 2).contains("    2> const ws"));
    }
}
