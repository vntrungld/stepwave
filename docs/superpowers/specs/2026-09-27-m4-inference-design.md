# M4: real-time model inference in `core`, benchmark

Date: 2026-09-27
Status: approved in chat, awaiting written-spec review

## Goal

`core` loads an exported `.swm` model (M3) and runs it once per 10 ms hop as a
`MaskSource`, replacing the static EQ in the existing pipeline slot. The pipeline shape
from M1 does not change. The hot path stays allocation-, lock- and panic-free. A
benchmark proves the cost is well under 1 ms per frame. The offline CLI can use the model,
and falls back to the static EQ when no usable model is present (CLAUDE.md rule 6).

A trained model is not required for M4. Tests use a seeded random-weights `.swm` fixture,
which contains no game-derived data and is committed.

## Decisions (from brainstorming)

| Decision | Choice | Reason |
|---|---|---|
| Strength control | Linear scaling `gain = model_gain × profile.strength_db / header.strength_db`, then clamp to the header's gain range. Applies to boosts and ducks | One line of code; matches the future strength knob; `cs2` profile (6 dB) equals the training strength, so the default is unchanged |
| Inference implementation | Hand-rolled f32 Dense + GRU, no new dependencies | ~110k MACs per frame, estimated 20–60 µs; CLAUDE.md rule 4 prefers hand-rolled; easy to verify line by line against `swm_ref.py` |
| Fallback | CLI `--mode auto` (default) uses the model if it loads, else warns and uses the static EQ | Rule 6: never fail the audio path because of a missing or bad model |

## Components

### `core/src/swm.rs` — `SwmModel`

```rust
pub struct SwmModel { /* header fields + tensors as Vec<f32> */ }
impl SwmModel {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CoreError>;
    pub fn load(path: &Path) -> Result<Self, CoreError>;
    pub fn strength_db(&self) -> f32;          // training strength from the header
    pub fn gain_db_range(&self) -> (f32, f32);
}
```

The format is the M3 `.swm` v1:
- `b"SWM1"`;
- u32 LE version (must be 1);
- u32 LE header length;
- UTF-8 JSON header (parsed with `serde_json`);
- f32 LE tensors in the header's `tensors` order.

Validation (any failure → `CoreError::InvalidModel(String)` naming the field):
- magic and version;
- header is valid JSON;
- `arch == "dense-gru2-dense"` and `gate_order == "rzn"`;
- `sizes` = inputs 64, dense 64, hidden 96, bands 32;
- `sample_rate == 48000`, `hop == 480`, `num_bands == 32`;
- `feature.mean` and `feature.std` each have 32 finite values, with std > 0;
- `gain_db_range` is `[lo, hi]` with lo < hi;
- `strength_db` is finite and > 0;
- the `tensors` list equals the fixed v1 layout (names and shapes as in
  `stepwave_model.swm.tensor_layout`);
- payload length equals the sum of the shapes exactly (no truncation, no trailing bytes);
- every weight is finite.

Unknown extra header keys (e.g. `target`, `created`, `source`) are ignored.

### `core/src/gru.rs` — `ModelRunner`

```rust
pub struct ModelRunner { /* weights, h1[96], h2[96], prev energies[32], scratch */ }
impl ModelRunner {
    pub fn new(model: &SwmModel) -> Self;                       // allocates everything
    pub fn step(&mut self, energies_db: &[f32; 32], gains_db: &mut [f32; 32]);
    pub fn reset(&mut self);                                    // zero state, "first frame" again
}
```

`step` implements exactly the maths in the `training/model/src/stepwave_model/swm_ref.py`
docstring, in f32:
- **Input features:** x = [(E − mean)/std, (E − E_prev)/std]. The delta is 0 on the first
  frame after `new`/`reset`.
- **Dense:** y = ReLU(W_inp·x + b_inp).
- **Two GRUs:** PyTorch (r, z, n) gate order, zero initial hidden state:
  - r = σ(W_ir·x + b_ir + W_hr·h + b_hr)
  - z = σ(W_iz·x + b_iz + W_hz·h + b_hz)
  - n = tanh(W_in·x + b_in + r ⊙ (W_hn·h + b_hn))
  - h' = (1 − z) ⊙ n + z ⊙ h
- **Output:** gain = lo + (hi − lo)·σ(W_out·h2 + b_out).

Weights are stored row-major as in the file. Matrix–vector products are plain loops over
contiguous rows, so the compiler can auto-vectorise them. `step` never allocates, locks or
panics, and uses no indexing that can fail at runtime (fixed-size arrays or slices checked
in `new`).

### `core/src/mask.rs` — `ModelMask`, and `MaskSource::reset`

- The `MaskSource` trait gains `fn reset(&mut self) {}`, a no-op by default.
  `Processor::reset()` calls `self.mask.reset()`.
- `ModelMask::new(model: &SwmModel, strength_db: f32) -> ModelMask`:
  - precomputes `scale = strength_db / model.strength_db()` and the clamp range;
  - holds an `ErbBands` and a `ModelRunner`.
