# M3: train the CS2 footstep model, export `.swm`, offline A/B evaluation

Date: 2026-09-27
Status: approved in chat, awaiting written-spec review

## Goal

Train a small causal GRU that looks only at the game mix and predicts a 32-band ERB gain
mask. The mask boosts footsteps by `strength_db` and, only while footsteps are present,
ducks gunfire, explosions and ambience. The work also covers exporting the model to a
versioned weights file (`models/cs2.swm`) and measuring it offline against bypass and the
static fallback EQ: on the held-out synthetic set, and by ear on real CS2 recordings.

Real-time inference in Rust is M4. M3 runs the model in Python only. The one Rust change
is an external-mask entry point, so evaluation hears exactly what the plugin will do.

## Decisions (from brainstorming)

| Decision | Choice | Reason |
|---|---|---|
| Where to train | The user's Windows PC with an RTX 3070 (8 GB); data copied by drive or LAN share | The dev laptop has no NVIDIA GPU. A ~110k-param GRU fits trivially in 8 GB |
| Data pipeline | Precompute per-band energies of the mix and of every stem once (`prep`). Build target masks inside the data loader from those energies | The Windows box then needs only `uv` + PyTorch and a few GB of numbers, with no game audio, no Rust and no Source2Viewer. Changing strength or duck levels needs no re-prep |
| Duck scope | Masking classes are ducked only while footsteps are active (±150 ms). With no footsteps the target is 0 dB everywhere | A constant duck would just be a static EQ. "Do nothing without footsteps" makes false boosts well defined |
| Real recordings | The user records CS2 demo playback with `pw-record` into `training/data/real/`. Listening and activity stats only | There is no ground truth on real audio. The synthetic val set carries the numbers |
| Smoother in training | Not modelled. Evaluation always runs the real core smoother | Keeps training simple. The measured numbers still reflect what the user will hear |

## Architecture

```
laptop (CachyOS)                       Windows PC (RTX 3070)          laptop
datagen mix → sets/<set>/…  ─► trainer prep ─► features/<set>/ ═copy═► trainer train ─► runs/<run>/best.pt
                                                                       ═copy back═► trainer export ─► models/cs2.swm
                                                                                    trainer eval   ─► runs/<run>/eval/
```

A new uv project `training/model/` (package `stepwave_model`, CLI `trainer`) follows the
layout and tooling of `training/datagen`: typer with an `@app.callback()`, ruff, pytest,
Python 3.12 pinned.

| Command | Runs on | Needs `stepwave_py` | Does |
|---|---|---|---|
| `trainer prep <set>` | laptop | yes | FLAC stems → per-band energies → `training/data/features/<set>/` |
| `trainer train <features_dir> --out <runs_dir>/<run>` | 3070 PC (any device) | no | trains, writes `best.pt`, `last.pt`, `train.json`, `log.csv` |
| `trainer export <run_dir> --out models/cs2.swm` | any | no | writes `.swm`, then checks numpy-vs-torch parity |
| `trainer eval <swm> --features <dir> --set <set>` | laptop | yes | metrics report and A/B listening files |

Dependencies:
- `stepwave-py` is an optional extra (`[project.optional-dependencies] audio`), together
  with `soundfile`. Only `prep` and `eval` import it. A plain `uv sync` on Windows builds
  no Rust.
- `torch` resolves through `[tool.uv.sources]`: the CUDA 12.8 index
  (`https://download.pytorch.org/whl/cu128`) on Windows, the CPU index on Linux. Uses a
  `sys_platform` marker.
- Also `numpy`, `typer`, `matplotlib` (report plots).

All generated data lives under the git-ignored `training/data/`: `features/`, `runs/`,
`real/`, `eval/`. `models/*.swm` stays git-ignored as the existing `.gitignore` already says
("publish via releases"); the file is about 450 KB of learned numbers and no game audio.

## Components

### Core: `ExternalMask` and limiter switch (Rust)

