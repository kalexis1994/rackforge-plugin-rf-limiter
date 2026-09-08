//! The RackForge adapter.
//!
//! Everything musical lives in `rf-limiter-dsp`. This file only translates
//! between the host's block-based ABI and the engine's one-frame-in,
//! one-frame-out interface, and it holds the two rules the host cares about:
//! no allocation after activation, and no work in the audio callback that
//! could block.

#![cfg_attr(target_arch = "wasm32", no_std)]

use rackforge_plugin_sdk::{MidiEvent, ParameterEvent, Processor, export_processor};
use rf_limiter_dsp::Engine;

const MAX_INPUT_CHANNELS: u32 = 2;
const MAX_OUTPUT_CHANNELS: u32 = 2;

#[derive(Default)]
pub struct RfLimiterProcessor {
    engine: Engine,
}

impl Processor for RfLimiterProcessor {
    fn prepare(
        &mut self,
        sample_rate: f64,
        _maximum_frames: u32,
        input_channels: u32,
        output_channels: u32,
    ) -> bool {
        // A limiter with nothing coming in has nothing to hold down.
        if input_channels == 0 || input_channels > MAX_INPUT_CHANNELS {
            return false;
        }
        if output_channels == 0 || output_channels > MAX_OUTPUT_CHANNELS {
            return false;
        }
        self.engine.prepare(sample_rate)
    }

    fn set_parameter(&mut self, index: u32, value: f64) -> bool {
        self.engine.set_parameter(index, value)
    }

    fn get_parameter(&self, index: u32) -> Option<f64> {
        self.engine.parameter(index)
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn load_preset(&mut self, id: &str) -> bool {
        self.engine.load_preset(id)
    }

    fn save_state(&self, destination: &mut [u8]) -> Option<usize> {
        self.engine.save_state(destination)
    }

    fn load_state(&mut self, state: &[u8]) -> bool {
        self.engine.load_state(state)
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _midi: &[MidiEvent],
        parameters: &[ParameterEvent],
        frames: u32,
        input_channels: u32,
        output_channels: u32,
    ) {
        let input_channels = input_channels as usize;
        let output_channels = output_channels as usize;
        let mut parameter_index = 0;

        for frame in 0..frames as usize {
            // Sample-accurate automation: apply everything scheduled for this
            // frame before the frame is processed.
            while let Some(event) = parameters.get(parameter_index) {
                if event.frame as usize != frame {
                    break;
                }
                let _ = self.engine.set_parameter(event.index, event.value);
                parameter_index += 1;
            }

            let (left, right) = match input_channels {
                0 => (0.0, 0.0),
                1 => {
                    let mono = input.get(frame).copied().unwrap_or(0.0);
                    (mono, mono)
                }
                channels => {
                    let base = frame * channels;
                    (
                        input.get(base).copied().unwrap_or(0.0),
                        input.get(base + 1).copied().unwrap_or(0.0),
                    )
                }
            };

            let (left, right) = self.engine.process(left, right);
            let base = frame * output_channels;
            for channel in 0..output_channels {
                let value = if channel == 0 { left } else { right };
                if let Some(slot) = output.get_mut(base + channel) {
                    *slot = value;
                }
            }
        }
    }
}

export_processor!(
    RfLimiterProcessor,
    max_frames = 4096,
    max_input_channels = 2,
    max_output_channels = 2,
    max_midi_events = 64,
    max_parameter_events = 256,
    max_transfer_bytes = 4096
);

#[cfg(all(target_arch = "wasm32", not(test)))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_limiter_contract::index::{CEILING, INPUT, OUTPUT};
    use rf_limiter_dsp::STATE_BYTES;

    fn prepared() -> RfLimiterProcessor {
        let mut processor = RfLimiterProcessor::default();
        assert!(processor.prepare(48_000.0, 256, 2, 2));
        processor
    }

    /// A 440 Hz sine at `amplitude` through `blocks` blocks of 256; the peak
    /// of the second half.
    fn render(
        processor: &mut RfLimiterProcessor,
        amplitude: f32,
        blocks: usize,
        events: &[ParameterEvent],
    ) -> f32 {
        let mut input = [0.0_f32; 512];
        let mut output = [0.0_f32; 512];
        let mut peak = 0.0_f32;
        for block in 0..blocks {
            for frame in 0..256 {
                let position = (block * 256 + frame) as f32;
                let x =
                    amplitude * libm::sinf(core::f32::consts::TAU * 440.0 * position / 48_000.0);
                input[frame * 2] = x;
                input[frame * 2 + 1] = x;
            }
            let scheduled: &[ParameterEvent] = if block == 0 { events } else { &[] };
            processor.process(&input, &mut output, &[], scheduled, 256, 2, 2);
            if block >= blocks / 2 {
                for sample in output {
                    peak = peak.max(libm::fabsf(sample));
                }
            }
        }
        peak
    }

    #[test]
    fn it_refuses_a_configuration_it_cannot_serve() {
        let mut processor = RfLimiterProcessor::default();
        assert!(!processor.prepare(48_000.0, 256, 0, 2));
        assert!(!processor.prepare(48_000.0, 256, 3, 2));
        assert!(!processor.prepare(48_000.0, 256, 2, 3));
        assert!(!processor.prepare(192_000.0, 256, 2, 2));
        assert!(processor.prepare(48_000.0, 256, 1, 2));
    }

    #[test]
    fn a_quiet_signal_passes_at_unity_and_a_hot_one_is_held() {
        let mut processor = prepared();
        let quiet = render(&mut processor, 0.2, 40, &[]);
        assert!((quiet - 0.2).abs() < 0.002, "quiet {quiet}");
        assert!(processor.set_parameter(CEILING, -6.0));
        let ceiling = libm::powf(10.0, -6.0 / 20.0);
        let hot = render(&mut processor, 1.0, 40, &[]);
        assert!(
            hot <= ceiling * 1.001 && hot > ceiling * 0.95,
            "hot {hot} against {ceiling}"
        );
    }

    #[test]
    fn automation_lands_on_its_frame() {
        let mut processor = prepared();
        // An output trim of -20 dB scheduled at frame 128 of the first block
        // halves nothing before it and everything after.
        let event = ParameterEvent {
            frame: 128,
            index: OUTPUT,
            value: -20.0,
        };
        let peak = render(&mut processor, 0.1, 4, &[event]);
        let expected = 0.1 * libm::powf(10.0, -20.0 / 20.0);
        assert!(
            (peak - expected).abs() < 0.002,
            "peak {peak}, expected {expected}"
        );
        assert_eq!(processor.get_parameter(OUTPUT), Some(-20.0));
    }

    #[test]
    fn state_round_trips_and_a_bad_length_is_refused() {
        let mut processor = prepared();
        assert!(processor.set_parameter(INPUT, 6.5));
        let mut block = [0_u8; 4096];
        assert_eq!(processor.save_state(&mut block), Some(STATE_BYTES));
        let mut other = prepared();
        assert!(other.load_state(&block[..STATE_BYTES]));
        assert_eq!(other.get_parameter(INPUT), Some(6.5));
        assert!(!other.load_state(&block[..STATE_BYTES - 4]));
    }

    #[test]
    fn every_factory_setting_loads() {
        let mut processor = prepared();
        for preset in rf_limiter_contract::PRESETS.iter() {
            assert!(processor.load_preset(preset.id), "{}", preset.id);
        }
    }
}
