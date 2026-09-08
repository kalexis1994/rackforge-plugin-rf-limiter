//! Factory settings.
//!
//! A preset here is nothing but a list of parameter values. The packager
//! renders the same table into `metadata/presets.json`, so the catalog
//! RackForge shows and the settings the engine loads cannot disagree.

use crate::Settings;
use crate::index::*;

pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub values: &'static [(u32, f32)],
}

pub const PRESET_COUNT: usize = 4;

pub const PRESETS: [Preset; PRESET_COUNT] = [
    Preset {
        id: "transparent",
        name: "Transparent",
        description: "One decibel of headroom, two milliseconds of lookahead, the release left to the programme.",
        values: &[],
    },
    Preset {
        id: "stage",
        name: "Stage",
        description: "Three decibels in, a third of a decibel of headroom: loud, and never over.",
        values: &[(INPUT, 3.0), (CEILING, -0.3), (LOOKAHEAD, 3.0)],
    },
    Preset {
        id: "safety",
        name: "Safety",
        description: "Three decibels of headroom and a long lookahead: a net under the instrument, rarely felt.",
        values: &[
            (CEILING, -3.0),
            (LOOKAHEAD, 5.0),
            (RELEASE, 200.0),
            (AUTO_RELEASE, 0.0),
        ],
    },
    Preset {
        id: "broadcast",
        name: "Broadcast",
        description: "A ceiling at minus one dBTP, unlinked sides, a fast fixed release.",
        values: &[
            (CEILING, -1.0),
            (LINK, 0.0),
            (RELEASE, 40.0),
            (AUTO_RELEASE, 0.0),
        ],
    },
];

/// The settings a preset describes: the defaults, with the preset's values
/// over them. `None` for an id no preset carries.
pub fn settings_for(id: &str) -> Option<Settings> {
    let preset = PRESETS.iter().find(|preset| preset.id == id)?;
    let mut settings = Settings::default();
    for (index, value) in preset.values {
        if !settings.set(*index, *value as f64) {
            return None;
        }
    }
    Some(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_loads_and_stays_inside_the_contract() {
        for preset in PRESETS.iter() {
            assert!(
                settings_for(preset.id).is_some(),
                "{} does not load",
                preset.id
            );
        }
        assert!(settings_for("nowhere").is_none());
    }

    #[test]
    fn preset_identifiers_are_unique() {
        for (position, preset) in PRESETS.iter().enumerate() {
            for other in &PRESETS[position + 1..] {
                assert_ne!(preset.id, other.id);
            }
        }
    }
}
