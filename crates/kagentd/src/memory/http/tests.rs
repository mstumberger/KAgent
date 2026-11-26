//! The memory routes (api.md section 6), through the functions the handlers wrap.

use axum::http::StatusCode;
use serde_json::json;

use super::*;
use crate::memory::testing::{hub, hub_off, new};
use crate::memory::{MACHINE, USER, today};
use crate::server::HttpError;

type Refusal = (StatusCode, &'static str, String);

/// The route's answer, or its status, code and message (`HttpError` has no `Debug`).
fn answered<T>(r: Result<T, HttpError>) -> Result<T, Refusal> {
    r.map_err(|HttpError(status, code, message)| (status, code, message))
}

fn refused<T>(r: Result<T, HttpError>) -> Refusal {
    match answered(r) {
        Ok(_) => panic!("refused"),
        Err(refusal) => refusal,
    }
}

#[test]
fn the_routes_validate_what_they_are_sent() {
    let (hub, _dir, _events) = hub();
    for (body, why) in [
        (json!({}), "name is required"),
        (json!({"name": "printer", "type": "machine"}), "content is required"),
        (json!({"name": "printer", "content": "x"}), "type must be"),
        (json!({"name": "Printer", "type": "machine", "content": "x"}), "is not valid"),
        (json!({"name": "user", "type": "user", "content": "x"}), "core file"),
        (json!({"name": "p", "type": "fact", "content": "x"}), "type must be"),
        (json!({"name": "p", "type": "user", "content": "   "}), "content is required"),
        (json!({"name": "p", "type": "user", "content": 5}), "content is required"),
        (json!({"name": "p", "type": "user", "content": "x", "descrption": "typo"}), "unknown field 'descrption'"),
        (json!({"name": "p", "type": "user", "content": "sk-0123456789abcdef"}), "do not store secrets in memory"),
    ] {
        let err = refused(insert(&hub, &body));
        assert_eq!((err.0, err.1), (StatusCode::BAD_REQUEST, "invalid_input"), "{body}: {}", err.2);
        assert!(err.2.contains(why), "{body}: {}", err.2);
    }
    // A memory of the owner's.
    let m = answered(insert(&hub, &json!({"name": "printer", "description": "where it is", "type": "machine", "content": "The printer is in the hallway."}))).unwrap();
    assert_eq!((m.source, m.reviewed, m.description.as_str()), (MemorySource::User, true, "where it is"));
    assert_eq!(answered(insert(&hub, &json!({"name": "printer", "type": "user", "content": "again"}))).unwrap_err().0, StatusCode::CONFLICT, "409, not an overwrite");

    // PUT: 404 for an unknown memory; a body that changes nothing is not accepted.
    assert_eq!(refused(edit(&hub, "nope", &json!({"reviewed": true}))).0, StatusCode::NOT_FOUND);
    for (body, why) in [(json!({}), "nothing to change"), (json!({"reviewed": "yes"}), "reviewed must be"), (json!({"content": "x", "other": 1}), "unknown field 'other'"), (json!({"content": "   "}), "content is required"), (json!({"description": 5}), "must be text")] {
        let err = refused(edit(&hub, "printer", &body));
        assert_eq!((err.0, err.1), (StatusCode::BAD_REQUEST, "invalid_input"), "{body}: {}", err.2);
        assert!(err.2.contains(why), "{body}: {}", err.2);
    }
    // A core file changes its content only (it may be emptied).
    assert!(refused(edit(&hub, USER, &json!({"type": "machine"}))).2.contains("core file"));
    assert!(refused(edit(&hub, MACHINE, &json!({"description": "d"}))).2.contains("core file"));
    let core = answered(edit(&hub, USER, &json!({"content": " Marko. Short answers. "}))).unwrap();
    assert_eq!((core.body.as_str(), core.r#type, core.source), ("Marko. Short answers.", MemoryType::User, MemorySource::User));
    assert!(answered(edit(&hub, MACHINE, &json!({"content": ""}))).unwrap().body.is_empty());

    // The agent's memory: opening marks it seen; an edit makes it the owner's.
    let (written, _) = hub.save(&new("router", MemoryType::Machine, "MikroTik hEX at 192.168.88.1."), None, SaveMode::Overwrite).unwrap();
    assert!(written.unreviewed());
    let seen = answered(edit(&hub, "router", &json!({"reviewed": true}))).unwrap();
    assert_eq!((seen.reviewed, seen.source, seen.body.as_str()), (true, MemorySource::Agent, written.body.as_str()), "the flag alone changes nothing else");
    assert!(answered(edit(&hub, "router", &json!({"reviewed": false}))).is_ok(), "and back to not seen");
    assert!(!hub.get("router").unwrap().reviewed, "and back");
    let edited = answered(edit(&hub, "router", &json!({"type": "reference", "content": "MikroTik hEX at 192.168.88.2"}))).unwrap();
    assert_eq!((edited.source, edited.reviewed, edited.r#type, edited.created, edited.description.as_str()), (MemorySource::User, true, MemoryType::Reference, written.created, written.description.as_str()), "a field left out keeps its value");
    assert!(refused(edit(&hub, "router", &json!({"type": "fact"}))).2.contains("type must be"));

    // Forget: 409 for a core file, 404 for an unknown memory.
    let forgotten = answered(mark_forgotten(&hub, "printer")).unwrap();
    assert_eq!(forgotten.valid_until, Some(today()), "forgotten from today on");
    assert!(forgotten.superseded(today()) && hub.store().path("printer").exists());
    assert_eq!(refused(mark_forgotten(&hub, USER)).0, StatusCode::CONFLICT);
    assert_eq!(refused(mark_forgotten(&hub, "nope")).0, StatusCode::NOT_FOUND);
    // DELETE: 204; a core file is 409.
    assert_eq!(refused(erase(&hub, MACHINE)).0, StatusCode::CONFLICT);
    answered(erase(&hub, "printer")).unwrap();
    assert!(!hub.store().path("printer").exists());
    assert_eq!(refused(erase(&hub, "printer")).0, StatusCode::NOT_FOUND);
    assert_eq!(refused(erase(&hub, "nope")).0, StatusCode::NOT_FOUND);
}

#[test]
fn the_overview_and_the_search_answer_the_shape_of_the_spec() {
    let (hub, _dir, _events) = hub();
    let empty = overview(&hub);
    assert!(empty.enabled && empty.memories.is_empty());
    assert_eq!(empty.core.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), [MACHINE, USER]);
    assert_eq!((empty.stats.files, empty.stats.unreviewed), (2, 0), "the two core files count");
    assert!(empty.core.iter().all(|m| m.content.is_some()));
    http_create(&hub, "printer", "user", "The printer is in the hallway.");
    hub.save(&new("router", MemoryType::Machine, "MikroTik hEX at 192.168.88.1."), None, SaveMode::Overwrite).unwrap();
    hub.forget("printer").unwrap();
    let o = overview(&hub);
    assert_eq!(o.memories.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["printer", "router"], "the forgotten one is still listed, and says so");
    let printer = &o.memories[0];
    assert_eq!((printer.source, printer.reviewed, printer.valid_until.as_ref().map(std::string::ToString::to_string)), (MemorySource::User, true, Some(today().to_string())));
    assert!(printer.superseded_by.is_none() && printer.path.ends_with("printer.md") && printer.bytes > 0);
    assert_eq!((o.stats.files, o.stats.unreviewed), (4, 1), "a forgotten memory is not one that waits for review");
    let all = hub.all();
    assert_eq!(o.stats.bytes, all.iter().map(|m| m.bytes).sum::<u64>());
    let value = serde_json::to_value(&o).unwrap();
    assert_eq!(value["memories"][0], json!({"name": "printer", "description": "The printer is in the hallway.", "type": "user", "path": hub.store().path("printer"), "bytes": all.iter().find(|m| m.name == "printer").unwrap().bytes, "createdAt": today().to_string(), "updatedAt": today().to_string(), "source": "user", "reviewed": true, "validUntil": today().to_string(), "content": "The printer is in the hallway."}), "api.md MemoryView");

    // GET /api/memory/search?q=
    assert_eq!(refused(query_of(&Default::default())).1, "invalid_input", "q is required");
    assert_eq!(refused(query_of(&[("q".to_string(), "  ".to_string())].into_iter().collect())).2, "q is required: the words to look for");
    assert_eq!(answered(query_of(&[("q".to_string(), " router ".to_string())].into_iter().collect())).unwrap(), "router");
    assert!(find(&hub, "printer").is_empty(), "a forgotten memory is not found by a search");
    let found = find(&hub, "router");
    assert_eq!((found.len(), found[0].name.as_str(), found[0].content.is_none()), (1, "router", true), "a search sends no content");
}

/// memory.md section 2: with memory off the files stay editable in the UI.
#[test]
fn with_memory_off_the_files_stay_editable() {
    let hub = hub_off();
    let o = overview(&hub);
    assert!(!o.enabled, "the UI shows the switch off");
    let m = answered(insert(&hub, &json!({"name": "printer", "type": "machine", "content": "The printer is in the hallway."}))).unwrap();
    assert_eq!(m.name, "printer");
    assert_eq!(answered(edit(&hub, "printer", &json!({"reviewed": true}))).unwrap().name, "printer");
    assert_eq!(find(&hub, "printer").len(), 1);
    answered(mark_forgotten(&hub, "printer")).unwrap();
    answered(erase(&hub, "printer")).unwrap();
}

/// `create` by hand, for a memory of the owner's.
fn http_create(hub: &MemoryHub, name: &str, r#type: &str, content: &str) {
    answered(insert(hub, &json!({"name": name, "type": r#type, "content": content}))).unwrap();
}
