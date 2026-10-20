use super::{
    cpu_thread_budget, image_worker_capacity_for, transcription_threads,
    IMAGE_CONCURRENCY_HEADROOM, IMAGE_JOB_RESERVATION,
};

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
