# stepwave-model

Footstep band-mask model for stepwave: `prep` (laptop) → `train` (any torch device, e.g.
the Windows RTX 3070 PC) → `export` → `eval` (laptop). Generated data stays in the
git-ignored `training/data/`.

```sh
cd training/model && uv sync --extra audio && cd ../..
alias trainer="uv run --project training/model trainer"
```
