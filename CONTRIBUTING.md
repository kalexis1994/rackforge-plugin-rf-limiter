# Contributing to RF-Limiter

## The rule that matters most

A constant that shapes the behaviour should be traceable to something outside
somebody's taste: a time, a kernel width, a floor, a measurement. When that is
not yet true, say so where the code is.

## Working on it

```bash
cargo test --workspace          # everything
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

Before opening a pull request, build the package once — it regenerates the
metadata, builds the component, runs the tests and proves the component still
packs:

```bash
pwsh tools/build-package.ps1     # or: bash tools/build-package.sh
```

On a machine whose default Rust is windows-gnu, pass
`+stable-x86_64-pc-windows-msvc`: the RackForge runtime only builds under
MSVC (the build script does it for you).

## Generated files are generated

`plugin/package/metadata/*.json` and `plugin/package/branding/*.png` are
outputs. Edit the contract or the generator, never the JSON:

```bash
cargo run -p rf-limiter-lab -- metadata
cargo run -p rf-limiter-lab -- metadata --check    # what CI runs
python tools/generate-branding.py
```

The version lives in `plugin/package/rackforge-plugin.toml` and nowhere else;
the runtime descriptor is generated from it.

## Adding a control

1. Add it to `rf-limiter-contract` (`lib.rs`, `index.rs`) at the end and bump
   `PARAMETER_COUNT`. State is a flat block of `f32`s keyed by length, so
   adding parameters is a state-version change: bump `state_version` in the
   manifest and add the old count to `PREVIOUS_PARAMETER_COUNTS`.
2. Read it in `engine.rs`'s `apply_settings`.
3. Tests: what the limiter is supposed to *do*, not what the code currently
   returns. "A hot signal never crosses the ceiling", "a quiet one passes at
   unity", "true peak sees what lies between samples".
4. `cargo run -p rf-limiter-lab -- metadata`.

The interface needs no change: it builds itself from the schema.

## Style

The house voice is descriptive, never boastful. Comments explain *why* a value
is what it is, not what the line does. No trademarks, no brand names, no
product names.
