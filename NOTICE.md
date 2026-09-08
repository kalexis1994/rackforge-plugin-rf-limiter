# Notices

RF-Limiter is an independent RackForge plugin implemented in Rust and
distributed under GPL-3.0-only.

It implements a lookahead peak limiter from first principles: a windowed-sinc
inter-sample peak detector, a one-pole release, a sliding-window minimum and
a box ramp. These are textbook signal-processing constructions; no third-party
code, artwork, trademark or brand name is included in this repository or in
the plugin package.

Third-party code: the plugin depends on `rackforge-plugin-sdk` (MIT OR
Apache-2.0) and on `libm` (MIT/Apache-2.0).