- `mask::ExternalMask::new(gains_db: Vec<[f32; 32]>)`
  - `next_mask` copies frame `k` on its k-th call and holds the last frame after the end.
    An empty sequence gives 0 dB.
  - The Vec is allocated at construction. `next_mask` never allocates.
  - It is an offline/test source, and its doc comment says so.
- `Processor::set_limiter_enabled(&mut self, on: bool)`: default `true`. When off, the
  output skips the limiter, so a per-stem measurement is linear.
- Frame alignment contract: the k-th `next_mask` call happens for the same analysis frame
  whose energies `FeatureExtractor::band_energies_db` returns at index `k` for the same
  input. A test pins this.

### Binding: `apply_band_gains`

```python
stepwave_py.apply_band_gains(left: f32[n], right: f32[n], gains_db: f32[frames, 32],
                             limiter: bool = True) -> tuple[f32[n], f32[n]]
```
- Builds a `Processor::with_mask(ExternalMask)` at 48 kHz.
- Latency is compensated like the CLI: feed 960 zeros of tail, drop the first 960 output
  samples. Output length equals input length.
- `gains_db.shape[0]` must equal the frame count `band_energies_db` returns for the same
  input, otherwise `ValueError`. Mismatched `left`/`right` lengths or a wrong
  second dimension also raise `ValueError`.

Two small helpers so evaluation can use the exact Rust numbers instead of Python copies:

```python
stepwave_py.smooth_gains_db(gains_db: f32[frames, 32]) -> f32[frames, 32]   # core GainSmoother
stepwave_py.static_eq_gains_db(profile_json: str) -> f32[32]                 # StaticEqMask gains
```
`static_eq_gains_db` raises `ValueError` with the core error text for an invalid profile.

### `prep`

For every clip in `sets/<set>/{train,val}/`:
- Read `mix.flac` and the five stems.
- Compute `band_energies_db` for each of the six signals and store float16 `[frames, 32]`
  arrays.
- Per split, write one concatenated array per signal: `mix.npy`, `footsteps.npy`,
  `gunfire.npy`, `explosions.npy`, `ambience.npy`, `other.npy`. Also write `index.npy`
  (int64 `[clips, 2]`: frame offset and frame count), and `clips.json` (clip ids in index
order).
- Write `manifest.json` with the set name, the source manifest fingerprint of each split,
  the `stepwave_py` constants (`SAMPLE_RATE`, `HOP`, `NUM_BANDS`), the frame count, and
  the mix-feature `mean`/`std` per band computed on **train only**.
- Parallel over clips with a process pool. Resumable per split: skip the split if its
  manifest fingerprint matches, `--force` rebuilds it.
- 50 h ≈ 18 M frames × 32 × 6 × 2 B ≈ 7 GB.

### Target (`targets.py`, one torch implementation used by training and evaluation)

Per frame and band, with powers `P_c = 10^(E_c/10)`:
```
active   = E_footsteps − E_mix ≥ −25 dB   in any band, dilated by ±15 frames (150 ms)
P_desired = s²·P_fs + d_gun²·P_gun + d_exp²·P_exp + d_amb²·P_amb + P_other     (active frames)
P_desired = P_total                                                          (inactive frames)
P_total   = Σ P_c
target_db = clip(10·log10(P_desired / P_total), −12, +12)
```

The `active` flag is per frame, not per band. Defaults live in `configs/cs2.toml`:

| Parameter | Default |
|---|---|
| `s` | +6 dB (`profiles/cs2.json` `strength_db`) |
| `d_gun`, `d_exp` | −6 dB |
| `d_amb` | −3 dB |
| `other` | 0 dB |
| Activity threshold | −25 dB |
| Dilation | 15 frames |
| Gain range | ±12 dB |

Stem powers are summed, rather than taken from the mix energy, so the ratio stays
consistent. The band energies carry the analyser's floor, so `P_total > 0`.

### Features (`features.py`, torch)

Input per frame is 64 values:
- the normalised mix energies `(E_mix − mean)/std`;
- first-order deltas of the raw mix energies, `E[t] − E[t−1]`, divided by `std`, with 0 at
  `t = 0`.

This formula is the reference the M4 Rust port will reproduce.

