//! RF-Limiter: a lookahead true-peak limiter.
//!
//! * [`lookahead`] holds the parts — the delay, the sample history, the
//!   sliding-window minimum and the box ramp — none of which allocate;
//! * [`Engine`] is the limiter: it measures, decides and applies.
//!
//! Two rules hold everywhere. Nothing allocates after activation. And every
//! constant that shapes the behaviour is named for what it is — a time, a
//! kernel width, a floor — so that changing it means changing a decision
//! rather than nudging a number.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod engine;
pub mod lookahead;
pub mod math;

pub use engine::{Engine, STATE_BYTES};
pub use lookahead::{DETECT_DELAY, LOOKAHEAD_MAX, MAXIMUM_SAMPLE_RATE};
