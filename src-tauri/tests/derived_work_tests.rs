// Tests exercising the crate's public API from outside shipped source.

use onecopy_lib::derived_work::{priority_candidates, settings_from_config, SectionPriority};
use onecopy_lib::index_store;

#[test]
fn selected_visible_and_section_backlog_keep_their_priority_without_duplicates() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derived-priority-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('selected', 1, 'image');
         INSERT INTO contents (hash, byte_size, kind) VALUES ('visible', 1, 'image');
         INSERT INTO contents (hash, byte_size, kind) VALUES ('backlog', 1, 'image');
         INSERT INTO contents (hash, byte_size, kind, derived_at_utc)
           VALUES ('ready', 1, 'image', 'done');
         INSERT INTO contents (hash, byte_size, kind) VALUES ('video', 1, 'video');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/selected.jpg', '/', 'selected.jpg', 'image', 'selected', 120, 'metadata');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/visible.jpg', '/', 'visible.jpg', 'image', 'visible', 130, 'metadata');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/backlog.jpg', '/', 'backlog.jpg', 'image', 'backlog', 140, 'metadata');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/ready.jpg', '/', 'ready.jpg', 'image', 'ready', 150, 'metadata');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/video.mov', '/', 'video.mov', 'video', 'video', 160, 'metadata');",
    )
    .unwrap();
    conn.execute(
        "UPDATE contents SET derived_version = ?1 WHERE hash = 'ready'",
        [onecopy_lib::preview::DERIVE_VERSION],
    )
    .unwrap();

    let settings = settings_from_config(None, dir.path());
    let section = SectionPriority {
        kind: "image".to_string(),
        start_ms: Some(100),
        end_ms: Some(200),
    };
    let visible = vec![
        "visible".to_string(),
        "selected".to_string(),
        "ready".to_string(),
    ];
    let candidates =
        priority_candidates(&conn, &settings, Some("selected"), &visible, Some(&section)).unwrap();

    assert_eq!(candidates, ["selected", "visible", "backlog"]);
}
