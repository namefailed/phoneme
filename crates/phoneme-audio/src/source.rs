//! Abstraction over an audio sample source.
//!
//! Production code uses [`CpalSource`] which wraps a CPAL input stream;
//! tests use [`SyntheticSource`] which is hand-fed sample buffers.

use crate::format::AudioConfig;
use phoneme_core::error::{Error, Result};
use std::sync::Arc;
use tokio::sync::mpsc;

/// One block of i16 samples (already converted to Phoneme's canonical format:
/// 16-bit mono PCM at 16 kHz).
pub type SampleBlock = Vec<i16>;

/// An asynchronous source of audio sample blocks. Implementations must convert
/// to Phoneme's canonical format (16-bit, 16 kHz, mono) before yielding.
#[async_trait::async_trait]
pub trait Source: Send {
    /// Configuration that this source produces. Always reports the canonical
    /// format that downstream consumers will see (after any internal
    /// conversion).
    fn config(&self) -> AudioConfig;

    /// Pull the next block of samples. Returns `Ok(None)` when the source has
    /// been stopped and drained.
    async fn next_block(&mut self) -> Result<Option<SampleBlock>>;

    /// Stop the underlying capture. After calling, `next_block` should return
    /// `Ok(None)` shortly.
    async fn stop(&mut self) -> Result<()>;
}

/// A synthetic source: backed by an mpsc channel that tests push samples into.
///
/// Closing the sender side causes `next_block` to return `None`.
pub struct SyntheticSource {
    cfg: AudioConfig,
    rx: mpsc::Receiver<SampleBlock>,
}

impl SyntheticSource {
    pub fn new(cfg: AudioConfig) -> (Self, SyntheticSink) {
        let (tx, rx) = mpsc::channel(64);
        (Self { cfg, rx }, SyntheticSink { tx })
    }
}

/// Companion handle that tests use to push samples and then close.
#[derive(Clone)]
pub struct SyntheticSink {
    tx: mpsc::Sender<SampleBlock>,
}

impl SyntheticSink {
    pub async fn push(&self, block: SampleBlock) -> Result<()> {
        self.tx
            .send(block)
            .await
            .map_err(|_| Error::Internal("synthetic sink dropped".into()))
    }

    /// Close the sink, causing the matched source to return `None`.
    pub fn close(self) {
        drop(self.tx);
    }
}

#[async_trait::async_trait]
impl Source for SyntheticSource {
    fn config(&self) -> AudioConfig {
        self.cfg
    }

    async fn next_block(&mut self) -> Result<Option<SampleBlock>> {
        Ok(self.rx.recv().await)
    }

    async fn stop(&mut self) -> Result<()> {
        self.rx.close();
        Ok(())
    }
}

/// Cancellation handle held by callers; dropping it tells the CPAL source to
/// stop the underlying stream.
pub type StopHandle = Arc<tokio::sync::Notify>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn synthetic_yields_blocks_then_none_on_close() {
        let cfg = AudioConfig::phoneme_default();
        let (mut src, sink) = SyntheticSource::new(cfg);
        sink.push(vec![1, 2, 3]).await.unwrap();
        sink.push(vec![4, 5, 6]).await.unwrap();
        sink.close();
        assert_eq!(src.next_block().await.unwrap(), Some(vec![1, 2, 3]));
        assert_eq!(src.next_block().await.unwrap(), Some(vec![4, 5, 6]));
        assert_eq!(src.next_block().await.unwrap(), None);
    }

    #[tokio::test]
    async fn synthetic_stop_drains_then_returns_none() {
        let cfg = AudioConfig::phoneme_default();
        let (mut src, _sink) = SyntheticSource::new(cfg);
        src.stop().await.unwrap();
        assert_eq!(src.next_block().await.unwrap(), None);
    }
}
