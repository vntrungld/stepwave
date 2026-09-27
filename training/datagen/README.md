# stepwave-datagen

Synthetic training data for the footstep model. Game audio never leaves `training/data/`
(git-ignored) — never commit extracted sounds, catalogs or generated sets.

## Setup

```sh
cd training/datagen && uv sync && cd ../..
# Source2Viewer-CLI (Linux build) from
# https://github.com/ValveResourceFormat/ValveResourceFormat/releases
# unzipped into training/data/tools/ (or on PATH).
```

After changing Rust code in `core` or `bindings/python`, rebuild the binding:
`cd training/datagen && uv sync --reinstall-package stepwave-py`.

## Pipeline (run from the repo root)

```sh
alias datagen="uv run --project training/datagen datagen"
datagen extract                 # VPK sounds/ → training/data/raw (skips if unchanged)
datagen catalog                 # → catalog.csv + catalog-report.md (review "other" list)
datagen hrtf                    # MIT KEMAR SOFA → training/data/hrtf
datagen mix cs2 --split train   # 50 h by default (config split.train_hours)
datagen mix cs2 --split val     # 5 h, held-out map + surface only
datagen stats cs2               # sets/cs2/report/report.md; exit 1 on leaks/sum errors
```

`mix` is resumable: re-run the same command after an interruption. The same name, split
and `--seed` always reproduce the same bytes.

## Tuning

- `rules/cs2.toml` — which extracted paths are footsteps / gunfire / explosions /
  ambience; everything else is `other`. Re-run `catalog` after editing.
- `configs/cs2.toml` — holdouts, set sizes, scene/render/bus ranges.

## Output

`training/data/sets/<name>/<split>/<clip>/`: `mix.flac` and one stem per class
(stereo, 48 kHz, 24-bit), plus `meta.json` describing every event. Stems sum to the mix.
Features and target masks are computed at training time (M3) via `stepwave_py`.

The `stats` report also shows measured mix RMS per class alongside the leak and
stem-sum checks.
