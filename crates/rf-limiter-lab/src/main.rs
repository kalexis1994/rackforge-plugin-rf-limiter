//! The RF-Limiter bench.
//!
//! Three jobs, all of them about keeping the project honest:
//!
//! * `metadata` renders the package's JSON from the contract, so the schema the
//!   host validates and the engine's behaviour come from one place;
//! * `render` puts a file, or a test burst, through the limiter and writes a
//!   file you can listen to;
//! * `ceiling` measures what the bench meter would: the highest sample that
//!   got out against the ceiling that was set, at a range of drive.

mod manifest;
mod metadata;

use std::path::{Path, PathBuf};

use rf_limiter_contract::index::{CEILING, INPUT, REDUCTION};
use rf_limiter_contract::{PARAMETERS, PRESETS};
use rf_limiter_dsp::Engine;
use rf_limiter_dsp::math::{db_to_gain, gain_to_db};

const SAMPLE_RATE: u32 = 48_000;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let command = arguments.first().map(String::as_str).unwrap_or("help");
    let result = match command {
        "metadata" => metadata_command(&arguments[1..]),
        "render" => render_command(&arguments[1..]),
        "ceiling" => ceiling_command(&arguments[1..]),
        "presets" => presets_command(),
        "parameters" => parameters_command(),
        _ => {
            usage();
            Ok(())
        }
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn usage() {
    println!(
        "rf-limiter-lab <command>

  metadata [--check]              render package metadata from the contract
  parameters                      list every parameter and its range
  presets                         list the factory settings
  render --out <file.wav>         render a signal through the limiter
         [--preset <id>] [--input <file.wav>] [--seconds <n>]
         [--set <id>=<value> ...]
  ceiling [--preset <id>] [--set <id>=<value> ...]
                                  the highest output sample against the ceiling,
                                  for a sine driven from -12 to +18 dB

Values may be given by parameter identifier or index:
  --set limiter.ceiling=-3 --set limiter.lookahead=5"
    );
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("the lab crate sits two levels below the repository root")
}

fn metadata_command(arguments: &[String]) -> Result<(), String> {
    let check = arguments.iter().any(|argument| argument == "--check");
    let package = repository_root().join("plugin").join("package");
    let identity = manifest::read(&package.join("rackforge-plugin.toml"))?;
    metadata::write(&package, &identity, check)?;
    if check {
        println!("metadata matches the contract");
    }
    Ok(())
}

fn parameters_command() -> Result<(), String> {
    for parameter in PARAMETERS.iter() {
        println!(
            "{:>3}  {:<22} {:<10} {}",
            parameter.index, parameter.id, parameter.page, parameter.name
        );
    }
    Ok(())
}

fn presets_command() -> Result<(), String> {
    for preset in PRESETS.iter() {
        println!(
            "{:<14} {:<14} {}",
            preset.id, preset.name, preset.description
        );
    }
    Ok(())
}

struct Options {
    preset: Option<String>,
    overrides: Vec<(u32, f64)>,
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    seconds: f32,
}

fn parse_options(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options {
        preset: None,
        overrides: Vec::new(),
        input: None,
        output: None,
        seconds: 4.0,
    };
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].as_str();
        let value = || {
            arguments
                .get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag {
            "--preset" => options.preset = Some(value()?),
            "--set" => options.overrides.push(parse_assignment(&value()?)?),
            "--input" => options.input = Some(PathBuf::from(value()?)),
            "--out" => options.output = Some(PathBuf::from(value()?)),
            "--seconds" => {
                options.seconds = value()?
                    .parse()
                    .map_err(|_| "--seconds needs a number".to_owned())?
            }
            other => return Err(format!("unknown option {other}")),
        }
        index += 2;
    }
    Ok(options)
}

fn parse_assignment(assignment: &str) -> Result<(u32, f64), String> {
    let (key, value) = assignment
        .split_once('=')
        .ok_or_else(|| format!("expected id=value, got {assignment}"))?;
    let index = if let Ok(index) = key.parse::<u32>() {
        index
    } else {
        PARAMETERS
            .iter()
            .find(|parameter| parameter.id == key)
            .map(|parameter| parameter.index)
            .ok_or_else(|| format!("no parameter is called {key}"))?
    };
    let value = value
        .parse::<f64>()
        .map_err(|_| format!("{value} is not a number"))?;
    Ok((index, value))
}

