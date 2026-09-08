//! The parts a lookahead limiter is made of, each without allocation.
//!
//! * [`Delay`] holds the audio back while the gain is worked out ahead of it;
//! * [`History`] keeps the last eight samples for the true-peak interpolator;
//! * [`MinWindow`] answers "what is the smallest gain any sample in the next
//!   N will need" in constant time per sample, with a monotonic queue;
//! * [`BoxAverage`] turns that stepped answer into a ramp that reaches the
//!   reduction exactly when the peak arrives and never a sample late.
//!
//! Every buffer is sized for the longest lookahead at the highest sample rate
//! the plugin accepts, so activation never allocates and the audio callback
//! never can.

/// The highest sample rate the buffers are sized for. Activation above this is
/// refused rather than silently shortening the lookahead.
pub const MAXIMUM_SAMPLE_RATE: f32 = 96_000.0;

/// The longest lookahead, in samples: ten milliseconds at 96 kHz.
pub const LOOKAHEAD_MAX: usize = 960;

/// How many samples the true-peak interpolator looks past the sample it
/// reports on: the centre of an eight-tap kernel.
pub const DETECT_DELAY: usize = 4;

/// The audio delay ring: the detector's delay plus twice the longest
/// lookahead, rounded up to a power of two so the index is a mask.
pub const DELAY_SAMPLES: usize = 2048;

/// Room for the longest window plus one, rounded up to a power of two.
pub const WINDOW_CAPACITY: usize = 1024;

const _: () = assert!(DETECT_DELAY + 2 * LOOKAHEAD_MAX < DELAY_SAMPLES);
const _: () = assert!(LOOKAHEAD_MAX + 1 < WINDOW_CAPACITY);

pub struct Delay {
    buffer: [f32; DELAY_SAMPLES],
    write: usize,
}

impl Default for Delay {
    fn default() -> Self {
        Self {
            buffer: [0.0; DELAY_SAMPLES],
            write: 0,
        }
    }
}

impl Delay {
    #[inline]
    pub fn push(&mut self, sample: f32) {
        self.buffer[self.write] = sample;
        self.write = (self.write + 1) & (DELAY_SAMPLES - 1);
    }

    /// The sample pushed `age` pushes ago; `0` is the one just pushed.
    #[inline]
    pub fn read(&self, age: usize) -> f32 {
        debug_assert!(age < DELAY_SAMPLES);
        self.buffer[(self.write + DELAY_SAMPLES - 1 - age) & (DELAY_SAMPLES - 1)]
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
    }
}

/// The last eight samples, oldest first.
#[derive(Default)]
pub struct History {
    taps: [f32; 8],
    write: usize,
}

impl History {
    #[inline]
    pub fn push(&mut self, sample: f32) {
        self.taps[self.write] = sample;
        self.write = (self.write + 1) & 7;
    }

    /// Tap `k`, with `0` the oldest of the eight and `7` the newest.
    #[inline]
    pub fn at(&self, k: usize) -> f32 {
        self.taps[(self.write + k) & 7]
    }

    pub fn clear(&mut self) {
        self.taps.fill(0.0);
        self.write = 0;
    }
}

/// The minimum of the last `width + 1` values pushed, in amortised constant
/// time: a monotonic queue of candidates, each stamped with when it arrived.
pub struct MinWindow {
    values: [f32; WINDOW_CAPACITY],
    stamps: [u32; WINDOW_CAPACITY],
    head: usize,
    len: usize,
    clock: u32,
}

impl Default for MinWindow {
    fn default() -> Self {
        Self {
            values: [1.0; WINDOW_CAPACITY],
            stamps: [0; WINDOW_CAPACITY],
            head: 0,
            len: 0,
            clock: 0,
        }
    }
}

