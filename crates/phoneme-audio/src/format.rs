//! Audio format types shared across capture, encoding, and conversion.

use serde::{Deserialize, Serialize};

/// Sample rate in Hz. Phoneme always records at 16 kHz, but this type lets us
/// be explicit about what the CPAL device offered vs. what we save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SampleRate(
    /// Rate in samples per second (Hz).
    pub u32,
);

impl SampleRate {
    /// 16 kHz — Phoneme's canonical recording rate (the rate the transcription
    /// models expect).
    pub const HZ_16K: Self = Self(16_000);
    /// 44.1 kHz — a common consumer-device capture rate seen on import.
    pub const HZ_44_1K: Self = Self(44_100);
    /// 48 kHz — the typical WASAPI mix-format rate offered by Windows devices.
    pub const HZ_48K: Self = Self(48_000);

    /// The rate as a plain `u32` in Hz.
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

/// Number of audio channels. Phoneme always saves mono.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Channels(
    /// Channel count. For interleaved buffers this is the stride between
    /// successive samples of the same channel.
    pub u8,
);

impl Channels {
    /// One channel.
    pub const MONO: Self = Self(1);
    /// Two channels (interleaved left/right when carried in a sample buffer).
    pub const STEREO: Self = Self(2);

    /// The channel count as a plain `u8`.
    pub fn as_u8(&self) -> u8 {
        self.0
    }
}

/// Phoneme's canonical recording format: 16-bit PCM, 16 kHz mono.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioConfig {
    /// Samples per second of the buffer this config describes.
    pub sample_rate: SampleRate,
    /// Channel layout of the buffer this config describes.
    pub channels: Channels,
}

impl AudioConfig {
    /// Phoneme's canonical recording format: 16 kHz mono. Every recording and
    /// every imported file ends up in this shape before transcription.
    pub const fn phoneme_default() -> Self {
        Self {
            sample_rate: SampleRate::HZ_16K,
            channels: Channels::MONO,
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self::phoneme_default()
    }
}
