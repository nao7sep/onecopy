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

/// Owns a local server task; a failed assertion cancels its socket work.
struct Server {
    addr: std::net::SocketAddr,
    requests: tokio::sync::oneshot::Receiver<String>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) { self.task.abort(); }
}
impl Server {
    async fn stop(&mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}
async fn respond_once(response: Option<String>) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, requests) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let Ok((mut stream, _)) = listener.accept().await else { return; };
        let mut buf = [0u8; 8192];
        let mut request = Vec::new();
        loop {
            let Ok(n) = stream.read(&mut buf).await else { return; };
            request.extend_from_slice(&buf[..n]);
            if n == 0 || request.windows(4).any(|w| w == b"\r\n\r\n") { break; }
        }
        let _ = tx.send(String::from_utf8_lossy(&request).into_owned());
        if let Some(response) = response {
            let _ = stream.write_all(response.as_bytes()).await;
        } else {
            std::future::pending::<()>().await;
        }
    });
    Server { addr, requests, task }
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
    let mut server = respond_once(Some(tag_response("v1.0.0"))).await;
    let addr = server.addr;
    let url = format!("http://{addr}/repos/nao7sep/onecopy/releases/latest");

    let result = request_latest_tag(&url).await;

    let request = tokio::time::timeout(Duration::from_secs(5), &mut server.requests).await;
    server.stop().await;
    assert_eq!(result, Ok("v1.0.0".to_string()));
    let request = request.unwrap().unwrap();
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
    let mut server = respond_once(Some(
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
    )).await;
    let addr = server.addr;
    let url = format!("http://{addr}/repos/nao7sep/onecopy/releases/latest");

    let result = request_latest_tag(&url).await;

    let request = tokio::time::timeout(Duration::from_secs(5), &mut server.requests).await;
    server.stop().await;
    assert!(result.is_err());
    assert!(request.unwrap().is_ok());
}

// Start virtual time only after the server has actually read the request.
#[tokio::test]
async fn a_request_that_never_gets_a_response_is_bounded_by_the_fixed_timeout() {
    let mut server = respond_once(None).await;
    let url = format!("http://{}/repos/nao7sep/onecopy/releases/latest", server.addr);
    let request = request_latest_tag(&url);
    tokio::pin!(request);
    let ready = tokio::select! {
        result = &mut request => panic!("request finished before the silent server received it: {result:?}"),
        ready = tokio::time::timeout(Duration::from_secs(5), &mut server.requests) => ready,
    };
    // Settle the server before reporting readiness errors as well.
    if !matches!(ready, Ok(Ok(_))) {
        server.stop().await;
        panic!("silent server did not receive the request: {ready:?}");
    }
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let result = request.await;
    let elapsed = started.elapsed();
    server.stop().await;
    assert!(result.is_err());
    assert!((REQUEST_TIMEOUT - Duration::from_secs(1)..REQUEST_TIMEOUT + Duration::from_secs(1)).contains(&elapsed),
        "the request timeout did not end the established wait: {elapsed:?}");
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
