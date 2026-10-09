// Restore from Deleted files, through
// the crate's public API. Numbers in comments are the blueprint's edge cases.

use onecopy_lib::file_names::{FolderNames, RenameStyle};
use onecopy_lib::restore::*;
use onecopy_lib::trash::{
    list_root, trash_file, EntryStatus, TrashContext, TrashEntry, TrashKind, TrashRole,
    TRASH_DIR_NAME,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// The pure planner

fn entry(id: &str, relative: &str) -> TrashEntry {
    TrashEntry {
        id: id.to_string(),
        day: "20260927-utc".to_string(),
        stored_name: id.rsplit('/').next().unwrap().to_string(),
        stored_path: format!("/r/.onecopy-trash/{id}"),
        original_relative: Some(relative.to_string()),
        deleted_at_utc: "2026-09-27T10:00:00.000Z".to_string(),
        size: 10,
        mtime_ms: Some(1),
        group: format!("item:op:{relative}"),
        kind: Some(TrashKind::Delete),
        operation: Some("op".to_string()),
        item: Some(relative.to_string()),
        role: Some(TrashRole::Main),
        moved_to: None,
        status: EntryStatus::Restorable,
        main_restored_as: None,
    }
}

fn candidate(entry: TrashEntry) -> Candidate {
    let target = Path::new("/r").join(entry.original_relative.clone().unwrap());
    Candidate {
        entry,
        target: Some(target),
        missing_folders: Vec::new(),
        blocked: None,
        target_state: TargetState::Absent,
    }
}

fn with_state(mut candidate: Candidate, state: TargetState) -> Candidate {
    candidate.target_state = state;
    candidate
}

fn in_family(mut entry: TrashEntry, group: &str, role: TrashRole) -> TrashEntry {
    entry.group = group.to_string();
    entry.role = Some(role);
    entry
}

fn plan(candidates: &[Candidate], occupied: &[&str]) -> RestorePlan {
    let occupied: HashSet<PathBuf> = occupied.iter().map(PathBuf::from).collect();
    plan_restore(
        candidates,
        FolderNames::new(true, true),
        RenameStyle::SpaceNumber,
        &mut |path| !occupied.contains(path),
    )
}

fn targets(plan: &RestorePlan) -> Vec<(String, Option<String>)> {
    plan.steps
        .iter()
        .map(|step| {
            (
                step.entry.id.clone(),
                match &step.action {
                    Action::Restore { target, .. } => Some(target.to_string_lossy().into_owned()),
                    Action::Skip(skip) => Some(format!("skip:{skip:?}")),
                },
            )
        })
        .collect()
}

fn target_of(plan: &RestorePlan, id: &str) -> String {
    targets(plan)
        .into_iter()
        .find(|(candidate, _)| candidate == id)
        .and_then(|(_, target)| target)
        .unwrap()
}

#[test]
fn a_clean_selection_goes_home_without_a_review() {
    let plan = plan(&[candidate(entry("d/a.jpg", "trips/a.jpg"))], &[]);
    assert_eq!(target_of(&plan, "d/a.jpg"), "/r/trips/a.jpg");
    assert!(!review_of(&plan, Path::new("/r"), &[]).needed(), "D5: nothing to decide");
}

#[test]
fn missing_folders_are_recreated_and_named_in_the_review() {
    // 1, 2: a missing or renamed folder is recreated under its old name.
    let mut missing = candidate(entry("d/a.jpg", "gone/deeper/a.jpg"));
    missing.missing_folders = vec![PathBuf::from("/r/gone"), PathBuf::from("/r/gone/deeper")];
    let plan = plan(&[missing], &[]);
    assert_eq!(plan.folders, [PathBuf::from("/r/gone"), PathBuf::from("/r/gone/deeper")]);
    let review = review_of(&plan, Path::new("/r"), &[]);
    assert_eq!(review.folders, ["gone", "gone/deeper"]);
    assert!(review.needed());
}

#[test]
fn identical_bytes_are_already_there_and_different_ones_are_renamed() {
    // 13, 19: the same bytes at the original path are left alone.
    let plan_same = plan(&[with_state(candidate(entry("d/a.jpg", "a.jpg")), TargetState::Identical)], &[]);
    assert_eq!(target_of(&plan_same, "d/a.jpg"), "skip:AlreadyThere");
    // 14, 15, 19: different content, or a folder, takes the Rename suffix.
    let plan_other = plan(
        &[with_state(candidate(entry("d/a.jpg", "a.jpg")), TargetState::Occupied)],
        &["/r/a 2.jpg"],
    );
    assert_eq!(target_of(&plan_other, "d/a.jpg"), "/r/a 3.jpg");
    let review = review_of(&plan_other, Path::new("/r"), &[]);
    assert!(review.files[0].renamed && review.needed());
}

#[test]
fn a_family_shares_one_suffix_so_its_companions_still_pair() {
    // 23: the main is renamed, so its companion in the same folder takes the
    // same suffix even though the companion's own name was free.
    let main = in_family(entry("d/x.jpg", "a/x.jpg"), "item:op:x", TrashRole::Main);
    let sidecar = in_family(entry("d/x.xmp", "a/x.xmp"), "item:op:x", TrashRole::Companion);
    let plan = plan(
        &[with_state(candidate(main), TargetState::Occupied), candidate(sidecar)],
        &[],
    );
    assert_eq!(target_of(&plan, "d/x.jpg"), "/r/a/x 2.jpg");
    assert_eq!(target_of(&plan, "d/x.xmp"), "/r/a/x 2.xmp");
    // The main is placed before its companion.
    assert_eq!(plan.steps[0].entry.id, "d/x.jpg");
}

#[test]
fn copies_in_other_folders_keep_their_own_names() {
    // 20: a whole group returns to its own paths; only the folder with a
    // conflict renames.
    let one = in_family(entry("d/x.jpg", "a/x.jpg"), "item:op:x", TrashRole::Main);
    let two = in_family(entry("d/x-2.jpg", "b/x.jpg"), "item:op:x", TrashRole::Main);
    let plan = plan(&[with_state(candidate(one), TargetState::Occupied), candidate(two)], &[]);
    assert_eq!(target_of(&plan, "d/x.jpg"), "/r/a/x 2.jpg");
    assert_eq!(target_of(&plan, "d/x-2.jpg"), "/r/b/x.jpg");
}

#[test]
fn the_newest_of_several_versions_takes_the_original_name() {
    // 17, 18, D9.
    let mut old = entry("20260901-utc/a.jpg", "a.jpg");
    old.deleted_at_utc = "2026-09-01T10:00:00.000Z".to_string();
    old.group = "item:op1:a".to_string();
    let mut new = entry("20260927-utc/a.jpg", "a.jpg");
    new.group = "item:op2:a".to_string();
    let plan_both = plan(&[candidate(old.clone()), candidate(new.clone())], &[]);
    assert_eq!(target_of(&plan_both, "20260927-utc/a.jpg"), "/r/a.jpg");
    assert_eq!(target_of(&plan_both, "20260901-utc/a.jpg"), "/r/a 2.jpg");
    let plan_one = plan(&[candidate(old)], &[]);
    assert_eq!(target_of(&plan_one, "20260901-utc/a.jpg"), "/r/a.jpg");
}

#[test]
fn names_that_the_folder_treats_as_one_are_planned_as_one() {
    // 49, 50: case and Unicode normalization fold together on macOS-style
    // volumes, so the second selected file is renamed rather than failing.
    let mut first = entry("d/IMG.jpg", "IMG.jpg");
    first.group = "item:op:1".to_string();
    let mut second = entry("d/img.jpg", "img.jpg");
    second.group = "item:op:2".to_string();
    second.deleted_at_utc = "2026-09-26T10:00:00.000Z".to_string();
    let case_plan = plan(&[candidate(first), candidate(second)], &[]);
    assert_eq!(target_of(&case_plan, "d/img.jpg"), "/r/img 2.jpg");

    let mut composed = entry("d/café.jpg", "caf\u{e9}.jpg");
    composed.group = "item:op:3".to_string();
    let mut decomposed = entry("d/cafe\u{301}.jpg", "cafe\u{301}.jpg");
    decomposed.group = "item:op:4".to_string();
    decomposed.deleted_at_utc = "2026-09-26T10:00:00.000Z".to_string();
    let nfc_plan = plan(&[candidate(composed), candidate(decomposed)], &[]);
    assert!(target_of(&nfc_plan, "d/cafe\u{301}.jpg").ends_with(" 2.jpg"));

    // A case-sensitive folder without normalization keeps them apart.
    let mut a = entry("d/A.jpg", "A.jpg");
    a.group = "item:op:5".to_string();
    let mut b = entry("d/a.jpg", "a.jpg");
    b.group = "item:op:6".to_string();
    let apart = plan_restore(
        &[candidate(a), candidate(b)],
        FolderNames::new(false, false),
        RenameStyle::ParenthesizedNumber,
        &mut |_| true,
    );
    assert_eq!(target_of(&apart, "d/a.jpg"), "/r/a.jpg");
}

#[test]
fn unrestorable_entries_are_skipped_with_their_reason() {
    // 30, 32, 51, 65, 3, 4, 12.
    let mut cases = Vec::new();
    for (id, status) in [
        ("d/changed.jpg", EntryStatus::Changed),
        ("d/lossy.jpg", EntryStatus::Unrepresentable),
        ("d/excluded.jpg", EntryStatus::Excluded),
    ] {
        let mut e = entry(id, id.trim_start_matches("d/"));
        e.status = status;
        cases.push(candidate(e));
    }
    for (id, blocked) in [
        ("d/link.jpg", Skip::FolderIsLink),
        ("d/file.jpg", Skip::FileInTheWay),
        ("d/drive.jpg", Skip::OtherDrive),
    ] {
        let mut c = candidate(entry(id, id.trim_start_matches("d/")));
        c.blocked = Some(blocked);
        cases.push(c);
    }
    let plan = plan(&cases, &[]);
    let skips: Vec<_> = targets(&plan).into_iter().map(|(_, target)| target.unwrap()).collect();
    assert_eq!(
        skips,
        [
            "skip:Changed",
            "skip:Unrepresentable",
            "skip:Excluded",
            "skip:FolderIsLink",
            "skip:FileInTheWay",
            "skip:OtherDrive",
        ]
    );
    assert!(review_of(&plan, Path::new("/r"), &[]).needed(), "skips are reviewed");
}

#[test]
fn a_main_without_its_companion_notes_the_companion_stays() {
    // 21, 22, 24: either half restores alone to its own original name.
    let main = in_family(entry("d/x.jpg", "a/x.jpg"), "item:op:x", TrashRole::Main);
    let sidecar = in_family(entry("d/x.xmp", "a/x.xmp"), "item:op:x", TrashRole::Companion);
    let main_only = plan(&[candidate(main.clone())], &[]);
    let review = review_of(&main_only, Path::new("/r"), &[main.clone(), sidecar.clone()]);
    assert_eq!(review.companions_left, ["a/x.xmp"]);
    assert!(!review.needed(), "a note, not a reason to stop");

    let companion_only = plan(&[candidate(sidecar)], &[]);
    assert_eq!(target_of(&companion_only, "d/x.xmp"), "/r/a/x.xmp");
}

#[test]
fn a_displaced_destination_file_comes_back_renamed_beside_its_replacement() {
    // 27, D12: never swapped back.
    let mut displaced = entry("d/x.jpg", "out/x.jpg");
    displaced.kind = Some(TrashKind::OverwriteDisplaced);
    displaced.item = None;
    let plan = plan(&[with_state(candidate(displaced), TargetState::Occupied)], &[]);
    assert_eq!(target_of(&plan, "d/x.jpg"), "/r/out/x 2.jpg");
}

#[test]
fn the_token_changes_with_any_target_or_observation() {
    // 16: a path occupied since the review changes the plan and its token.
    let root = Path::new("/r");
    let free = plan(&[candidate(entry("d/a.jpg", "a.jpg"))], &[]);
    let taken = plan(&[with_state(candidate(entry("d/a.jpg", "a.jpg")), TargetState::Occupied)], &[]);
    assert_eq!(plan_token(root, &free), plan_token(root, &free));
    assert_ne!(plan_token(root, &free), plan_token(root, &taken));
    let mut resized = entry("d/a.jpg", "a.jpg");
    resized.size = 11;
    assert_ne!(plan_token(root, &free), plan_token(root, &plan(&[candidate(resized)], &[])));
}

// ---------------------------------------------------------------------------
// Execution over temporary trees

struct Fixture {
    dir: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    conn: rusqlite::Connection,
    settings: onecopy_lib::scanner::ScanSettings,
}

fn fixture(label: &str, indexed: bool) -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix(&format!("onecopy-restore-{label}-"))
        .tempdir()
        .unwrap();
    let root = dir.path().join("photos");
    let data = dir.path().join("apphome");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let config = serde_json::json!({ "sourceDirs": if indexed { vec![root.to_string_lossy()] } else { vec![] } });
    let settings = onecopy_lib::scanner::settings_from_config(Some(&config), &data, 0);
    let conn = onecopy_lib::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    Fixture {
        dir,
        root,
        data,
        conn,
        settings,
    }
}

