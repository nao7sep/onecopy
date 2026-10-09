use super::*;
use std::sync::Arc;

#[test]
fn managed_networks_refuse_plain_http() {
    assert!(assert_https("https://example.test/artifact").is_ok());
    assert!(assert_https("http://example.test/artifact")
        .unwrap_err()
        .contains("refusing non-https"));
}

#[test]
fn network_runtime_has_io_and_async_file_drivers() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-acquisition-runtime-")
        .tempdir()
        .unwrap();
    let durable = dir.path().join("durable.partial");
    network_runtime().unwrap().block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (client, server) =
            tokio::join!(tokio::net::TcpStream::connect(address), listener.accept(),);
        assert!(client.is_ok());
        assert!(server.is_ok());

        let mut file = tokio::fs::File::create(&durable).await.unwrap();
        file.write_all(b"durable bytes").await.unwrap();
        file.sync_all().await.unwrap();
    });
    assert_eq!(std::fs::read(durable).unwrap(), b"durable bytes");
}

#[test]
fn network_waits_are_promptly_cancellable_and_whole_bounded() {
    let runtime = network_runtime().unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let trigger = cancelled.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(25));
        trigger.store(true, Ordering::Relaxed);
    });
    let started = std::time::Instant::now();
    let error = runtime
        .block_on(cancellable_with_timeout(
            std::future::pending::<Result<(), String>>(),
            &cancelled,
            Duration::from_secs(5),
            "test request",
        ))
        .unwrap_err();
    assert_eq!(error, CANCELLED_ERROR);
    assert!(started.elapsed() < Duration::from_secs(1));

    let not_cancelled = AtomicBool::new(false);
    let error = runtime
        .block_on(cancellable_with_timeout(
            std::future::pending::<Result<(), String>>(),
            &not_cancelled,
            Duration::from_millis(25),
            "test request",
        ))
        .unwrap_err();
    assert!(error.contains("timed out"));
}

#[test]
fn pinned_downloads_have_scaled_deadlines_and_exact_byte_ceilings() {
    let bytes = 1_624_555_275;
    let timeout = download_whole_timeout(Some(bytes));
    assert!(timeout > Duration::from_secs(12 * 60 * 60));
    assert!(timeout <= MAX_DOWNLOAD_TIMEOUT);
    assert!(ensure_download_within_ceiling(bytes, Some(bytes)).is_ok());
    assert!(ensure_download_within_ceiling(bytes + 1, Some(bytes)).is_err());
    assert!(ensure_download_within_ceiling(UNPINNED_MAX_DOWNLOAD_BYTES + 1, None).is_err());

    let operation = OperationDeadline::for_install(Some(bytes));
    assert!(operation.timeout > timeout);
}

#[test]
fn ffmpeg_extraction_has_an_exact_uncompressed_byte_ceiling() {
    assert!(ensure_ffmpeg_extract_within_ceiling(FFMPEG_MAX_EXTRACTED_BYTES).is_ok());
    assert!(ensure_ffmpeg_extract_within_ceiling(FFMPEG_MAX_EXTRACTED_BYTES + 1).is_err());
}

#[test]
fn staged_cleanup_runs_during_unwinding() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-acquisition-unwind-")
        .tempdir()
        .unwrap();
    let staged = dir.path().join("artifact.partial");
    std::fs::write(&staged, b"partial").unwrap();
    let _ = std::panic::catch_unwind({
        let staged = staged.clone();
        move || {
            let _cleanup = RemoveFilesOnDrop::new(vec![staged]);
            panic!("simulated worker panic");
        }
    });
    assert!(!staged.exists());
}

