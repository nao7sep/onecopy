// The generalized managed-dependency registry (Phase 28's foundation).
//
// The contract that matters: N entries share one facts file WITHOUT sharing
// fate — one entry's install, check, or corruption can never touch another's
// facts — and a model's presence check is size-exact, so a truncated download
// that somehow reached the models directory reads not-installed rather than
// installed-broken.

use onecopy_lib::binaries::{BinaryFacts, BinaryStatus};
use onecopy_lib::binaries_manager::*;
use onecopy_lib::paths::MODELS_DIR_NAME;

fn home(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("onecopy-deps-{label}-"))
        .tempdir()
        .unwrap()
}

#[test]
fn the_registry_carries_ffmpeg_and_the_whisper_model() {
    // Display order is registry order; adding an entry is the whole
    // registration, so this doubles as the "declare it deliberately" pin the
    // schema test uses for tables.
    let ids: Vec<&str> = DEPENDENCIES.iter().map(|d| d.id).collect();
    assert_eq!(
        ids,
        [
            "ffmpeg",
            "siglip2-large-vision",
            "whisper-large-v3-turbo",
            "ultraface-rfb640",
            "hsemotion-enet-b2",
        ]
    );

    let embedding = spec_of("siglip2-large-vision").unwrap();
    let embedding_pin = embedding.pinned.as_ref().unwrap();
    assert!(embedding_pin.url.starts_with("https://"));
    assert_eq!(embedding_pin.sha256.len(), 64);
    assert!(
        embedding_pin.url.contains("vision_model"),
        "the VISION tower alone — the text tower and the combined model are \
         megabytes this app would never run"
    );

    let whisper = spec_of("whisper-large-v3-turbo").unwrap();
    let pinned = whisper.pinned.as_ref().expect("models carry a pin");
    assert!(pinned.url.starts_with("https://"));
    assert_eq!(pinned.sha256.len(), 64);
    assert!(pinned.bytes > 1_000_000_000, "large-v3-turbo is ~1.6 GB");

    // The face pair: every model entry carries a complete pin, and the two
    // stay distinct artifacts (the score needs BOTH installed).
    for id in ["ultraface-rfb640", "hsemotion-enet-b2"] {
        let pin = spec_of(id).unwrap().pinned.as_ref().expect("models carry a pin");
        assert!(pin.url.starts_with("https://"));
        assert_eq!(pin.sha256.len(), 64);
        assert!(pin.bytes > 1_000_000);
    }

    // Every model states when its artifact was PUBLISHED — the only honest
    // answer to "how old is this?", and the thing that made these two face
    // models' age visible enough to act on.
    for spec in DEPENDENCIES.iter().filter(|d| d.pinned.is_some()) {
        let released = spec.pinned.as_ref().unwrap().released;
        assert!(
            released.len() == 10 && released.starts_with("20"),
            "{} carries an ISO release date, got {released:?}",
            spec.id
        );
    }
}

#[test]
#[serial_test::serial(backup_store)]
fn entries_share_the_facts_file_without_sharing_fate() {
    let dir = home("fate");
    let ffmpeg = BinaryFacts {
        installed_version: Some("9.0".into()),
        latest_known_version: Some("9.0".into()),
        last_checked_at_utc: Some("2026-08-01T00:00:00.000Z".into()),
    };
    let whisper = BinaryFacts {
        installed_version: Some("1fc70f774d38".into()),
        latest_known_version: Some("1fc70f774d38".into()),
        last_checked_at_utc: Some("2026-08-02T00:00:00.000Z".into()),
    };
    save_facts_for(dir.path(), "ffmpeg", &ffmpeg).unwrap();
    save_facts_for(dir.path(), "whisper-large-v3-turbo", &whisper).unwrap();

    // Both persist independently through the one file.
    assert_eq!(load_facts_for(dir.path(), "ffmpeg"), ffmpeg);
    assert_eq!(load_facts_for(dir.path(), "whisper-large-v3-turbo"), whisper);

    // Updating one leaves the other byte-for-byte alone.
    let newer = BinaryFacts {
        latest_known_version: Some("9.1".into()),
        ..ffmpeg.clone()
    };
    save_facts_for(dir.path(), "ffmpeg", &newer).unwrap();
    assert_eq!(load_facts_for(dir.path(), "whisper-large-v3-turbo"), whisper);

    // The historical `{"ffmpeg": …}` reader still answers.
    assert_eq!(load_facts(dir.path()), newer);
}

