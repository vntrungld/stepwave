# M2: synthetic training-data generator (`training/datagen`)

Date: 2026-09-27
Status: approved in chat, awaiting written-spec review

## Goal

Produce reproducible, labelled training data for the CS2 footstep model (M3) with no manual
labelling. The generator extracts CS2 sound assets, classifies them, mixes seeded 10 s
stereo scenes with known ground-truth stems, and reports dataset statistics.

M2 stores **audio**: the mixture plus one stem per class. Features and target masks are
computed later (M3), through Python bindings to the Rust `core`. This means training sees
exactly the features the real-time plugin computes, and changing `strength_db` or the
feature set never requires regenerating data.

## Decisions (from brainstorming)

| Decision | Choice | Reason |
|---|---|---|
| Sound source | Extract CS2 assets locally with Source2Viewer-CLI | Real game sounds. The VPK is at `~/.local/share/Steam/steamapps/common/Counter-Strike Global Offensive/game/csgo/pak01_dir.vpk` on the Linux dev box |
| Classes | footsteps, gunfire, explosions, ambience, other | Enough for boost + duck. "other" is neither boosted nor ducked |
| Storage | Raw audio stems; features from Rust via `pyo3` | Bit-exact feature parity with the plugin; no regeneration when features change |
| Spatialisation | Real HRTF from a public SOFA set (default MIT KEMAR) | CS2 uses HRTF; realistic stereo teaches real localisation cues |
| Generation | Offline, seeded, written to disk | Reproducible, listenable, and allows statistics |
| Target masks | Not in M2 (computed in M3 from stems) | Keeps `strength_db` a training-time choice |

**Legal:** game assets and anything derived from them (extracted WAVs, catalog, mixed sets)
live only under git-ignored `training/data/`. They are never committed or redistributed.

## Layout

```
bindings/python/                 crate `stepwave-py` (pyo3 + maturin) → Python module `stepwave_py`
core/src/erb.rs                  + ErbBands::band_energies_db (new, used by the binding)
training/datagen/
  pyproject.toml                 package `stepwave-datagen`, console script `datagen`
  rules/cs2.toml                 path-pattern → class/surface/map rules (committed)
  configs/cs2.toml               holdouts, set sizes, mixing ranges (committed)
  src/stepwave_datagen/
    cli.py                       `datagen extract|catalog|hrtf|mix|stats`
    extract.py                   runs Source2Viewer-CLI
    rules.py                     loads rules, classifies a relative path
    catalog.py                   scans raw/, decodes, resamples, measures, writes catalog.csv
    hrtf.py                      downloads SOFA; loads + resamples HRIRs to 48 kHz; nearest lookup
    scene.py                     seeded scene sampling (what, when, where, how loud)
    render.py                    per-source rendering: distance gain/LPF, HRTF, reverb
    bus.py                       mix-bus compressor + loudness, applied identically to stems
    writer.py                    FLAC + meta.json per clip; resumable
    stats.py                     report.md, plots, leak check, sum check, listening files
    config.py                    typed config loading (dataclasses)
  tests/                         pytest, synthetic audio and a synthetic HRTF only
training/data/                   (git-ignored)
  raw/  catalog.csv  catalog-report.md  hrtf/  sets/<name>/
```

## Dependencies

Python ≥ 3.11 managed with `uv`, linted/formatted with `ruff`.

| Purpose | Package |
|---|---|
| Arrays, DSP | `numpy`, `scipy` (`signal.resample_poly`, `signal.fftconvolve`, `signal.butter`/`sosfilt`) |
| Audio I/O (WAV/FLAC) | `soundfile` |
| SOFA HRTF reading | `sofar` |
| CLI | `typer` |
| Progress | `tqdm` |
| Plots | `matplotlib` |
| Rust bindings | `pyo3`, `numpy` (Rust crate), built with `maturin` |
| Tests | `pytest` |

Config and rules use TOML (stdlib `tomllib`).

## Components

### `stepwave-py` binding and `core` addition
- New in `core`: `ErbBands::band_energies_db(&self, spectrum: &[Complex32], out: &mut [f32; NUM_BANDS])`.
  It is the transpose of the existing interpolation: each bin's power `|X|²` goes to band
  `i` with weight `1 − w` and to band `i + 1` with weight `w`. It is normalised by each
  band's weight sum and converted with `10·log10(x + 1e-10)`. It does not allocate. Rust
  tests cover it.
