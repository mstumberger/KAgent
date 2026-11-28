use chrono::Utc;
use kagent_core::journal::{Change, ChangeStatus};
use serde_json::json;

use super::git::parse;
use super::http::for_change;
use super::lines::{diff_texts, hunks, split};
use super::{Kind, Status};

fn kinds(h: &super::Hunk) -> Vec<(Kind, Option<usize>, Option<usize>, &str)> {
    h.lines.iter().map(|l| (l.kind, l.old, l.new, l.text.as_str())).collect()
}

#[test]
fn lines_split_at_lf_and_crlf_and_a_final_newline_ends_the_last_line() {
    assert_eq!(split("a\nb\n"), ["a", "b"]);
    assert_eq!(split("a\r\nb"), ["a", "b"]);
    assert_eq!(split("a\n\nb\n"), ["a", "", "b"]);
    assert!(split("").is_empty());
    assert_eq!(split("\n"), [""]);
}

#[test]
fn identical_texts_have_no_hunks_and_crlf_is_not_a_change() {
    let (h, a, r) = hunks("a\nb\n", "a\nb\n");
    assert!(h.is_empty() && (a, r) == (0, 0));
    let (h, a, r) = hunks("a\r\nb\r\n", "a\nb\n");
    assert!(h.is_empty() && (a, r) == (0, 0), "line endings are not part of a line");
}

#[test]
fn an_insert_a_delete_and_a_change() {
    let (h, a, r) = hunks("a\nb\nc\n", "a\nX\nb\nc\n");
    assert_eq!((h.len(), a, r), (1, 1, 0));
    assert_eq!(kinds(&h[0]), [(Kind::Ctx, Some(1), Some(1), "a"), (Kind::Add, None, Some(2), "X"), (Kind::Ctx, Some(2), Some(3), "b"), (Kind::Ctx, Some(3), Some(4), "c")]);
    assert_eq!((h[0].old_start, h[0].old_lines, h[0].new_start, h[0].new_lines), (1, 3, 1, 4));
    let (h, a, r) = hunks("a\nb\nc\n", "a\nc\n");
    assert_eq!((h.len(), a, r), (1, 0, 1));
    assert_eq!(h[0].lines[1].kind, Kind::Del);
    assert_eq!((h[0].lines[1].old, h[0].lines[1].new), (Some(2), None));
    let (h, a, r) = hunks("one\ntwo\nthree\n", "one\n2\nthree\n");
    assert_eq!((h.len(), a, r), (1, 1, 1));
    let k: Vec<Kind> = h[0].lines.iter().map(|l| l.kind).collect();
    assert_eq!(k, [Kind::Ctx, Kind::Del, Kind::Add, Kind::Ctx]);
}

#[test]
fn changes_far_apart_are_two_hunks_and_near_ones_are_one() {
    let old: String = (1..=30).map(|i| format!("line {i}\n")).collect();
    let far = old.replace("line 3\n", "THREE\n").replace("line 27\n", "TWENTY SEVEN\n");
    let (h, _, _) = hunks(&old, &far);
    assert_eq!(h.len(), 2);
    assert_eq!((h[0].old_start, h[0].old_lines), (1, 6), "line 3 with 2 lines of context before it and 3 after");
    assert_eq!((h[1].old_start, h[1].old_lines), (24, 7));
    let near = old.replace("line 10\n", "TEN\n").replace("line 16\n", "SIXTEEN\n");
    assert_eq!(hunks(&old, &near).0.len(), 1, "six lines between two changes: the contexts touch");
    let apart = old.replace("line 10\n", "TEN\n").replace("line 18\n", "EIGHTEEN\n");
    assert_eq!(hunks(&old, &apart).0.len(), 2);
}

#[test]
fn a_file_removed_or_made_empty_has_one_side_at_zero_lines() {
    let (h, a, r) = hunks("x\ny\n", "");
    assert_eq!((h.len(), a, r), (1, 0, 2));
    assert_eq!((h[0].old_start, h[0].old_lines, h[0].new_start, h[0].new_lines), (1, 2, 0, 0));
    let (h, a, r) = hunks("", "x\ny\n");
    assert_eq!((h.len(), a, r), (1, 2, 0));
    assert_eq!((h[0].old_start, h[0].old_lines, h[0].new_start, h[0].new_lines), (0, 0, 1, 2));
}