#[test]
fn a_day_that_gains_newer_records_after_restore_preparation_stays_untouched() {
    let f = fixture("newer-after-preparation", false);
    let blocked = f.deleted("missing/a.jpg", b"blocked", "a", TrashRole::Main);
    let today = f.root.join(TRASH_DIR_NAME).join(blocked.split('/').next().unwrap());
    let old_day = f.root.join(TRASH_DIR_NAME).join("20000101-utc");
    std::fs::rename(today, &old_day).unwrap();
    std::fs::remove_dir(f.root.join("missing")).unwrap();
    let healthy = f.deleted("b.jpg", b"healthy", "b", TrashRole::Main);
    let (plan, _, _) = f.plan(&["20000101-utc/a.jpg".to_string(), healthy]);
    let manifest = old_day.join(onecopy_lib::trash::MANIFEST_FILE_NAME);
    let mut bytes = std::fs::read(&manifest).unwrap();
    bytes.extend_from_slice(b"{\"formatVersion\":2,\"storedName\":\"a.jpg\"}\n");
    std::fs::write(&manifest, &bytes).unwrap();

    let outcome = f.execute(&plan);

    assert_eq!(outcome.failed, 1);
    assert_eq!(outcome.restored.len(), 1);
    assert!(!f.root.join("missing").exists());
    assert_eq!(std::fs::read(old_day.join("a.jpg")).unwrap(), b"blocked");
    assert_eq!(std::fs::read(&manifest).unwrap(), bytes);
    assert_eq!(std::fs::read(f.root.join("b.jpg")).unwrap(), b"healthy");
}

