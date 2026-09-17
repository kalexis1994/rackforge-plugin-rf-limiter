//! The limiter: a true-peak detector, a release envelope, a lookahead window
//! and a ramp, applied to audio held back exactly long enough.
//!
//! The gain for an output sample `m` is the mean, over the lookahead length,
//! of the smallest gain any sample in the lookahead ahead of it asks for. Each
//! term of that mean already includes sample `m`'s own requirement, so the
//! ramp is at the reduction *before* the peak arrives and a peak never gets
//! through; and because it is a mean rather than a step, the reduction lands
//! as a slope, which the ear reads as level rather than as a click.
//!
//! Two lookaheads of latency (plus the four samples the true-peak kernel
//! needs), which at the default two milliseconds is under five at 48 kHz.

use rf_limiter_contract::index::*;
use rf_limiter_contract::{PARAMETER_COUNT, PREVIOUS_PARAMETER_COUNTS, Settings, preset};

use crate::lookahead::{
    BoxAverage, DETECT_DELAY, Delay, History, LOOKAHEAD_MAX, MAXIMUM_SAMPLE_RATE, MinWindow,
};
use crate::math::{PI, abs, clamp, cos, db_to_gain, gain_to_db, one_pole, sanitise, sinc};

/// Length of the serialised state, in bytes.
pub const STATE_BYTES: usize = PARAMETER_COUNT * 4;

/// The programme-dependent release: a fast stage that lets a transient go
/// and a slow one that holds a sustained overshoot down, averaged. Their
/// times are the usual ones for a mastering limiter's "auto" position.
const AUTO_FAST_S: f32 = 0.05;
const AUTO_SLOW_S: f32 = 0.5;

/// How fast the gain-reduction meter lets go once the reduction has passed.
const METER_RELEASE_S: f32 = 0.6;

/// Below this the meter reads its floor rather than minus infinity.
const METER_FLOOR_DB: f32 = -30.0;
const LEVEL_FLOOR_DB: f32 = -60.0;
const LEVEL_CEILING_DB: f32 = 12.0;

/// The three interpolation phases between two samples, at four times the
/// rate: enough to see an inter-sample peak to within a tenth of a decibel.
const PHASES: [f32; 3] = [0.25, 0.5, 0.75];

#[derive(Default)]
struct Lane {
    delay: Delay,
    history: History,
    window: MinWindow,
    average: BoxAverage,
    env_fast: f32,
    env_slow: f32,
    env_fixed: f32,
}

impl Lane {
    fn rest(&mut self, length: usize) {
        self.delay.clear();
        self.history.clear();
        self.window.clear();
        self.average.reset(length, 1.0);
        self.env_fast = 1.0;
        self.env_slow = 1.0;
        self.env_fixed = 1.0;
    }
}

pub struct Engine {
    settings: Settings,
    sample_rate: f32,
    prepared: bool,

    input_gain: f32,
    ceiling: f32,
    output_gain: f32,
    lookahead: usize,
    auto_release: bool,
    linked: bool,
    true_peak: bool,
    delta_listen: bool,
    release_fixed: f32,
    release_fast: f32,
    release_slow: f32,
    meter_release: f32,

    /// The interpolation kernel, one row per phase.
    kernel: [[f32; 8]; 3],
    lanes: [Lane; 2],
    /// The smallest gain applied lately, letting go at the meter's rate.
    meter_gain: f32,
    meter_input: f32,
    meter_output: f32,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            sample_rate: 48_000.0,
            prepared: false,
            input_gain: 1.0,
            ceiling: 1.0,
            output_gain: 1.0,
            lookahead: 0,
            auto_release: true,
            linked: true,
            true_peak: true,
            delta_listen: false,
            release_fixed: 1.0,
            release_fast: 1.0,
            release_slow: 1.0,
            meter_release: 1.0,
            kernel: [[0.0; 8]; 3],
            lanes: [Lane::default(), Lane::default()],
            meter_gain: 1.0,
            meter_input: 0.0,
            meter_output: 0.0,
        }
    }
}

impl Engine {
    /// Sets the sample rate and puts every stage at rest. Refuses a rate the
    /// buffers were not sized for.
    pub fn prepare(&mut self, sample_rate: f64) -> bool {
        if !sample_rate.is_finite()
            || sample_rate <= 0.0
            || sample_rate > f64::from(MAXIMUM_SAMPLE_RATE)
        {
            return false;
        }
        self.sample_rate = sample_rate as f32;
        self.kernel = interpolation_kernel();
        self.prepared = true;
        self.apply_settings();
        self.reset();
        true
    }

