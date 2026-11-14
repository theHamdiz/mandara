use super::protocol::for_provider;
use super::*;
use serde_json::json;

fn settings(provider: Provider) -> Settings {
    Settings {
        provider,
        server: String::new(),
        model: "local-model".into(),
        use_api_key: false,
        context_bytes: DEFAULT_CONTEXT_BYTES,
        reply_tokens: DEFAULT_REPLY_TOKENS,
    }
}
fn message(role: Role, text: &str) -> Message {
    Message {
        role,
        content: text.into(),
    }
}

#[test]
fn provider_presets_keep_paths_and_credentials_scoped_to_the_server() {
    for provider in [
        Provider::Ollama,
        Provider::LlamaCpp,
        Provider::Jan,
        Provider::LmStudio,
        Provider::AnythingLlm,
        Provider::OpenAi,
    ] {
        let mut settings = settings(provider);
        assert_eq!(settings.server().unwrap(), provider.server());
        assert_eq!(
            settings.credential_key().unwrap().is_some(),
            provider == Provider::AnythingLlm
        );
        settings.server = " https://example.org/proxy/ ".into();
        settings.use_api_key = true;
        assert_eq!(
            settings.credential_key().unwrap().unwrap().server,
            "https://example.org/proxy"
        );
        settings.server = "http://user:secret@localhost:3001".into();
        assert!(settings.server().is_err());
    }
}

#[test]
fn protocols_use_native_chat_endpoints_and_workspace_session_partitioning() {
    let messages = [message(Role::User, "مرحبا")];
    for provider in [
        Provider::Ollama,
        Provider::LlamaCpp,
        Provider::Jan,
        Provider::LmStudio,
        Provider::AnythingLlm,
        Provider::OpenAi,
    ] {
        let protocol = for_provider(provider);
        let (path, body) = protocol
            .request(&settings(provider), &messages, "separate-session")
            .unwrap();
        match provider {
            Provider::Ollama => {
                assert_eq!(path, "api/chat");
                assert_eq!(body["options"]["num_predict"], DEFAULT_REPLY_TOKENS);
            }
            Provider::AnythingLlm => {
                assert_eq!(path, "v1/workspace/local-model/stream-chat");
                assert_eq!(body["sessionId"], "separate-session");
                assert_eq!(body["message"], "مرحبا");
                assert_eq!(body["mode"], "chat");
                assert!(body.get("reset").is_none());
            }
            _ => {
                assert_eq!(path, "chat/completions");
                assert_eq!(body["max_tokens"], DEFAULT_REPLY_TOKENS);
            }
        }
    }
    let mut workspace = settings(Provider::AnythingLlm);
    workspace.model = "../../other".into();
    assert!(
        for_provider(Provider::AnythingLlm)
            .request(&workspace, &messages, "s")
            .is_err()
    );
}

#[test]
fn streams_accept_native_unicode_chunks_keepalives_and_final_content() {
    let examples = [
        (
            Provider::Ollama,
            "{\"message\":{\"content\":\"مرحبا\"},\"done\":false}\n{\"message\":{\"content\":\"!\"},\"done\":true}\n",
        ),
        (
            Provider::LlamaCpp,
            ": ping\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"مرحبا\"},\"finish_reason\":null}]}\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"!\"},\"finish_reason\":\"stop\"}]}\n\n",
        ),
        (
            Provider::AnythingLlm,
            "data: {\"type\":\"textResponseChunk\",\"textResponse\":\"مرحبا\",\"close\":false}\n\ndata: {\"textResponse\":\"!\",\"close\":true,\"sources\":[]}\n\n",
        ),
    ];
    for (provider, wire) in examples {
        let mut output = String::new();
        stream::read(
            wire.as_bytes(),
            for_provider(provider),
            || false,
            &mut |text| {
                output.push_str(text);
                true
            },
        )
        .unwrap();
        assert_eq!(output, "مرحبا!");
    }
}

#[test]
fn malformed_truncated_and_oversized_streams_fail_and_cancellation_keeps_no_reply() {
    for wire in [
        "data: broken\n\n".into(),
        "data: {}\n\n".into(),
        format!("data: {}\n\n", "x".repeat(65 * 1024)),
        format!("data: {}\n\n", json!({"error":"private-server-detail"})),
    ] {
        assert!(
            stream::read(
                wire.as_bytes(),
                for_provider(Provider::Jan),
                || false,
                &mut |_| true
            )
            .is_err()
        );
    }
    let large = format!(
        "{}\n",
        json!({"message":{"content":"x".repeat(MAX_REPLY_BYTES+1)},"done":true})
    );
    assert!(
        stream::read(
            large.as_bytes(),
            for_provider(Provider::Ollama),
            || false,
            &mut |_| panic!("oversized content must not reach the UI")
        )
        .is_err()
    );
    stream::read(
        "not read".as_bytes(),
        for_provider(Provider::Jan),
        || true,
        &mut |_| panic!("cancelled"),
    )
    .unwrap();
}

#[test]
fn histories_drop_complete_old_turns_and_sessions_never_share_an_identity() {
    let mut messages: Vec<_> = (0..40)
        .map(|index| {
            message(
                if index % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                &"界".repeat(300),
            )
        })
        .collect();
    trim(&mut messages, *CONTEXT_RANGE.start());
    assert_eq!(messages.len() % 2, 0);
    assert_eq!(messages.first().unwrap().role, Role::User);
    assert!(
        messages
            .iter()
            .map(|message| message.content.len())
            .sum::<usize>()
            <= *CONTEXT_RANGE.start()
    );
    assert_ne!(session_id().unwrap(), session_id().unwrap());
    assert!(
        for_provider(Provider::Jan)
            .models(json!({"data":[{"id":"b"},{"id":"a"},{"id":"a"}]}))
            .unwrap()
            == ["a", "b"]
    );
    assert!(
        for_provider(Provider::AnythingLlm)
            .models(json!({"workspaces":[{"slug":"notes"}]}))
            .unwrap()
            == ["notes"]
    );
}

#[test]
fn cancelling_an_agent_interrupts_an_actual_blocked_response() {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buffer = [0; 4096];
        assert!(socket.read(&mut buffer).unwrap() > 0, "the client sent its request");
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        ready_tx.send(()).unwrap();
        while socket.read(&mut buffer).is_ok_and(|n| n > 0) {}
    });
    let session = Session::default();
    let request = session.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _online = crate::offline::online_fixture();
        let mut settings = settings(Provider::Jan);
        settings.server = url;
        let result = request.chat(
            &settings,
            &[message(Role::User, "Hello")],
            "fixture",
            None,
            |_| true,
        );
        done_tx.send(result).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    session.cancel();
    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("cancellation must interrupt a blocked socket")
        .unwrap();
    worker.join().unwrap();
    server.join().unwrap();
}
