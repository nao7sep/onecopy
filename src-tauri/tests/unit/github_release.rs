use super::*;

#[test]
fn release_tags_are_strict_and_compare_by_semantic_precedence() {
    let installed = Version::parse("1.9.9").unwrap();
    assert_eq!(
        compare_tag("v1.10.0", &installed),
        Ok((true, "1.10.0".into()))
    );
    assert_eq!(
        compare_tag("v1.9.9", &installed),
        Ok((false, "1.9.9".into()))
    );
    assert_eq!(
        compare_tag("v1.8.0", &installed),
        Ok((false, "1.8.0".into()))
    );
    for invalid in ["1.10.0", "v1.10", "v1.10.0-beta.1", "v01.10.0"] {
        assert!(compare_tag(invalid, &installed).is_err(), "{invalid}");
    }
}

#[test]
fn endpoint_is_fixed_repository_metadata() {
    assert_eq!(
        LATEST_RELEASE_API,
        "https://api.github.com/repos/nao7sep/onecopy/releases/latest"
    );
    assert_eq!(GITHUB_ACCEPT, "application/vnd.github+json");
    assert_eq!(GITHUB_API_VERSION, "2022-11-28");
    assert_eq!(USER_AGENT, "OneCopy");
    assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(10));
}

#[test]
fn the_response_cap_admits_an_ordinary_release_with_assets_and_notes() {
    // Measured shapes (R6-06): a 14-asset release with real release notes runs
    // to ~36 KiB; OneCopy's own 4-asset shape with a normal changelog section
    // leaves only ~7 KiB under the old 16 KiB cap. The cap must clear a
    // generously padded release body without rejecting it.
    assert!(MAX_RESPONSE_BYTES >= 128 * 1024);
    let padded_body = "x".repeat(64 * 1024);
    let payload = format!(r#"{{"tag_name":"v9.9.9","body":"{padded_body}"}}"#);
    assert!((payload.len() as u64) < MAX_RESPONSE_BYTES);
    assert_eq!(parse_latest_tag(payload.as_bytes()), Ok("v9.9.9".into()));
}

// R4.4 E4: request mechanics beyond the constants above, exercised through
// `request_latest_tag`'s own `url` seam against a local raw-socket listener —
// never a fake GitHub-shaped server, and never real network.

/// Accepts exactly one connection, hands the raw request bytes back over
/// `sender`, and writes `response` before closing. Nothing here understands
/// GitHub's API shape; it is a bare byte-level stand-in for "some server".
fn respond_once(response: &'static str) -> (std::net::SocketAddr, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 8192];
            let mut request = Vec::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                request.extend_from_slice(&buf[..n]);
                if n == 0 || request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&request).to_string());
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (addr, rx)
}

fn tag_response(tag: &str) -> String {
    let body = format!(r#"{{"tag_name":"{tag}"}}"#);
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn the_request_carries_its_fixed_headers_and_no_authorization() {
    let (addr, requests) = respond_once(Box::leak(tag_response("v1.0.0").into_boxed_str()));
    let url = format!("http://{addr}/repos/nao7sep/onecopy/releases/latest");

    let result = request_latest_tag(&url).await;

    assert_eq!(result, Ok("v1.0.0".to_string()));
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    let lower = request.to_ascii_lowercase();
    assert!(request.contains("GET /repos/nao7sep/onecopy/releases/latest"));
    assert!(lower.contains("accept: application/vnd.github+json"));
    assert!(lower.contains("x-github-api-version: 2022-11-28"));
    assert!(lower.contains("user-agent: onecopy"));
    assert!(
        !lower.contains("authorization:"),
        "the release check is unauthenticated: {request}"
    );
}

#[tokio::test]
async fn a_failed_response_is_not_retried() {
    let (addr, requests) = respond_once(
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    let url = format!("http://{addr}/repos/nao7sep/onecopy/releases/latest");

    let result = request_latest_tag(&url).await;

    assert!(result.is_err());
    // The listener accepts exactly one connection; a retry would hang here
    // waiting for a second one that never arrives, instead of finishing.
    assert!(requests.recv_timeout(Duration::from_secs(5)).is_ok());
}

// The clock is paused: once the request waits on a silent server, tokio
// advances virtual time straight to the next timer, so the fixed timeout is
// observed exactly without being waited out.
#[tokio::test(start_paused = true)]
async fn a_request_that_never_gets_a_response_is_bounded_by_the_fixed_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        // Accept and hold the connection open with no response, forever.
        if let Ok((stream, _)) = listener.accept() {
            std::thread::sleep(Duration::from_secs(60));
            drop(stream);
        }
    });
    let url = format!("http://{addr}/repos/nao7sep/onecopy/releases/latest");

    let started = tokio::time::Instant::now();
    let result = request_latest_tag(&url).await;
    let elapsed = started.elapsed();

    assert!(result.is_err());
    assert!(
        (REQUEST_TIMEOUT..REQUEST_TIMEOUT + Duration::from_secs(1)).contains(&elapsed),
        "the 10 s request timeout did not end the wait: {elapsed:?}"
    );
}

#[tokio::test]
async fn the_attempt_marker_write_runs_before_the_request_and_a_failed_write_sends_none() {
    let calls: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let ok_calls = calls.clone();
    let ok_write = async move {
        ok_calls.lock().unwrap().push("write");
        Ok(())
    };
    let ok_request = async {
        calls.lock().unwrap().push("request");
        Ok::<_, String>("ok")
    };
    let result = write_attempt_marker_then(ok_write, ok_request).await;
    assert_eq!(result, Ok("ok"));
    assert_eq!(*calls.lock().unwrap(), vec!["write", "request"]);

    let never_requested = std::sync::Arc::new(std::sync::Mutex::new(false));
    let flag = never_requested.clone();
    let failing_write = async { Err::<(), String>("write failed".to_string()) };
    let request_that_must_not_run = async move {
        *flag.lock().unwrap() = true;
        Ok::<_, String>("unreachable")
    };
    let result = write_attempt_marker_then(failing_write, request_that_must_not_run).await;
    assert_eq!(result, Err("write failed".to_string()));
    assert!(
        !*never_requested.lock().unwrap(),
        "a failed attempt-marker write must send no request"
    );
}

#[test]
fn response_validation_requires_one_string_tag_name() {
    assert_eq!(
        parse_latest_tag(br#"{"tag_name":"v1.2.3","body":"ignored"}"#),
        Ok("v1.2.3".into())
    );
    for invalid in [
        br#"{}"#.as_slice(),
        br#"{"tag_name":7}"#.as_slice(),
        b"not-json".as_slice(),
    ] {
        assert!(parse_latest_tag(invalid).is_err());
    }
}
