# Models

Trained models are published as GitHub Release assets, never committed to git.

| Model | Release | Trained on | Notes |
|---|---|---|---|
| `cs2.swm` (run cs2-003) | [`model-cs2-003`](https://github.com/vntrungld/stepwave/releases/tag/model-cs2-003) | Synthetic mixtures of CS2 sounds | Use with `profiles/cs2.json` (strength 7 dB) |

## Provenance of the CS2 model

The CS2 model was trained on synthetic mixtures built from sound files extracted from
Counter-Strike 2's VPK archives with Source 2 Viewer (see
[`training/datagen`](../training/datagen/README.md)). The model file contains only
network weights and feature statistics — no audio — but it is derived from Valve's
copyrighted game assets.

stepwave is not affiliated with or endorsed by Valve. Counter-Strike 2 is a trademark of
Valve Corporation. If Valve asks for the model to be taken down, it will be removed.

The extracted game sounds themselves are never published.

## Install

Download `cs2.swm` from the release and put it next to the profiles:

- Linux: `~/.config/stepwave/models/cs2.swm`
- Windows: `%LOCALAPPDATA%\stepwave\models\cs2.swm`

Then run `stepwave reload`. Without a model, stepwave falls back to the profile's static
EQ.

## Train your own

Everything needed is in [`training/`](../training): extract the sounds from your own
copy of CS2, generate mixtures with `training/datagen` and train with `training/model`.
