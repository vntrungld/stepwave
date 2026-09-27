//! Stereo-linked sample-peak limiter: instant attack, no lookahead, −1 dBFS ceiling.
//! True-peak (oversampled) detection is deferred; see the M0+M1 spec.

use crate::stft::SAMPLE_RATE;

/// −1 dBFS.
pub const CEILING: f32 = 0.891_251;
pub const RELEASE_MS: f32 = 50.0;

pub struct Limiter {
    gain: f32,
    release: f32,
}

impl Limiter {
    pub fn new() -> Self {
        let samples = RELEASE_MS / 1000.0 * SAMPLE_RATE as f32;
        Self {
            gain: 1.0,
            release: (-1.0 / samples).exp(),
        }
    }

    pub fn process(&mut self, left: &mut f32, right: &mut f32) {
        // A NaN/±inf sample must never reach the output: silence it before peak
        // detection so it can't poison `peak`/`gain` or survive the clamp below
        // (NaN.clamp() returns NaN).
        if !left.is_finite() {
            *left = 0.0;
        }
        if !right.is_finite() {
            *right = 0.0;
        }
        let peak = left.abs().max(right.abs());
        let limit = if peak > CEILING { CEILING / peak } else { 1.0 };
        // Drop instantly; recover towards `limit` without ever exceeding it.
        self.gain = if limit < self.gain {
            limit
        } else {
            limit + (self.gain - limit) * self.release
        };
        *left = (*left * self.gain).clamp(-CEILING, CEILING);
        *right = (*right * self.gain).clamp(-CEILING, CEILING);
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
    }
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sine;

    fn run(l: &[f32], r: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut lim = Limiter::new();
        l.iter()
            .zip(r)
            .map(|(&a, &b)| {
                let (mut a, mut b) = (a, b);
                lim.process(&mut a, &mut b);
                (a, b)
            })
            .unzip()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn loud_signal_never_exceeds_ceiling() {
        let x = sine(1000.0, 3.98, 48_000); // +12 dBFS
        let (l, r) = run(&x, &x);
        assert!(peak(&l) <= CEILING && peak(&r) <= CEILING);
    }

    #[test]
    fn quiet_signal_passes_unchanged() {
        let x = sine(440.0, 0.5, 4_800);
        let (l, _) = run(&x, &x);
        assert_eq!(l, x);
    }

    #[test]
    fn gain_is_linked_across_channels() {
        let loud = sine(1000.0, 2.0, 4_800);
        let quiet: Vec<f32> = loud.iter().map(|v| v * 0.25).collect();
        let (l, r) = run(&loud, &quiet);
        for (a, b) in l.iter().zip(&r) {
            assert!((a * 0.25 - b).abs() < 1e-6);
        }
    }

    #[test]
    fn non_finite_samples_are_replaced_and_stay_within_ceiling() {
        let mut lim = Limiter::new();
        for (mut l, mut r) in [
            (f32::NAN, 0.0f32),
            (f32::INFINITY, f32::NEG_INFINITY),
            (f32::NAN, f32::NAN),
            (0.0, f32::INFINITY),
        ] {
            lim.process(&mut l, &mut r);
            assert!(l.is_finite() && r.is_finite(), "got ({l}, {r})");
            assert!(l.abs() <= CEILING && r.abs() <= CEILING, "got ({l}, {r})");
        }
    }

    #[test]
    fn passes_quiet_signal_normally_after_non_finite_spike() {
        let mut lim = Limiter::new();
        let mut nan_l = f32::NAN;
        let mut nan_r = f32::INFINITY;
        lim.process(&mut nan_l, &mut nan_r);

        // The spike is replaced with silence before peak detection, so it never
        // triggers gain reduction: the limiter's gain stays at unity and a
        // following quiet signal (well under the ceiling) must pass unchanged.
        let quiet = sine(440.0, 0.5, 4_800);
        let mut out = quiet.clone();
        let mut out_r = quiet.clone();
        for (a, b) in out.iter_mut().zip(out_r.iter_mut()) {
            lim.process(a, b);
        }
        assert_eq!(out, quiet);
    }
}
