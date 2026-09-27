# M0 + M1: workspace, CI, and the `core` DSP pipeline

Date: 2026-09-27
Status: approved in chat, awaiting written-spec review

## Goal

Stand up the Rust workspace and CI (M0) and build the `core` crate plus an offline CLI
(M1). M1 builds the **final pipeline shape**:
STFT → ERB band mask → smoothing → shared L/R gain → iSTFT → limiter. The mask source is
a static EQ derived from the profile's `fallback_eq`. In M4 a model-driven mask source
plugs into the same slot, with no pipeline changes.

## Decisions (from brainstorming)

| Decision | Choice | Reason |
|---|---|---|
| M1 purpose | Foundation for the model (spectral pipeline now) | M4 then only swaps the mask source; costs ~20 ms latency up front |
| Mask resolution | 32 ERB bands, interpolated to FFT bins | One code path shared by the static EQ and the future model; static EQ may deviate ≤ ~1.5 dB from the ideal curve |
| Build vs reuse | Use existing crates wherever they fit | Avoid reinventing solved pieces; write only the domain-specific parts |
| Non-48 kHz input | CLI errors out | CLAUDE.md rule 5 (48 kHz native) |
| CI | GitHub Actions | Remote is on GitHub |

## Dependencies

| Purpose | Crate | Used in |
|---|---|---|
| Real FFT / inverse FFT (allocation-free with preallocated scratch) | `realfft` | core |
| Hann window | `apodize` | core (init only) |
| RBJ biquad coefficients for lowshelf/peak/highshelf | `biquad` | core (init only: evaluate magnitude response at band centres) |
| Profile parsing | `serde`, `serde_json` | core |
| Error types | `thiserror` | core |
| WAV I/O | `hound` | cli |
| Argument parsing | `clap` (derive) | cli |
| Error reporting | `anyhow` | cli |
| Float assertions | `approx` | dev-dependency |

Written in-house because no suitable crate exists: the ERB band layout and interpolation,
the gain smoother, and the limiter. Existing limiter crates allocate or lock in the
processing path.

## Workspace layout

```
Cargo.toml                 workspace (resolver 2, edition 2021)
core/                      crate `stepwave-core` (lib)
  src/lib.rs
  src/profile.rs           Profile + FallbackEq types (serde), validation
  src/stft.rs              streaming STFT/iSTFT, sqrt-Hann, 960/480
  src/erb.rs               32-band ERB layout, band centres, band→bin interpolation weights
  src/mask.rs              trait MaskSource; StaticEqMask
  src/smoother.rs          per-band one-pole attack/release in dB domain
  src/limiter.rs           peak limiter, −1 dBFS ceiling
  src/processor.rs         Processor: wires everything, public API
  tests/                   integration tests (synthetic signals generated in code)
  tests/fixtures/          reserved for small synthetic WAVs (none needed in M1)
cli/                       crate `stepwave-cli` (bin)
  src/main.rs
.github/workflows/ci.yml   fmt --check, clippy -D warnings, test (ubuntu-latest, stable)
```

## Components

### `profile`
- `Profile { id, name, match, model, strength_db, fallback_eq: Vec<EqBand>, preamp_db }`
  mirrors `profiles/cs2.json`. `EqBand { type: lowshelf|highshelf|peak, freq, gain, q }`.
- `Profile::from_json(&str) -> Result<Profile, CoreError>`. Validation: freq in (0, 24000),
  q > 0, finite gains.
- M1 ignores `model` and `strength_db`. Both are parsed and kept for M3/M4.

### `stft`
- Sample rate 48 000 Hz, window N = 960 (20 ms), hop H = 480 (10 ms), FFT size 960 →
  481 bins.
- Periodic sqrt-Hann analysis and synthesis windows. At 50 % overlap the product is Hann,
  which satisfies COLA, so reconstruction is perfect with unity mask.
- Streaming: input FIFO of H samples per channel; each full hop triggers one frame.
  Algorithmic latency = N = 960 samples (20 ms).
- All buffers (`realfft` plans, scratch, spectra, overlap-add) are allocated in `new()`.

### `erb`
- 32 bands spaced uniformly on the ERB-rate scale
  (`erb_rate(f) = 21.4 · log10(1 + 0.00437 f)`) from 50 Hz to 20 kHz. Band centres are
  computed once.
- Band→bin interpolation: each bin's gain (in dB) is interpolated linearly on the
  ERB-rate axis between the two neighbouring band centres. Bins below the first centre
  or above the last centre take the edge band's gain. The weights are a precomputed
  table of `(band_index, weight)` per bin.

### `mask`
```rust
pub trait MaskSource {
    /// Fill `gains_db` (len 32) for the current frame. `mid_spectrum` is the mid-channel
    /// STFT frame (481 bins). Must not allocate.
    fn next_mask(&mut self, mid_spectrum: &[Complex<f32>], gains_db: &mut [f32; 32]);
}
```
- `StaticEqMask::from_profile(&Profile)`: for each band centre, builds each `fallback_eq`
  filter with `biquad::Coefficients::from_params` and evaluates its magnitude response at
  that frequency. It sums the dB values over filters, adds `preamp_db`, and stores the
  32 gains. `next_mask` copies the stored gains and ignores the spectrum.
