use bytes::BytesMut;
use phoneme_ipc::codec::JsonLineCodec;
use phoneme_ipc::schema::Request;
use tokio_util::codec::{Decoder, Encoder};

#[test]
fn encode_appends_newline() {
    let mut codec = JsonLineCodec::<Request>::new();
    let mut buf = BytesMut::new();
    codec.encode(Request::RecordStatus, &mut buf).unwrap();
    let s = std::str::from_utf8(&buf).unwrap();
    assert!(s.ends_with('\n'));
    assert!(s.contains("record_status"));
}

#[test]
fn decode_complete_line_yields_value() {
    let mut codec = JsonLineCodec::<Request>::new();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(b"{\"type\":\"record_status\"}\n");
    let decoded = codec.decode(&mut buf).unwrap();
    assert!(matches!(decoded, Some(Request::RecordStatus)));
    assert!(buf.is_empty());
}

#[test]
fn decode_partial_line_returns_none_and_keeps_buffer() {
    let mut codec = JsonLineCodec::<Request>::new();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(b"{\"type\":\"record_st"); // no newline yet
    let decoded = codec.decode(&mut buf).unwrap();
    assert!(decoded.is_none());
    assert!(!buf.is_empty());
}

#[test]
fn decode_multiple_lines_returns_one_at_a_time() {
    let mut codec = JsonLineCodec::<Request>::new();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(b"{\"type\":\"record_status\"}\n{\"type\":\"daemon_status\"}\n");
    let a = codec.decode(&mut buf).unwrap();
    assert!(matches!(a, Some(Request::RecordStatus)));
    let b = codec.decode(&mut buf).unwrap();
    assert!(matches!(b, Some(Request::DaemonStatus)));
    let c = codec.decode(&mut buf).unwrap();
    assert!(c.is_none());
}

#[test]
fn decode_malformed_json_returns_error() {
    let mut codec = JsonLineCodec::<Request>::new();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(b"not json at all\n");
    assert!(codec.decode(&mut buf).is_err());
}

#[test]
fn decode_skips_blank_line_before_a_frame() {
    // A leading blank line must not stall the frame that follows it in the same
    // buffer — the decoder should skip it and return the real message.
    let mut codec = JsonLineCodec::<Request>::new();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(b"\n{\"type\":\"record_status\"}\n");
    let decoded = codec.decode(&mut buf).unwrap();
    assert!(matches!(decoded, Some(Request::RecordStatus)));
    assert!(buf.is_empty());
}

#[test]
fn decode_blank_line_only_returns_none() {
    // A lone blank line carries no frame: consume it and report "need more".
    let mut codec = JsonLineCodec::<Request>::new();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(b"\n");
    let decoded = codec.decode(&mut buf).unwrap();
    assert!(decoded.is_none());
    assert!(buf.is_empty());
}

#[test]
fn encode_then_decode_round_trips() {
    let mut codec = JsonLineCodec::<Request>::new();
    let mut buf = BytesMut::new();
    codec.encode(Request::DaemonStatus, &mut buf).unwrap();
    codec.encode(Request::Shutdown, &mut buf).unwrap();
    let a = codec.decode(&mut buf).unwrap().unwrap();
    let b = codec.decode(&mut buf).unwrap().unwrap();
    assert!(matches!(a, Request::DaemonStatus));
    assert!(matches!(b, Request::Shutdown));
}