#[test]
fn intra_line_ranges_mark_the_changed_part() {
    let (h, _, _) = hunks("let x = 1;\n", "let x = 2;\n");
    assert_eq!((h[0].lines[0].ranges.clone(), h[0].lines[1].ranges.clone()), (vec![[8, 9]], vec![[8, 9]]));
    // An inserted piece marks only the new line.
    let (h, _, _) = hunks("foo(a)\n", "foo(a, b)\n");
    assert!(h[0].lines[0].ranges.is_empty());
    assert_eq!(h[0].lines[1].ranges, vec![[5, 8]]);
    // Characters, not bytes.
    let (h, _, _) = hunks("caf\u{e9} one\n", "caf\u{e9} two\n");
    assert_eq!(h[0].lines[0].ranges, vec![[5, 8]]);
    // Unpaired lines carry none: two removed, one added.
    let (h, _, _) = hunks("a1\na2\n", "b1\n");
    assert_eq!(h[0].lines.iter().map(|l| l.ranges.len()).collect::<Vec<_>>(), [1, 0, 1]);
}

#[test]
fn a_middle_over_the_cell_limit_is_all_removed_then_all_added() {
    let old: String = (0..2100).map(|i| format!("old {i}\n")).collect();
    let new: String = (0..2100).map(|i| format!("new {i}\n")).collect();
    let (h, a, r) = hunks(&old, &new);
    assert_eq!((h.len(), a, r), (1, 2100, 2100));
    let k: Vec<Kind> = h[0].lines.iter().map(|l| l.kind).collect();
    assert!(k[..2100].iter().all(|k| *k == Kind::Del) && k[2100..].iter().all(|k| *k == Kind::Add));
}

#[test]
fn binary_and_large_texts_are_a_summary_only() {
    let b = diff_texts("x.bin", "a\0b", "c", Status::Modified);
    assert!(b.binary && b.hunks.is_empty() && b.added.is_none());
    let big = "x\n".repeat(600_000);
    let l = diff_texts("big.txt", &big, "y\n", Status::Modified);
    assert!(l.too_large && !l.binary && l.hunks.is_empty() && l.added.is_none());
    let ok = diff_texts("a.txt", "a\n", "b\n", Status::Modified);
    assert!(!ok.binary && !ok.too_large && ok.hunks.len() == 1 && (ok.added, ok.removed) == (Some(1), Some(1)));
}

const SAMPLE: &str = "\
diff --git a/src/a b.rs b/src/a b.rs
index 1111111..2222222 100644
--- a/src/a b.rs
+++ b/src/a b.rs
@@ -1,3 +1,3 @@ fn main()
 keep
-let x = 1;
+let x = 2;
 tail
\\ No newline at end of file
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 3333333..0000000
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-one
-two
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..4444444
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
diff --git a/old name.txt b/new name.txt
similarity index 80%
rename from old name.txt
rename to new name.txt
index 5555555..6666666 100644
--- a/old name.txt
+++ b/new name.txt
@@ -1,2 +1,2 @@
 same
-before
+after
diff --git a/pic.png b/pic.png
index 7777777..8888888 100644
Binary files a/pic.png and b/pic.png differ
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
";

#[test]
fn git_output_is_parsed_with_every_status() {
    let d = parse(SAMPLE, false);
    let by = |p: &str| d.files.iter().find(|f| f.path == p).unwrap_or_else(|| panic!("{p} in {:?}", d.files.iter().map(|f| &f.path).collect::<Vec<_>>()));
    assert_eq!(d.files.len(), 6);
    let a = by("src/a b.rs");
    assert_eq!((a.status, a.added, a.removed), (Status::Modified, Some(1), Some(1)));
    assert_eq!(a.hunks[0].header, "fn main()");
    assert_eq!(a.hunks[0].lines.iter().map(|l| (l.kind, l.old, l.new)).collect::<Vec<_>>(), [(Kind::Ctx, Some(1), Some(1)), (Kind::Del, Some(2), None), (Kind::Add, None, Some(2)), (Kind::Ctx, Some(3), Some(3))]);
    assert_eq!(a.hunks[0].lines[1].ranges, vec![[8, 9]], "the intra-line marks come with it");
    let g = by("gone.txt");
    assert_eq!((g.status, g.added, g.removed, g.hunks[0].new_lines), (Status::Deleted, Some(0), Some(2), 0));
    let n = by("new.txt");
    assert_eq!((n.status, n.added, n.hunks[0].lines[0].new), (Status::Added, Some(1), Some(1)));
    let r = by("new name.txt");
    assert_eq!((r.status, r.old_path.as_deref()), (Status::Renamed, Some("old name.txt")));
    let p = by("pic.png");
    assert!(p.binary && p.added.is_none() && p.hunks.is_empty());
    let m = by("run.sh");
    assert_eq!((m.status, m.hunks.len(), m.added), (Status::Modified, 0, Some(0)), "a mode change is a file with no lines");
    assert_eq!((d.added, d.removed, d.truncated), (3, 4, false));
}