#[test]
fn hashing_and_extraction_stop_when_cancelled() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-acquisition-cancel-")
        .tempdir()
        .unwrap();
    let file = dir.path().join("artifact.bin");
    std::fs::write(&file, vec![7u8; 2 * 1024 * 1024]).unwrap();
    let cancelled = AtomicBool::new(true);
    let deadline = OperationDeadline::for_install(Some(2 * 1024 * 1024));
    assert_eq!(
        file_sha256(&file, &cancelled, &deadline, |_, _| {}).unwrap_err(),
        CANCELLED_ERROR
    );

    let archive_path = dir.path().join("ffmpeg.zip");
    let archive_file = std::fs::File::create(&archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(archive_file);
    archive
        .start_file("ffmpeg.exe", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"binary bytes").unwrap();
    archive.finish().unwrap();

    let staged = dir.path().join("ffmpeg.staged");
    assert_eq!(
        extract_ffmpeg(&archive_path, &staged, "ffmpeg.exe", &cancelled, &deadline,)
            .unwrap_err(),
        CANCELLED_ERROR
    );
}

#[test]
fn hashing_reports_stable_byte_progress_through_completion() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-acquisition-hash-progress-")
        .tempdir()
        .unwrap();
    let file = dir.path().join("artifact.bin");
    let bytes = 2 * 1024 * 1024 + 17;
    std::fs::write(&file, vec![7u8; bytes]).unwrap();
    let cancelled = AtomicBool::new(false);
    let deadline = OperationDeadline::for_install(Some(bytes as u64));
    let mut snapshots = Vec::new();

    file_sha256(&file, &cancelled, &deadline, |done, total| {
        snapshots.push((done, total));
    })
    .unwrap();

    assert_eq!(snapshots.first(), Some(&(0, bytes as u64)));
    assert_eq!(snapshots.last(), Some(&(bytes as u64, bytes as u64)));
    assert!(snapshots.windows(2).all(|pair| pair[0].0 <= pair[1].0));
}

#[test]
fn cumulative_deadline_expires_local_work_too() {
    let deadline = OperationDeadline {
        started: std::time::Instant::now() - Duration::from_secs(2),
        timeout: Duration::from_secs(1),
    };
    let cancelled = AtomicBool::new(false);
    assert!(deadline
        .check(&cancelled)
        .unwrap_err()
        .contains("operation timed out"));
}

#[test]
fn publishing_replaces_an_existing_artifact() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-acquisition-publish-")
        .tempdir()
        .unwrap();
    let staged = dir.path().join("artifact.staged");
    let target = dir.path().join("artifact.bin");
    std::fs::write(&target, b"old").unwrap();
    std::fs::write(&staged, b"new").unwrap();

    publish_staged(&staged, &target).unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    assert!(!staged.exists());
}

fn btbn_asset(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "browser_download_url": format!("https://example.test/{name}"),
    })
}

#[test]
fn btbn_resolution_skips_rolling_latest_and_picks_the_newest_autobuild_tag() {
    // Newest-first, as GitHub's releases endpoint returns them: a rolling
    // `latest` release ahead of two autobuild releases. The rolling release
    // must never be treated as a candidate even though it sorts first.
    let releases = serde_json::json!([
        {
            "tag_name": "latest",
            "assets": [
                btbn_asset("ffmpeg-master-latest-win64-gpl.zip"),
                btbn_asset("checksums.sha256"),
            ],
        },
        {
            "tag_name": "autobuild-2026-09-27-00-00",
            "assets": [
                btbn_asset("ffmpeg-N-999999-gnewbuild00-win64-gpl.zip"),
                btbn_asset("ffmpeg-N-999999-gnewbuild00-win64-gpl-shared.zip"),
                btbn_asset("checksums.sha256"),
            ],
        },
        {
            "tag_name": "autobuild-2026-09-26-13-03",
            "assets": [
                btbn_asset("ffmpeg-N-126889-gb139ba11d8-win64-gpl.zip"),
                btbn_asset("checksums.sha256"),
            ],
        },
    ]);
    let resolved = resolve_btbn_windows_release(&releases).unwrap();
    assert_eq!(resolved.version, "autobuild-2026-09-27-00-00");
    assert_eq!(
        resolved.sums_asset,
        "ffmpeg-N-999999-gnewbuild00-win64-gpl.zip"
    );
    assert!(resolved
        .download_url
        .ends_with("ffmpeg-N-999999-gnewbuild00-win64-gpl.zip"));
    assert!(resolved.sums_url.ends_with("checksums.sha256"));
}