impl MinWindow {
    /// Pushes `value` and returns the minimum over it and the `width`
    /// values before it. `width` must stay below the capacity.
    #[inline]
    pub fn push(&mut self, value: f32, width: u32) -> f32 {
        // A candidate no smaller than the newcomer can never be the minimum
        // again while the newcomer is in the window: drop it.
        while self.len > 0 {
            let back = (self.head + self.len - 1) & (WINDOW_CAPACITY - 1);
            if self.values[back] >= value {
                self.len -= 1;
            } else {
                break;
            }
        }
        let slot = (self.head + self.len) & (WINDOW_CAPACITY - 1);
        self.values[slot] = value;
        self.stamps[slot] = self.clock;
        self.len += 1;
        // The front expires when it falls out of the window.
        while self.len > 0 && self.clock.wrapping_sub(self.stamps[self.head]) > width {
            self.head = (self.head + 1) & (WINDOW_CAPACITY - 1);
            self.len -= 1;
        }
        self.clock = self.clock.wrapping_add(1);
        self.values[self.head]
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.clock = 0;
    }
}

/// The mean of the last `length` values pushed.
///
/// The running sum is kept in double precision and rebuilt from the ring now
/// and then, so an hour of tiny rounding errors cannot drift the gain.
pub struct BoxAverage {
    ring: [f32; WINDOW_CAPACITY],
    write: usize,
    length: usize,
    sum: f64,
    since_rebuild: u32,
}

impl Default for BoxAverage {
    fn default() -> Self {
        Self {
            ring: [1.0; WINDOW_CAPACITY],
            write: 0,
            length: 1,
            sum: 1.0,
            since_rebuild: 0,
        }
    }
}

impl BoxAverage {
    /// Sets the length and fills the ring with `value`, so the average starts
    /// at rest rather than climbing out of whatever was there.
    pub fn reset(&mut self, length: usize, value: f32) {
        let length = length.clamp(1, WINDOW_CAPACITY);
        self.length = length;
        self.ring[..length].fill(value);
        self.write = 0;
        self.sum = f64::from(value) * length as f64;
        self.since_rebuild = 0;
    }

    #[inline]
    pub fn push(&mut self, value: f32) -> f32 {
        let leaving = self.ring[self.write];
        self.ring[self.write] = value;
        self.write += 1;
        if self.write >= self.length {
            self.write = 0;
        }
        self.sum += f64::from(value) - f64::from(leaving);
        self.since_rebuild += 1;
        if self.since_rebuild >= 4096 {
            self.since_rebuild = 0;
            self.sum = self.ring[..self.length]
                .iter()
                .map(|sample| f64::from(*sample))
                .sum();
        }
        (self.sum / self.length as f64) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delay_reads_what_was_pushed_that_many_samples_ago() {
        let mut delay = Delay::default();
        for n in 0..3000 {
            delay.push(n as f32);
        }
        assert_eq!(delay.read(0), 2999.0);
        assert_eq!(delay.read(1924), 2999.0 - 1924.0);
    }

    #[test]
    fn the_window_minimum_matches_a_brute_force_scan() {
        let mut window = MinWindow::default();
        let mut seen = std::vec::Vec::new();
        let width = 7;
        let mut seed = 12345_u32;
        for _ in 0..5000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let value = (seed >> 8) as f32 / (1u32 << 24) as f32;
            seen.push(value);
            let fast = window.push(value, width);
            let from = seen.len().saturating_sub(width as usize + 1);
            let slow = seen[from..].iter().copied().fold(f32::INFINITY, f32::min);
            assert_eq!(fast, slow);
        }
    }

    #[test]
    fn a_zero_width_window_is_the_value_itself() {
        let mut window = MinWindow::default();
        assert_eq!(window.push(0.5, 0), 0.5);
        assert_eq!(window.push(0.9, 0), 0.9);
        assert_eq!(window.push(0.1, 0), 0.1);
    }

    #[test]
    fn the_box_average_ramps_and_settles() {
        let mut average = BoxAverage::default();
        average.reset(4, 1.0);
        assert_eq!(average.push(0.0), 0.75);
        assert_eq!(average.push(0.0), 0.5);
        assert_eq!(average.push(0.0), 0.25);
        assert_eq!(average.push(0.0), 0.0);
        for _ in 0..10_000 {
            average.push(0.5);
        }
        assert!((average.push(0.5) - 0.5).abs() < 1.0e-6);
    }
}