#[test]
fn output_that_was_cut_drops_what_came_before_the_first_whole_file_and_says_so() {
    let cut = format!("  half a line of an earlier file\n{}", SAMPLE);
    let d = parse(&cut, true);
    assert!(d.truncated, "the answer says it is incomplete");
    assert_eq!(d.files.len(), 6, "the text before the first whole file is dropped, the files after it stay");
    assert_eq!(d.files[0].path, "src/a b.rs");
}

#[test]
fn a_file_section_over_one_megabyte_is_a_summary_with_its_counts() {
    let mut s = String::from("diff --git a/big.txt b/big.txt\n--- a/big.txt\n+++ b/big.txt\n@@ -1,2 +1,2 @@\n");
    for i in 0..60_000 {
        s.push_str(&format!("-removed line number {i} padding padding\n+added line number {i} padding padding\n"));
    }
    assert!(s.len() > 1_000_000);
    let d = parse(&s, false);
    assert_eq!(d.files.len(), 1);
    let f = &d.files[0];
    assert!(f.too_large && f.hunks.is_empty() && f.added == Some(60_000) && f.removed == Some(60_000));
}

fn change(op: serde_json::Value, undo: Option<serde_json::Value>) -> Change {
    Change { id: "chg_1".into(), ts: Utc::now(), session_id: "s".into(), call_id: "c".into(), tool: "edit_file".into(), summary: "edit".into(), op, undo, privileged: false, status: ChangeStatus::Applied, before: None, after: None, undone_at: None, note: None }
}

#[test]
fn a_journal_change_has_its_stored_before_and_after() {
    let dir = tempfile::tempdir().unwrap();
    let (bak, aft) = (dir.path().join("bak_1"), dir.path().join("aft_1"));
    std::fs::write(&bak, "a\nb\nc\n").unwrap();
    std::fs::write(&aft, "a\nB\nc\n").unwrap();
    let c = change(json!({"action": "edit_file", "path": "/home/me/x.txt", "afterBackup": aft}), Some(json!({"action": "restore_file", "path": "/home/me/x.txt", "backup": bak})));
    let v = for_change(&c, |_| panic!("the stored copy is used")).unwrap();
    assert_eq!((v["after"].as_str(), v["path"].as_str(), v["before"].as_str(), v["afterText"].as_str()), (Some("stored"), Some("/home/me/x.txt"), Some("a\nb\nc\n"), Some("a\nB\nc\n")));
    assert_eq!(v["diff"]["files"][0]["status"], "modified");
    assert_eq!(v["diff"]["files"][0]["hunks"][0]["lines"][1]["kind"], "del");
    assert_eq!(v["diff"]["files"][0]["hunks"][0]["lines"][1]["ranges"], json!([[0, 1]]));
}

#[test]
fn a_change_from_before_this_stage_uses_the_file_as_it_is_now() {
    let dir = tempfile::tempdir().unwrap();
    let bak = dir.path().join("bak_1");
    std::fs::write(&bak, "old\n").unwrap();
    let c = change(json!({"action": "write_file", "path": "/home/me/x.txt"}), Some(json!({"action": "restore_file", "path": "/home/me/x.txt", "backup": bak})));
    let v = for_change(&c, |p| (p == "/home/me/x.txt").then(|| "new\n".to_string())).unwrap();
    assert_eq!((v["after"].as_str(), v["afterText"].as_str()), (Some("now"), Some("new\n")));
    // Undone and no stored copy: nothing to show.
    let mut undone = c.clone();
    undone.undone_at = Some(Utc::now());
    assert!(for_change(&undone, |_| Some("x".into())).is_err());
    // A file that did not exist has no backup: added.
    let created = change(json!({"action": "write_file", "path": "/home/me/n.txt"}), Some(json!({"action": "restore_file", "path": "/home/me/n.txt", "backup": null})));
    let v = for_change(&created, |_| Some("fresh\n".into())).unwrap();
    assert_eq!(v["diff"]["files"][0]["status"], "added");
    assert_eq!(v["before"].as_str(), Some(""));
}

#[test]
fn a_change_that_is_not_a_file_edit_is_a_conflict() {
    let c = change(json!({"action": "command", "argv": ["x"]}), Some(json!({"action": "command"})));
    let e = for_change(&c, |_| None).unwrap_err();
    assert_eq!(e.0, axum::http::StatusCode::CONFLICT);
    assert!(e.2.contains("no stored copy"));
    let gone = change(json!({"action": "edit_file", "path": "/x"}), Some(json!({"action": "restore_file", "backup": "/no/such/backup"})));
    assert_eq!(for_change(&gone, |_| None).unwrap_err().0, axum::http::StatusCode::CONFLICT);
    let root = change(json!({"op": "write_file"}), None);
    assert_eq!(for_change(&root, |_| None).unwrap_err().0, axum::http::StatusCode::CONFLICT);
}
