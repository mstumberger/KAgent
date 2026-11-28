//! The changes to tickets and what they write into the history (tickets.md sections 2 and 4).

use chrono::Utc;
use serde::Deserialize;

use super::store::{project_id, project_name, ProjectFile};
use super::*;

/// What a new ticket is made of; a field left out takes its default.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Create {
    pub project: Option<String>,
    pub title: String,
    pub description: String,
    pub state: Option<State>,
    pub priority: Option<Priority>,
    pub labels: Vec<String>,
    pub parent: Option<String>,
    pub links: Vec<Link>,
    pub assignee: Option<String>,
    pub done_when: Vec<String>,
}

/// What a change may touch; a field left out is kept. `parent` and `assignee` take `null` to clear.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Update {
    /// Not changeable: a value here is refused with a clear message.
    pub project: Option<serde_json::Value>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub state: Option<State>,
    pub priority: Option<Priority>,
    pub labels: Option<Vec<String>>,
    #[serde(deserialize_with = "nullable")]
    pub parent: Option<Option<String>>,
    pub links: Option<Vec<Link>>,
    #[serde(deserialize_with = "nullable")]
    pub assignee: Option<Option<String>>,
    pub done_when: Option<Vec<String>>,
    /// Appended as a note.
    pub note: Option<String>,
}

/// A selection for `GET /api/tickets`; every part is optional.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// A folder path, or `inbox`.
    pub project: Option<String>,
    pub state: Option<State>,
    pub label: Option<String>,
    pub text: Option<String>,
}

