//! Audio format types shared across capture, encoding, and conversion.

use serde::{Deserialize, Serialize};

/// Sample rate in Hz. Phoneme always records at 16 kHz, but this type lets us
/// be explicit about what the CPAL device offered vs. what we save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SampleRate(pub u32);

impl SampleRate {
    pub const HZ_16K: Self = Self(16_000);
    pub const HZ_44_1K: Self = Self(44_100);
    pub const HZ_48K: Self = Self(48_000);

    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

/// Number of audio channels. Phoneme always saves mono.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Channels(pub u8);

impl Channels {
    pub const MONO: Self = Self(1);
    pub const STEREO: Self = Self(2);

    pub fn as_u8(&self) -> u8 {
        self.0
    }
}

/// Phoneme's canonical recording format: 16-bit PCM, 16 kHz mono.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioConfig {
    pub sample_rate: SampleRate,
    pub channels: Channels,
}

impl AudioConfig {
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
