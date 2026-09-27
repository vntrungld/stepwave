# stepwave-model

Footstep band-mask model for stepwave (M3). The pipeline is split across two machines:

| Step | Machine | Command |
|---|---|---|
| 1. Generate data | laptop | `datagen mix cs2 --split train`, `--split val`, `datagen stats cs2` |
| 2. Band energies | laptop | `trainer prep cs2` → `training/data/features/cs2/` (~7 GB for 50 h) |
| 3. Train | Windows PC (RTX 3070) | `trainer train <features> --out <run>` |
| 4. Export | any | `trainer export <run> --out models/cs2.swm` |
| 5. Evaluate | laptop | `trainer eval models/cs2.swm --set cs2` |

The features directory contains numbers only (band energies), no game audio, but it is
derived from game assets: keep it private, like everything in `training/data/`.
`models/*.swm` is git-ignored too.

## Laptop setup (CachyOS)

```sh
cd training/model && uv sync --extra audio && cd ../..
alias trainer="uv run --project training/model trainer"
trainer prep cs2                       # resumable; --force rebuilds
```

After changing Rust code in `core` or `bindings/python`:
`cd training/model && uv sync --extra audio --reinstall-package stepwave-py`.

`trainer train` also runs on the laptop's CPU (e.g. `trainer train <features> --out
<run> --device cpu --max-steps 50` as a smoke test), but a full run is much slower than
on the RTX 3070 — use the Windows PC below for real training.

## Windows training PC (RTX 3070)

1. Install uv (PowerShell): `powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"`
   and git, then `git clone` this repo.
2. Copy `training/data/features/cs2/` from the laptop (USB drive or LAN share), e.g. to
   `D:\stepwave\features\cs2`.
3. In the repo: `cd training\model; uv sync; cd ..\..`. This installs the CUDA 12.8 torch
   build. No Rust is needed: the `audio` extra is not installed.
4. Check the GPU: `uv run --project training/model python -c "import torch; print(torch.cuda.is_available())"`
   should print `True`.
5. Train from the repo root:
   `uv run --project training/model trainer train D:\stepwave\features\cs2 --out D:\stepwave\runs\cs2-001`.
   Each epoch prints its duration and the false-boost rate. `--resume` continues an
   interrupted run.
6. Copy `D:\stepwave\runs\cs2-001\` back to the laptop as `training/data/runs/cs2-001/`.

## Export and evaluate (laptop)

```sh
trainer export training/data/runs/cs2-001 --out models/cs2.swm
trainer eval models/cs2.swm --set cs2    # → training/data/eval/cs2/report.md
```

The report compares off / static EQ / model on the held-out val set:
- footstep gain (goal ≥ +4 dB);
- SNR improvement (goal ≥ +6 dB);
- false-boost rate (goal < 2 %).

It also includes loudness-matched listening files.

## Recording real CS2 audio (for listening only)

1. Start a CS2 demo (`playdemo <name>` in the console) with game audio going to your
   default output.
2. Find the output sink: `pactl get-default-sink`.
3. Record 1–2 minutes per take:
   `pw-record --target "$(pactl get-default-sink).monitor" --rate 48000 --channels 2 training/data/real/<name>.wav`
4. Record 5–10 takes. Mix quiet rounds (footsteps only) with firefights.

`trainer eval` renders every 48 kHz stereo file in `training/data/real/` as
`real/<name>_{off,eq,model}.flac`, plus a gain heatmap. Other formats are skipped with a
note. Real recordings have no ground truth, so the report only shows how often the model
is boosting. Listen for artefacts and for boosts without footsteps.