- Python module `stepwave_py`:
  - `SAMPLE_RATE`, `WIN`, `HOP`, `BINS`, `NUM_BANDS`
  - `erb_centres_hz() -> ndarray[32]`
  - `band_energies_db(left, right) -> ndarray[frames, 32]` (float32 in; mid = (L+R)/2 via
    `StftChannel`; one row per hop, the same framing as `Processor`)
- The crate is built with `maturin` and consumed by `training/datagen` as a uv path
  dependency. It is a workspace member.

### `extract`
- `datagen extract --vpk <path> [--cli <Source2Viewer-CLI>]` exports every `sounds/`
  entry to `training/data/raw/` as WAV.
- It checks that the CLI binary exists and is executable. If not, it prints the GitHub
  release URL (ValveResourceFormat) and exits non-zero. The exact CLI flags are verified
  during implementation against the installed version.
- It skips work if `raw/` already holds an extraction with the same VPK mtime (recorded in
  `raw/.extract.json`).

### `rules` + `catalog`
- `rules/cs2.toml` is an ordered list of `{ pattern (glob on path relative to raw/), class,
  surface?, map? }`. The first match wins. Unmatched paths get class `other`. Capture of
  surface/map from a path segment is supported (`{surface}` / `{map}` placeholders in the
  pattern). The initial rules are written after inspecting the real extracted tree (first
  plan task). They are committed; the audio is not.
- `datagen catalog` decodes each file, downmixes to mono, and resamples to 48 kHz with
  `resample_poly`. It drops files that are shorter than 20 ms, digital silence (peak
  < −80 dBFS), or undecodable. It writes `catalog.csv` with columns
  `path, class, surface, map, duration_s, peak_dbfs, rms_dbfs`, and the processed mono
  audio to `raw/_48k/` as FLAC.
- `catalog-report.md` lists counts per class/surface/map, all `other` paths (for rule
  review) and all dropped files with reasons.

### `hrtf`
- `datagen hrtf` downloads the MIT KEMAR SOFA file (URL in `configs/cs2.toml`) to
  `training/data/hrtf/`.
- Loading resamples the HRIRs to 48 kHz and builds a nearest-direction lookup on
  (azimuth, elevation).

### `scene` (sampling; pure function of the clip seed)
Each clip is 10 s, stereo, 48 kHz. Clip seed = hash(set seed, clip index). All defaults
below live in `configs/cs2.toml`.
- **ambience:** exactly 1 file per clip (looped if shorter), random start offset.
- **footsteps:** with probability 0.7, 1–3 sequences; otherwise none. A sequence is one
  surface, one direction/distance (fixed per sequence), and steps every 0.3–0.6 s over a
  random 1–10 s span, drawing step files from that surface.
- **gunfire:** 0–3 bursts. Each burst is either "own" (azimuth 0°, 0.5 m, dry) or
  "enemy" (random direction, 5–40 m).
- **explosions:** 0–1. **other:** 0–2.
- **position:** azimuth U(0, 360)°, elevation U(−20, 30)°, distance U(1, 40) m, unless
  overridden above.
- **footstep SNR:** footstep stem RMS relative to the RMS of all other stems combined,
  U(−30, 0) dB, applied by scaling the footstep stem after rendering. Silent regions are
  excluded from the RMS measurement: only samples where the stem envelope is above −60 dB
  of its peak are counted.

### `render`
For each source:
1. gain = 1 / max(distance, 0.5 m)
2. air-absorption low-pass with cutoff = 20 kHz · (1 m / distance)^0.5, clamped to
   [2 kHz, 20 kHz]
3. convolve with the nearest HRIR pair (L/R)
4. reverb: convolve with a synthetic stereo RIR (decorrelated exponentially decaying noise,
   RT60 U(0.2, 1.2) s per clip, shared by all sources in the clip), wet/dry ratio
   U(0.05, 0.3) per source. Own gunfire gets no reverb or HRTF.

Rendered sources are summed into their class stem (5 stems, each stereo).

### `bus`
- mix = sum of the 5 stems.
- Compressor on the mix (stereo-linked, threshold −18 dBFS, ratio 3:1, attack 5 ms,
  release 100 ms) produces a gain curve `g[n]`.
- Loudness: the mixture is scaled so that its RMS is U(−35, −15) dBFS, then `g` is
  multiplied by that scale.
