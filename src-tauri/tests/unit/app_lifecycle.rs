use super::Lifecycle;

#[test]
fn final_shutdown_closes_admission_once_and_never_reopens_it() {
    let lifecycle = Lifecycle::new();

    assert!(!lifecycle.shutting_down());
    assert_eq!(lifecycle.publish_if_running(|| 7), Some(7));
    assert!(lifecycle.begin_shutdown());
    assert!(lifecycle.shutting_down());
    assert!(!lifecycle.begin_shutdown());
    assert!(lifecycle.shutting_down());
    assert_eq!(lifecycle.publish_if_running(|| 7), None);
}
