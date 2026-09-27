# stepwave

Per-game AI audio enhancer for competitive FPS games — a software take on "footstep
boost" sound cards like the Fosi Audio C3 StepSense.

A tiny real-time neural net recognises footsteps in game audio and boosts only them
(while taming gunfire/explosions), preserving the stereo image. Runs as a VST3 plugin
in Equalizer APO (Windows) or an LV2 plugin in PipeWire (Linux) — no game memory
access, no injection.

**Status:** planning / scaffolding. See [CLAUDE.md](CLAUDE.md) for architecture and roadmap.

> Game sound assets used for training are never included in this repository.
