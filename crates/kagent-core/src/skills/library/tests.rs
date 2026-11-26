//! Unit tests of the library (skills.md section 11, scenarios S18 items 2 to 4).

use super::*;

fn skill(root: &Path, dir: &str, name: &str, description: &str) -> PathBuf {
    let dir = root.join(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\n\nBody of {name}.\n")).unwrap();
    dir
}

fn root(path: &Path, source: SkillSource) -> SkillRoot {
    SkillRoot { path: path.to_path_buf(), source }
}

fn names(library: &SkillLibrary) -> Vec<&str> {
    library.available().map(|e| e.name.as_str()).collect()
}

#[test]
fn user_wins_over_imported_which_wins_over_bundled() {
    let dir = tempfile::tempdir().unwrap();
    let (bundled, imported, user) = (dir.path().join("b"), dir.path().join("i"), dir.path().join("u"));
    skill(&bundled, "slow-machine", "slow-machine", "the playbook");
    skill(&imported, "slow-machine", "slow-machine", "imported copy");
    skill(&imported, "wifi", "wifi", "imported only");
    skill(&user, "slow-machine", "slow-machine", "written here");
    skill(&user, "alpha", "alpha", "user only");
    // The roots in no particular order.
    let library = SkillLibrary::scan(&[root(&imported, SkillSource::Imported), root(&bundled, SkillSource::Bundled), root(&user, SkillSource::User)], &[]);
    let winner = library.get("slow-machine").unwrap();
    assert_eq!((winner.source, winner.description.as_str(), winner.shadowed), (SkillSource::User, "written here", false));
    assert_eq!(winner.dir, user.join("slow-machine"));
    assert!(winner.path.ends_with("slow-machine/SKILL.md") && winner.available());
    let shadowed: Vec<(SkillSource, &str)> = library.shadowed().iter().map(|e| (e.source, e.description.as_str())).collect();
    assert_eq!(shadowed, [(SkillSource::Imported, "imported copy"), (SkillSource::Bundled, "the playbook")]);
    // Scenario S18 item 3: the index names the winner, user first, each alphabetical.
    assert_eq!(names(&library), ["alpha", "slow-machine", "wifi"]);
    let index = library.index(3000);
    assert!(index.starts_with("\nSkills (call load_skill with the name before using one):\n"), "{index}");
    assert!(index.contains("- slow-machine: written here\n"), "{index}");
}

#[test]
fn disabled_hides_a_bundled_skill_from_the_index() {
    let dir = tempfile::tempdir().unwrap();
    skill(&dir.path().join("b"), "slow-machine", "slow-machine", "why is it slow");
    skill(&dir.path().join("b"), "wifi-problems", "wifi-problems", "no signal");
    let roots = [root(&dir.path().join("b"), SkillSource::Bundled)];
    let library = SkillLibrary::scan(&roots, &["slow-machine".to_string()]);
    assert_eq!(names(&library), ["wifi-problems"]);
    assert!(!library.get("slow-machine").unwrap().enabled);
    assert!(!library.index(3000).contains("slow-machine"));
    assert_eq!(SkillLibrary::scan(&roots, &["nothing".into()]).available_count(), 2, "a name that matches no skill changes nothing");
}

#[test]
fn the_same_real_path_counts_once() {
    let dir = tempfile::tempdir().unwrap();
    let real = skill(&dir.path().join("elsewhere"), "one", "one", "the original");
    let imported = dir.path().join("imported");
    std::fs::create_dir(&imported).unwrap();
    std::os::unix::fs::symlink(&real, imported.join("a")).unwrap();
    std::os::unix::fs::symlink(&real, imported.join("b")).unwrap();
    let library = SkillLibrary::scan(&[root(&imported, SkillSource::Imported)], &[]);
    assert_eq!(library.all().count(), 1, "two links to one skill are one skill");
    assert_eq!(library.get("one").unwrap().dir, imported.join("a"));
}

#[test]
fn an_oversized_body_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    skill(dir.path(), "small", "small", "fits");
    let big = skill(dir.path(), "big", "big", "on the limit");
    let roots = [root(dir.path(), SkillSource::User)];
    std::fs::write(big.join("SKILL.md"), format!("---\nname: big\n---\n{}", "a".repeat(MAX_BODY_BYTES))).unwrap();
    let library = SkillLibrary::scan(&roots, &[]);
    assert_eq!(names(&library), ["big", "small"]);
    assert_eq!(library.get("big").unwrap().size_bytes, MAX_BODY_BYTES as u64);
    std::fs::write(big.join("SKILL.md"), format!("---\nname: big\n---\n{}", "a".repeat(MAX_BODY_BYTES + 1))).unwrap();
    assert_eq!(names(&SkillLibrary::scan(&roots, &[])), ["small"], "one byte more is skipped");
}

#[test]
fn without_a_frontmatter_name_the_directory_is_the_name() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("SKILL.md"), "# Heading\n\nBody").unwrap();
    skill(dir.path(), "renamed", "real-name", "d");
    let invalid = dir.path().join("Bad_Name");
    std::fs::create_dir_all(&invalid).unwrap();
    std::fs::write(invalid.join("SKILL.md"), "No frontmatter at all").unwrap();
    let library = SkillLibrary::scan(&[root(dir.path(), SkillSource::User)], &[]);
    assert_eq!(names(&library), ["plain", "real-name"]);
    let plain = library.get("plain").unwrap();
    assert_eq!((plain.description.as_str(), plain.body.as_str()), ("# Heading", "# Heading\n\nBody"));
    assert_eq!(library.all().count(), 2, "a directory name that is not a valid id is skipped");
}