### Model (`model.py`)

```
Linear(64→64) + ReLU → GRU(64→96) → GRU(96→96) → Linear(96→32) → sigmoid
gain_db = −12 + 24·sigmoid
```
- It is causal (unidirectional GRUs) and has about 110k parameters.
- `forward(x[B,T,64], h0=None) → (gain_db[B,T,32], h)`. It uses the PyTorch GRU gate
  convention `(r, z, n)`, which the `.swm` records.

### Loss

```
L = mean((ŷ − y)²)  +  λ · mean_over_inactive_frames(relu(ŷ)²)      λ = 4
```
`ŷ`, `y` are in dB. The second term penalises any boost where no footsteps are present.

### Training (`train.py`)

- Loads the whole feature arrays into RAM.
- Each step samples random 400-frame (4 s) windows from random train clips; batch 128.
  - A window is read with one extra leading frame, so the first delta is the true
    difference, then that frame is dropped.
  - Features and targets are built on the fly on the training device.
  - No multi-worker DataLoader (Windows-safe).
- AdamW, lr 1e-3, weight decay 1e-4, cosine decay, gradient clip 1.0, 30 epochs. An epoch
  is `train_frames / (128·400)` steps.
- After every epoch: validation on full-length val clips, logging the val loss and the
  false-boost rate.
  - Keeps `best.pt` (lowest val loss) and `last.pt`.
  - Early stop after 5 epochs with no improvement.
- `--device auto|cpu|cuda` (auto picks CUDA when available). Seeded. Prints seconds per
  epoch.
- `train.json` records:
  - the config;
  - the features `manifest.json` (copied);
  - the git commit if available;
  - the device;
  - the torch version.
- `--resume` continues from `last.pt`.
- `--max-steps` caps training for smoke tests.

### Export: `.swm` v1 (`swm.py`, `export.py`)

```
bytes 0..4   magic  b"SWM1"
u32 LE       format version = 1
u32 LE       header length N
N bytes      header JSON (UTF-8)
payload      f32 LE tensors, concatenated in header order, no padding
```

The header JSON contains:
- `arch`: `"dense-gru2-dense"`, sizes 64/64/96/96/32;
- `gate_order`: `"rzn"`;
- `gain_db_range`: `[-12, 12]`;
- `sample_rate` 48000, `hop` 480, `num_bands` 32;
- `feature`: `{"mean": [32], "std": [32], "delta": "raw_diff_over_std"}`;
- `strength_db`, `duck_db`;
- `tensors`: a list of `{name, shape}` in payload order;
- `param_count`;
- `features_fingerprint`;
- `created`.

Checks:
- The reader validates the magic, the version, that the payload length matches the
  shapes, and finite values.
- `export` also runs `swm_ref.py`, a pure-numpy reference forward pass (hand-written GRU),
  on a seeded random 1000-frame input. It must match torch within 1e-3 dB max abs,
  otherwise the command exits non-zero and deletes the file.
- `swm_ref.py` is the spec M4's Rust GRU is tested against.

### Evaluation (`evaluate.py`)

**Synthetic val set:**
- Run the model on each val clip's mix features to get `gains_db[frames,32]`.
- Using `apply_band_gains(..., limiter=False)`, apply the same gains to every stem
  separately, from the FLACs in `sets/<set>/val`.
- Per clip and class, measure the output/input energy ratio, pooled over the relevant
  frames.

Metrics, reported for three modes (bypass, static EQ from `profiles/cs2.json`, model):

| Metric | Definition | Goal (flagged ✅/❌, never a hard failure) |
|---|---|---|
| Footstep gain | Energy ratio of the footstep stem over active frames, dB | ≥ +4 dB |
| SNR improvement | Change of footstep-to-rest energy ratio over active frames, dB | ≥ +6 dB |
| False-boost rate | % of inactive frames whose smoothed gain exceeds +2 dB in any band | < 2 % |
| Per-class change | Gunfire / explosions / ambience gain, active vs inactive frames | info |

For the static-EQ mode the gains are the constant `StaticEqMask` values.