    pub fn reset(&mut self) {
        let length = self.lookahead + 1;
        for lane in &mut self.lanes {
            lane.rest(length);
        }
        self.meter_gain = 1.0;
        self.meter_input = 0.0;
        self.meter_output = 0.0;
    }

    pub fn set_parameter(&mut self, index: u32, value: f64) -> bool {
        if !self.settings.set(index, value) {
            return false;
        }
        let lookahead_before = self.lookahead;
        self.apply_settings();
        if self.lookahead != lookahead_before {
            // A new lookahead is a new delay and a new window: start both
            // clean rather than reading through the seam.
            self.reset();
        }
        true
    }

    /// A parameter's value: the setting, or, for the meter, the reading.
    pub fn parameter(&self, index: u32) -> Option<f64> {
        match index {
            REDUCTION => {
                return Some(f64::from(clamp(
                    gain_to_db(self.meter_gain),
                    METER_FLOOR_DB,
                    0.0,
                )));
            }
            INPUT_LEVEL => {
                return Some(f64::from(clamp(
                    gain_to_db(self.meter_input),
                    LEVEL_FLOOR_DB,
                    LEVEL_CEILING_DB,
                )));
            }
            OUTPUT_LEVEL => {
                return Some(f64::from(clamp(
                    gain_to_db(self.meter_output),
                    LEVEL_FLOOR_DB,
                    LEVEL_CEILING_DB,
                )));
            }
            _ => {}
        }
        self.settings.get(index)
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The samples of delay between input and output at the current setting.
    pub fn latency(&self) -> usize {
        DETECT_DELAY + 2 * self.lookahead
    }

    pub fn load_preset(&mut self, id: &str) -> bool {
        let Some(settings) = preset::settings_for(id) else {
            return false;
        };
        self.settings = settings;
        self.apply_settings();
        self.reset();
        true
    }

    pub fn save_state(&self, destination: &mut [u8]) -> Option<usize> {
        let target = destination.get_mut(..STATE_BYTES)?;
        for (slot, value) in target
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.settings.as_array())
        {
            *slot = value.to_le_bytes();
        }
        Some(STATE_BYTES)
    }

    /// Restores a saved block, including one written by an earlier build.
    /// Its length identifies its layout; anything else is refused whole.
    pub fn load_state(&mut self, state: &[u8]) -> bool {
        if !state.len().is_multiple_of(4) {
            return false;
        }
        let count = state.len() / 4;
        if count != PARAMETER_COUNT && !PREVIOUS_PARAMETER_COUNTS.contains(&count) {
            return false;
        }
        let mut values = [0.0_f32; PARAMETER_COUNT];
        for (value, word) in values.iter_mut().zip(state.as_chunks::<4>().0) {
            *value = f32::from_le_bytes(*word);
        }
        let Some(settings) = Settings::from_slice(&values[..count]) else {
            return false;
        };
        self.settings = settings;
        self.apply_settings();
        self.reset();
        true
    }

    fn apply_settings(&mut self) {
        let settings = self.settings;
        self.input_gain = db_to_gain(settings.value(INPUT));
        self.ceiling = db_to_gain(settings.value(CEILING));
        self.output_gain = db_to_gain(settings.value(OUTPUT));
        self.auto_release = settings.engaged(AUTO_RELEASE);
        self.linked = settings.engaged(LINK);
        self.true_peak = settings.engaged(TRUE_PEAK);
        self.delta_listen = settings.engaged(DELTA_LISTEN);
        let rate = self.sample_rate;
        self.release_fixed = one_pole(settings.value(RELEASE) * 0.001, rate);
        self.release_fast = one_pole(AUTO_FAST_S, rate);
        self.release_slow = one_pole(AUTO_SLOW_S, rate);
        self.meter_release = one_pole(METER_RELEASE_S, rate);
        let lookahead = settings.value(LOOKAHEAD) * 0.001 * rate;
        self.lookahead = (lookahead + 0.5) as usize;
        if self.lookahead > LOOKAHEAD_MAX {
            self.lookahead = LOOKAHEAD_MAX;
        }
    }

    /// The peak the lane's sample four pushes back reaches, between samples
    /// included when true-peak detection is on, after the input gain.
    #[inline]
    fn peak(&self, lane: &Lane) -> f32 {
        let mut peak = abs(lane.history.at(3));
        if self.true_peak {
            for row in &self.kernel {
                let mut value = 0.0;
                for (k, coefficient) in row.iter().enumerate() {
                    value += coefficient * lane.history.at(k);
                }
                let value = abs(value);
                if value > peak {
                    peak = value;
                }
            }
        }
        peak * self.input_gain
    }