#[test]
fn a_day_that_gains_newer_records_during_restore_volume_checks_is_not_renamed() {
    let f = fixture("newer-before-rename", false);
    let id = f.deleted("a.jpg", b"blocked", "a", TrashRole::Main);
    let (plan, _, _) = f.plan(&[id]);
    let stored = Path::new(&plan.steps[0].entry.stored_path);
    let manifest = stored.parent().unwrap().join(onecopy_lib::trash::MANIFEST_FILE_NAME);
    let mut bytes = std::fs::read(&manifest).unwrap();
    bytes.extend_from_slice(b"{\"formatVersion\":2}\n");
    let injected = std::cell::Cell::new(false);

    let (outcome, _) = execute(
        &f.conn, &f.root, &plan, &f.settings,
        &|path| {
            if !injected.replace(true) {
                std::fs::write(&manifest, &bytes).unwrap();
            }
            onecopy_lib::file_identity::volume_of(path)
        },
        &|| false, &mut |_| {},
    ).unwrap();

    assert!(injected.get());
    assert_eq!(outcome.failed, 1);
    assert!(outcome.restored.is_empty());
    assert!(!f.root.join("a.jpg").exists());
    assert_eq!(std::fs::read(stored).unwrap(), b"blocked");
    assert_eq!(std::fs::read(manifest).unwrap(), bytes);
}

