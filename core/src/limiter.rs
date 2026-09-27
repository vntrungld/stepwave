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
}
