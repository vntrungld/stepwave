# stepwave

Per-game AI audio enhancer for competitive FPS games — a software take on "footstep
boost" sound cards like the Fosi Audio C3 StepSense.

A tiny real-time neural net recognises footsteps in game audio and boosts only them
(while taming gunfire/explosions), preserving the stereo image. Runs as a VST3 plugin
in Equalizer APO (Windows) or an LV2 plugin in PipeWire (Linux) — no game memory
access, no injection.

**Status:** planning / scaffolding. See [CLAUDE.md](CLAUDE.md) for architecture and roadmap.

> Game sound assets used for training are never included in this repository.

## Offline CLI (M1)

```sh
cargo run --release -p stepwave-cli -- in.wav out.wav --profile cs2
cargo run --release -p stepwave-cli -- in.wav out-bypass.wav --profile cs2 --bypass  # A/B reference
```

Input must be 48 kHz (mono or stereo). Output is 48 kHz stereo 32-bit float, time-aligned
with the input. M1 applies the profile's static fallback EQ through the full spectral
pipeline; the model arrives in M4.