#[test]
fn a_corrupt_facts_file_self_heals_per_entry() {
    let dir = home("corrupt");
    std::fs::write(dir.path().join(DEPENDENCIES_FILE_NAME), b"{ not json").unwrap();
    assert_eq!(load_facts_for(dir.path(), "ffmpeg"), BinaryFacts::default());
    assert_eq!(
        load_facts_for(dir.path(), "whisper-large-v3-turbo"),
        BinaryFacts::default()
    );
}

#[test]
fn a_models_presence_check_is_size_exact() {
    // A truncated model that somehow reached models/ must read NOT-INSTALLED:
    // whisper.cpp would reject or garble it, and "installed but broken" is
    // exactly the state the conventions' honest-state rule forbids showing.
    let dir = home("truncated");
    let spec = spec_of("whisper-large-v3-turbo").unwrap();
    let target = installed_path(dir.path(), spec);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"way too short to be a model").unwrap();

    let state = state_of(dir.path(), spec);
    assert_eq!(state.status, BinaryStatus::NotInstalled);
}

#[test]
#[serial_test::serial(backup_store)]
fn a_repinned_model_surfaces_as_update_available_with_no_check_at_all() {
    // A model has no upstream to ask: "latest" is the pin compiled into this
    // build. So `state_of` DERIVES it, an app update that re-pins shows
    // update-available on its own, and `check_entry` refuses the entry
    // outright rather than stamping a lookup that never happened.
    let dir = home("repin");
    let spec = spec_of("whisper-large-v3-turbo").unwrap();

    // Simulate an install under an OLDER pin — including a stale "latest",
    // which the derivation must override rather than trust.
    let old = BinaryFacts {
        installed_version: Some("aaaaaaaaaaaa".into()),
        latest_known_version: Some("aaaaaaaaaaaa".into()),
        last_checked_at_utc: Some("2026-08-01T00:00:00.000Z".into()),
    };
    save_facts_for(dir.path(), spec.id, &old).unwrap();
    // Present on disk at the pinned size, so status derivation sees installed.
    let target = installed_path(dir.path(), spec);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&target).unwrap();
    file.set_len(spec.pinned.as_ref().unwrap().bytes).unwrap();

    let state = state_of(dir.path(), spec);
    assert_eq!(state.status, BinaryStatus::UpdateAvailable, "no check needed");
    assert_eq!(
        state.facts.latest_known_version.as_deref(),
        Some(&spec.pinned.as_ref().unwrap().sha256[..12]),
        "latest is derived from the pin, not from stored facts"
    );
    assert!(!state.checkable, "a model has no upstream to ask");
    assert!(spec_of("ffmpeg").is_some_and(|_| state_of(dir.path(), spec_of("ffmpeg").unwrap()).checkable));

    let refused = check_entry(dir.path(), spec.id).expect_err("models cannot be checked");
    assert!(refused.contains("ships with the app"), "honest refusal, got: {refused}");
    // And nothing was written: no fake "checked at" stamp appeared.
    assert_eq!(
        load_facts_for(dir.path(), spec.id).last_checked_at_utc.as_deref(),
        Some("2026-08-01T00:00:00.000Z"),
        "a refused check writes nothing"
    );
}

#[test]
fn installed_paths_split_binaries_from_models() {
    let dir = home("paths");
    let ffmpeg = installed_path(dir.path(), spec_of("ffmpeg").unwrap());
    let model = installed_path(dir.path(), spec_of("whisper-large-v3-turbo").unwrap());
    assert!(ffmpeg.starts_with(dir.path().join(BIN_DIR_NAME)));
    assert!(model.starts_with(dir.path().join(MODELS_DIR_NAME)));
    assert_ne!(ffmpeg, model);
}
