use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[test]
fn identical_requested_previews_share_one_result() {
    let key = format!("test-preview-{}", crate::nanoid::generate().unwrap());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let first_key = key.clone();
    let leader = std::thread::spawn(move || {
        coalesce_requested_preview(&first_key, || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok("canonical".to_string())
        })
    });
    started_rx.recv().unwrap();

    let observed = REQUESTED_PREVIEWS
        .lock()
        .unwrap()
        .get(&key)
        .unwrap()
        .clone();
    let executions = Arc::new(AtomicUsize::new(0));
    let follower_executions = executions.clone();
    let follower_key = key.clone();
    let follower = std::thread::spawn(move || {
        coalesce_requested_preview(&follower_key, || {
            follower_executions.fetch_add(1, Ordering::SeqCst);
            Ok("wrong".to_string())
        })
    });

    let deadline = Instant::now() + Duration::from_secs(1);
    while Arc::strong_count(&observed) < 4 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let joined = Arc::strong_count(&observed) >= 4;
    release_tx.send(()).unwrap();
    assert!(joined, "follower did not join the active request");

    assert_eq!(leader.join().unwrap().unwrap(), ("canonical".to_string(), false));
    assert_eq!(follower.join().unwrap().unwrap(), ("canonical".to_string(), true));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}
