use super::{
    cpu_thread_budget, image_worker_capacity_for, transcription_threads,
    IMAGE_CONCURRENCY_HEADROOM, IMAGE_JOB_RESERVATION,
};

#[test]
fn response_admission_accounts_for_held_and_cloned_leases() {
    use std::sync::{Arc, Mutex};
    let ledger = Arc::new(Mutex::new(0));
    let first = super::reserve_response_bytes_for(&ledger, 30, Some(100)).unwrap();
    let clone = Arc::clone(&first);
    let second = super::reserve_response_bytes_for(&ledger, 20, Some(100)).unwrap();
    assert!(super::reserve_response_bytes_for(&ledger, 1, Some(100)).is_err());
    drop(first);
    assert_eq!(*ledger.lock().unwrap(), 50);
    drop(clone);
    assert_eq!(*ledger.lock().unwrap(), 20);
    drop(second);
    assert_eq!(*ledger.lock().unwrap(), 0);
    assert!(super::reserve_response_bytes_for(&ledger, 50, Some(100)).is_ok());
}

#[test]
fn response_admission_refuses_unknown_memory_oversize_and_overflow() {
    use std::sync::{Arc, Mutex};
    let ledger = Arc::new(Mutex::new(0));
    assert!(super::reserve_response_bytes_for(&ledger, 1, None).is_err());
    assert!(super::reserve_response_bytes_for(&ledger, 51, Some(100)).is_err());
    assert!(super::reserve_response_bytes_for(&ledger, u64::MAX, Some(u64::MAX)).is_err());
    assert_eq!(*ledger.lock().unwrap(), 0);
    // Exercise checked aggregate addition without attempting an allocation.
    *ledger.lock().unwrap() = 1;
    assert!(super::reserve_response_bytes_for(&ledger, u64::MAX, Some(u64::MAX)).is_err());
    assert_eq!(*ledger.lock().unwrap(), 1);
}

#[test]
fn concurrent_response_admission_never_exceeds_the_shared_budget() {
    use std::sync::{Arc, Barrier, Mutex};
    let ledger = Arc::new(Mutex::new(0));
    let admitted = Arc::new(Barrier::new(9));
    let release = Arc::new(Barrier::new(9));
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..8 {
            let ledger = Arc::clone(&ledger);
            let admitted = Arc::clone(&admitted);
            let release = Arc::clone(&release);
            workers.push(scope.spawn(move || {
                let lease = super::reserve_response_bytes_for(&ledger, 10, Some(80));
                admitted.wait();
                release.wait();
                lease.is_ok()
            }));
        }
        admitted.wait();
        assert_eq!(*ledger.lock().unwrap(), 40);
        release.wait();
        assert_eq!(workers.into_iter().map(|worker| worker.join().unwrap()).filter(|admitted| *admitted).count(), 4);
    });
    assert_eq!(*ledger.lock().unwrap(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_speculative_pages_are_not_available_twice() {
    // Native fixture: speculative pages are a subset of reported free pages.
    let mut statistics: libc::vm_statistics64_data_t = unsafe { std::mem::zeroed() };
    statistics.free_count = 12;
    statistics.speculative_count = 8;
    statistics.inactive_count = 3;
    statistics.purgeable_count = 2;
    assert_eq!(super::macos_reclaimable_pages(&statistics), 17);
    statistics.speculative_count = 0;
    assert_eq!(super::macos_reclaimable_pages(&statistics), 17);
}

// Unmeasurable memory pauses every model and PCM gate, so the native query
// must answer on each platform that ships.
#[cfg(any(target_os = "macos", windows))]
#[test]
fn the_host_reports_its_available_memory() {
    let available = super::available_memory_bytes().expect("available memory is measurable");
    assert!(available > 0);
    assert!(super::require_available(1, "A test").is_ok());
}

#[test]
fn active_image_work_leaves_cpu_headroom() {
    let abundant = Some(IMAGE_CONCURRENCY_HEADROOM + 64 * IMAGE_JOB_RESERVATION);
    assert_eq!(image_worker_capacity_for(8, abundant, false), 4);
    assert_eq!(image_worker_capacity_for(8, abundant, true), 7);
}

#[test]
fn image_concurrency_falls_back_to_one_when_memory_is_unknown_or_tight() {
    assert_eq!(image_worker_capacity_for(32, None, true), 1);
    assert_eq!(image_worker_capacity_for(32, Some(1024), true), 1);
}

#[test]
fn image_concurrency_obeys_the_aggregate_memory_budget() {
    let for_three_workers = IMAGE_CONCURRENCY_HEADROOM + 3 * IMAGE_JOB_RESERVATION;
    assert_eq!(
        image_worker_capacity_for(32, Some(for_three_workers), true),
        3
    );
}

// ffmpeg and ONNX must never default to every physical core: they
// share the same interactive-headroom budget Whisper already respects.
#[test]
fn cpu_thread_budget_never_claims_every_core() {
    assert_eq!(transcription_threads(1), 1);
    assert_eq!(transcription_threads(2), 1);
    for cores in 2..=256usize {
        let threads = transcription_threads(cores);
        assert!(threads >= 1, "{cores} cores");
        assert!(threads as usize <= cores / 2, "{cores} cores get {threads}, more than half");
    }
    // Clamped, not unbounded: a much larger machine gets no more threads.
    let ceiling = transcription_threads(usize::MAX);
    assert_eq!(transcription_threads(1024), ceiling);
    assert!((ceiling as usize) < 1024 / 2);

    // The real machine's budget is the same formula, never zero and never
    // more than the clamp — asserted structurally since the actual core
    // count is environment-dependent.
    let budget = cpu_thread_budget();
    assert!((1..=ceiling).contains(&budget));
}


#[test]
fn encoded_image_budget_limits_buffering_codecs_and_allows_streaming_to_skip_the_tail() {
    use std::io::{Cursor, Read, Seek, SeekFrom};
    let mut exact = super::ImageReadBudget { inner: Cursor::new(b"12345678"), remaining: 8 };
    let mut bytes = Vec::new();
    exact.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"12345678");
    exact.seek(SeekFrom::Start(0)).unwrap();
    assert_eq!(exact.read(&mut [0]).unwrap_err().kind(), std::io::ErrorKind::OutOfMemory);

    let image = image::DynamicImage::new_rgb8(2, 2);
    let mut jpeg = Cursor::new(Vec::new());
    image.write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
    jpeg.get_mut().resize(4 * 1024 * 1024, 0);
    jpeg.set_position(0);
    let error = super::decode_reader(jpeg, 2 * 1024 * 1024).unwrap_err();
    assert!(super::is_decode_limit(&error), "{error}");

    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    png.get_mut().resize(4 * 1024 * 1024, 0);
    png.set_position(0);
    let decoded = super::decode_reader(png, 2 * 1024 * 1024).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (2, 2));
}