pub struct Listing {
    pub projects: Vec<ProjectInfo>,
    pub tickets: Vec<Ticket>,
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn clean_title(t: &str) -> Result<String, TicketError> {
    let t = t.trim();
    if t.is_empty() || chars(t) > MAX_TITLE {
        return Err(invalid(format!("title: 1 to {MAX_TITLE} characters (it has {})", chars(t))));
    }
    if t.contains('\n') {
        return Err(invalid("title: one line"));
    }
    Ok(t.to_string())
}

fn clean_description(d: &str) -> Result<String, TicketError> {
    if chars(d) > MAX_DESCRIPTION {
        return Err(invalid(format!("description: at most {MAX_DESCRIPTION} characters (it has {}); keep the details in a linked file or in notes", chars(d))));
    }
    Ok(d.to_string())
}

fn clean_labels(labels: &[String]) -> Result<Vec<String>, TicketError> {
    let mut out: Vec<String> = Vec::new();
    for l in labels {
        let l = l.trim();
        if l.is_empty() || chars(l) > MAX_LABEL || l.contains(',') {
            return Err(invalid(format!("labels: 1 to {MAX_LABEL} characters each, no comma (got {l:?})")));
        }
        if !out.iter().any(|x| x == l) {
            out.push(l.to_string());
        }
    }
    if out.len() > MAX_LABELS {
        return Err(invalid(format!("labels: at most {MAX_LABELS} (got {})", out.len())));
    }
    Ok(out)
}

fn clean_done_when(lines: &[String]) -> Result<Vec<String>, TicketError> {
    let out: Vec<String> = lines.iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if out.len() > MAX_DONE_WHEN {
        return Err(invalid(format!("doneWhen: at most {MAX_DONE_WHEN} lines (got {})", out.len())));
    }
    if let Some(l) = out.iter().find(|l| chars(l) > MAX_DONE_WHEN_LINE) {
        return Err(invalid(format!("doneWhen: a line is at most {MAX_DONE_WHEN_LINE} characters ({:?}...)", head(l))));
    }
    Ok(out)
}

fn head(s: &str) -> String {
    s.chars().take(30).collect()
}

fn clean_links(links: &[Link]) -> Result<Vec<Link>, TicketError> {
    if links.len() > MAX_LINKS {
        return Err(invalid(format!("links: at most {MAX_LINKS} (got {})", links.len())));
    }
    let mut out = Vec::new();
    for l in links {
        let value = l.value.trim().to_string();
        if value.is_empty() || chars(&value) > MAX_LINK_VALUE {
            return Err(invalid(format!("links: a value is 1 to {MAX_LINK_VALUE} characters")));
        }
        let ok = match l.kind {
            LinkKind::Conversation => value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            LinkKind::Commit => (7..=40).contains(&value.len()) && value.chars().all(|c| c.is_ascii_hexdigit()),
            LinkKind::Range => crate::gitref::check_range(&value).is_ok(),
            LinkKind::Url => value.starts_with("http://") || value.starts_with("https://"),
        };
        if !ok {
            let want = match l.kind {
                LinkKind::Conversation => "a session id".to_string(),
                LinkKind::Commit => "a commit id of 7 to 40 hex characters".into(),
                LinkKind::Range => crate::gitref::check_range(&value).err().map_or("a git range".into(), |e| format!("a git range ({e})")),
                LinkKind::Url => "an http or https URL".into(),
            };
            return Err(invalid(format!("links: {value:?} is not {want}")));
        }
        let link = Link { kind: l.kind, value, label: l.label.trim().chars().take(100).collect() };
        if !out.contains(&link) {
            out.push(link);
        }
    }
    Ok(out)
}

fn clean_assignee(a: Option<&str>) -> Result<Option<String>, TicketError> {
    match a.map(str::trim).filter(|a| !a.is_empty()) {
        None => Ok(None),
        Some("owner") => Ok(Some("owner".into())),
        Some(other) => Err(invalid(format!("assignee: the owner or nobody (got {other:?}); a worker comes with the workflow runs"))),
    }
}

/// A parent must be a ticket of the same project and must not make a cycle.
fn check_parent(file: &ProjectFile, own_key: Option<&str>, parent: &str) -> Result<(), TicketError> {
    if own_key == Some(parent) {
        return Err(invalid("parent: a ticket cannot be its own parent"));
    }
    if !file.tickets.iter().any(|t| t.key == parent) {
        return Err(invalid(format!("parent: no ticket {parent} in this project")));
    }
    let mut at = Some(parent.to_string());
    let mut steps = 0;
    while let Some(k) = at {
        if own_key == Some(k.as_str()) {
            return Err(invalid(format!("parent: {parent} is below this ticket: that would make a cycle")));
        }
        at = file.tickets.iter().find(|t| t.key == k).and_then(|t| t.parent.clone());
        steps += 1;
        if steps > MAX_TICKETS {
            break;
        }
    }
    Ok(())
}

fn entry(by: By, what: impl Into<String>) -> Entry {
    Entry { at: Utc::now(), by, what: what.into() }
}

fn cap_history(t: &mut Ticket) {
    if t.history.len() > MAX_HISTORY {
        let extra = t.history.len() - MAX_HISTORY;
        t.history.drain(..extra);
    }
}

fn number_of(key: &str) -> Option<(&str, u64)> {
    let (prefix, n) = key.rsplit_once('-')?;
    Some((prefix, n.parse().ok()?))
}

impl Tickets {
    /// A new ticket. `project` is the folder, `None` for the Inbox.
    pub fn create(&self, input: Create, by: By) -> Result<Ticket, TicketError> {
        let _guard = self.lock.lock().unwrap();
        let project = input.project.clone();
        let title = clean_title(&input.title)?;
        let description = clean_description(&input.description)?;
        let labels = clean_labels(&input.labels)?;
        let done_when = clean_done_when(&input.done_when)?;
        let links = clean_links(&input.links)?;
        let assignee = clean_assignee(input.assignee.as_deref())?;
        let mut file = match self.load(project.as_deref())? {
            Some(f) => f,
            None => self.fresh(project.as_deref()),
        };
        if file.tickets.len() >= MAX_TICKETS {
            return Err(TicketError::Limit(format!(
                "{} already has {MAX_TICKETS} tickets, the most a project holds: delete done or dropped ones, or use another project",
                project_name(project.as_deref())
            )));
        }
        if let Some(parent) = input.parent.as_deref().filter(|p| !p.is_empty()) {
            check_parent(&file, None, parent)?;
        }
        let key = format!("{}-{}", file.prefix, file.next);
        file.next += 1;
        let now = Utc::now();
        let ticket = Ticket {
            key: key.clone(),
            project: project.clone(),
            title,
            description,
            state: input.state.unwrap_or(State::Todo),
            priority: input.priority.unwrap_or_default(),
            labels,
            parent: input.parent.filter(|p| !p.is_empty()),
            links,
            assignee,
            done_when,
            notes: Vec::new(),
            history: vec![entry(by, "created")],
            created_at: now,
            updated_at: now,
        };
        file.tickets.push(ticket.clone());
        self.save(&file)?;
        self.announce(&key, project.as_deref(), false);
        Ok(ticket)
    }

    /// Find the file that holds `key`.
    fn locate(&self, key: &str) -> Result<ProjectFile, TicketError> {
        let not_found = || TicketError::NotFound(format!("no ticket {key}"));
        let (prefix, _) = number_of(key).ok_or_else(not_found)?;
        let (files, _) = self.load_all();
        let file = files.into_iter().find(|f| f.prefix == prefix && f.tickets.iter().any(|t| t.key == key)).ok_or_else(not_found)?;
        Ok(file)
    }

    pub fn get(&self, key: &str) -> Result<Ticket, TicketError> {
        let file = self.locate(key)?;
        Ok(file.tickets.into_iter().find(|t| t.key == key).expect("located"))
    }

