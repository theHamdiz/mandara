//! Native loopback requests cover authentication, the bar-only read and model controls.
use super::*;
use std::io::Write as _;
use std::net::TcpListener;

#[test]
fn authenticated_status_avoids_the_catalog_and_model_controls_use_the_same_key() {
    let _online = crate::offline::online_fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        for expected in ["/api/version", "/api/ps", "/api/generate", "/api/generate"] {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("test server: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
                assert!(head.len() < 8192);
            }
            let head = String::from_utf8(head).unwrap();
            assert_eq!(head.split_whitespace().nth(1), Some(expected));
            assert!(
                head.to_ascii_lowercase()
                    .contains("authorization: bearer fixture-api-key\r\n")
            );
            let length = head
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            socket.read_exact(&mut body).unwrap();
            if expected == "/api/generate" {
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["model"],
                    "fixture-model"
                );
            }
            let body = match expected {
                "/api/version" => r#"{"version":"fixture"}"#,
                "/api/ps" => r#"{"models":[]}"#,
                _ => "{}",
            };
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let key = Secret::new("fixture-api-key".into()).unwrap();
    assert!(
        fetch_authenticated(&url, Some(&key), false)
            .unwrap()
            .installed
            .is_empty()
    );
    load_authenticated(&url, "fixture-model", Some(&key)).unwrap();
    unload_authenticated(&url, "fixture-model", Some(&key)).unwrap();
    server.join().unwrap();
}
