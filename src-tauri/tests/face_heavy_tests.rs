// Face scoring on the production models: every fictional face in the shared
// corpus is found and scored, and a scene without a face is not.

use onecopy_lib::{face, preview};

use super::heavy_support::{corpus_file, corpus_json, library};

const FACELESS: &str = "jpeg-baseline.jpg";

#[test]
#[ignore = "heavy: real managed tools, models and the shared corpus; run by npm run test:full"]
#[serial_test::serial(heavy)]
fn production_models_score_every_fictional_face_and_no_faceless_scene() {
    let identities = corpus_json("photos/faces/identities.json");
    let faces: Vec<String> = identities["identities"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|identity| ["reference", "variation"].map(|key| identity[key].as_str().unwrap().to_string()))
        .collect();
    let mut files: Vec<_> = faces
        .iter()
        .map(|name| corpus_file(&format!("photos/faces/{name}")))
        .collect();
    files.push(corpus_file(&format!("formats/image/{FACELESS}")));
    let library = library("faces", &files, serde_json::json!({ "scoreFaces": true }));

    let derived = preview::derive_images_pending(
        &library.conn,
        &library.cache,
        library.settings.thumb_edge,
        library.settings.preview_long_edge,
        Some(library.ffmpeg()),
        None,
    )
    .unwrap();
    assert_eq!((derived.derived, derived.failed), (files.len() as u64, 0));

    let models = library
        .settings
        .face_models
        .clone()
        .expect("the face models are installed in the app home");
    let (mut scored, mut failed, mut after) = (0, 0, None);
    loop {
        let stats = face::face_scores_pending(
            &library.conn,
            &library.cache,
            Some((models.runtime.as_deref(), &models.detector, &models.emotion)),
            &[],
            |_| {},
            |_| {},
            |_, _| {},
            after.as_deref(),
            &|| false,
        )
        .unwrap();
        scored += stats.scored;
        failed += stats.failed;
        if !stats.candidates_found {
            break;
        }
        after = stats.last_attempted_hash;
    }
    assert_eq!((scored, failed), (files.len() as u64, 0));

    let score = |name: &str| -> f64 {
        library
            .conn
            .query_row(
                "SELECT face_score FROM contents WHERE hash = ?1",
                [library.hash_of(name)],
                |row| row.get(0),
            )
            .unwrap()
    };
    for name in &faces {
        let value = score(name);
        assert!(value > 0.0 && value <= 1.0, "{name} holds a face, scored {value}");
    }
    assert_eq!(score(FACELESS), 0.0, "{FACELESS} holds no face");
}