    /// Change a ticket. The answer is the ticket and the history lines this change added.
    pub fn update(&self, key: &str, input: Update, by: By) -> Result<(Ticket, Vec<String>), TicketError> {
        let _guard = self.lock.lock().unwrap();
        if input.project.is_some() {
            return Err(invalid("project cannot be changed: make a new ticket in the other project"));
        }
        let mut file = self.locate(key)?;
        let idx = file.tickets.iter().position(|t| t.key == key).expect("located");
        let mut t = file.tickets[idx].clone();
        let mut said: Vec<String> = Vec::new();
        if let Some(title) = &input.title {
            let title = clean_title(title)?;
            if title != t.title {
                said.push("title".into());
                t.title = title;
            }
        }
        if let Some(d) = &input.description {
            let d = clean_description(d)?;
            if d != t.description {
                said.push("description".into());
                t.description = d;
            }
        }
        if let Some(s) = input.state {
            if s != t.state {
                said.push(format!("state: {} -> {}", t.state.word(), s.word()));
                t.state = s;
            }
        }
        if let Some(p) = input.priority {
            if p != t.priority {
                said.push(format!("priority: {} -> {}", t.priority.word(), p.word()));
                t.priority = p;
            }
        }
        if let Some(l) = &input.labels {
            let l = clean_labels(l)?;
            if l != t.labels {
                said.push(format!("labels: {}", if l.is_empty() { "none".into() } else { l.join(", ") }));
                t.labels = l;
            }
        }
        if let Some(d) = &input.done_when {
            let d = clean_done_when(d)?;
            if d != t.done_when {
                said.push("doneWhen".into());
                t.done_when = d;
            }
        }
        if let Some(l) = &input.links {
            let l = clean_links(l)?;
            if l != t.links {
                said.push("links".into());
                t.links = l;
            }
        }
        if let Some(p) = &input.parent {
            let p = p.clone().filter(|p| !p.is_empty());
            if let Some(parent) = &p {
                check_parent(&file, Some(key), parent)?;
            }
            if p != t.parent {
                said.push(format!("parent: {} -> {}", t.parent.as_deref().unwrap_or("none"), p.as_deref().unwrap_or("none")));
                t.parent = p;
            }
        }
        if let Some(a) = &input.assignee {
            let a = clean_assignee(a.as_deref())?;
            if a != t.assignee {
                said.push(format!("assignee: {} -> {}", t.assignee.as_deref().unwrap_or("nobody"), a.as_deref().unwrap_or("nobody")));
                t.assignee = a;
            }
        }
        if let Some(n) = input.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            if chars(n) > MAX_NOTE {
                return Err(invalid(format!("note: at most {MAX_NOTE} characters (it has {})", chars(n))));
            }
            if t.notes.len() >= MAX_NOTES {
                return Err(TicketError::Limit(format!("{key} has {MAX_NOTES} notes, the most one ticket holds")));
            }
            t.notes.push(Note { at: Utc::now(), by, text: n.to_string() });
            said.push("note added".into());
        }
        if said.is_empty() {
            return Ok((t, said));
        }
        t.updated_at = Utc::now();
        t.history.extend(said.iter().map(|w| entry(by, w.clone())));
        cap_history(&mut t);
        file.tickets[idx] = t.clone();
        self.save(&file)?;
        self.announce(key, t.project.as_deref(), false);
        Ok((t, said))
    }

    /// Delete a ticket; its children lose their parent. The key is not reused (the counter only grows).
    pub fn delete(&self, key: &str) -> Result<(), TicketError> {
        let _guard = self.lock.lock().unwrap();
        let mut file = self.locate(key)?;
        file.tickets.retain(|t| t.key != key);
        for t in file.tickets.iter_mut().filter(|t| t.parent.as_deref() == Some(key)) {
            t.parent = None;
            t.updated_at = Utc::now();
            t.history.push(entry(By::Owner, format!("parent: {key} -> none ({key} was deleted)")));
            cap_history(t);
        }
        self.save(&file)?;
        self.announce(key, file.project.as_deref(), true);
        Ok(())
    }

    /// The tickets that match `filter`, and every project (the Inbox always).
    pub fn list(&self, filter: &Filter) -> Listing {
        let (files, _) = self.load_all();
        let mut projects: Vec<ProjectInfo> = files
            .iter()
            .map(|f| ProjectInfo { id: project_id(f.project.as_deref()), path: f.project.clone(), name: project_name(f.project.as_deref()), prefix: f.prefix.clone(), count: f.tickets.len() })
            .collect();
        if !projects.iter().any(|p| p.path.is_none()) {
            projects.insert(0, ProjectInfo { id: "inbox".into(), path: None, name: "Inbox".into(), prefix: store::INBOX_PREFIX.into(), count: 0 });
        }
        projects.sort_by(|a, b| (a.path.is_some(), a.name.to_lowercase()).cmp(&(b.path.is_some(), b.name.to_lowercase())));
        let text = filter.text.as_deref().map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty());
        let mut tickets = Vec::new();
        for f in files {
            let wanted = match filter.project.as_deref() {
                None => true,
                Some("inbox") => f.project.is_none(),
                Some(p) => f.project.as_deref() == Some(p),
            };
            if !wanted {
                continue;
            }
            for t in f.tickets {
                if filter.state.is_some_and(|s| s != t.state) {
                    continue;
                }
                if filter.label.as_deref().is_some_and(|l| !t.labels.iter().any(|x| x.eq_ignore_ascii_case(l))) {
                    continue;
                }
                if let Some(q) = &text {
                    if !(t.key.to_lowercase().contains(q) || t.title.to_lowercase().contains(q) || t.description.to_lowercase().contains(q)) {
                        continue;
                    }
                }
                tickets.push(t);
            }
        }
        Listing { projects, tickets }
    }
}
