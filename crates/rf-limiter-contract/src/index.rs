//! Named parameter indexes.
//!
//! The engine and the packager both address parameters by number. Naming them
//! here — and asserting in tests that each name still points at the identifier
//! it claims — keeps a renumbering from quietly rewiring a knob.

pub const INPUT: u32 = 0;
pub const CEILING: u32 = 1;
pub const RELEASE: u32 = 2;
pub const AUTO_RELEASE: u32 = 3;
pub const LOOKAHEAD: u32 = 4;
pub const LINK: u32 = 5;
pub const TRUE_PEAK: u32 = 6;
pub const OUTPUT: u32 = 7;
/// Read-only: how much gain the limiter is taking away, in dB at or below 0.
pub const REDUCTION: u32 = 8;
pub const DELTA_LISTEN: u32 = 9;
/// Read-only peak level after input drive and before limiting.
pub const INPUT_LEVEL: u32 = 10;
/// Read-only peak level at the final output.
pub const OUTPUT_LEVEL: u32 = 11;
