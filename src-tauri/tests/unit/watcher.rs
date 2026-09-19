use super::generation_is_live;

#[test]
fn replacement_and_shutdown_each_retire_an_owned_generation() {
    assert!(generation_is_live(4, 4, false));
    assert!(!generation_is_live(5, 4, false));
    assert!(!generation_is_live(4, 4, true));
}
