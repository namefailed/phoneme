//! Newline-delimited JSON codec for tokio_util.
//!
//! Frames messages as `serde_json::to_string(&value) + "\n"`. Decodes by
//! scanning for the next newline and parsing the line.

use bytes::{Buf, BytesMut};
use serde::{de::DeserializeOwned, Serialize};
use std::io;
use std::marker::PhantomData;
use tokio_util::codec::{Decoder, Encoder};

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