fn engine_for(options: &Options) -> Result<Engine, String> {
    let mut engine = Engine::default();
    if !engine.prepare(f64::from(SAMPLE_RATE)) {
        return Err("the engine refused the sample rate".into());
    }
    if let Some(preset) = &options.preset
        && !engine.load_preset(preset)
    {
        return Err(format!("no preset is called {preset}"));
    }
    for (index, value) in &options.overrides {
        if !engine.set_parameter(*index, *value) {
            return Err(format!("parameter {index} refused {value}"));
        }
    }
    Ok(engine)
}

/// Reads a WAV as stereo frames at the bench rate, or, with no file, makes a
/// test signal: a tone that steps up six decibels every second.
fn source(options: &Options) -> Result<Vec<(f32, f32)>, String> {
    if let Some(path) = &options.input {
        let mut reader = hound::WavReader::open(path).map_err(|error| error.to_string())?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;
        let scale = match spec.sample_format {
            hound::SampleFormat::Float => 1.0,
            hound::SampleFormat::Int => 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32,
        };
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .map(|sample| sample.map_err(|error| error.to_string()))
                .collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => reader
                .samples::<i32>()
                .map(|sample| {
                    sample
                        .map(|value| value as f32 * scale)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<_, _>>()?,
        };
        return Ok(samples
            .chunks(channels)
            .map(|frame| (frame[0], frame.get(1).copied().unwrap_or(frame[0])))
            .collect());
    }
    let total = (options.seconds * SAMPLE_RATE as f32) as usize;
    Ok((0..total)
        .map(|n| {
            let second = n / SAMPLE_RATE as usize;
            let amplitude = db_to_gain(-12.0 + 6.0 * second as f32);
            let x = amplitude
                * (2.0 * core::f32::consts::PI * 220.0 * n as f32 / SAMPLE_RATE as f32).sin();
            (x, x)
        })
        .collect())
}

fn render_command(arguments: &[String]) -> Result<(), String> {
    let options = parse_options(arguments)?;
    let output = options
        .output
        .clone()
        .ok_or("render needs --out <file.wav>")?;
    let mut engine = engine_for(&options)?;
    let frames = source(&options)?;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&output, spec).map_err(|error| error.to_string())?;
    let mut peak = 0.0_f32;
    for (left, right) in frames {
        let (l, r) = engine.process(left, right);
        peak = peak.max(l.abs()).max(r.abs());
        writer.write_sample(l).map_err(|error| error.to_string())?;
        writer.write_sample(r).map_err(|error| error.to_string())?;
    }
    writer.finalize().map_err(|error| error.to_string())?;
    println!(
        "wrote {} (peak {:.2} dBFS, ceiling {:.1} dBTP, reduction now {:.1} dB)",
        output.display(),
        gain_to_db(peak),
        engine.parameter(CEILING).unwrap_or(0.0),
        engine.parameter(REDUCTION).unwrap_or(0.0)
    );
    Ok(())
}

fn ceiling_command(arguments: &[String]) -> Result<(), String> {
    let options = parse_options(arguments)?;
    let mut engine = engine_for(&options)?;
    let ceiling = engine.parameter(CEILING).unwrap_or(0.0) as f32;
    println!("ceiling {ceiling:.1} dBTP, 440 Hz, half a second per step");
    println!(
        "{:>8} {:>10} {:>10} {:>10}",
        "drive dB", "out dBFS", "over dB", "GR dB"
    );
    let mut drive = -12.0_f32;
    while drive <= 18.0 {
        engine.set_parameter(INPUT, f64::from(drive));
        let mut peak = 0.0_f32;
        let total = SAMPLE_RATE as usize / 2;
        for n in 0..total {
            let x = (2.0 * core::f32::consts::PI * 440.0 * n as f32 / SAMPLE_RATE as f32).sin();
            let (l, r) = engine.process(x, x);
            if n > total / 4 {
                peak = peak.max(l.abs()).max(r.abs());
            }
        }
        let out = gain_to_db(peak);
        println!(
            "{drive:>8.1} {out:>10.2} {:>10.2} {:>10.1}",
            out - ceiling,
            engine.parameter(REDUCTION).unwrap_or(0.0)
        );
        drive += 6.0;
    }
    Ok(())
}