impl Fixture {
    /// Writes `relative` with `bytes` and deletes it into this root's
    /// Deleted files as one operation's item; returns its entry id.
    fn deleted(&self, relative: &str, bytes: &[u8], item: &str, role: TrashRole) -> String {
        let file = self.root.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, bytes).unwrap();
        let context = TrashContext::new(TrashKind::Delete, "op")
            .item(Some(item.to_string()))
            .role(role);
        let record = trash_file(&file, &self.root, None, &context).unwrap();
        let stored = PathBuf::from(&record.stored_path);
        format!(
            "{}/{}",
            stored.parent().unwrap().file_name().unwrap().to_string_lossy(),
            record.stored_name
        )
    }

    fn plan(&self, ids: &[String]) -> (RestorePlan, RestoreReview, String) {
        let listing = list_root(&self.root, &self.data).unwrap();
        let candidates = candidates(
            &self.root,
            &listing,
            ids,
            &onecopy_lib::file_identity::volume_of,
            &|| false,
        )
        .unwrap();
        let plan = plan_restore(
            &candidates,
            FolderNames::for_directory(&self.root),
            RenameStyle::SpaceNumber,
            &mut |path| name_available(path),
        );
        let review = review_of(&plan, &self.root, &listing.entries);
        let token = plan_token(&self.root, &plan);
        (plan, review, token)
    }

    fn execute(&self, plan: &RestorePlan) -> RestoreOutcome {
        execute(
            &self.conn,
            &self.root,
            plan,
            &self.settings,
            &onecopy_lib::file_identity::volume_of,
            &|| false,
            &mut |_| {},
        )
        .unwrap()
        .0
    }

    fn restore(&self, ids: &[String]) -> RestoreOutcome {
        let (plan, _, _) = self.plan(ids);
        self.execute(&plan)
    }

    fn issue_keys(&self) -> Vec<(String, String)> {
        let mut statement = self
            .conn
            .prepare("SELECT kind, COALESCE(message_key, '') FROM active_issues ORDER BY id")
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn manifest_events(&self) -> usize {
        let trash = self.root.join(TRASH_DIR_NAME);
        std::fs::read_dir(&trash)
            .unwrap()
            .map(|day| std::fs::read_to_string(day.unwrap().path().join("manifest.jsonl")).unwrap_or_default())
            .map(|text| text.lines().filter(|line| line.contains("\"event\":\"restored\"")).count())
            .sum()
    }
}

#[test]
fn a_file_returns_to_a_recreated_folder_and_leaves_a_restored_line() {
    let f = fixture("recreate", false);
    let id = f.deleted("2016/spain/beach.jpg", b"beach", "i1", TrashRole::Main);
    std::fs::remove_dir_all(f.root.join("2016")).unwrap();

    let (plan, review, _) = f.plan(std::slice::from_ref(&id));
    assert_eq!(review.folders, ["2016", "2016/spain"]);
    let outcome = f.execute(&plan);
    assert_eq!(outcome.restored.len(), 1);
    assert_eq!(std::fs::read(f.root.join("2016/spain/beach.jpg")).unwrap(), b"beach");
    assert_eq!(f.manifest_events(), 1);
    assert!(list_root(&f.root, &f.data).unwrap().entries.is_empty(), "no longer listed");
}

#[test]
fn an_occupied_path_is_never_replaced() {
    let f = fixture("occupied", false);
    let id = f.deleted("a.jpg", b"deleted version", "i1", TrashRole::Main);
    std::fs::write(f.root.join("a.jpg"), b"new file there").unwrap();
    let outcome = f.restore(&[id]);
    assert_eq!(outcome.restored, [f.root.join("a 2.jpg").to_string_lossy().into_owned()]);
    assert_eq!(std::fs::read(f.root.join("a.jpg")).unwrap(), b"new file there");
    assert_eq!(std::fs::read(f.root.join("a 2.jpg")).unwrap(), b"deleted version");
}

#[test]
fn identical_bytes_stay_in_deleted_files_as_already_there() {
    // 13, 64: zero bytes too.
    let f = fixture("identical", false);
    let same = f.deleted("same.jpg", b"same", "i1", TrashRole::Main);
    std::fs::write(f.root.join("same.jpg"), b"same").unwrap();
    let empty = f.deleted("empty.txt", b"", "i2", TrashRole::Main);
    std::fs::write(f.root.join("empty.txt"), b"").unwrap();
    let outcome = f.restore(&[same, empty]);
    assert_eq!(outcome.already_present, 2);
    assert!(outcome.restored.is_empty());
    assert_eq!(outcome.failed, 0);
    assert_eq!(list_root(&f.root, &f.data).unwrap().entries.len(), 2, "both stay");
    assert!(f.issue_keys().is_empty(), "already there is not a failure");
}

