use super::*;

use std::io::Read;

#[test]
fn owner_shutdown_joins_its_idle_listener_and_closes_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let Claim::Primary { lock, listener } = claim(dir.path()).unwrap() else { panic!("primary"); };
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let (started, ready) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let mut service = Box::pin(run_listener(listener, stopped, || panic!("no activation")));
            // Poll the actual loop into its idle wait, then hand ownership back.
            std::future::poll_fn(|cx| {
                assert!(std::future::Future::poll(service.as_mut(), cx).is_pending());
                std::task::Poll::Ready(())
            }).await;
            started.send(()).unwrap();
            service.await.unwrap();
        });
    });
    let owner = Owner { _lock: lock, stop, worker: Mutex::new(Some(worker)) };
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    owner.request_shutdown();
    owner.join();
    assert!(owner.worker.lock().unwrap().is_none());
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn simultaneous_claims_have_exactly_one_owner() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::sync::Arc::new(dir.path().to_path_buf());
    let start = std::sync::Arc::new(std::sync::Barrier::new(3));
    let hold = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let root = root.clone();
        let start = start.clone();
        let hold = hold.clone();
        workers.push(std::thread::spawn(move || {
            start.wait();
            let claim = claim(&root);
            hold.wait();
            matches!(claim.unwrap(), Claim::Primary { .. })
        }));
    }
    start.wait();
    hold.wait();

    assert_eq!(
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|primary| *primary)
            .count(),
        1
    );
}

#[test]
fn exactly_one_claim_owns_a_root_and_drop_releases_it() {
    let dir = tempfile::tempdir().unwrap();
    let first = claim(dir.path()).unwrap();
    assert!(matches!(first, Claim::Primary { .. }));

    let second = claim(dir.path()).unwrap();
    assert!(matches!(second, Claim::Secondary { .. }));

    drop(second);
    drop(first);
    assert!(matches!(claim(dir.path()).unwrap(), Claim::Primary { .. }));
}

#[test]
fn secondary_activation_uses_the_endpoint_published_under_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let Claim::Primary {
        lock: _lock,
        listener,
    } = claim(dir.path()).unwrap()
    else {
        panic!("first claim must own the root");
    };
    listener.set_nonblocking(false).unwrap();
    let Claim::Secondary { endpoint_path } = claim(dir.path()).unwrap() else {
        panic!("second claim must be secondary");
    };

    let receiver = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        request
    });
    notify_primary(&endpoint_path).unwrap();
    assert_eq!(receiver.join().unwrap(), "activate");
}

#[tokio::test]
async fn activation_frame_accepts_fragmentation_and_rejects_wrong_or_incomplete_frames() {
    use tokio::io::AsyncWriteExt;
    for (bytes, expected) in [(b"activate".as_slice(), true), (b"activat".as_slice(), false), (b"invalid!".as_slice(), false)] {
        // The capacity forces a frame across several physical reads.
        let (mut sender, mut receiver) = tokio::io::duplex(3);
        let (sent, accepted) = tokio::join!(async {
            sender.write_all(bytes).await.unwrap();
            sender.shutdown().await.unwrap();
        }, read_activation(&mut receiver));
        let _ = sent;
        assert_eq!(accepted, expected);
    }
}

#[tokio::test(start_paused = true)]
async fn activation_deadline_covers_the_whole_frame_not_each_fragment() {
    use tokio::io::AsyncWriteExt;
    let (mut sender, mut receiver) = tokio::io::duplex(8);
    let read = tokio::spawn(async move { read_activation(&mut receiver).await });
    sender.write_all(b"act").await.unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(450)).await;
    sender.write_all(b"iv").await.unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(51)).await;
    assert!(!read.await.unwrap());
}

#[tokio::test]
async fn actual_listener_activates_and_shutdown_drops_a_stalled_peer() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let (activated, mut received) = tokio::sync::mpsc::unbounded_channel();
    let service = tokio::spawn(run_listener(listener, stopped, move || {
        activated.send(()).unwrap();
        Ok(())
    }));
    let mut sender = tokio::net::TcpStream::connect(address).await.unwrap();
    sender.write_all(b"activate").await.unwrap();
    received.recv().await.unwrap();
    let mut stalled = tokio::net::TcpStream::connect(address).await.unwrap();
    stalled.write_all(b"act").await.unwrap();
    stop.send_replace(true);
    // Shutdown is an independent wake, including during connection reads.
    service.await.unwrap().unwrap();
    let mut byte = [0];
    assert!(matches!(stalled.read(&mut byte).await, Ok(0) | Err(_)));
    assert!(TcpStream::connect(address).is_err());
}
