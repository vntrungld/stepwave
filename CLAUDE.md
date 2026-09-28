# CLAUDE.md — stepwave

Instructions for Claude Code working in this repo. Read this fully before making changes.

## What this project is

**stepwave** (step + sound wave) is a software alternative to the Fosi Audio C3 "StepSense" gaming DAC: a small
real-time neural network that listens to game audio, recognises tactically important cues
(footsteps first; later reloads, doors, utility) and **dynamically** boosts them while
ducking masking sounds (own gunfire, explosions, ambience). It is NOT a static EQ.

Target: ~+6–9 dB on footsteps, no audible artefacts, stereo image preserved, <20 ms added
latency, <2% of one CPU core. Runs as an audio plugin loaded by a system-wide host so it
never touches game memory or files (anti-cheat safe by design).

Owner: Lam Duc (vntrungld). Primary games: **CS2** (Windows, FACEIT) and PUBG. Dev machine
runs CachyOS/KDE (Wayland, PipeWire) and dual-boots Windows. RTX 4060 Ti 16GB for training.

## Architecture

```
game audio ──► host (Equalizer APO on Windows / native PipeWire node on Linux)
                 │
                 ▼
   VST3 plugin / `stepwave` daemon ──► core DSP lib (Rust, no_std-friendly hot path)
                                        ├─ STFT 20 ms window / 10 ms hop @ 48 kHz
                                        ├─ features: 32 ERB bands (log energy) + deltas, mid/side
                                        ├─ model: small GRU → per-band gain mask (0.25..4.0)
                                        ├─ gain smoothing (attack ~5 ms, release ~80 ms)
                                        ├─ SAME mask applied to L and R (never independent!)
                                        └─ output limiter (true-peak, −1 dBFS)
                 ▲
app (tray daemon) ── detects foreground game process → selects per-game model + strength
```

### Directory layout (planned)

| Path | Purpose |
|---|---|
| `core/` | Rust crate: STFT, ERB features, model inference, mask application, limiter. The only place DSP lives. |
| `plugins/vst3/` | VST3 wrapper (use `nih-plug`) — loaded by Equalizer APO's VST host on Windows. |
| `daemon/` | Linux: `stepwave` binary — native PipeWire sink node running `core`, per-app stream routing, CLI control over a Unix socket (M5). |
| `app/` | Tray daemon: foreground-process detection → writes active profile; Windows + Linux backends. |
| `training/datagen/` | Python: synthetic mixture generator (see Data). |
| `training/model/` | PyTorch model, training loop, export to weights file consumed by `core`. |
| `profiles/` | Per-game JSON: process match rules, model file, strength, fallback static EQ. |
| `docs/` | Design notes, measurements, listening-test logs. |

## Key design rules (do not violate)

1. **Stereo image is sacred.** Compute ONE mask from a mid (L+R) representation (optionally
   informed by side), apply identical gains to both channels. Independent per-channel masks
   destroy localisation — the whole point of the product.
2. **Real-time safety in the audio callback:** no allocation, no locks, no logging, no syscalls,
   no panics. Preallocate everything in `initialize()`. Use lock-free atomics/triple buffers for
   parameter + model swaps.
3. **Latency budget:** total algorithmic latency ≤ 20 ms (window 20 ms, hop 10 ms, no lookahead
   by default). Report latency to the host correctly.
4. **CPU inference, not GPU.** Model is tiny; GPU scheduling would contend with the game. Hand-rolled
   GRU + dense with SIMD, or `tract`/`ort` only if it stays alloc-free in the hot path.
5. **48 kHz native.** Don't resample inside the plugin; refuse/bypass other rates in v0.
6. **Always have a safe fallback:** if the model fails to load, bypass cleanly (unity gain).
7. **Headroom:** boosted output passes through the limiter; never clip.
8. Keep model weights format simple & versioned (header + little-endian f32), not pickle.

## Data (the part that decides quality)

No manual labelling. Build **synthetic mixtures with known ground truth**:

- Sources per game: footsteps (by surface), own/enemy gunfire, grenades/explosions, ambience,
  voice lines, UI. For CS2, extract sound assets with Source 2 Viewer from the VPKs.
  **Game assets are for private training only — never commit them or redistribute.**
  `training/data/` is git-ignored.