#[test]
fn a_zero_byte_file_restores_beside_a_different_empty_folder() {
    // 64 with 15: a folder at the path is not the same bytes.
    let f = fixture("zero-byte", false);
    let id = f.deleted("empty.txt", b"", "i1", TrashRole::Main);
    std::fs::create_dir(f.root.join("empty.txt")).unwrap();
    let outcome = f.restore(&[id]);
    assert_eq!(outcome.restored, [f.root.join("empty 2.txt").to_string_lossy().into_owned()]);
}

#[test]
fn a_family_comes_back_under_one_suffix() {
    // 20, 23 end to end.
    let f = fixture("family", false);
    let main = f.deleted("a/x.jpg", b"image", "item", TrashRole::Main);
    let sidecar = f.deleted("a/x.xmp", b"sidecar", "item", TrashRole::Companion);
    let copy = f.deleted("b/x.jpg", b"image", "item", TrashRole::Main);
    std::fs::write(f.root.join("a/x.jpg"), b"someone else").unwrap();
    let outcome = f.restore(&[sidecar, main, copy]);
    assert_eq!(outcome.restored.len(), 3);
    assert!(f.root.join("a/x 2.jpg").exists());
    assert!(f.root.join("a/x 2.xmp").exists());
    assert!(f.root.join("b/x.jpg").exists());
}

#[cfg(unix)]
#[test]
fn a_companion_restored_after_its_main_came_back_renamed_says_it_will_not_pair() {
    // 24: the companion returns under its own original name, and the review
    // says so, naming where its main file is.
    let f = fixture("companion-after-main", false);
    let main = f.deleted("trip/IMG.jpg", b"main", "i1", TrashRole::Main);
    let companion = f.deleted("trip/IMG.xmp", b"sidecar", "i1", TrashRole::Companion);
    std::fs::write(f.root.join("trip/IMG.jpg"), b"a different photo").unwrap();
    let outcome = f.restore(std::slice::from_ref(&main));
    assert_eq!(outcome.restored.len(), 1);
    assert!(f.root.join("trip/IMG 2.jpg").exists());

    let (plan, review, _) = f.plan(std::slice::from_ref(&companion));
    let file = &review.files[0];
    assert_eq!(file.target.as_deref(), Some("trip/IMG.xmp"));
    assert!(!file.renamed);
    assert_eq!(file.main_restored_as.as_deref(), Some("trip/IMG 2.jpg"));
    assert!(review.needed(), "the review states it before anything moves");
    f.execute(&plan);
    assert!(f.root.join("trip/IMG.xmp").exists());
}

#[test]
fn a_companion_restored_after_its_main_came_back_unchanged_needs_no_review() {
    let f = fixture("companion-after-main-home", false);
    let main = f.deleted("trip/IMG.jpg", b"main", "i1", TrashRole::Main);
    let companion = f.deleted("trip/IMG.xmp", b"sidecar", "i1", TrashRole::Companion);
    f.restore(std::slice::from_ref(&main));

    let (_, review, _) = f.plan(std::slice::from_ref(&companion));
    assert_eq!(review.files[0].main_restored_as, None);
    assert!(!review.needed());
}

#[cfg(unix)]
#[test]
fn a_folder_that_became_a_link_is_refused_and_never_followed() {
    // 3.
    let f = fixture("link", false);
    let id = f.deleted("album/a.jpg", b"a", "i1", TrashRole::Main);
    let elsewhere = f.dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::remove_dir(f.root.join("album")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.root.join("album")).unwrap();
    let outcome = f.restore(&[id]);
    assert_eq!(outcome.failed, 1);
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none(), "nothing went through the link");
    assert_eq!(f.issue_keys(), [("restore-error".to_string(), "notice.restoreFolderBlocked".to_string())]);
}

#[cfg(windows)]
#[test]
fn a_folder_that_became_a_junction_is_refused_and_never_followed() {
    // 3, with the link Windows makes without privilege.
    let f = fixture("junction", false);
    let id = f.deleted("album/a.jpg", b"a", "i1", TrashRole::Main);
    let elsewhere = f.dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::remove_dir(f.root.join("album")).unwrap();
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(f.root.join("album"))
        .arg(&elsewhere)
        .output()
        .unwrap();
    assert!(made.status.success(), "mklink /J failed: {}", String::from_utf8_lossy(&made.stderr));
    let outcome = f.restore(&[id]);
    assert_eq!(outcome.failed, 1);
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none(), "nothing went through the junction");
    assert_eq!(f.issue_keys(), [("restore-error".to_string(), "notice.restoreFolderBlocked".to_string())]);
}

#[test]
fn a_file_in_the_way_of_a_folder_fails_that_file_only() {
    // 4.
    let f = fixture("file-in-way", false);
    let blocked = f.deleted("album/a.jpg", b"a", "i1", TrashRole::Main);
    let fine = f.deleted("b.jpg", b"b", "i2", TrashRole::Main);
    std::fs::remove_dir(f.root.join("album")).unwrap();
    std::fs::write(f.root.join("album"), b"not a folder").unwrap();
    let outcome = f.restore(&[blocked, fine]);
    assert_eq!((outcome.failed, outcome.restored.len()), (1, 1));
    assert!(f.root.join("b.jpg").exists());
}