    /// One stereo frame in, one out. A mono input arrives on both sides.
    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let inputs = [sanitise(left), sanitise(right)];
        let mut wanted = [1.0_f32; 2];
        let mut input_peak = 0.0_f32;
        for (lane, input) in self.lanes.iter_mut().zip(inputs) {
            lane.delay.push(input);
            lane.history.push(input);
        }
        for (slot, lane) in wanted.iter_mut().zip(self.lanes.iter()) {
            let peak = self.peak(lane);
            input_peak = input_peak.max(peak);
            let projected = peak * self.output_gain;
            if projected > self.ceiling {
                *slot = self.ceiling / projected;
            }
        }
        if self.linked {
            let both = if wanted[0] < wanted[1] {
                wanted[0]
            } else {
                wanted[1]
            };
            wanted = [both; 2];
        }

        let width = self.lookahead as u32;
        let age = DETECT_DELAY + 2 * self.lookahead;
        let mut outputs = [0.0_f32; 2];
        let mut block_min = 1.0_f32;
        for ((lane, target), output) in self.lanes.iter_mut().zip(wanted).zip(outputs.iter_mut()) {
            // The release envelope lets the gain climb back towards one at the
            // chosen rate, and drops to any smaller demand at once.
            lane.env_fixed += (1.0 - lane.env_fixed) * self.release_fixed;
            lane.env_fast += (1.0 - lane.env_fast) * self.release_fast;
            lane.env_slow += (1.0 - lane.env_slow) * self.release_slow;
            if target < lane.env_fixed {
                lane.env_fixed = target;
            }
            if target < lane.env_fast {
                lane.env_fast = target;
            }
            if target < lane.env_slow {
                lane.env_slow = target;
            }
            let envelope = if self.auto_release {
                0.5 * (lane.env_fast + lane.env_slow)
            } else {
                lane.env_fixed
            };
            let ahead = lane.window.push(envelope, width);
            let gain = lane.average.push(ahead);
            if gain < block_min {
                block_min = gain;
            }
            let dry = lane.delay.read(age) * self.input_gain * self.output_gain;
            let sample = dry * gain;
            // The ramp holds the ceiling by construction; the clamp is the
            // net under it, for the rounding the mean leaves behind.
            let held = clamp(sample, -self.ceiling, self.ceiling);
            *output = sanitise(if self.delta_listen {
                clamp(dry - held, -self.ceiling, self.ceiling)
            } else {
                held
            });
        }

        self.meter_gain += (1.0 - self.meter_gain) * self.meter_release;
        if block_min < self.meter_gain {
            self.meter_gain = block_min;
        }
        self.meter_input += (0.0 - self.meter_input) * self.meter_release;
        if input_peak > self.meter_input {
            self.meter_input = input_peak;
        }
        let output_peak = abs(outputs[0]).max(abs(outputs[1]));
        self.meter_output += (0.0 - self.meter_output) * self.meter_release;
        if output_peak > self.meter_output {
            self.meter_output = output_peak;
        }
        (outputs[0], outputs[1])
    }
}