**Real recordings** (`training/data/real/*.wav|flac`, 48 kHz stereo, otherwise skipped
with a warning):
- Write `<eval out>/real/<name>_{off,eq,model}.flac` through the real processor with
  the limiter on, loudness-matched to the bypass RMS for fair listening.
- Write a gain heatmap PNG (32 bands × time).
- Report the % of frames with any band above +2 dB.

Recording guide (in the README):
- play a CS2 demo;
- `pw-record --target <sink>.monitor --rate 48000 --channels 2 training/data/real/<name>.wav`;
- 5–10 clips of 1–2 min, a mix of quiet rounds and firefights.

**Report:** `training/data/eval/<swm stem>/report.md` (or `--out`) with the A/B/C table, a per-band mean-gain plot,
a gain histogram, and 5 val listening examples (`val_XX_{off,eq,model}.flac`).

## Error handling

- `TrainerError` messages are printed by the CLI and give exit code 1, the same pattern as
  `DatagenError`.
- Missing set, missing extra (`stepwave_py` not installed → a hint to run
  `uv sync --extra audio`), a manifest whose constants differ from the binding, and a
  features fingerprint mismatch on `--resume` all raise `TrainerError`.
- NaN loss aborts training with the step number and keeps `last.pt` from the previous
  epoch.
- `.swm` read errors name the offending field.

## Testing

Only synthetic signals (sines, seeded noise) are used; there are no game assets.

| Test | Assertion |
|---|---|
| Rust `ExternalMask` | Given the StaticEqMask gains for every frame → output identical to the static-EQ processor; holds the last frame; empty → unity |
| Rust frame alignment | Energies from `band_energies_db` frame k correspond to the k-th `next_mask` call (impulse in one hop → max energy frame index == frame whose gain change affects it) |
| Rust limiter switch | Limiter off with a +12 dB mask on a loud sine → peak exceeds the ceiling; on → ≤ −1 dBFS |
| Binding `apply_band_gains` | 0 dB gains reproduce input within −90 dB; +6 dB on all bands → +6 dB ± 0.1 in the steady state; shape errors → `ValueError` |
| `prep` | A tiny synthetic set (written in the test) → arrays of the right shape and dtype, index offsets consistent, mean/std from train only, resume skip and `--force` |
| Targets | No footsteps → exactly 0 dB; footsteps only → +s dB; gunfire only while active → −6 dB; clipping at ±12; dilation reaches ±15 frames |
| Features | Deltas are 0 at t=0; normalisation uses the manifest mean/std |
| Model | Output shape, gain range, causality (changing input at t≥k leaves outputs before k unchanged) |
| Loss | Positive prediction on inactive frames costs λ times more than MSE alone |
| Training smoke | 30 steps on CPU on a tiny set: loss decreases; `best.pt`, `last.pt`, `train.json` written; `--resume` continues |
| `.swm` | Write→read round trip; bad magic / version / truncated payload rejected; numpy ref matches torch |
| Eval | On a tiny set with a stub "model" that outputs +6 dB on active frames: footstep gain ≈ +6 dB, false-boost rate 0 %; report file written |

CI: a new `model` job runs `uv sync --extra audio`, `ruff check`, `ruff format --check`
and `pytest` on CPU, with torch from the CPU index.

## Execution after implementation

These are operational steps, run by the user with guidance:
1. Laptop: `datagen mix cs2 --split train` (50 h), then `--split val`, then `datagen stats cs2`.
2. Laptop: `trainer prep cs2`, then copy `training/data/features/cs2/` to the Windows PC.
3. PC: `uv sync`, then `trainer train …`. Record the seconds per epoch.
4. Copy `runs/<run>/` back. Then run `trainer export`, `trainer eval`, and listen.
5. Commit the eval summary to `docs/measurements/`; keep `models/cs2.swm` local (git-ignored).

## Out of scope for M3

- Rust GRU inference and its benchmark (M4).
- Plugins (M5/M6).
- The PUBG and generic models.
- Training-time augmentation.
- Side/mid features.
- Modelling the smoother inside training.
- Strength control at runtime.