#[test]
fn a_different_drive_at_the_original_path_fails_that_file() {
    // 12, 53: never a cross-volume copy (the volume answer is the seam).
    let f = fixture("other-drive", false);
    let id = f.deleted("mounted/a.jpg", b"a", "i1", TrashRole::Main);
    let mount = f.root.join("mounted");
    let listing = list_root(&f.root, &f.data).unwrap();
    let volume = |path: &Path| -> std::io::Result<u64> {
        Ok(if path.starts_with(&mount) { 2 } else { 1 })
    };
    let observed = candidates(&f.root, &listing, &[id.clone()], &volume, &|| false).unwrap();
    assert_eq!(observed[0].blocked, Some(Skip::OtherDrive));
    // A drive mounted there after the review: execution refuses too.
    let (plan, _, _) = f.plan(&[id]);
    let (outcome, _) = execute(&f.conn, &f.root, &plan, &f.settings, &volume, &|| false, &mut |_| {}).unwrap();
    assert_eq!(outcome.failed, 1);
    assert!(!mount.join("a.jpg").exists());
}

#[test]
fn a_stored_file_changed_after_the_review_is_refused() {
    // 30 at execution time.
    let f = fixture("changed-late", false);
    let id = f.deleted("a.jpg", b"original", "i1", TrashRole::Main);
    let (plan, _, _) = f.plan(std::slice::from_ref(&id));
    let stored = f.root.join(TRASH_DIR_NAME).join(&id);
    std::fs::write(&stored, b"edited in the trash").unwrap();
    let outcome = f.execute(&plan);
    assert_eq!(outcome.failed, 1);
    assert!(!f.root.join("a.jpg").exists());
    assert_eq!(f.issue_keys(), [("restore-error".to_string(), "notice.restoreChanged".to_string())]);
}

#[test]
fn a_path_occupied_after_the_review_fails_that_file_and_is_never_renamed_silently() {
    // 16 during execution.
    let f = fixture("occupied-late", false);
    let id = f.deleted("a.jpg", b"deleted", "i1", TrashRole::Main);
    let (plan, _, token) = f.plan(std::slice::from_ref(&id));
    std::fs::write(f.root.join("a.jpg"), b"arrived meanwhile").unwrap();
    // Confirming would see a different plan...
    assert_ne!(f.plan(std::slice::from_ref(&id)).2, token);
    // ...and the frozen plan itself never replaces or renames.
    let outcome = f.execute(&plan);
    assert_eq!(outcome.failed, 1);
    assert_eq!(std::fs::read(f.root.join("a.jpg")).unwrap(), b"arrived meanwhile");
    assert!(!f.root.join("a 2.jpg").exists());
    assert_eq!(f.issue_keys(), [("restore-error".to_string(), "notice.restoreOccupied".to_string())]);
}

#[test]
fn a_stored_file_taken_by_someone_else_fails_as_missing() {
    // 47, 48: another process or the user moved it first.
    let f = fixture("taken", false);
    let id = f.deleted("a.jpg", b"a", "i1", TrashRole::Main);
    let (plan, _, _) = f.plan(std::slice::from_ref(&id));
    std::fs::rename(f.root.join(TRASH_DIR_NAME).join(&id), f.dir.path().join("taken.jpg")).unwrap();
    let outcome = f.execute(&plan);
    assert_eq!(outcome.failed, 1);
    assert_eq!(f.issue_keys(), [("restore-error".to_string(), "notice.restoreMissing".to_string())]);
}

#[test]
fn an_id_no_longer_listed_is_skipped_as_missing() {
    // 29.
    let f = fixture("gone", false);
    let (plan, review, _) = f.plan(&["20260101-utc/nothing.jpg".to_string()]);
    assert_eq!(review.files[0].skip, Some(Skip::Missing));
    let outcome = f.execute(&plan);
    assert_eq!(outcome.failed, 1);
}