/// Eight-tap windowed-sinc rows for the three quarter phases between the
/// fourth-newest sample and the one after it. Each row sums to one, so a
/// steady signal interpolates to itself.
fn interpolation_kernel() -> [[f32; 8]; 3] {
    let mut kernel = [[0.0_f32; 8]; 3];
    for (row, phase) in kernel.iter_mut().zip(PHASES) {
        let mut sum = 0.0;
        for (k, tap) in row.iter_mut().enumerate() {
            let position = k as f32 + 0.5;
            let window =
                0.42 - 0.5 * cos(2.0 * PI * position / 8.0) + 0.08 * cos(4.0 * PI * position / 8.0);
            *tap = window * sinc(phase - (k as f32 - 3.0));
            sum += *tap;
        }
        for tap in row.iter_mut() {
            *tap /= sum;
        }
    }
    kernel
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::sin;

    const RATE: f32 = 48_000.0;

    fn prepared() -> Engine {
        let mut engine = Engine::default();
        assert!(engine.prepare(f64::from(RATE)));
        engine
    }

    /// Runs a stereo sine of `amplitude` at `frequency` for `seconds` and
    /// returns the output peak after the first tenth of a second.
    fn peak_of(
        engine: &mut Engine,
        amplitude: f32,
        frequency: f32,
        phase: f32,
        seconds: f32,
    ) -> f32 {
        let total = (seconds * RATE) as usize;
        let settle = (0.1 * RATE) as usize;
        let mut peak = 0.0_f32;
        for n in 0..total {
            let x = amplitude * sin(2.0 * PI * frequency * n as f32 / RATE + phase);
            let (l, r) = engine.process(x, x);
            if n > settle {
                peak = peak.max(abs(l)).max(abs(r));
            }
        }
        peak
    }

    #[test]
    fn it_refuses_a_rate_the_buffers_were_not_sized_for() {
        let mut engine = Engine::default();
        assert!(!engine.prepare(192_000.0));
        assert!(engine.prepare(96_000.0));
    }

    #[test]
    fn a_signal_under_the_ceiling_passes_at_unity() {
        let mut engine = prepared();
        let peak = peak_of(&mut engine, 0.25, 440.0, 0.0, 0.5);
        assert!((peak - 0.25).abs() < 0.002, "peak {peak}");
        assert!(engine.parameter(REDUCTION).unwrap() > -0.05);
    }

    #[test]
    fn a_hot_signal_never_crosses_the_ceiling() {
        let mut engine = prepared();
        assert!(engine.set_parameter(CEILING, -1.0));
        let ceiling = db_to_gain(-1.0);
        for frequency in [55.0, 440.0, 3_000.0] {
            let peak = peak_of(&mut engine, 2.0, frequency, 0.3, 0.5);
            assert!(
                peak <= ceiling * 1.0005,
                "{frequency} Hz: peak {peak} over {ceiling}"
            );
            assert!(
                peak > ceiling * 0.9,
                "{frequency} Hz: peak {peak} well under the ceiling"
            );
        }
        // Six decibels over the ceiling asks for six decibels off.
        let reduction = engine.parameter(REDUCTION).unwrap();
        assert!((-7.5..=-6.0).contains(&reduction), "reduction {reduction}");
    }

    #[test]
    fn output_trim_is_accounted_for_before_the_final_ceiling() {
        let mut engine = prepared();
        assert!(engine.set_parameter(CEILING, -3.0));
        assert!(engine.set_parameter(OUTPUT, 12.0));
        let peak = peak_of(&mut engine, 0.5, 440.0, 0.2, 0.5);
        let ceiling = db_to_gain(-3.0);
        assert!(peak <= ceiling * 1.0005, "peak {peak} over {ceiling}");
        assert!(peak > ceiling * 0.9, "peak {peak} well under {ceiling}");
    }

    #[test]
    fn level_meters_follow_the_driven_input_and_final_output() {
        let mut engine = prepared();
        assert!(engine.set_parameter(INPUT, 6.0));
        assert!(engine.set_parameter(CEILING, -6.0));
        peak_of(&mut engine, 0.5, 440.0, 0.0, 0.5);
        let input = engine.parameter(INPUT_LEVEL).unwrap();
        let output = engine.parameter(OUTPUT_LEVEL).unwrap();
        assert!((-0.5..=0.5).contains(&input), "input meter {input}");
        assert!((-6.5..=-5.5).contains(&output), "output meter {output}");
    }

    #[test]
    fn delta_listen_is_silent_below_the_ceiling_and_reveals_limiting() {
        let mut engine = prepared();
        assert!(engine.set_parameter(DELTA_LISTEN, 1.0));
        let quiet = peak_of(&mut engine, 0.25, 440.0, 0.0, 0.5);
        assert!(quiet < 1.0e-4, "quiet delta {quiet}");

        engine.reset();
        let removed = peak_of(&mut engine, 2.0, 440.0, 0.0, 0.5);
        assert!(removed > 0.5, "removed signal {removed}");
        assert!(removed <= db_to_gain(-1.0) * 1.0005);
    }

    #[test]
    fn the_lookahead_holds_the_ceiling_at_every_setting() {
        for lookahead_ms in [0.0, 0.5, 2.0, 10.0] {
            let mut engine = prepared();
            assert!(engine.set_parameter(LOOKAHEAD, lookahead_ms));
            assert!(engine.set_parameter(CEILING, -3.0));
            assert!(engine.set_parameter(TRUE_PEAK, 0.0));
            let ceiling = db_to_gain(-3.0);
            // A burst: silence, then a hard step to full scale.
            let mut peak = 0.0_f32;
            for n in 0..(RATE as usize / 2) {
                let x = if n > RATE as usize / 4 { 1.0 } else { 0.0 };
                let (l, _) = engine.process(x, x);
                peak = peak.max(abs(l));
            }
            assert!(peak <= ceiling * 1.0005, "{lookahead_ms} ms: peak {peak}");
        }
    }

    #[test]
    fn true_peak_sees_what_lies_between_samples() {
        // A sine at a quarter of the rate, sampled at odd eighths of a cycle:
        // every sample sits at 0.707 of the amplitude, the waveform reaches
        // all of it between them.
        let amplitude = 1.0;
        let expected_sample_peak = amplitude * core::f32::consts::FRAC_1_SQRT_2;

        let mut blind = prepared();
        assert!(blind.set_parameter(TRUE_PEAK, 0.0));
        assert!(blind.set_parameter(CEILING, -1.0));
        let blind_peak = peak_of(&mut blind, amplitude, RATE / 4.0, PI / 4.0, 0.5);
        assert!(
            (blind_peak - expected_sample_peak).abs() < 0.01,
            "blind {blind_peak}"
        );

        let mut sighted = prepared();
        assert!(sighted.set_parameter(CEILING, -1.0));
        let sighted_peak = peak_of(&mut sighted, amplitude, RATE / 4.0, PI / 4.0, 0.5);
        // The true peak is 0 dBTP; the ceiling is -1 dBTP; the samples come
        // down by about a decibel.
        let expected = expected_sample_peak * db_to_gain(-1.0);
        assert!(
            (sighted_peak - expected).abs() < 0.02,
            "sighted {sighted_peak}, expected {expected}"
        );
    }

    #[test]
    fn the_meter_lets_go_after_the_peak() {
        let mut engine = prepared();
        peak_of(&mut engine, 2.0, 440.0, 0.0, 0.3);
        assert!(engine.parameter(REDUCTION).unwrap() < -5.0);
        for _ in 0..(3.0 * RATE) as usize {
            engine.process(0.0, 0.0);
        }
        assert!(engine.parameter(REDUCTION).unwrap() > -0.1);
    }

    #[test]
    fn unlinked_sides_are_limited_on_their_own() {
        let mut engine = prepared();
        assert!(engine.set_parameter(LINK, 0.0));
        assert!(engine.set_parameter(CEILING, -6.0));
        let total = (0.5 * RATE) as usize;
        let mut right_peak = 0.0_f32;
        for n in (0..total).skip(4_800) {
            let x = 2.0 * sin(2.0 * PI * 440.0 * n as f32 / RATE);
            let (_, r) = engine.process(x, 0.1 * x);
            right_peak = right_peak.max(abs(r));
        }
        // The quiet side is a fifth of the loud one, which is under the
        // ceiling: unlinked, it is untouched.
        assert!((right_peak - 0.2).abs() < 0.01, "right {right_peak}");
    }

    #[test]
    fn state_round_trips_and_a_bad_length_is_refused() {
        let mut engine = prepared();
        assert!(engine.set_parameter(CEILING, -4.5));
        assert!(engine.set_parameter(LINK, 0.0));
        let mut block = [0_u8; STATE_BYTES];
        assert_eq!(engine.save_state(&mut block), Some(STATE_BYTES));
        let mut other = prepared();
        assert!(other.load_state(&block));
        assert_eq!(other.parameter(CEILING), Some(-4.5));
        assert_eq!(other.parameter(LINK), Some(0.0));
        assert!(!other.load_state(&block[..7]));
        assert!(!other.load_state(&block[..8]));
    }

    #[test]
    fn version_one_state_loads_with_delta_off_and_live_meters_at_rest() {
        let defaults = Settings::default().as_array();
        let mut old = [0_u8; 9 * 4];
        for (slot, value) in old.as_chunks_mut::<4>().0.iter_mut().zip(defaults) {
            *slot = value.to_le_bytes();
        }
        let mut engine = prepared();
        assert!(engine.load_state(&old));
        assert_eq!(engine.parameter(DELTA_LISTEN), Some(0.0));
        assert_eq!(engine.parameter(INPUT_LEVEL), Some(-60.0));
        assert_eq!(engine.parameter(OUTPUT_LEVEL), Some(-60.0));
    }

    #[test]
    fn every_preset_loads() {
        let mut engine = prepared();
        for preset in rf_limiter_contract::PRESETS.iter() {
            assert!(engine.load_preset(preset.id), "{}", preset.id);
        }
        assert!(!engine.load_preset("nowhere"));
    }

    #[test]
    fn the_kernel_rows_interpolate_a_constant_to_itself() {
        for row in interpolation_kernel() {
            let sum: f32 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1.0e-5);
        }
    }
}
