use super::*;
use kagent_core::skills::{plan_delete, remove_skill, SkillLibrary, SkillRoot};
use super::copy::{exists, staging_dir, too_large};
use std::os::unix::fs::PermissionsExt;

fn skill(root: &Path, name: &str, description: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\n\nBody of {name}.\n")).unwrap();
    dir
}

/// SHA-256 of a skill folder.
fn tree_hash(dir: &Path) -> String {
    hash_of(&skill_files(dir).unwrap(), dir).unwrap()
}

fn held(name: &str, dir: &Path, source: SkillSource) -> Installed {
    Installed { name: name.into(), source, real_dir: std::fs::canonicalize(dir).unwrap() }
}

fn ask(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

trait Named {
    fn name(&self) -> &str;
}
macro_rules! named {
    ($($t:ty),*) => {$(
        impl Named for $t {
            fn name(&self) -> &str {
                &self.name
            }
        }
    )*};
}
named!(ImportItem, Skipped, Candidate);
impl Named for String {
    fn name(&self) -> &str {
        self
    }
}

fn names_of<T: Named>(items: &[T]) -> Vec<&str> {
    items.iter().map(Named::name).collect()
}

#[tokio::test]
async fn two_folders_that_share_a_real_path_list_a_skill_once_and_a_taken_name_is_skipped() {
    // Scenario S21 item 1; skills.md sections 7 and 11.
    let home = tempfile::tempdir().unwrap();
    let (zcode, agents, mine) = (home.path().join(".zcode/skills"), home.path().join(".agents/skills"), home.path().join("config/skills"));
    std::fs::create_dir_all(&zcode).unwrap();
    skill(&agents, "alpha", "from the shared folder");
    // ZCode's alpha is the shared one, reached through a link.
    std::os::unix::fs::symlink(agents.join("alpha"), zcode.join("alpha")).unwrap();
    skill(&agents, "beta", "another one");
    skill(&agents, "zeta", "a name the owner uses already");
    skill(&mine, "zeta", "written by the owner");
    let installed = [held("zeta", &mine.join("zeta"), SkillSource::User)];
    let imported_root = home.path().join("config/skills-imported");
    std::fs::create_dir_all(&imported_root).unwrap();

    let found = detect(home.path(), &installed);
    assert_eq!(found.len(), KNOWN.len(), "every folder of the table is a row");
    assert_eq!((found[0].agent, found[0].exists, found[0].path.clone()), ("ZCode", true, zcode.clone()));
    assert_eq!(names_of(&found[0].skills), ["alpha"], "the row of ZCode holds the skill reached through the link");
    assert_eq!(names_of(&found[1].skills), ["beta", "zeta"], "the shared folder does not list alpha a second time: one real path");
    assert!(!found[2].exists && found[2].skills.is_empty() && found[2].path == home.path().join(".claude/skills"), "a tool that is not installed");
    let zeta = &found[1].skills[1];
    assert_eq!((zeta.installed, zeta.duplicate_of.as_deref()), (true, Some("zeta")), "installed: the owner has the name");
    assert!(!found[1].skills[0].installed && found[1].skills[0].duplicate_of.is_none(), "beta can be imported");

    // The plan takes the skill once; the owner's name is skipped, an unknown one is not found.
    let plan = plan_import(&zcode, &ask(&["alpha"]), ImportMode::Copy, &installed, &imported_root).unwrap();
    assert_eq!(plan.items.iter().map(|i| i.real_dir.clone()).collect::<Vec<_>>(), [std::fs::canonicalize(agents.join("alpha")).unwrap()], "the link leads to the folder");
    assert!(plan.skipped.is_empty() && plan.items[0].hash.len() == 64);
    let plan = plan_import(&agents, &ask(&["alpha", "zeta", "nope", "alpha"]), ImportMode::Copy, &installed, &imported_root).unwrap();
    assert_eq!(names_of(&plan.items), ["alpha"], "a name asked twice is planned once");
    assert_eq!(
        plan.skipped,
        [Skipped { name: "zeta".into(), reason: "name exists".into() }, Skipped { name: "nope".into(), reason: "not found".into() }]
    );
    assert!(plan_import(&agents, &ask(&["../escape"]), ImportMode::Copy, &installed, &imported_root).is_err(), "a name that is not an id is not planned");
    assert!(plan_import(&home.path().join("nowhere"), &ask(&["alpha"]), ImportMode::Copy, &installed, &imported_root).is_err(), "a folder that is not there");
    assert!(std::fs::read_dir(&imported_root).unwrap().next().is_none(), "a plan writes nothing");

    // Imported once (from ZCode's folder), the skill is not imported again from the other
    // folder: its folder is there, whether the library is looked at afresh or not.
    let plan = plan_import(&zcode, &ask(&["alpha"]), ImportMode::Copy, &installed, &imported_root).unwrap();
    assert_eq!(names_of(&apply_import(&plan, ImportMode::Copy, &imported_root).await.imported), ["alpha"]);
    let fresh = [installed[0].clone(), held("alpha", &imported_root.join("alpha"), SkillSource::Imported)];
    for installed in [&installed[..], &fresh[..]] {
        let plan = plan_import(&agents, &ask(&["alpha"]), ImportMode::Copy, installed, &imported_root).unwrap();
        assert!(plan.items.is_empty(), "never overwritten");
        assert_eq!((names_of(&plan.skipped), plan.skipped[0].reason.as_str()), (vec!["alpha"], "name exists"));
    }
    let found = detect(home.path(), &fresh);
    assert_eq!((found[0].skills[0].installed, found[0].skills[0].duplicate_of.as_deref()), (true, Some("alpha")), "the dialog says it is installed now");
}

#[tokio::test]
async fn copy_takes_files_only_and_a_link_takes_the_folder_whose_target_stays_on_delete() {
    // Scenario S21 item 3; skills.md section 7.
    let home = tempfile::tempdir().unwrap();
    let (tool, imported_root, outside) = (home.path().join("tool"), home.path().join("config/skills-imported"), home.path().join("outside"));
    let gamma = skill(&tool, "gamma", "with scripts and links");
    std::fs::create_dir_all(gamma.join("scripts")).unwrap();
    std::fs::write(gamma.join("scripts/check.sh"), "#!/bin/sh\necho ok\n").unwrap();
    std::fs::set_permissions(gamma.join("scripts/check.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "not part of the skill").unwrap();
    std::os::unix::fs::symlink(&outside, gamma.join("link-out")).unwrap();
    std::os::unix::fs::symlink("SKILL.md", gamma.join("link-file")).unwrap();
    skill(&tool, "big", "over the limit");
    std::fs::File::create(tool.join("big/blob.bin")).unwrap().set_len(MAX_SKILL_BYTES + 1).unwrap();
    skill(&tool, "delta", "to be linked");
    std::fs::create_dir_all(&imported_root).unwrap();
    let none: Vec<Installed> = Vec::new();

    // Copy: files only; a skill over 20 MB is skipped and nothing of it is written.
    let plan = plan_import(&tool, &ask(&["gamma", "big"]), ImportMode::Copy, &none, &imported_root).unwrap();
    assert_eq!((names_of(&plan.items), names_of(&plan.skipped)), (vec!["gamma"], vec!["big"]));
    assert_eq!(plan.skipped[0].reason, "more than 20 MB");
    let out = apply_import(&plan, ImportMode::Copy, &imported_root).await;
    assert_eq!((names_of(&out.imported), names_of(&out.skipped)), (vec!["gamma"], vec!["big"]), "the plan's skips are in the answer");
    assert_eq!(out.skipped[0].reason, "more than 20 MB");
    let copy = imported_root.join("gamma");
    assert_eq!(std::fs::read_to_string(copy.join("SKILL.md")).unwrap(), std::fs::read_to_string(gamma.join("SKILL.md")).unwrap());
    assert_eq!(std::fs::read_to_string(copy.join("scripts/check.sh")).unwrap(), "#!/bin/sh\necho ok\n");
    assert_eq!(std::fs::metadata(copy.join("scripts/check.sh")).unwrap().permissions().mode() & 0o777, 0o755, "a script keeps its mode");
    assert!(!std::fs::symlink_metadata(copy.join("link-out")).is_ok() && !std::fs::symlink_metadata(copy.join("link-file")).is_ok(), "links inside are not copied");
    assert!(!imported_root.join("big").exists() && !staging_dir(&imported_root).exists(), "nothing of the big one, no staging folder left");
    let regular: Vec<SkillFile> = skill_files(&gamma).unwrap().into_iter().filter(|f| !f.link).collect();
    assert_eq!(hash_of(&regular, &gamma).unwrap(), tree_hash(&copy), "the copy has the content of the files");

    // Link: the folder itself is not copied; a skill over the limit may be linked.
    let plan = plan_import(&tool, &ask(&["big", "gamma"]), ImportMode::Symlink, &none, &imported_root).unwrap();
    assert_eq!((names_of(&plan.items), names_of(&plan.skipped)), (vec!["big"], vec!["gamma"]), "gamma is there already");
    assert_eq!(plan.skipped[0].reason, "name exists");
    let out = apply_import(&plan, ImportMode::Symlink, &imported_root).await;
    assert_eq!(names_of(&out.imported), ["big"]);
    assert!(std::fs::symlink_metadata(imported_root.join("big")).unwrap().file_type().is_symlink());

    // Delete of a link through the library's rule: the link goes, the target stays.
    let plan = plan_import(&tool, &ask(&["delta"]), ImportMode::Symlink, &none, &imported_root).unwrap();
    apply_import(&plan, ImportMode::Symlink, &imported_root).await;
    let link = imported_root.join("delta");
    assert_eq!(std::fs::read_link(&link).unwrap(), std::fs::canonicalize(tool.join("delta")).unwrap(), "the link points at the folder with links resolved");
    let library = SkillLibrary::scan(&[SkillRoot { path: imported_root.clone(), source: SkillSource::Imported }], &[]);
    assert_eq!(library.all().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["big", "delta", "gamma"]);
    let deletion = plan_delete(&library, &imported_root, "delta", true).unwrap();
    assert!(deletion.link);
    remove_skill(&deletion).await.unwrap();
    assert!(!std::fs::symlink_metadata(&link).is_ok(), "the link is gone");
    assert!(tool.join("delta/SKILL.md").is_file(), "the target is not");
}

#[tokio::test]
async fn a_copy_lands_in_one_step_and_never_over_what_is_there() {
    let home = tempfile::tempdir().unwrap();
    let (tool, imported_root) = (home.path().join("tool"), home.path().join("config/skills-imported"));
    std::fs::create_dir_all(&imported_root).unwrap();
    for name in ["gamma", "delta", "eta", "theta"] {
        skill(&tool, name, "d");
    }
    stage(&tool.join("gamma"), &imported_root.join("gamma"), &imported_root, false).await.unwrap();
    assert!(imported_root.join("gamma/SKILL.md").is_file() && !staging_dir(&imported_root).exists(), "the copy is there and the staging folder is gone");

    // Whatever has the name stays: a folder, a file, a link that leads nowhere.
    std::fs::create_dir_all(imported_root.join("delta")).unwrap();
    std::fs::write(imported_root.join("delta/keep.txt"), "keep").unwrap();
    std::fs::write(imported_root.join("eta"), "a file").unwrap();
    std::os::unix::fs::symlink(home.path().join("nowhere"), imported_root.join("theta")).unwrap();
    for name in ["delta", "eta", "theta"] {
        let error = stage(&tool.join(name), &imported_root.join(name), &imported_root, false).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "{name}: {error}");
    }
    assert_eq!(std::fs::read_to_string(imported_root.join("delta/keep.txt")).unwrap(), "keep");
    assert!(!imported_root.join("delta/SKILL.md").exists());
    assert_eq!(std::fs::read_to_string(imported_root.join("eta")).unwrap(), "a file");
    assert!(std::fs::symlink_metadata(imported_root.join("theta")).unwrap().file_type().is_symlink());
    assert!(!staging_dir(&imported_root).exists(), "a refused copy leaves nothing behind");
    let error = link(&tool.join("gamma"), &imported_root.join("gamma")).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "a link does not overwrite either");
    assert!(imported_root.join("gamma/SKILL.md").is_file());

    // Replace (a skill of a source whose content changed): the old content goes whole.
    std::fs::write(tool.join("delta/added.md"), "added in the source").unwrap();
    stage(&tool.join("delta"), &imported_root.join("delta"), &imported_root, true).await.unwrap();
    assert!(imported_root.join("delta/SKILL.md").is_file() && imported_root.join("delta/added.md").is_file());
    assert!(!imported_root.join("delta/keep.txt").exists(), "nothing of the old copy stays");
    assert!(!staging_dir(&imported_root).exists());
}

#[test]
fn a_change_to_a_skill_changes_its_hash_and_the_rest_is_read_from_the_request() {
    let home = tempfile::tempdir().unwrap();
    let gamma = skill(&home.path().join("tool"), "gamma", "d");
    let before = tree_hash(&gamma);
    assert_eq!(before.len(), 64, "SHA-256 as hex");
    std::fs::write(gamma.join("SKILL.md"), "---\nname: gamma\ndescription: changed\n---\n\nOther body.\n").unwrap();
    let after = tree_hash(&gamma);
    assert_ne!(before, after, "a changed file changes the hash");
    std::fs::write(gamma.join("extra.md"), "more").unwrap();
    assert_ne!(tree_hash(&gamma), after, "an added file changes the hash");
    // A link is hashed as a link, and never read through.
    std::os::unix::fs::symlink("extra.md", gamma.join("alias.md")).unwrap();
    assert_ne!(tree_hash(&gamma), after);
    let files = skill_files(&gamma).unwrap();
    assert_eq!(files.iter().find(|f| f.rel == Path::new("alias.md")).map(|f| (f.link, f.size)), Some((true, 0)));
    assert_eq!(bytes_of(&files), std::fs::read(gamma.join("extra.md")).unwrap().len() as u64 + std::fs::metadata(gamma.join("SKILL.md")).unwrap().len());

    // The mode is copy unless the request says symlink.
    assert_eq!(ImportMode::parse(None), Ok(ImportMode::Copy));
    assert_eq!(ImportMode::parse(Some("copy")), Ok(ImportMode::Copy));
    assert_eq!(ImportMode::parse(Some("symlink")), Ok(ImportMode::Symlink));
    assert!(ImportMode::parse(Some("move")).unwrap_err().contains("copy or symlink"));

    // A folder of the table is named by its tool; any other folder by its path.
    std::fs::create_dir_all(home.path().join(".claude/skills")).unwrap();
    assert_eq!(origin_label(&home.path().join(".claude/skills"), home.path()), "Claude Code");
    assert_eq!(origin_label(&home.path().join("tool"), home.path()), home.path().join("tool").display().to_string());

    // What a failed write is called.
    assert_eq!(reason_of(&too_large()), "more than 20 MB");
    assert_eq!(reason_of(&exists()), "name exists");
    assert_eq!(reason_of(&io::Error::other("disk full")), "cannot be written: disk full");
}