- M4 adds `ModelMask`, which fills the same slot.

### `smoother`
- Per-band one-pole filter in the dB domain, updated once per frame (100 Hz). Attack
  (gain rising) uses a 5 ms time constant and release uses 80 ms. Because the attack time
  constant is shorter than the hop, attack is effectively instant at frame rate. This is
  accepted and documented.
- Initial state is the first mask, so the static EQ does not fade in on startup.

### `processor`
```rust
pub struct Processor { /* all state preallocated */ }
impl Processor {
    pub fn new(profile: &Profile, sample_rate: u32) -> Result<Self, CoreError>; // rejects != 48000
    pub fn latency_samples(&self) -> usize;                                    // 960
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]);            // any block size, in place
    pub fn reset(&mut self);
}
```
Per hop:
1. Analyse L and R. Mid = (L + R) / 2 in the spectral domain (FFT is linear, so no third
   FFT is needed).
2. `mask.next_mask(mid, gains_db)` → smoother → band→bin interpolation → linear gains
   (one 481-element array).
3. Multiply the **same** gain array into both the L and R spectra. Then iSTFT and
   overlap-add.
4. The limiter processes the stereo output sample by sample with a linked gain (one gain
   for both channels).

Safety: `process` never allocates, locks, logs or panics. If any smoothed gain is
non-finite, that frame is passed through at unity. Indexing uses preallocated
fixed-size buffers.

### `limiter`
- Stereo-linked sample-peak limiter with a −1 dBFS ceiling (0.891). Gain drops
  instantly when a sample would exceed the ceiling, which guarantees no overshoot. Gain
  recovers with a 50 ms release.
- It has no lookahead, so it adds no latency. Instant gain reduction can distort
  transients slightly, which is acceptable for the fallback path.
- True-peak (oversampled) detection is deferred to a later milestone. For now the
  ceiling is on sample peak only. This is a known deviation from CLAUDE.md rule 7's
  "true-peak" wording and is recorded here on purpose.

### `cli`
```
stepwave-cli <in.wav> <out.wav> --profile <id-or-path> [--bypass]
```
- `--profile cs2` resolves to `profiles/cs2.json`. An argument that is a path is used
  as-is.
- Reads with `hound`. Accepts 16/24-bit int or 32-bit float, mono or stereo. Mono is
  duplicated to stereo. Any sample rate other than 48 kHz, or more than 2 channels,
  produces an error.
- Output: 32-bit float WAV, stereo, 48 kHz. The CLI compensates latency: it feeds 960
  zero samples of tail and drops the first 960 output samples, so the output is aligned
  with the input and has the same length.
- `--bypass` runs the chain with a unity mask. This is for A/B listening.
- Prints a short summary: input/output peak dBFS and the processing time per 10 ms frame
  (µs).

## Error handling

- `CoreError` (thiserror): `InvalidProfile(String)`, `UnsupportedSampleRate(u32)`,
  `Json(serde_json::Error)`.
- CLI: `anyhow` context on every file operation. It exits non-zero with a readable
  message.
- The real-time path has no error returns. Non-finite gains fall back to unity for that
  frame.

## Testing

All test signals are generated in code (sines, seeded white noise). There are no game
assets.

| Test | Assertion |
|---|---|
| Perfect reconstruction | Unity mask, white noise → output equals input delayed by 960 samples, error < −90 dB |
| ERB layout | 32 strictly increasing centres, first ≈ 50 Hz, last ≈ 20 kHz; interpolation weights sum to 1 per bin |
| Static EQ response | cs2 profile, white noise, long average spectrum → measured gain at 120 Hz, 2.5 kHz, 5 kHz within 1.5 dB of the `biquad` analytic response (including preamp) |
| Stereo image | Correlated noise panned 70/30 → output L/R energy ratio within 0.1 dB of input; L/R correlation within 0.01 |
| No clipping | +12 dBFS sine sweep → output peak ≤ −1 dBFS (0.891) |
| Smoother | Step up reaches ≥ 85 % (1 − e⁻²) in one frame; step down follows the 80 ms constant within 10 % |
| Rate rejection | `Processor::new(_, 44100)` → `UnsupportedSampleRate` |
| Block-size independence | Same input processed in blocks of 1, 64, 480, 1000 → identical output |
| Profile parse | `profiles/cs2.json` parses. Invalid q/freq is rejected |
| CLI end-to-end | Generate a WAV in a temp dir, run the binary, check output length, rate and channels |

CI (`.github/workflows/ci.yml`) runs on push and PR: `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## Out of scope for M0 + M1

The model, the `.swm` format, plugins, the tray app, true-peak limiting, resampling,
strength control and profile hot-swap.
