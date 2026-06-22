use futures::StreamExt;
use phoneme_ipc::{
    DaemonEvent, IpcError, IpcErrorKind, NamedPipeListener, NamedPipeTransport, Request, Response,
    ServerRequest, Transport,
};

/// Generate a unique pipe name for parallel test runs.
fn unique_pipe_name(label: &str) -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("phoneme-test-{label}-{pid}-{nanos}")
}

#[tokio::test]
async fn client_sends_request_server_responds_ok() {
    let name = unique_pipe_name("ok");
    let mut listener = NamedPipeListener::bind(&name).expect("bind");

    let server_handle = tokio::spawn(async move {
        let mut conn = listener.accept().await.expect("accept");
        let req = conn.recv().await.expect("recv").expect("some");
        assert!(matches!(req, ServerRequest::Known(r) if matches!(*r, Request::DaemonStatus)));
        conn.send_response(Response::Ok(serde_json::json!({
            "running": true,
            "pid": 1234,
        })))
        .await
        .expect("send");
    });

    // Give the listener a beat to be ready.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut client = NamedPipeTransport::connect(&name).await.expect("connect");
    let resp = client
        .request(Request::DaemonStatus)
        .await
        .expect("request");
    match resp {
        Response::Ok(val) => {
            assert_eq!(val["running"], true);
            assert_eq!(val["pid"], 1234);
        }
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    }

    server_handle.await.expect("server task");
}

#[tokio::test]
async fn client_receives_err_response() {
    let name = unique_pipe_name("err");
    let mut listener = NamedPipeListener::bind(&name).expect("bind");

    let server_handle = tokio::spawn(async move {
        let mut conn = listener.accept().await.expect("accept");
        let _ = conn.recv().await.expect("recv");
        conn.send_response(Response::Err(IpcError {
            kind: IpcErrorKind::AlreadyRecording,
            message: "in flight".into(),
        }))
        .await
        .expect("send");
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut client = NamedPipeTransport::connect(&name).await.expect("connect");
    let resp = client
        .request(Request::RecordStart {
            mode: phoneme_core::RecordMode::Hold,
            in_place: false,
            recipe_id: None,
            whisper_model: None,
            source: None,
        })
        .await
        .expect("request");
    match resp {
        Response::Err(e) => {
            assert_eq!(e.kind, IpcErrorKind::AlreadyRecording);
        }
        _ => panic!("expected err"),
    }

    server_handle.await.expect("server task");
}

#[tokio::test]
async fn server_handles_sequential_clients() {
    let name = unique_pipe_name("seq");
    let mut listener = NamedPipeListener::bind(&name).expect("bind");

    let server_handle = tokio::spawn(async move {
        for _ in 0..3 {
            let mut conn = listener.accept().await.expect("accept");
            let _ = conn.recv().await.expect("recv");
            conn.send_response(Response::Ok(serde_json::Value::Null))
                .await
                .expect("send");
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    for _ in 0..3 {
        let mut client = NamedPipeTransport::connect(&name).await.expect("connect");
        let _ = client
            .request(Request::DaemonStatus)
            .await
            .expect("request");
    }

    server_handle.await.expect("server task");
}

#[tokio::test]
async fn subscribe_streams_events_in_order() {
    let name = unique_pipe_name("sub");
    let mut listener = NamedPipeListener::bind(&name).expect("bind");

    // Three distinct, canonical-shaped ids to assert ordered delivery.
    let ids = [
        "20260519T143500001",
        "20260519T143500002",
        "20260519T143500003",
    ];

    let server_ids = ids;
    let server_handle = tokio::spawn(async move {
        let mut conn = listener.accept().await.expect("accept");
        // The subscribe contract: SubscribeEvents elicits NO Response — the
        // connection turns into a one-way DaemonEvent stream from here on.
        let req = conn.recv().await.expect("recv").expect("some");
        assert!(matches!(req, ServerRequest::Known(r) if matches!(*r, Request::SubscribeEvents)));
        for s in server_ids {
            conn.send_event(DaemonEvent::TranscriptionStarted {
                id: phoneme_core::RecordingId::from_string(s.to_string()),
            })
            .await
            .expect("send_event");
        }
        // Hold the connection open until the client has drained the stream so
        // the events aren't lost to an early hang-up.
        let _ = conn.recv().await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut client = NamedPipeTransport::connect(&name).await.expect("connect");
    let mut events = client.subscribe().await.expect("subscribe");

    for expected in ids {
        let event = events
            .next()
            .await
            .expect("event present")
            .expect("event ok");
        match event {
            DaemonEvent::TranscriptionStarted { id } => assert_eq!(id.as_str(), expected),
            other => panic!("expected TranscriptionStarted({expected}), got {other:?}"),
        }
    }

    drop(events);
    server_handle.await.expect("server task");
}

#[tokio::test]
async fn subscribe_carries_buffered_event_across_codec_swap() {
    // Regression guard for the read_buf hand-off in subscribe(): an event line
    // that landed in the Response-codec's read buffer (because the server wrote
    // it right behind the response, before the client reframed to DaemonEvent)
    // must survive into_parts() and still be delivered, not dropped.
    let name = unique_pipe_name("subbuf");
    let mut listener = NamedPipeListener::bind(&name).expect("bind");

    // Two distinct ids; both get written before the client reframes its codec.
    let ids = ["20260519T143500010", "20260519T143500011"];

    let server_ids = ids;
    let server_handle = tokio::spawn(async move {
        let mut conn = listener.accept().await.expect("accept");
        // Answer the request, then immediately push two events on its heels so
        // they ride into the client's read_buf alongside (or right after) the
        // Response — i.e. they're already buffered when subscribe() swaps codecs.
        let req = conn.recv().await.expect("recv").expect("some");
        assert!(matches!(req, ServerRequest::Known(r) if matches!(*r, Request::DaemonStatus)));
        conn.send_response(Response::Ok(serde_json::Value::Null))
            .await
            .expect("send_response");
        for s in server_ids {
            conn.send_event(DaemonEvent::TranscriptionStarted {
                id: phoneme_core::RecordingId::from_string(s.to_string()),
            })
            .await
            .expect("send_event");
        }
        // Drain the SubscribeEvents request (no Response per contract) and keep
        // the pipe open until the client finishes reading.
        let _ = conn.recv().await;
        let _ = conn.recv().await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut client = NamedPipeTransport::connect(&name).await.expect("connect");
    let resp = client
        .request(Request::DaemonStatus)
        .await
        .expect("request");
    assert!(matches!(resp, Response::Ok(_)));

    // Give the trailing event lines a beat to reach the client's pipe buffer so
    // the swap genuinely has buffered bytes to carry over.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut events = client.subscribe().await.expect("subscribe");
    for expected in ids {
        let event = events
            .next()
            .await
            .expect("buffered event present")
            .expect("event ok");
        match event {
            DaemonEvent::TranscriptionStarted { id } => assert_eq!(id.as_str(), expected),
            other => panic!("expected TranscriptionStarted({expected}), got {other:?}"),
        }
    }

    drop(events);
    server_handle.await.expect("server task");
}

#[tokio::test]
async fn second_bind_to_same_name_fails() {
    let name = unique_pipe_name("dup");
    let _first = NamedPipeListener::bind(&name).expect("first bind");
    let second = NamedPipeListener::bind(&name);
    assert!(
        second.is_err(),
        "second bind should fail with first_pipe_instance set"
    );
}