- Mixer randomises: SNR (footsteps −30..0 dB rel. mix), count/timing, HRTF/pan position,
  distance (level + LPF), room reverb (RIRs), game-like compression.
- Target: ideal ratio mask pushing footsteps up by `strength` dB and masking classes down,
  per ERB band; model learns to predict it from the mixture.
- Hold out whole maps / surfaces for validation. Add a set of real gameplay recordings
  (e.g. CS2 demos) for listening tests only.

## Model

- Input: 32 ERB log-energies + first-order deltas (+ optional side/mid ratio per band).
- Net: Dense(→64) → GRU(96) → GRU(96) → Dense(32, sigmoid) mapped to gain range. ~100–300k params.
- Loss: weighted MSE on log-gain + penalty for boosting when no footsteps present (false positive
  "pumping" is the worst artefact).
- One model per game (`profiles/*.json` points to it); a generic FPS model as fallback.
- Export: `training/model/export.py` → `models/<game>.swm` read by `core`.

## Profiles

`profiles/cs2.json` shape:

```json
{
  "id": "cs2",
  "name": "Counter-Strike 2",
  "match": { "windows": ["cs2.exe"], "linux": ["cs2"] },
  "model": "models/cs2.swm",
  "strength_db": 6.0,
  "fallback_eq": [
    { "type": "lowshelf", "freq": 120, "gain": -4, "q": 0.7 },
    { "type": "peak", "freq": 2500, "gain": 3, "q": 1.0 },
    { "type": "peak", "freq": 5000, "gain": 2, "q": 1.0 }
  ],
  "preamp_db": -4
}
```

## Platform integration

- **Windows:** Equalizer APO + its VST host. The tray app detects the foreground process
  (`GetForegroundWindow` → `GetWindowThreadProcessId` → image name) and writes the active
  profile id to a small file/shared memory the plugin polls (lock-free). APO applies per
  device, not per app — document this; optional VB-Cable route for per-app.
- **Linux:** the `stepwave` daemon owns a native PipeWire virtual sink that runs `core`
  directly (no LV2/filter-chain), watches the registry for streams whose
  `application.process.binary` matches a profile and routes **only that stream** into the sink
  via `target.object` metadata (true per-app processing). Controlled with `stepwave on|off|
  toggle|mode|strength|profile|reload`. See `docs/superpowers/specs/2026-09-28-m5-linux-daemon-design.md`.
- **Android:** out of scope for now.

## Fair-play note

Output-side audio processing is equivalent to what hardware like the Fosi C3 does. Do NOT add
features that read game memory, inject into processes, or render a visual "sound radar" overlay
in the core product — keep the project clearly on the audio-processing side. Remind the user to
check FACEIT/tournament rules.

## Roadmap

- [ ] M0: repo scaffolding, Rust workspace, CI (fmt, clippy, tests)
- [ ] M1: `core` with STFT/ERB + **static EQ fallback** + limiter; offline CLI `stepwave-cli in.wav out.wav --profile cs2`
- [ ] M2: `training/datagen` synthetic mixer + dataset stats
- [ ] M3: train CS2 model, export, offline A/B on real recordings (measure footstep gain, false-boost rate)
- [ ] M4: real-time inference in `core`, benchmark (µs per 10 ms frame, must be < 1 ms)
- [ ] M5: Linux `stepwave` daemon — native PipeWire sink running `core` + per-app stream routing + CLI control (Linux first — dev box)
- [ ] M6: VST3 plugin + Equalizer APO setup guide + Windows tray app (for FACEIT CS2)
- [ ] M7: PUBG model, generic FPS model, profile UI

## Conventions

- Rust stable, edition 2021; `cargo fmt` + `cargo clippy -- -D warnings` must pass.
- Python ≥3.11 managed with `uv`; `ruff` for lint/format.
- Every DSP change needs an offline test: render a fixture WAV and assert on metrics
  (gain in footstep band, no clipping, L/R correlation preserved). Put fixtures under
  `core/tests/fixtures/` — synthetic only, no game assets.
- Commit messages: conventional commits (`feat(core): ...`).
- Communicate with the user in Vietnamese; code, comments and docs in English.