#[test]
fn btbn_resolution_rejects_a_release_list_with_no_autobuild_tag() {
    let releases = serde_json::json!([
        {
            "tag_name": "latest",
            "assets": [btbn_asset("ffmpeg-master-latest-win64-gpl.zip")],
        }
    ]);
    assert!(resolve_btbn_windows_release(&releases)
        .unwrap_err()
        .contains("no autobuild-* release"));
}

#[test]
fn btbn_resolution_requires_the_win64_gpl_asset_and_checksums() {
    let missing_gpl_asset = serde_json::json!([
        {
            "tag_name": "autobuild-2026-09-26-13-03",
            "assets": [
                btbn_asset("ffmpeg-N-126889-gb139ba11d8-win64-gpl-shared.zip"),
                btbn_asset("checksums.sha256"),
            ],
        }
    ]);
    assert!(resolve_btbn_windows_release(&missing_gpl_asset)
        .unwrap_err()
        .contains("no win64 GPL asset"));

    let missing_checksums = serde_json::json!([
        {
            "tag_name": "autobuild-2026-09-26-13-03",
            "assets": [btbn_asset("ffmpeg-N-126889-gb139ba11d8-win64-gpl.zip")],
        }
    ]);
    assert!(resolve_btbn_windows_release(&missing_checksums)
        .unwrap_err()
        .contains("no checksums.sha256"));
}

#[cfg(target_os = "macos")]
#[test]
fn macho_arm64_detection_covers_thin_fat_and_foreign() {
    let mut thin_arm = vec![0xCF, 0xFA, 0xED, 0xFE];
    thin_arm.extend_from_slice(&0x0100_000Cu32.to_le_bytes());
    assert!(macho_has_arm64(&thin_arm));

    let mut thin_x86 = vec![0xCF, 0xFA, 0xED, 0xFE];
    thin_x86.extend_from_slice(&0x0100_0007u32.to_le_bytes());
    assert!(!macho_has_arm64(&thin_x86));

    let mut fat = Vec::new();
    fat.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
    fat.extend_from_slice(&2u32.to_be_bytes());
    fat.extend_from_slice(&0x0100_0007u32.to_be_bytes());
    fat.extend_from_slice(&[0u8; 16]);
    fat.extend_from_slice(&0x0100_000Cu32.to_be_bytes());
    fat.extend_from_slice(&[0u8; 16]);
    assert!(macho_has_arm64(&fat));

    let mut fat_x86 = Vec::new();
    fat_x86.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
    fat_x86.extend_from_slice(&1u32.to_be_bytes());
    fat_x86.extend_from_slice(&0x0100_0007u32.to_be_bytes());
    fat_x86.extend_from_slice(&[0u8; 16]);
    assert!(!macho_has_arm64(&fat_x86));
    assert!(!macho_has_arm64(b"#!/bin/sh\n"));
    assert!(!macho_has_arm64(&[]));
}

#[test]
fn a_failed_download_names_no_url_or_signature() {
    let dir = tempfile::tempdir().unwrap();
    // Nothing listens on the discard port, so the request fails at once.
    let url = "https://127.0.0.1:9/ffmpeg.zip?X-Amz-Signature=secret-signature";
    let error = download_to(
        url,
        &dir.path().join("ffmpeg.partial"),
        &AtomicBool::new(false),
        &OperationDeadline::for_install(Some(1024)),
        Some(1024),
        |_, _| {},
    )
    .unwrap_err();
    assert!(!error.contains("secret-signature"), "{error}");
    assert!(!error.contains("127.0.0.1:9/"), "{error}");

    let refused = assert_https("http://cdn.example.test/file?sig=secret-signature").unwrap_err();
    assert!(refused.contains("cdn.example.test"), "{refused}");
    assert!(!refused.contains("secret-signature"), "{refused}");
}
