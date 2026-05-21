use phoneme_ipc::{NamedPipeTransport, Request, Response, Transport};

#[tokio::test]
#[ignore = "requires daemon running"]
async fn smoke_daemon_status() {
    let mut t = NamedPipeTransport::connect("phoneme-daemon").await.unwrap();
    let r = t.request(Request::DaemonStatus).await.unwrap();
    match r {
        Response::Ok(v) => {
            assert_eq!(v["running"], true);
        }
        _ => panic!("expected ok"),
    }
}