#[test]
fn the_index_respects_the_budget_and_lists_the_rest_as_names() {
    let dir = tempfile::tempdir().unwrap();
    // Every line is `- a0: dddddddddd` plus a newline: 17 characters.
    for n in 0..8 {
        skill(dir.path(), &format!("a{n}"), &format!("a{n}"), "dddddddddd");
    }
    let library = SkillLibrary::scan(&[root(dir.path(), SkillSource::Bundled)], &[]);
    assert_eq!(library.index(usize::MAX).lines().filter(|l| l.starts_with("- ")).count(), 8);
    // 50 characters hold two whole lines (34); the third would make 51.
    let index = library.index(50);
let lines: Vec<&str> = index.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines[1..3], ["- a0: dddddddddd", "- a1: dddddddddd"]);
    assert_eq!(lines[3], "Also available (names only): a2, a3, a4, a5, a6, a7");
    assert_eq!(lines.len(), 4, "heading, the lines that fit, one names-only line");
    // A line longer than the whole budget is not cut: its name is listed instead.
    skill(dir.path(), "huge", "huge", &"d".repeat(80));
    let index = SkillLibrary::scan(&[root(dir.path(), SkillSource::User)], &[]).index(50);
    assert!(!index.contains("huge:") && index.ends_with("a2, a3, a4, a5, a6, a7, huge\n"), "{index}");
}

#[test]
fn refresh_reports_an_added_an_edited_and_a_removed_skill() {
    let dir = tempfile::tempdir().unwrap();
    let roots = [root(dir.path(), SkillSource::User)];
    skill(dir.path(), "slow-machine", "slow-machine", "old");
    let mut library = SkillLibrary::scan(&roots, &[]);
    assert!(!library.refresh_if_changed(&[]), "nothing changed");

    let beta = skill(dir.path(), "beta", "beta", "new skill");
    assert!(library.refresh_if_changed(&[]), "a directory was added");
    assert!(library.get("beta").is_some());

    // Edited: the same size, so only the modification time tells.
    let path = beta.join("SKILL.md");
    std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace("new skill", "old skill")).unwrap();
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(SystemTime::now() + std::time::Duration::from_secs(2)).unwrap();
    assert!(library.refresh_if_changed(&[]), "a file was edited");
    assert_eq!(library.get("beta").unwrap().description, "old skill");
    assert!(!library.refresh_if_changed(&[]), "the same state twice");

    std::fs::remove_dir_all(&beta).unwrap();
    assert!(library.refresh_if_changed(&[]), "a directory was removed");
    assert!(library.get("beta").is_none() && library.get("slow-machine").is_some());

    assert!(library.refresh_if_changed(&["slow-machine".into()]), "the disabled list changed");
    assert!(!library.get("slow-machine").unwrap().enabled);
    assert!(!library.refresh_if_changed(&["slow-machine".into()]));
}

#[test]
fn a_folder_of_skills_linked_into_a_root_is_scanned_two_levels_deep() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    skill(&store, "x", "x", "from the folder");
    skill(&store, "y", "y", "from the folder");
    let imported = dir.path().join("imported");
    std::fs::create_dir(&imported).unwrap();
    std::os::unix::fs::symlink(&store, imported.join("store-link")).unwrap();
    let mut library = SkillLibrary::scan(&[root(&imported, SkillSource::Imported)], &[]);
    assert_eq!(names(&library), ["x", "y"]);
    assert_eq!(library.get("x").unwrap().source, SkillSource::Imported);
    assert!(library.get("x").unwrap().dir.ends_with("store-link/x"));
    // A skill added inside the linked folder is seen too.
    skill(&store, "a", "a", "new");
    assert!(library.refresh_if_changed(&[]));
    assert_eq!(names(&library), ["a", "x", "y"]);
}

#[test]
fn the_bundled_skill_creator_is_listed_and_loads() {
    // Scenario S20 item 6, and the rules of skills.md section 5 for the bundled skill.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    let library = SkillLibrary::scan(&[root(&repo, SkillSource::Bundled)], &[]);
    let entry = library.get("skill-creator").expect("skill-creator ships with KAgent");
    assert!(entry.available() && entry.source == SkillSource::Bundled && entry.enabled);
    assert!(library.index(3000).contains("- skill-creator: "), "listed in the prompt");
    let raw = std::fs::read_to_string(&entry.path).unwrap();
    assert!(raw.lines().count() <= 60, "under 60 lines: {}", raw.lines().count());
    assert!(!raw.contains('\u{2014}'), "no em dashes");
    assert!(!entry.when_to_use.is_empty(), "when_to_use holds what the description cannot");
    let body = entry.body.to_lowercase();
    for said in ["a skill is a short", "third person", "`process_list`, `journal`, `package`", "a good answer looks like", "remember how something was done", "likely to recur", "done twice", "secrets", "absolute paths"] {
        assert!(body.contains(said), "the body says '{said}': {body}");
    }
    assert!(entry.body.trim_end().ends_with("\""), "it ends with the example of a good answer");
}
