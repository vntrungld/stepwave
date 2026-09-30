# stepwave

Per-game AI audio enhancer for competitive FPS games — a software take on "footstep
boost" sound cards like the Fosi Audio C3 StepSense.

A tiny real-time neural net recognises footsteps in game audio and boosts only them
(while taming gunfire/explosions), preserving the stereo image. Runs as a VST3 plugin
in Equalizer APO (Windows) or a native PipeWire daemon (Linux) — no game memory
access, no injection.

**Status:** planning / scaffolding. See [CLAUDE.md](CLAUDE.md) for architecture and roadmap.

> Game sound assets used for training are never included in this repository. The trained
> CS2 model is published as a release asset; see [docs/models.md](docs/models.md) for its
> provenance.

## Offline CLI (M1)

```sh
cargo run --release -p stepwave-cli -- in.wav out.wav --profile cs2
cargo run --release -p stepwave-cli -- in.wav out-bypass.wav --profile cs2 --bypass  # A/B reference
```

Input must be 48 kHz (mono or stereo). Output is 48 kHz stereo 32-bit float, time-aligned
with the input. M1 applies the profile's static fallback EQ through the full spectral
pipeline; the model arrives in M4.

## Linux daemon (M5)

`stepwave` creates a PipeWire sink named `stepwave`, moves **only** matching game streams
into it (by process binary, from each profile's `match.linux`) and plays the processed
audio on your default output. Other apps are never touched; if the daemon stops, the game
falls back to the device by itself.

```sh
cargo install --path daemon
mkdir -p ~/.config/stepwave/profiles ~/.config/stepwave/models ~/.config/systemd/user
cp profiles/*.json ~/.config/stepwave/profiles/
# Download the model (see docs/models.md) or copy your own; without a model the
# daemon uses the profile's static EQ:
gh release download model-cs2-003 -R vntrungld/stepwave -p cs2.swm -D ~/.config/stepwave/models
cp daemon/stepwave.service ~/.config/systemd/user/
systemctl --user daemon-reload && systemctl --user enable --now stepwave

stepwave status                 # what is routed and running
stepwave toggle                 # A/B: processing <-> bypass (bind to a KDE shortcut)
stepwave mode auto|model|eq|bypass
stepwave strength 7             # dB, until reload/restart
stepwave profile cs2            # pin a profile (e.g. to test with a video); --auto to unpin
stepwave reload                 # re-read profiles and models
```

## Training data (M2)

Synthetic, labelled mixtures are generated from locally extracted game sounds by
[`training/datagen`](training/datagen/README.md). Game assets are never committed.
