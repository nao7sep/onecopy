// The names the core sends and the frontend keys on, checked where both sides
// meet: this fixture. This side proves the fixture says what the core sends;
// the frontend suite proves the frontend handles what the fixture says. A
// rename on either side then fails one of the two, instead of compiling clean
// and surfacing as a surface that never opens or a state nothing renders.

use serde_json::{json, Value};

fn fixture() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/core-frontend-names.json");
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

const REGENERATE: &str = "update tests/fixtures/core-frontend-names.json and the frontend that reads it";

#[test]
fn menu_items_send_the_events_the_fixture_names() {
    let surface = |id| crate::menu::main_surface_event(id).expect("the item opens a Main surface");
    assert_eq!(
        fixture()["menuEvents"],
        json!({
            "openSettings": surface(crate::menu::OPEN_SETTINGS_MENU_ID),
            "openAbout": surface(crate::menu::OPEN_ABOUT_MENU_ID),
        }),
        "{REGENERATE}"
    );
}

#[test]
fn item_keys_are_spelled_and_parsed_as_the_fixture_spells_them() {
    let examples = fixture()["itemKeys"].as_array().unwrap().clone();
    assert!(examples.iter().any(|example| example["hash"].is_null()), "a path-keyed example");
    assert!(examples.iter().any(|example| example["hash"].is_string()), "a hash-keyed example");
    for example in examples {
        let hash = example["hash"].as_str();
        let path_id = example["pathId"].as_i64().unwrap();
        let key = example["key"].as_str().unwrap();
        assert_eq!(crate::indexed_file::item_key(hash, path_id), key, "{REGENERATE}");
        let parsed = crate::indexed_file::parse_item_key(key).unwrap();
        assert_eq!(parsed, if hash.is_some() { (hash, None) } else { (None, Some(path_id)) });
    }
}

#[test]
fn background_work_rows_use_the_ids_states_and_reasons_the_fixture_names() {
    let ids: Vec<&str> = crate::derived_state::WorkClass::ALL.into_iter().map(|class| class.id()).collect();
    let states = serde_json::to_value(crate::background_work::ClassState::ALL).unwrap();
    assert_eq!(
        fixture()["backgroundWork"],
        json!({
            "classIds": ids,
            "classStates": states,
            "reasons": crate::derived_state::WORK_REASONS,
        }),
        "{REGENERATE}"
    );
}

#[test]
fn transcripts_report_the_statuses_and_events_the_fixture_names() {
    use crate::derived_state::{FAILED, PENDING, READY};
    use crate::derived_work::{TRANSCRIBE_CANCELLED, TRANSCRIBE_DONE, TRANSCRIBE_ERROR, TRANSCRIBE_PROGRESS};
    let fixture = fixture();
    assert_eq!(
        fixture["transcriptStatus"],
        json!({ "pending": PENDING, "ready": READY, "failed": FAILED }),
        "{REGENERATE}"
    );
    assert_eq!(
        fixture["transcribeEvents"],
        json!({
            "progress": TRANSCRIBE_PROGRESS,
            "done": TRANSCRIBE_DONE,
            "error": TRANSCRIBE_ERROR,
            "cancelled": TRANSCRIBE_CANCELLED,
        }),
        "{REGENERATE}"
    );
}
