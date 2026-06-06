use phoneme_ipc::{
    IpcError, IpcErrorKind, NamedPipeListener, NamedPipeTransport, Request, Response, Transport,
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
        assert!(matches!(req, Request::DaemonStatus));
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
async fn second_bind_to_same_name_fails() {
    let name = unique_pipe_name("dup");
    let _first = NamedPipeListener::bind(&name).expect("first bind");
    let second = NamedPipeListener::bind(&name);
    assert!(
        second.is_err(),
        "second bind should fail with first_pipe_instance set"
    );
}
