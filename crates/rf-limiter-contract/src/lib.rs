//! The public contract of RF-Limiter: every parameter the host can see and
//! the flat settings block that becomes plugin state.
//!
//! Nothing here performs audio work. The engine reads this table, the packager
//! renders `metadata/parameters.json` from it, and the web surface receives the
//! same schema back from the host, so the three can never drift apart.

#![no_std]

pub mod index;
pub mod preset;

pub use preset::{PRESET_COUNT, PRESETS, Preset, settings_for};

/// Number of public parameters. Also the length of the state block in `f32`s.
pub const PARAMETER_COUNT: usize = 12;

/// The parameter count of each earlier state layout, so a block saved by an
/// older build can still be read. Length is the only thing that identifies a
/// layout here, which is why parameters are only ever appended.
pub const PREVIOUS_PARAMETER_COUNTS: [usize; 1] = [9];

/// Editor pages. RackForge renders them in `order`; the web surface uses the
/// same identifiers to group its controls.
pub struct PageSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub order: i32,
}

pub const PAGES: [PageSpec; 2] = [
    PageSpec {
        id: "limiter",
        name: "Limiter",
        order: 0,
    },
    PageSpec {
        id: "output",
        name: "Output",
        order: 1,
    },
];

/// How a continuous control travels. Logarithmic needs a minimum above zero.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Taper {
    Linear,
    Logarithmic,
}

/// The parameter kinds RF-Limiter uses: a subset of the RackForge parameter
/// schema. A meter is read-only and reports what the engine is doing; a Rack
/// Slot editing an isolated instance sees it at rest, the PLAY chain sees it
/// live.
#[derive(Clone, Copy)]
pub enum Kind {
    Float {
        minimum: f32,
        maximum: f32,
        default: f32,
        step: f32,
        unit: Option<&'static str>,
        taper: Taper,
    },
    Boolean {
        default: bool,
    },
    Enum {
        default: u32,
        choices: &'static [&'static str],
    },
    Meter {
        minimum: f32,
        maximum: f32,
        unit: Option<&'static str>,
    },
}

/// Control hint published to RackForge surfaces (LITTLE, controller mappings).
#[derive(Clone, Copy)]
pub enum Control {
    Knob,
    Toggle,
    List,
    Meter,
}

impl Control {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Knob => "knob",
            Self::Toggle => "toggle",
            Self::List => "list",
            Self::Meter => "meter",
        }
    }
}

pub struct ParameterSpec {
    pub index: u32,
    pub id: &'static str,
    pub name: &'static str,
    pub page: &'static str,
    pub order: i32,
    pub kind: Kind,
    pub control: Control,
}

impl ParameterSpec {
    pub const fn default_value(&self) -> f32 {
        match self.kind {
            Kind::Float { default, .. } => default,
            Kind::Boolean { default } => {
                if default {
                    1.0
                } else {
                    0.0
                }
            }
            Kind::Enum { default, .. } => default as f32,
            Kind::Meter { maximum, .. } => maximum,
        }
    }

    pub const fn is_meter(&self) -> bool {
        matches!(self.kind, Kind::Meter { .. })
    }

