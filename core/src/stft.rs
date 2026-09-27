//! Streaming STFT/iSTFT: 20 ms periodic sqrt-Hann window, 10 ms hop, 48 kHz.

use std::sync::Arc;

pub use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

pub const SAMPLE_RATE: u32 = 48_000;
pub const WIN: usize = 960;
pub const HOP: usize = 480;
pub const BINS: usize = WIN / 2 + 1;
pub const BIN_HZ: f32 = SAMPLE_RATE as f32 / WIN as f32;

/// One channel of analysis + overlap-add synthesis. All buffers are allocated in `new`.
pub struct StftChannel {
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    frame: Vec<f32>,
    spectrum: Vec<Complex32>,
    ola: Vec<f32>,
    scratch_fwd: Vec<Complex32>,
    scratch_inv: Vec<Complex32>,
}

impl StftChannel {
    pub fn new(planner: &mut RealFftPlanner<f32>) -> Self {
        let fwd = planner.plan_fft_forward(WIN);
        let inv = planner.plan_fft_inverse(WIN);
        // Periodic Hann = symmetric Hann of length WIN + 1 without its last sample.
        let window = apodize::hanning_iter(WIN + 1)
            .take(WIN)
            .map(|w| (w as f32).sqrt())
            .collect();
        Self {
            spectrum: fwd.make_output_vec(),
            scratch_fwd: fwd.make_scratch_vec(),
            scratch_inv: inv.make_scratch_vec(),
            fwd,
            inv,
            window,
            input: vec![0.0; WIN],
            frame: vec![0.0; WIN],
            ola: vec![0.0; WIN],
        }
    }

    /// Push one hop of input and compute the spectrum of the latest window.
    pub fn analyze(&mut self, hop_in: &[f32; HOP]) {
        self.input.copy_within(HOP.., 0);
        self.input[WIN - HOP..].copy_from_slice(hop_in);
        for ((f, &x), &w) in self.frame.iter_mut().zip(&self.input).zip(&self.window) {
            *f = x * w;
        }
        // Buffer lengths are fixed at construction, so this cannot fail.
        let _ = self.fwd.process_with_scratch(
            &mut self.frame,
            &mut self.spectrum,
            &mut self.scratch_fwd,
        );
    }

    pub fn spectrum(&self) -> &[Complex32] {
        &self.spectrum
    }

    pub fn spectrum_mut(&mut self) -> &mut [Complex32] {
        &mut self.spectrum
    }

    /// Inverse-transform the current spectrum, overlap-add, and emit one finished hop.
    pub fn synthesize(&mut self, hop_out: &mut [f32; HOP]) {
        // realfft's inverse rejects non-zero imaginary parts at DC and Nyquist.
        self.spectrum[0].im = 0.0;
        self.spectrum[BINS - 1].im = 0.0;
        let _ = self.inv.process_with_scratch(
            &mut self.spectrum,
            &mut self.frame,
            &mut self.scratch_inv,
        );
        let scale = 1.0 / WIN as f32;
        for ((o, &x), &w) in self.ola.iter_mut().zip(&self.frame).zip(&self.window) {
            *o += x * w * scale;
        }
        hop_out.copy_from_slice(&self.ola[..HOP]);
        self.ola.copy_within(HOP.., 0);
        self.ola[WIN - HOP..].fill(0.0);
    }

    pub fn reset(&mut self) {
        self.input.fill(0.0);
        self.ola.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::white_noise;
    use approx::assert_abs_diff_eq;

    #[test]
    fn constants_match_design() {
        assert_eq!(BINS, 481);
        assert_eq!(BIN_HZ, 50.0);
    }

    #[test]
    fn window_satisfies_cola_for_squared_window() {
        let mut planner = RealFftPlanner::new();
        let ch = StftChannel::new(&mut planner);
        for (a, b) in ch.window[..HOP].iter().zip(&ch.window[HOP..]) {
            assert_abs_diff_eq!(a * a + b * b, 1.0, epsilon = 1e-6);
        }
    }

    #[test]
    #[allow(clippy::chunks_exact_to_as_chunks)]
    fn unity_round_trip_lags_by_one_hop() {
        let mut planner = RealFftPlanner::new();
        let mut ch = StftChannel::new(&mut planner);
        let input = white_noise(1, HOP * 40, 0.5);
        let mut output = vec![0.0; input.len()];
        for (i, o) in input.chunks_exact(HOP).zip(output.chunks_exact_mut(HOP)) {
            ch.analyze(i.try_into().unwrap());
            ch.synthesize(o.try_into().unwrap());
        }
        for (y, x) in output[HOP..].iter().zip(&input) {
            assert_abs_diff_eq!(*y, *x, epsilon = 1e-5);
        }
    }

    #[test]
    fn reset_clears_history() {
        let mut planner = RealFftPlanner::new();
        let mut ch = StftChannel::new(&mut planner);
        let loud = [1.0; HOP];
        ch.analyze(&loud);
        ch.reset();
        let mut out = [0.0; HOP];
        ch.analyze(&[0.0; HOP]);
        ch.synthesize(&mut out);
        assert!(out.iter().all(|v| *v == 0.0));
    }
}
