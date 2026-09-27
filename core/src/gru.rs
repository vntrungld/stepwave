//! Real-time inference for the `.swm` v1 network: normalised energies + deltas →
//! Dense(64)+ReLU → GRU(96) → GRU(96) → Dense(32) → sigmoid → gain range (dB).
//! Mirrors `training/model/src/stepwave_model/swm_ref.py` (the spec) in f32.
//! Everything is allocated in `new`; `step` and `reset` never allocate or panic.

use crate::erb::NUM_BANDS;
use crate::swm::{GruWeights, SwmModel, DENSE, GATES, HIDDEN, INPUTS};

pub struct ModelRunner {
    mean: [f32; NUM_BANDS],
    inv_std: [f32; NUM_BANDS],
    lo: f32,
    span: f32,
    inp_w: Vec<f32>,
    inp_b: Vec<f32>,
    gru1: GruWeights,
    gru2: GruWeights,
    out_w: Vec<f32>,
    out_b: Vec<f32>,
    prev: [f32; NUM_BANDS],
    primed: bool,
    x: [f32; INPUTS],
    dense: [f32; DENSE],
    h1: [f32; HIDDEN],
    h2: [f32; HIDDEN],
    gi: [f32; GATES],
    gh: [f32; GATES],
}

fn sigmoid(v: f32) -> f32 {
    1.0 / (1.0 + (-v).exp())
}

/// `y = W·x + b` for row-major `w` with `y.len()` rows of `x.len()` columns.
fn affine(w: &[f32], b: &[f32], x: &[f32], y: &mut [f32]) {
    for ((row, bias), out) in w.chunks_exact(x.len()).zip(b).zip(y.iter_mut()) {
        *out = bias + row.iter().zip(x).map(|(a, v)| a * v).sum::<f32>();
    }
}

/// One PyTorch-convention GRU step (gates r, z, n), updating `h` in place.
fn gru_step(
    w: &GruWeights,
    x: &[f32],
    h: &mut [f32; HIDDEN],
    gi: &mut [f32; GATES],
    gh: &mut [f32; GATES],
) {
    affine(&w.w_ih, &w.b_ih, x, gi);
    affine(&w.w_hh, &w.b_hh, h, gh);
    for k in 0..HIDDEN {
        let r = sigmoid(gi[k] + gh[k]);
        let z = sigmoid(gi[HIDDEN + k] + gh[HIDDEN + k]);
        let n = (gi[2 * HIDDEN + k] + r * gh[2 * HIDDEN + k]).tanh();
        h[k] = (1.0 - z) * n + z * h[k];
    }
}

impl ModelRunner {
    pub fn new(model: &SwmModel) -> Self {
        let (lo, hi) = model.gain_db_range();
        let mut inv_std = [0.0; NUM_BANDS];
        for (i, s) in inv_std.iter_mut().zip(model.std()) {
            *i = 1.0 / s;
        }
        Self {
            mean: *model.mean(),
            inv_std,
            lo,
            span: hi - lo,
            inp_w: model.inp_weight().to_vec(),
            inp_b: model.inp_bias().to_vec(),
            gru1: model.gru1().clone(),
            gru2: model.gru2().clone(),
            out_w: model.out_weight().to_vec(),
            out_b: model.out_bias().to_vec(),
            prev: [0.0; NUM_BANDS],
            primed: false,
            x: [0.0; INPUTS],
            dense: [0.0; DENSE],
            h1: [0.0; HIDDEN],
            h2: [0.0; HIDDEN],
            gi: [0.0; GATES],
            gh: [0.0; GATES],
        }
    }

    /// One 10 ms frame: mid band energies (dB) in, band gains (dB) out.
    #[allow(clippy::needless_range_loop)] // parallel indexing into energies/mean/inv_std/prev
    pub fn step(&mut self, energies_db: &[f32; NUM_BANDS], gains_db: &mut [f32; NUM_BANDS]) {
        let (norm, delta) = self.x.split_at_mut(NUM_BANDS);
        for b in 0..NUM_BANDS {
            let e = energies_db[b];
            norm[b] = (e - self.mean[b]) * self.inv_std[b];
            delta[b] = if self.primed {
                (e - self.prev[b]) * self.inv_std[b]
            } else {
                0.0
            };
        }
        self.prev = *energies_db;
        self.primed = true;

        affine(&self.inp_w, &self.inp_b, &self.x, &mut self.dense);
        for v in self.dense.iter_mut() {
            *v = v.max(0.0);
        }
        gru_step(
            &self.gru1,
            &self.dense,
            &mut self.h1,
            &mut self.gi,
            &mut self.gh,
        );
        gru_step(
            &self.gru2,
            &self.h1,
            &mut self.h2,
            &mut self.gi,
            &mut self.gh,
        );
        affine(&self.out_w, &self.out_b, &self.h2, gains_db);
        for g in gains_db.iter_mut() {
            *g = self.lo + self.span * sigmoid(*g);
        }
    }

    /// Start a new stream: zero hidden state, next frame has zero delta.
    pub fn reset(&mut self) {
        self.h1 = [0.0; HIDDEN];
        self.h2 = [0.0; HIDDEN];
        self.primed = false;
    }
}