- **The same `g[n]` is applied to every stem**, so `sum(stems) == mix` holds to
  float32 precision. Final peaks are checked. If a clip would exceed −1 dBFS, the whole
  clip (mix and stems) is scaled down; clips are never clipped.

### `writer`
- `sets/<name>/<split>/<clip_id>/` contains `mix.flac`, `footsteps.flac`, `gunfire.flac`,
  `explosions.flac`, `ambience.flac`, `other.flac` (48 kHz, stereo, 24-bit FLAC), and
  `meta.json`.
- `meta.json` holds: seed, set/split, RT60, loudness scale, footstep SNR, and per event
  {source path, class, surface, map, onset_s, azimuth, elevation, distance, gain, wet}.
- `sets/<name>/manifest.json` holds: config snapshot, catalog hash, git commit, and
  counts.
- **Resumable:** a clip directory containing a complete `meta.json` (written last) is
  skipped. Generation uses a `ProcessPoolExecutor` (default: all cores).

### Splits
- `configs/cs2.toml` names the held-out maps and surfaces. The default is 1 map + 1
  surface, chosen after extraction.
- The **val** split uses held-out items only for ambience/footsteps, and all items for
  other classes. The **train** split never uses them.
- Default sizes: train 50 h (18 000 clips), val 5 h (1 800 clips), overridable by CLI
  (`--hours`, `--split`). Estimated disk use for defaults is 60–80 GB.

### `stats`
`datagen stats <set>` writes `sets/<name>/report/` with:
- `report.md` covering clip count, duration, fraction with footsteps, and distributions of
  SNR / distance / azimuth / surface / map / loudness (PNG plots).
- Mean ERB-band energy per class, computed via `stepwave_py.band_energies_db`.
- **Leak check:** held-out maps/surfaces must not appear in train. On violation it exits
  non-zero.
- **Sum check:** `sum(stems) − mix` must be < −80 dB relative to the mix for every clip.
  Violations are listed and the command exits non-zero.
- 5 listening files, each `mix` followed by `footsteps` only, concatenated with 0.5 s
  silence between them.

## Error handling

- Missing Source2Viewer-CLI, VPK, catalog, or HRTF: a clear message naming the fix and the
  command to run, then a non-zero exit.
- Undecodable or silent source files: logged in `catalog-report.md` and skipped. They
  never abort a run.
- `mix` refuses to start if any required class (for the chosen split, after holdout
  filtering) has zero files, and names the class.
- Any worker exception during `mix` is reported with the clip id. Other clips continue,
  and the run exits non-zero at the end.

## Testing

pytest, with synthetic audio and a synthetic HRTF only (no game assets):

| Test | Assertion |
|---|---|
| Rules | Sample paths map to the expected class/surface/map; unknown paths → `other`; first match wins |
| Catalog | Resampling 44.1 → 48 kHz keeps duration; silent/short files are dropped with a reason |
| Determinism | Same seed → byte-identical mix and meta; different seed → different |
| Stem sum | `sum(stems) == mix` within −80 dB for random scenes |
| SNR | Measured footstep SNR equals `meta.json` value within 0.5 dB |
| Spatial | Source at azimuth 90° (left, SOFA convention) → left RMS > right RMS by > 3 dB with a synthetic HRTF |
| Footstep-free fraction | Over 500 scenes, the no-footstep fraction is 0.3 ± 0.06 |
| Splits | Train scenes never reference held-out map/surface; val footsteps/ambience only use held-out items |
| No clipping | All written clips peak ≤ −1 dBFS |
| Resume | A second `mix` run with the same args writes nothing new |
| Binding (Rust) | `band_energies_db` on a sine puts the maximum in the band whose centre is nearest the sine frequency; allocation-free signature |
| Binding (Python) | `stepwave_py.band_energies_db` equals values computed by a Rust test fixture for the same input (a committed JSON fixture under `bindings/python/tests/fixtures/`, produced by a Rust test) |

CI gains a Python job: `uv sync`, `maturin develop` for the binding, `ruff check`,
`ruff format --check`, and `pytest`. Rust CI stays unchanged, apart from the new crate
being built and tested by the existing `cargo` steps.

## Out of scope for M2

Target-mask computation and training (M3), real gameplay recordings (M3 listening tests),
PUBG and other games, on-the-fly mixing, and per-surface footstep boost targets.
