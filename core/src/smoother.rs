//! Per-band one-pole gain smoothing in dB, updated once per hop.

use crate::erb::NUM_BANDS;
use crate::stft::{HOP, SAMPLE_RATE};

pub const ATTACK_MS: f32 = 5.0;
pub const RELEASE_MS: f32 = 80.0;

fn coefficient(time_constant_ms: f32) -> f32 {
    let hop_ms = HOP as f32 * 1000.0 / SAMPLE_RATE as f32;
    (-hop_ms / time_constant_ms).exp()
}

pub struct GainSmoother {
    state: [f32; NUM_BANDS],
    attack: f32,
    release: f32,
    primed: bool,
}

impl GainSmoother {
    pub fn new() -> Self {
        Self {
            state: [0.0; NUM_BANDS],
            attack: coefficient(ATTACK_MS),
            release: coefficient(RELEASE_MS),
            primed: false,
        }
    }

    /// Attack applies when a band's gain moves away from 0 dB (a boost growing or a
    /// cut deepening), release when it returns towards 0 dB. So a footstep is boosted
    /// and a gunshot is ducked from their first frames, and both let go slowly.
    pub fn process(&mut self, target_db: &[f32; NUM_BANDS], out_db: &mut [f32; NUM_BANDS]) {
        if self.primed {
            for (s, &t) in self.state.iter_mut().zip(target_db) {
                let engaging = (t > *s && t > 0.0) || (t < *s && t < 0.0);
                let c = if engaging { self.attack } else { self.release };
                *s = t + (*s - t) * c;
            }
        } else {
            self.state = *target_db;
            self.primed = true;
        }
        *out_db = self.state;
    }

    pub fn reset(&mut self) {
        self.primed = false;
    }
}

impl Default for GainSmoother {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(s: &mut GainSmoother, target: f32) -> f32 {
        let mut out = [0.0; NUM_BANDS];
        s.process(&[target; NUM_BANDS], &mut out);
        out[0]
    }

    #[test]
    fn first_frame_jumps_to_target() {
        let mut s = GainSmoother::new();
        assert_eq!(step(&mut s, -4.0), -4.0);
    }

    #[test]
    fn attack_reaches_85_percent_in_one_frame() {
        let mut s = GainSmoother::new();
        step(&mut s, 0.0);
        assert!(step(&mut s, 6.0) >= 0.85 * 6.0);
    }

    #[test]
    fn release_follows_80_ms_time_constant() {
        let mut s = GainSmoother::new();
        step(&mut s, 6.0);
        let mut g = 0.0;
        for _ in 0..8 {
            g = step(&mut s, 0.0); // 8 hops = 80 ms
        }
        let remaining = g / 6.0;
        let expected = (-1.0f32).exp();
        assert!(
            (remaining - expected).abs() <= 0.1 * expected,
            "remaining {remaining}"
        );
    }

    #[test]
    fn duck_engages_within_one_frame() {
        // A masking sound (gunfire) must be cut from its first frames, not 80 ms later.
        let mut s = GainSmoother::new();
        step(&mut s, 0.0);
        assert!(step(&mut s, -6.0) <= 0.85 * -6.0);
    }

    #[test]
    fn boost_to_duck_engages_within_one_frame() {
        let mut s = GainSmoother::new();
        step(&mut s, 6.0);
        assert!(step(&mut s, -6.0) <= -0.85 * 6.0 + 0.15 * 6.0);
    }

    #[test]
    fn duck_recovers_with_80_ms_time_constant() {
        let mut s = GainSmoother::new();
        step(&mut s, -6.0);
        let mut g = 0.0;
        for _ in 0..8 {
            g = step(&mut s, 0.0);
        }
        let remaining = g / -6.0;
        let expected = (-1.0f32).exp();
        assert!(
            (remaining - expected).abs() <= 0.1 * expected,
            "remaining {remaining}"
        );
    }

    #[test]
    fn reset_rearms_the_jump() {
        let mut s = GainSmoother::new();
        step(&mut s, 6.0);
        s.reset();
        assert_eq!(step(&mut s, -2.0), -2.0);
    }
}