    /// Accepts a host value and returns the canonical stored value, or `None`
    /// when the value is outside the declared contract. RackForge validates
    /// first; this is the engine's own guard so a broken caller cannot poison
    /// the audio thread.
    pub fn canonicalize(&self, value: f64) -> Option<f32> {
        if !value.is_finite() {
            return None;
        }
        match self.kind {
            Kind::Float {
                minimum, maximum, ..
            }
            | Kind::Meter {
                minimum, maximum, ..
            } => {
                let value = value as f32;
                (value >= minimum && value <= maximum).then_some(value)
            }
            Kind::Boolean { .. } => {
                if value == 0.0 {
                    Some(0.0)
                } else if value == 1.0 {
                    Some(1.0)
                } else {
                    None
                }
            }
            Kind::Enum { choices, .. } => {
                let rounded = value as i64;
                if value != rounded as f64 || rounded < 0 {
                    return None;
                }
                (rounded < choices.len() as i64).then_some(rounded as f32)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
const fn decibels(
    index: u32,
    id: &'static str,
    name: &'static str,
    page: &'static str,
    order: i32,
    minimum: f32,
    maximum: f32,
    default: f32,
) -> ParameterSpec {
    ParameterSpec {
        index,
        id,
        name,
        page,
        order,
        kind: Kind::Float {
            minimum,
            maximum,
            default,
            step: 0.1,
            unit: Some("dB"),
            taper: Taper::Linear,
        },
        control: Control::Knob,
    }
}

const fn switch(
    index: u32,
    id: &'static str,
    name: &'static str,
    page: &'static str,
    order: i32,
    default: bool,
) -> ParameterSpec {
    ParameterSpec {
        index,
        id,
        name,
        page,
        order,
        kind: Kind::Boolean { default },
        control: Control::Toggle,
    }
}

const fn meter(
    index: u32,
    id: &'static str,
    name: &'static str,
    order: i32,
    minimum: f32,
    maximum: f32,
) -> ParameterSpec {
    ParameterSpec {
        index,
        id,
        name,
        page: "output",
        order,
        kind: Kind::Meter {
            minimum,
            maximum,
            unit: Some("dB"),
        },
        control: Control::Meter,
    }
}

pub const PARAMETERS: [ParameterSpec; PARAMETER_COUNT] = [
    decibels(0, "limiter.input", "Input", "limiter", 0, -24.0, 24.0, 0.0),
    ParameterSpec {
        index: 1,
        id: "limiter.ceiling",
        name: "Ceiling",
        page: "limiter",
        order: 1,
        kind: Kind::Float {
            minimum: -20.0,
            maximum: 0.0,
            default: -1.0,
            step: 0.1,
            unit: Some("dBTP"),
            taper: Taper::Linear,
        },
        control: Control::Knob,
    },
    ParameterSpec {
        index: 2,
        id: "limiter.release",
        name: "Release",
        page: "limiter",
        order: 2,
        kind: Kind::Float {
            minimum: 10.0,
            maximum: 1000.0,
            default: 100.0,
            step: 1.0,
            unit: Some("ms"),
            taper: Taper::Logarithmic,
        },
        control: Control::Knob,
    },
    switch(
        3,
        "limiter.auto_release",
        "Auto Release",
        "limiter",
        3,
        true,
    ),
    ParameterSpec {
        index: 4,
        id: "limiter.lookahead",
        name: "Lookahead",
        page: "limiter",
        order: 4,
        kind: Kind::Float {
            minimum: 0.0,
            maximum: 10.0,
            default: 2.0,
            step: 0.1,
            unit: Some("ms"),
            taper: Taper::Linear,
        },
        control: Control::Knob,
    },
    switch(5, "limiter.link", "Stereo Link", "limiter", 5, true),
    switch(6, "limiter.true_peak", "True Peak", "limiter", 6, true),
    decibels(7, "output.trim", "Output", "output", 0, -24.0, 24.0, 0.0),
    ParameterSpec {
        index: 8,
        id: "output.reduction",
        name: "Gain Reduction",
        page: "output",
        order: 1,
        kind: Kind::Meter {
            minimum: -30.0,
            maximum: 0.0,
            unit: Some("dB"),
        },
        control: Control::Meter,
    },
    switch(9, "output.delta_listen", "Delta Listen", "output", 2, false),
    meter(10, "meter.input", "Input Peak", 3, -60.0, 12.0),
    meter(11, "meter.output", "Output Peak", 4, -60.0, 12.0),
];

/// The flat settings block: one `f32` per parameter, in index order. It is
/// the plugin's state, so its layout is the contract's.
#[derive(Clone, Copy)]
pub struct Settings {
    values: [f32; PARAMETER_COUNT],
}

impl Default for Settings {
    fn default() -> Self {
        let mut values = [0.0_f32; PARAMETER_COUNT];
        let mut index = 0;
        while index < PARAMETER_COUNT {
            values[index] = PARAMETERS[index].default_value();
            index += 1;
        }
        Self { values }
    }
}

impl Settings {
    /// Sets a control. A meter is the engine's to write, never the host's.
    pub fn set(&mut self, index: u32, value: f64) -> bool {
        let Some(spec) = PARAMETERS.get(index as usize) else {
            return false;
        };
        if spec.is_meter() {
            return false;
        }
        let Some(canonical) = spec.canonicalize(value) else {
            return false;
        };
        self.values[index as usize] = canonical;
        true
    }

    pub fn get(&self, index: u32) -> Option<f64> {
        self.values.get(index as usize).map(|value| *value as f64)
    }

    pub fn value(&self, index: u32) -> f32 {
        self.values[index as usize]
    }

    pub fn engaged(&self, index: u32) -> bool {
        self.values[index as usize] >= 0.5
    }

    pub fn as_array(&self) -> [f32; PARAMETER_COUNT] {
        self.values
    }

    pub fn from_array(values: [f32; PARAMETER_COUNT]) -> Option<Self> {
        Self::from_slice(&values)
    }

    /// Reads a state block that may have been written by an earlier build.
    ///
    /// Layouts are identified by length, and parameters are only ever appended,
    /// so a shorter block is an older one: its values are applied and anything
    /// added since keeps its default. A length that belongs to no layout, or a
    /// value outside the contract, is refused outright rather than partially
    /// applied. A meter's stored value is ignored: it is the engine's reading,
    /// not a setting.
    pub fn from_slice(values: &[f32]) -> Option<Self> {
        if values.len() != PARAMETER_COUNT && !PREVIOUS_PARAMETER_COUNTS.contains(&values.len()) {
            return None;
        }
        let mut settings = Self::default();
        for (index, value) in values.iter().enumerate() {
            if PARAMETERS[index].is_meter() {
                continue;
            }
            if !settings.set(index as u32, *value as f64) {
                return None;
            }
        }
        Some(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_parameter_declares_its_own_index_and_a_known_page() {
        for (position, spec) in PARAMETERS.iter().enumerate() {
            assert_eq!(spec.index as usize, position, "{}", spec.id);
            assert!(
                PAGES.iter().any(|page| page.id == spec.page),
                "{} refers to an undeclared page",
                spec.id
            );
        }
    }

    #[test]
    fn parameter_identifiers_are_unique_and_host_legal() {
        for (position, spec) in PARAMETERS.iter().enumerate() {
            assert!(
                spec.id.bytes().all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || b".-_".contains(&byte)),
                "{} is not a legal RackForge identifier",
                spec.id
            );
            for other in &PARAMETERS[position + 1..] {
                assert_ne!(spec.id, other.id);
            }
        }
    }

    #[test]
    fn a_logarithmic_taper_never_starts_at_zero() {
        for spec in PARAMETERS.iter() {
            if let Kind::Float {
                taper: Taper::Logarithmic,
                minimum,
                ..
            } = spec.kind
            {
                assert!(
                    minimum > 0.0,
                    "{} tapers logarithmically from zero",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn defaults_round_trip_through_validation() {
        let settings = Settings::default();
        let restored = Settings::from_array(settings.as_array()).expect("defaults are valid");
        assert_eq!(settings.as_array(), restored.as_array());
    }

    #[test]
    fn the_meter_is_not_a_setting() {
        let mut settings = Settings::default();
        assert!(!settings.set(index::REDUCTION, -6.0));
        assert!(settings.set(index::CEILING, -3.0));
        assert!(!settings.set(index::CEILING, 1.0));
        assert!(!settings.set(index::LINK, 0.5));
        assert!(settings.set(index::LINK, 0.0));
    }

    #[test]
    fn named_indexes_point_at_the_identifiers_they_claim() {
        let expect = |index: u32, id: &str| assert_eq!(PARAMETERS[index as usize].id, id);
        expect(index::INPUT, "limiter.input");
        expect(index::CEILING, "limiter.ceiling");
        expect(index::RELEASE, "limiter.release");
        expect(index::AUTO_RELEASE, "limiter.auto_release");
        expect(index::LOOKAHEAD, "limiter.lookahead");
        expect(index::LINK, "limiter.link");
        expect(index::TRUE_PEAK, "limiter.true_peak");
        expect(index::OUTPUT, "output.trim");
        expect(index::REDUCTION, "output.reduction");
        expect(index::DELTA_LISTEN, "output.delta_listen");
        expect(index::INPUT_LEVEL, "meter.input");
        expect(index::OUTPUT_LEVEL, "meter.output");
    }
}