- `next_mask(mid, gains)` per frame:
  1. Compute `ErbBands::band_energies_db(mid)`. This is the same computation `prep`
     used through `FeatureExtractor`, and the M3 test pins that the frames align.
  2. Call `runner.step`.
  3. Multiply by `scale` and clamp to the gain range.
- `reset` resets the runner.
- A non-finite strength is rejected where the profile is validated. A strength of 0 gives
  unity gain.
- One mask from mid, applied to both L and R (rule 1), is unchanged.

### `Processor`

- New constructor:
  `Processor::with_model(profile: &Profile, model: &SwmModel, sample_rate: u32) -> Result<Self, CoreError>`.
  It builds a `ModelMask` with `profile.strength_db`.
- `Processor::new` (static EQ) and `with_mask` are unchanged.
- `CoreError` gains `InvalidModel(String)` and `Io(std::io::Error)` (for `load`).

### CLI

```
stepwave-cli <in.wav> <out.wav> --profile <id-or-path> [--mode auto|model|eq|bypass] [--model <path>]
```

- **Model path:** `--model` if given; otherwise the profile's `model` field. A relative
  path resolves against the parent of the profile file's directory (the repo root for
  `profiles/cs2.json` → `models/cs2.swm`).
- **Modes:**
  - `auto` (default): try to load the model; on any error print
    `warning: model <path> unusable (<reason>); using static EQ` to stderr and use the
    static EQ.
  - `model`: load failure is an error, with exit code 1.
  - `eq`: static EQ.
  - `bypass`: unity mask.
- The existing `--bypass` flag stays as an alias for `--mode bypass`. It conflicts with an
  explicit `--mode`.
- The summary line adds the mode actually used (`mode: model|eq|bypass`) next to the
  existing peak dBFS and µs-per-frame figures.

### Benchmark

- `core/benches/model.rs` uses `criterion` (a dev-dependency). It measures:
  - `ModelMask::next_mask` for one frame, with the fixture model;
  - `Processor::process` for one 480-sample stereo hop, in model mode and in static-EQ mode.
- The results (machine, µs per frame, headroom versus 1 ms) go into
  `docs/measurements/m4-benchmark.md`.
- CI builds the benchmarks (`cargo bench --no-run`) but does not run them.
- **Pass criterion:** under 1 ms per frame. The expected figure is tens of µs.

## Fixture generation

`training/model/scripts/make_swm_fixture.py` (run with `uv run --project training/model`):
- builds a `BandMaskNet` with `torch.manual_seed(1234)`;
- exports it through the M3 `export` path from a synthetic checkpoint (random feature
  mean/std) to `core/tests/fixtures/model_random.swm`;
- writes `core/tests/fixtures/model_random_expected.json`: 200 frames of seeded random
  band energies (dB, in −100..0), and the gains `swm_ref.gains_from_energies` produces for
  them (f64, rounded to 1e-6);
- stays deterministic, so re-running it reproduces identical files.

The fixture weighs about 440 KB of random numbers and is committed. It holds no
game-derived data.

## Error handling

- Loading and validation errors are `CoreError::InvalidModel` or `Io`, with messages that
  name the file and field.
- Nothing in `step` or `next_mask` returns an error. The existing processor guard already
  passes a frame through at unity if a mask value is non-finite.
- The CLI auto mode never fails because of the model. The model mode fails loudly.

## Testing

All test data is synthetic: the random-weights fixture, sines and seeded noise.

| Test | Assertion |
|---|---|
| Parity | `ModelRunner` gains vs `model_random_expected.json` for all 200 frames: max abs error ≤ 1e-3 dB |
| Reset | After `reset`, running the same 200 frames reproduces the first run exactly |
| Reader rejects | Bad magic, version 2, truncated payload, trailing bytes, NaN weight, wrong arch, wrong gate order, wrong sizes, wrong tensor list, non-JSON header, std ≤ 0, strength ≤ 0 each give `InvalidModel` naming the problem |
| Strength | Profile strength 3 dB vs 6 dB: gains are exactly half before clamping; strength 0 → all gains 0 |
| Processor with model | Block-size independence (1, 64, 480, 1000); +12 dBFS input never exceeds −1 dBFS; stereo 70/30 pan keeps its L/R energy ratio within 0.1 dB and its correlation within 0.01; `reset()` equals a fresh processor |
| No allocation | A counting global allocator (test binary only) sees zero allocations across 100 `next_mask` calls and 100 `process` hops with the model |
| CLI | `auto` with a missing model → exit 0, warning on stderr, `mode: eq`; `--mode model` with a missing model → exit 1; `--mode model --model <fixture>` → exit 0, `mode: model`; `--bypass` still works; `--bypass --mode eq` rejected |
| Benchmark | Compiles in CI (`cargo bench --no-run`) |

## Out of scope for M4

- Plugins and live model or profile hot-swap (M5).
- Hand-written SIMD, unless the benchmark shows it is needed.
- True-peak limiting.
- PUBG and generic models.
- Training the real model: the user runs that on the RTX 3070 PC with the M3 tooling.
