//! Pre-roll ring buffer.
//!
//! A small fixed-capacity ring buffer that retains the most recent N canonical
//! samples (16-bit mono PCM at 16 kHz). The daemon feeds the idle microphone
//! capture into this buffer between recordings so that, on RecordStart, the
//! last few hundred milliseconds of audio can be prepended to the new
//! recording — preventing the first syllable from being clipped.
//!
//! The buffer holds plain `i16` samples and lives entirely in memory; it is
//! continuously overwritten and is never persisted unless a recording actually
//! starts.

use crate::format::SampleRate;

/// A fixed-capacity ring buffer retaining the last `capacity` `i16` samples.
///
/// Pushing more samples than `capacity` overwrites the oldest. A `capacity` of
/// 0 makes every operation a no-op (the buffer always reports empty).
#[derive(Debug, Clone)]
pub struct PreRollBuffer {
    /// Backing storage, length == `capacity` once filled. We track `len` and
    /// `head` so the buffer can act as a circular queue without reallocating.
    buf: Vec<i16>,
    /// Maximum number of samples retained.
    capacity: usize,
    /// Number of valid samples currently stored (`<= capacity`).
    len: usize,
    /// Index of the *oldest* sample within `buf` (only meaningful once full).
    head: usize,
}

impl PreRollBuffer {
    /// Create a ring buffer that retains the last `capacity` samples.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
            capacity,
            len: 0,
            head: 0,
        }
    }

    /// Create a ring buffer sized to hold the last `ms` milliseconds of audio
    /// at the given sample rate (mono). Rounds down to whole samples.
    pub fn with_duration_ms(ms: u32, rate: SampleRate) -> Self {
        let capacity = (ms as u64 * rate.as_u32() as u64 / 1000) as usize;
        Self::with_capacity(capacity)
    }

    /// Maximum number of samples the buffer can retain.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of samples currently retained.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the buffer holds no samples.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append a block of samples, overwriting the oldest as needed so that only
    /// the most recent `capacity` samples are retained.
    pub fn push(&mut self, block: &[i16]) {
        if self.capacity == 0 {
            return;
        }

        // If the incoming block alone meets or exceeds capacity, only its tail
        // can survive — reset and keep the last `capacity` samples.
        if block.len() >= self.capacity {
            let tail = &block[block.len() - self.capacity..];
            self.buf.clear();
            self.buf.extend_from_slice(tail);
            self.len = self.capacity;
            self.head = 0;
            return;
        }

        for &s in block {
            if self.len < self.capacity {
                // Still filling: append linearly. `head` stays at 0.
                self.buf.push(s);
                self.len += 1;
            } else {
                // Full: overwrite the oldest sample and advance the head.
                self.buf[self.head] = s;
                self.head = (self.head + 1) % self.capacity;
            }
        }
    }

    /// Drain the buffer into a freshly allocated `Vec`, ordered oldest →
    /// newest, and reset the buffer to empty.
    pub fn drain(&mut self) -> Vec<i16> {
        let out = self.to_vec();
        self.clear();
        out
    }

    /// Copy the retained samples (oldest → newest) without consuming them.
    pub fn to_vec(&self) -> Vec<i16> {
        if self.len < self.capacity {
            // Not yet wrapped: storage is already in order.
            self.buf.clone()
        } else {
            // Wrapped: stitch [head..end] + [0..head].
            let mut out = Vec::with_capacity(self.len);
            out.extend_from_slice(&self.buf[self.head..]);
            out.extend_from_slice(&self.buf[..self.head]);
            out
        }
    }

    /// Discard all retained samples.
    pub fn clear(&mut self) {
        self.buf.clear();
        self.len = 0;
        self.head = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_capacity_is_a_no_op() {
        let mut rb = PreRollBuffer::with_capacity(0);
        rb.push(&[1, 2, 3, 4]);
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);
        assert_eq!(rb.to_vec(), Vec::<i16>::new());
        assert_eq!(rb.drain(), Vec::<i16>::new());
    }

    #[test]
    fn retains_only_the_last_n_when_overfilled() {
        let mut rb = PreRollBuffer::with_capacity(4);
        rb.push(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(rb.len(), 4);
        // Oldest (1, 2) dropped; newest four retained in order.
        assert_eq!(rb.to_vec(), vec![3, 4, 5, 6]);
    }

    #[test]
    fn fills_without_wrapping_when_under_capacity() {
        let mut rb = PreRollBuffer::with_capacity(8);
        rb.push(&[1, 2, 3]);
        assert_eq!(rb.len(), 3);
        assert!(!rb.is_empty());
        assert_eq!(rb.to_vec(), vec![1, 2, 3]);
    }

    #[test]
    fn overwrites_oldest_across_multiple_pushes() {
        let mut rb = PreRollBuffer::with_capacity(4);
        rb.push(&[1, 2, 3]); // [1,2,3]
        rb.push(&[4, 5]); // capacity hit, then wrap: drop 1 -> [2,3,4,5]
        assert_eq!(rb.to_vec(), vec![2, 3, 4, 5]);
        rb.push(&[6]); // drop 2 -> [3,4,5,6]
        assert_eq!(rb.to_vec(), vec![3, 4, 5, 6]);
        rb.push(&[7, 8]); // drop 3,4 -> [5,6,7,8]
        assert_eq!(rb.to_vec(), vec![5, 6, 7, 8]);
    }

    #[test]
    fn input_exactly_capacity_keeps_all() {
        let mut rb = PreRollBuffer::with_capacity(3);
        rb.push(&[10, 20, 30]);
        assert_eq!(rb.to_vec(), vec![10, 20, 30]);
        // A second exact-capacity block fully replaces the contents.
        rb.push(&[40, 50, 60]);
        assert_eq!(rb.to_vec(), vec![40, 50, 60]);
    }

    #[test]
    fn input_smaller_than_buffer_accumulates() {
        let mut rb = PreRollBuffer::with_capacity(5);
        rb.push(&[1]);
        rb.push(&[2]);
        rb.push(&[3]);
        assert_eq!(rb.to_vec(), vec![1, 2, 3]);
        assert_eq!(rb.len(), 3);
    }

    #[test]
    fn drain_returns_contents_and_empties() {
        let mut rb = PreRollBuffer::with_capacity(4);
        rb.push(&[1, 2, 3, 4, 5]); // -> [2,3,4,5]
        let drained = rb.drain();
        assert_eq!(drained, vec![2, 3, 4, 5]);
        assert!(rb.is_empty());
        assert_eq!(rb.to_vec(), Vec::<i16>::new());
        // After draining, the buffer is reusable.
        rb.push(&[9, 9]);
        assert_eq!(rb.to_vec(), vec![9, 9]);
    }

    #[test]
    fn clear_empties_the_buffer() {
        let mut rb = PreRollBuffer::with_capacity(4);
        rb.push(&[1, 2, 3]);
        rb.clear();
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);
    }

    #[test]
    fn with_duration_ms_sizes_by_sample_rate() {
        // 500 ms at 16 kHz mono = 8000 samples.
        let rb = PreRollBuffer::with_duration_ms(500, SampleRate::HZ_16K);
        assert_eq!(rb.capacity(), 8000);
        // 0 ms => empty/no-op buffer.
        let rb = PreRollBuffer::with_duration_ms(0, SampleRate::HZ_16K);
        assert_eq!(rb.capacity(), 0);
    }

    #[test]
    fn wrapped_order_is_correct_after_many_small_pushes() {
        // Push 100 samples one at a time into a capacity-10 buffer; the result
        // must be the last 10 in order.
        let mut rb = PreRollBuffer::with_capacity(10);
        for i in 0..100i16 {
            rb.push(&[i]);
        }
        let expected: Vec<i16> = (90..100).collect();
        assert_eq!(rb.to_vec(), expected);
    }
}
