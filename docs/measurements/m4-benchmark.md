# M4 benchmark: model inference cost per 10 ms frame

Date: 2026-09-27. Machine: Intel(R) Core(TM) Ultra 9 285H, 16 CPUs. rustc 1.98.1 (48a229cea 2026-09-01).
Command: `cargo bench -p stepwave-core --bench model` (criterion 0.5, release profile).
Model: `core/tests/fixtures/model_random.swm` (random weights; same architecture and cost
as a trained cs2 model, 109,792 parameters).

| Benchmark | What it measures | Time (median) | Share of the 1 ms budget |
|---|---|---|---|
| `model_mask_next_mask` | band energies + GRU for one frame | 31.162 µs | 3.12 % |
| `process_hop_model` | full pipeline, one 480-sample stereo hop, model mask | 38.200 µs | 3.82 % |
| `process_hop_eq` | full pipeline, one hop, static EQ mask | 7.8751 µs | 0.79 % |

One hop is 10 ms of audio, so `process_hop_model` / 10 ms is the fraction of one core the
plugin needs (CLAUDE.md target: < 2 %): 38.200 µs / 10 000 µs = 0.382 %.
