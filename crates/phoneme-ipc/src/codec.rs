//! Newline-delimited JSON codec for tokio_util.
//!
//! Frames messages as `serde_json::to_string(&value) + "\n"`. Decodes by
//! scanning for the next newline and parsing the line.

use bytes::{Buf, BytesMut};
use serde::{de::DeserializeOwned, Serialize};
use std::io;
use std::marker::PhantomData;
use tokio_util::codec::{Decoder, Encoder};

/// Upper bound on a single NDJSON frame. A malicious or buggy peer that never
/// sends a newline would otherwise grow the decode buffer without limit and
/// OOM the daemon; we error once the buffered (unterminated) data exceeds this.
/// 8 MiB is far above any legitimate request/response/event.
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct JsonLineCodec<T>(PhantomData<T>);

impl<T> JsonLineCodec<T> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T> Default for JsonLineCodec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: DeserializeOwned> Decoder for JsonLineCodec<T> {
    type Item = T;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<T>> {
        // Scan for complete lines, skipping empty ones. Returning `Ok(None)` on
        // a blank line (rather than continuing) would consume the newline but
        // leave any already-buffered frame *after* it unparsed until the next
        // read — stalling a request/response that arrives in the same buffer.
        loop {
            let Some(pos) = src.iter().position(|b| *b == b'\n') else {
                // No complete frame yet. Bail if the peer is flooding us with an
                // unterminated frame rather than buffering it unbounded.
                if src.len() > MAX_FRAME_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("IPC frame exceeds {MAX_FRAME_BYTES} bytes without a newline"),
                    ));
                }
                return Ok(None);
            };
            let line = src.split_to(pos);
            src.advance(1); // consume the newline
            if line.is_empty() {
                continue; // blank line: keep scanning for the next frame
            }
            let parsed = serde_json::from_slice::<T>(&line).map_err(io::Error::other)?;
            return Ok(Some(parsed));
        }
    }
}

impl<T: Serialize> Encoder<T> for JsonLineCodec<T> {
    type Error = io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> io::Result<()> {
        let bytes = serde_json::to_vec(&item).map_err(io::Error::other)?;
        dst.extend_from_slice(&bytes);
        dst.extend_from_slice(b"\n");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unterminated_frame_over_cap_errors_not_ooms() {
        let mut codec = JsonLineCodec::<String>::new();
        // No newline, just under the cap → keep buffering (Ok(None)).
        let mut under = BytesMut::from(vec![b'a'; MAX_FRAME_BYTES].as_slice());
        assert!(matches!(codec.decode(&mut under), Ok(None)));
        // Over the cap with still no newline → error instead of growing forever.
        let mut over = BytesMut::from(vec![b'a'; MAX_FRAME_BYTES + 1].as_slice());
        let err = codec
            .decode(&mut over)
            .expect_err("over-cap frame must error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn normal_frame_still_decodes() {
        let mut codec = JsonLineCodec::<String>::new();
        let mut buf = BytesMut::from("\"hello\"\n".as_bytes());
        let decoded = codec.decode(&mut buf).unwrap();
        assert_eq!(decoded.as_deref(), Some("hello"));
    }

    #[test]
    fn unknown_request_decodes_leniently_and_does_not_break_the_stream() {
        // The resilience guarantee behind the run_doctor regression: a client
        // ahead of this build sends a request variant we don't know. As a
        // `ServerRequest` it must decode to `Unknown` (not a codec error that
        // fuses the Framed stream), and a *subsequent* valid request on the same
        // connection must still decode — i.e. one unknown request can't break
        // the client's other commands.
        use crate::schema::{Request, ServerRequest};
        let mut codec = JsonLineCodec::<ServerRequest>::new();
        let mut buf = BytesMut::from(
            "{\"type\":\"some_future_request\"}\n{\"type\":\"daemon_status\"}\n".as_bytes(),
        );
        match codec.decode(&mut buf).expect("unknown frame must not error") {
            Some(ServerRequest::Unknown { .. }) => {}
            other => panic!("expected Unknown, got {other:?}"),
        }
        match codec.decode(&mut buf).expect("following frame must still decode") {
            Some(ServerRequest::Known(req)) => assert!(matches!(*req, Request::DaemonStatus)),
            other => panic!("expected Known(DaemonStatus), got {other:?}"),
        }
    }
}
