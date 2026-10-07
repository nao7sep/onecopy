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
    let for_three_workers = 1024 * 1024 * 1024 + 3 * IMAGE_JOB_RESERVATION;
    assert_eq!(
        image_worker_capacity_for(32, Some(for_three_workers), true),
        3
    );
}

// (W-L2) ffmpeg and ONNX must never default to every physical core: they
// share the same interactive-headroom budget Whisper already respects.
#[test]
fn cpu_thread_budget_never_claims_every_core() {
    assert_eq!(transcription_threads(1), 1);
    assert_eq!(transcription_threads(2), 1);
    assert_eq!(transcription_threads(8), 4, "half the cores, not all eight");
    assert_eq!(transcription_threads(64), 4, "clamped, not unbounded");

    // The real machine's budget is the same formula, never zero and never
    // more than the clamp — asserted structurally since the actual core
    // count is environment-dependent.
    let budget = cpu_thread_budget();
    assert!((1..=4).contains(&budget));
}
