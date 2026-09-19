use super::{
    image_worker_capacity_for, IMAGE_CONCURRENCY_HEADROOM, IMAGE_JOB_RESERVATION,
};

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