#[cfg(unix)]
#[test]
fn a_folder_that_refuses_writes_fails_its_file_and_others_continue() {
    // 55: permission denied on one target folder.
    use std::os::unix::fs::PermissionsExt;
    let f = fixture("read-only-folder", false);
    let locked = f.deleted("locked/a.jpg", b"a", "i1", TrashRole::Main);
    let open = f.deleted("open/b.jpg", b"b", "i2", TrashRole::Main);
    std::fs::set_permissions(f.root.join("locked"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let outcome = f.restore(&[locked, open]);
    std::fs::set_permissions(f.root.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!((outcome.failed, outcome.restored.len()), (1, 1));
    assert!(outcome.error.is_none(), "one folder's refusal does not stop the rest");
    assert_eq!(f.issue_keys(), [("restore-error".to_string(), "notice.restoreFailed".to_string())]);
}

#[test]
fn cancel_between_files_leaves_an_exact_partial_result() {
    // 57, 58.
    let f = fixture("cancel", false);
    let ids: Vec<String> = (0..4)
        .map(|n| f.deleted(&format!("f{n}.jpg"), b"x", &format!("i{n}"), TrashRole::Main))
        .collect();
    let (plan, _, _) = f.plan(&ids);
    let done = std::cell::Cell::new(0u64);
    let (outcome, _) = execute(
        &f.conn,
        &f.root,
        &plan,
        &f.settings,
        &onecopy_lib::file_identity::volume_of,
        &|| done.get() >= 2,
        &mut |progress| done.set(progress.files_done),
    )
    .unwrap();
    assert!(outcome.cancelled);
    assert_eq!((outcome.restored.len(), outcome.unstarted), (2, 2));
    assert_eq!(list_root(&f.root, &f.data).unwrap().entries.len(), 2);
}

#[test]
fn cancelling_while_planning_does_no_filesystem_work() {
    // 58: nothing observed, nothing moved.
    let f = fixture("cancel-planning", false);
    let id = f.deleted("a.jpg", b"a", "i1", TrashRole::Main);
    let listing = list_root(&f.root, &f.data).unwrap();
    let result = candidates(&f.root, &listing, &[id], &onecopy_lib::file_identity::volume_of, &|| true);
    assert_eq!(result.unwrap_err(), onecopy_lib::scanner::CANCELLED);
    assert!(!f.root.join("a.jpg").exists());
}

#[test]
fn a_restored_line_never_breaks_emptying() {
    // 44: Empty measured before a restore removes nothing afterwards.
    let f = fixture("empty-after", false);
    let a = f.deleted("a.jpg", b"a", "i1", TrashRole::Main);
    f.deleted("b.jpg", b"b", "i2", TrashRole::Main);
    let before = onecopy_lib::trash::overview(std::slice::from_ref(&f.root));
    f.restore(&[a]);
    let outcome = onecopy_lib::trash::empty_root_with_progress(
        &f.root.join(TRASH_DIR_NAME),
        &before[0].plan_token,
        &std::sync::atomic::AtomicBool::new(false),
        &|_| {},
        &|_, _| Ok(()),
    )
    .unwrap();
    assert!(outcome.plan_changed);
    let after = onecopy_lib::trash::overview(std::slice::from_ref(&f.root));
    assert_eq!(after[0].files, 1);
}

// ---------------------------------------------------------------------------
// The library

fn rows(f: &Fixture, name: &str) -> i64 {
    f.conn
        .query_row("SELECT COUNT(*) FROM paths WHERE file_name = ?1 AND missing = 0", [name], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_restored_source_file_is_indexed_without_the_watcher() {
    // 2.3: the restore re-reads the folder itself.
    let f = fixture("index", true);
    let id = f.deleted("trip/a.jpg", b"image-bytes", "i1", TrashRole::Main);
    assert_eq!(rows(&f, "a.jpg"), 0);
    f.restore(&[id]);
    assert_eq!(rows(&f, "a.jpg"), 1);
}

#[test]
fn a_restored_companion_pairs_again_by_the_ordinary_rule() {
    // 25, 22.
    let f = fixture("pair", true);
    std::fs::create_dir_all(f.root.join("trip")).unwrap();
    std::fs::write(f.root.join("trip/x.jpg"), b"image").unwrap();
    let sidecar = f.deleted("trip/x.xmp", b"sidecar", "i1", TrashRole::Companion);
    f.restore(&[sidecar]);
    onecopy_lib::scanner::pair_companions(&f.conn, true).unwrap();
    let paired: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM paths WHERE file_name = 'x.xmp' AND companion_of IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(paired, 1);
}

#[test]
fn identical_content_elsewhere_becomes_a_copy_of_that_item_again() {
    // 62, D10: no warning; hashing merges them.
    let f = fixture("duplicate", true);
    std::fs::create_dir_all(f.root.join("keep")).unwrap();
    std::fs::write(f.root.join("keep/x.jpg"), b"same-bytes").unwrap();
    let id = f.deleted("gone/x.jpg", b"same-bytes", "i1", TrashRole::Main);
    onecopy_lib::scanner::walk_root(&f.conn, &f.root, &f.settings.lists).unwrap();
    f.restore(&[id]);
    let cache = onecopy_lib::preview::CachePaths::new(f.dir.path().join("cache"));
    onecopy_lib::scanner::hash_pending(&f.conn, &cache).unwrap();
    let hashes: Vec<Option<String>> = f
        .conn
        .prepare("SELECT content_hash FROM paths WHERE file_name = 'x.jpg' AND missing = 0")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(hashes.len(), 2);
    assert!(hashes[0].is_some() && hashes[0] == hashes[1], "{hashes:?}");
}

#[test]
fn a_destination_restore_adds_no_row() {
    // 2.3, 28: destinations are not indexed.
    let f = fixture("destination", false);
    let id = f.deleted("out/x.jpg", b"x", "i1", TrashRole::Main);
    f.restore(&[id]);
    assert!(f.root.join("out/x.jpg").exists());
    assert_eq!(rows(&f, "x.jpg"), 0);
}

// ---------------------------------------------------------------------------
// Drives that stop answering, and quitting

const STALL_BOUND: std::time::Duration = std::time::Duration::from_millis(300);
const SETTLE: std::time::Duration = std::time::Duration::from_secs(10);

fn restore_with_rename_given_up_on(label: &str, land: bool) {
    use onecopy_lib::volume_io::{FakeStallingVolume, Op};
    let f = fixture(label, false);
    let first = f.deleted("a.jpg", b"first", "i1", TrashRole::Main);
    let second = f.deleted("b.jpg", b"second", "i2", TrashRole::Main);
    let (plan, _, _) = f.plan(&[first, second]);
    let volume = FakeStallingVolume::mount(&f.root, STALL_BOUND);
    volume.stall(&[Op::Rename], None);
    // 59: quitting has already asked to cancel; the rename in progress is
    // allowed its bound, never interrupted, and nothing after it starts.
    let outcome = execute(
        &f.conn,
        &f.root,
        &plan,
        &f.settings,
        &onecopy_lib::file_identity::volume_of,
        &|| false,
        &mut |_| {},
    )
    .unwrap()
    .0;
    assert_eq!(outcome.unknown, 1);
    assert_eq!(outcome.failed, 1);
    assert_eq!(outcome.unstarted, 1, "nothing further goes to a drive that stopped answering");
    assert!(outcome.error.is_some());
    assert_eq!(
        f.issue_keys(),
        [("restore-outcome-unknown".to_string(), "notice.restoreOutcomeUnknown".to_string())]
    );
    if land {
        volume.release();
    } else {
        volume.fail();
    }
    assert!(volume.wait_until_settled(SETTLE));
    // Newest deletion first: the step given up on is the plan's first.
    let Action::Restore { target, .. } = &plan.steps[0].action else {
        panic!("a restore step")
    };
    let stored = f.root.join(TRASH_DIR_NAME).join(&plan.steps[0].entry.id);
    let bytes = std::fs::read(if land { target } else { &stored }).unwrap();
    // Either in Deleted files or at its target, never both, never partial.
    assert_eq!(stored.exists(), !land);
    assert_eq!(target.exists(), land);
    assert!(bytes == b"first" || bytes == b"second", "never partial");
}

#[test]
fn a_restore_rename_given_up_on_that_lands_later_is_at_its_target() {
    restore_with_rename_given_up_on("rename-lands", true);
}

#[test]
fn a_restore_rename_given_up_on_that_fails_later_is_still_in_deleted_files() {
    restore_with_rename_given_up_on("rename-fails", false);
}

#[test]
fn a_drive_that_stops_answering_before_the_move_says_the_file_is_still_in_deleted_files() {
    // 6: the stop is the drive's, not the file's: the Issue must not claim
    // the file left Deleted files or that something blocks its folder.
    use onecopy_lib::volume_io::{FakeStallingVolume, Op};
    let f = fixture("stat-given-up", false);
    let first = f.deleted("a.jpg", b"first", "i1", TrashRole::Main);
    let second = f.deleted("b.jpg", b"second", "i2", TrashRole::Main);
    let (plan, _, _) = f.plan(&[first, second]);
    let volume = FakeStallingVolume::mount(&f.root, STALL_BOUND);
    volume.stall(&[Op::Stat], Some(&f.root.join(TRASH_DIR_NAME)));
    let outcome = f.execute(&plan);
    volume.release();
    assert!(volume.wait_until_settled(SETTLE));
    assert_eq!((outcome.failed, outcome.unknown, outcome.unstarted), (1, 0, 1));
    assert!(outcome.error.is_some());
    assert_eq!(
        f.issue_keys(),
        [("restore-error".to_string(), "notice.restoreFailed".to_string())]
    );
    assert_eq!(list_root(&f.root, &f.data).unwrap().entries.len(), 2, "nothing moved");
}

#[test]
fn only_a_configured_root_can_be_restored_into() {
    // 5, 8: the location must be one the settings name now.
    let roots = [PathBuf::from("/Volumes/A/Photos")];
    assert!(onecopy_lib::trash::owning_root_of(&roots, Path::new("/Volumes/A/Photos/.onecopy-trash")).is_ok());
    assert!(onecopy_lib::trash::owning_root_of(&roots, Path::new("/Volumes/B/.onecopy-trash")).is_err());
}

#[test]
fn a_hidden_name_restores_as_an_ordinary_file() {
    // 63: visibility is decided by the library afterwards, not by Restore.
    let f = fixture("hidden", true);
    let id = f.deleted(".secret.jpg", b"s", "i1", TrashRole::Main);
    let outcome = f.restore(&[id]);
    assert_eq!(outcome.restored.len(), 1);
    assert!(f.root.join(".secret.jpg").exists());
}

#[test]
fn ten_thousand_files_plan_quickly() {
    // 61: planning is linear in the selection.
    let candidates: Vec<Candidate> = (0..10_000)
        .map(|n| candidate(entry(&format!("d/f{n}.jpg"), &format!("album{}/f{n}.jpg", n % 50))))
        .collect();
    let started = std::time::Instant::now();
    let plan = plan(&candidates, &[]);
    assert_eq!(plan.steps.len(), 10_000);
    assert!(started.elapsed() < std::time::Duration::from_secs(3), "{:?}", started.elapsed());
}

// A process that stops after Restore's rename but before its "restored" line:
// the entry no longer lists, a new plan skips it as missing, and the next scan
// indexes the returned file.
#[test]
fn a_stop_after_the_restore_rename_lists_nothing_and_the_scan_finds_the_file() {
    let f = fixture("stop-after-rename", false);
    let id = f.deleted("a.jpg", b"returned", "i1", TrashRole::Main);
    std::fs::rename(f.root.join(TRASH_DIR_NAME).join(&id), f.root.join("a.jpg")).unwrap();

    assert!(list_root(&f.root, &f.data).unwrap().entries.is_empty());
    let (_, review, _) = f.plan(std::slice::from_ref(&id));
    assert_eq!(review.files[0].skip, Some(Skip::Missing));
    assert_eq!(f.manifest_events(), 0);
    onecopy_lib::scanner::walk_root(&f.conn, &f.root, &f.settings.lists).unwrap();
    let indexed: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM paths WHERE file_name = 'a.jpg' AND missing = 0", [], |r| r.get(0))
        .unwrap();
    assert_eq!(indexed, 1);
}
