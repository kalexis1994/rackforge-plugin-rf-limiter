# RF-Limiter

A lookahead true-peak limiter for [RackForge](https://github.com/kalexis1994/rackforge):
a ceiling in dBTP, a release that follows the programme or a fixed time,
stereo link, and a gain-reduction meter. The effect the piano goes through
last, so that the +6 dB the model gained in its trim never reaches the
converter.

> `v0.1.0` is the first working version. The engine runs, the package
> installs, and the limits are the ones stated here.

## How it holds the ceiling

The gain for an output sample is the mean, over the lookahead length, of the
smallest gain any sample in the lookahead ahead of it asks for. Every term of
that mean already includes the sample's own requirement, so the gain is at
the reduction *before* the peak arrives and a peak never gets through; and
because it is a mean rather than a step, the reduction lands as a slope, which
the ear reads as level rather than as a click. Two lookaheads of latency,
plus the four samples the true-peak kernel needs: under five milliseconds at
48 kHz with the default two-millisecond lookahead.

Between the detector and the window sits the release: a one-pole envelope
that climbs back to unity at the chosen rate and drops to any smaller demand
at once. *Auto Release* averages a fast stage (50 ms) and a slow one
(500 ms), so a transient is let go quickly and a sustained overshoot is held
down.

*True Peak* runs an eight-tap windowed-sinc interpolator at four times the
rate, so a peak that lies between two samples is seen to within a tenth of a
decibel. *Stereo Link* applies the smaller of the two sides' gains to both,
which keeps the image where it was.

The ceiling is held by construction; a sample clamp under it catches the
rounding the mean leaves behind.

## Controls

| Control | Range | What it is |
| --- | --- | --- |
| Input | −24 … +24 dB | Drive into the limiter. |
| Ceiling | −20 … 0 dBTP | The level nothing crosses. |
| Release | 10 … 1000 ms | The fixed release, when Auto Release is off. |
| Auto Release | on/off | Programme-dependent release. |
| Lookahead | 0 … 10 ms | How far ahead the detector looks; twice this is the latency. |
| Stereo Link | on/off | One gain for both sides. |
| True Peak | on/off | Inter-sample peak detection. |
| Output | −24 … +24 dB | Trim after the ceiling. |
| Gain Reduction | meter | How much is being taken away, in dB. |

## Build and install

The RackForge checkout must sit next to this one, because the packager lives
there.

```bash
pwsh tools/build-package.ps1
```

```bash
bash tools/build-package.sh
```

Either script regenerates the package metadata from the contract, builds the
WebAssembly component, runs the tests and packs
`artifacts/RF-Limiter.rfplugin`. Install it the way you install any RackForge
plugin — the desktop's Plugin Manager, or:

```bash
./target/release/rackforge-desktop.exe --install-plugin ../rackforge-plugin-rf-limiter/artifacts/RF-Limiter.rfplugin
```

In PLAY, open the FX drawer and add it after the instrument. In LIVE it is
an ordinary effect Slot.

## The bench

```bash
cargo run -p rf-limiter-lab -- ceiling --set limiter.ceiling=-3
cargo run -p rf-limiter-lab -- render --out limited.wav --input piano.wav --set limiter.input=6
```

`ceiling` drives a sine from −12 to +18 dB and prints the highest sample that
got out against the ceiling that was set; `render` puts a file, or a test
burst, through the limiter so it can be listened to.
