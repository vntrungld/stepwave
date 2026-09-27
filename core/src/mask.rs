//! Mask sources: produce per-band gains (dB) once per hop.

use biquad::{Coefficients, Hertz, Type};
use realfft::num_complex::Complex64;

use crate::erb::{ErbBands, NUM_BANDS};
use crate::gru::ModelRunner;
use crate::profile::{EqBand, EqKind, Profile};
use crate::stft::{Complex32, SAMPLE_RATE};
use crate::swm::SwmModel;
use crate::CoreError;

/// Slot for anything that decides band gains. M1: static EQ; M4: the model.
pub trait MaskSource: Send {
    /// Fill `gains_db` for the current frame. `mid_spectrum` has `BINS` entries.
    /// Must not allocate, lock or panic.
    fn next_mask(&mut self, mid_spectrum: &[Complex32], gains_db: &mut [f32; NUM_BANDS]);

    /// Forget any per-stream state (called by `Processor::reset`). Must not allocate.
    fn reset(&mut self) {}
}

/// 0 dB everywhere; used for bypass/A-B listening.
pub struct UnityMask;

impl MaskSource for UnityMask {
    fn next_mask(&mut self, _mid_spectrum: &[Complex32], gains_db: &mut [f32; NUM_BANDS]) {
        gains_db.fill(0.0);
    }
}

/// The profile's `fallback_eq` sampled at the ERB band centres, plus `preamp_db`.
pub struct StaticEqMask {
    gains_db: [f32; NUM_BANDS],
}

impl StaticEqMask {
    pub fn from_profile(profile: &Profile, bands: &ErbBands) -> Result<Self, CoreError> {
        let filters = profile
            .fallback_eq
            .iter()
            .map(eq_coefficients)
            .collect::<Result<Vec<_>, _>>()?;
        let mut gains_db = [profile.preamp_db; NUM_BANDS];
        for (gain, &hz) in gains_db.iter_mut().zip(bands.centres_hz()) {
            *gain += filters.iter().map(|c| magnitude_db(c, hz)).sum::<f32>();
        }
        Ok(Self { gains_db })
    }

    pub fn gains_db(&self) -> &[f32; NUM_BANDS] {
        &self.gains_db
    }
}

impl MaskSource for StaticEqMask {
    fn next_mask(&mut self, _mid_spectrum: &[Complex32], gains_db: &mut [f32; NUM_BANDS]) {
        *gains_db = self.gains_db;
    }
}

/// Offline/test source: replays a precomputed gain sequence, one row per hop, then holds
/// the last row (0 dB everywhere if the sequence is empty). The sequence is allocated by
/// the caller; `next_mask` never allocates. Not meant for plugins.
pub struct ExternalMask {
    frames: Vec<[f32; NUM_BANDS]>,
    next: usize,
}

impl ExternalMask {
    pub fn new(frames: Vec<[f32; NUM_BANDS]>) -> Self {
        Self { frames, next: 0 }
    }
}

impl MaskSource for ExternalMask {
    fn next_mask(&mut self, _mid_spectrum: &[Complex32], gains_db: &mut [f32; NUM_BANDS]) {
        match self.frames.get(self.next).or(self.frames.last()) {
            Some(row) => *gains_db = *row,
            None => gains_db.fill(0.0),
        }
        self.next = self.next.saturating_add(1);
    }
}

/// The trained model: mid band energies → per-band gains, scaled by the profile's strength
/// relative to the strength the model was trained with, clamped to the model's gain range.
pub struct ModelMask {
    runner: ModelRunner,
    bands: ErbBands,
    energies: [f32; NUM_BANDS],
    scale: f32,
    lo: f32,
    hi: f32,
}

impl ModelMask {
    pub fn new(model: &SwmModel, strength_db: f32) -> Self {
        let (lo, hi) = model.gain_db_range();
        Self {
            runner: ModelRunner::new(model),
            bands: ErbBands::new(),
            energies: [0.0; NUM_BANDS],
            scale: strength_db / model.strength_db(),
            lo,
            hi,
        }
    }
}

impl MaskSource for ModelMask {
    fn next_mask(&mut self, mid_spectrum: &[Complex32], gains_db: &mut [f32; NUM_BANDS]) {
        self.bands
            .band_energies_db(mid_spectrum, &mut self.energies);
        self.runner.step(&self.energies, gains_db);
        for g in gains_db.iter_mut() {
            *g = (*g * self.scale).clamp(self.lo, self.hi);
        }
    }

    fn reset(&mut self) {
        self.runner.reset();
    }
}

/// RBJ cookbook coefficients for one profile EQ band at 48 kHz.
pub fn eq_coefficients(band: &EqBand) -> Result<Coefficients<f32>, CoreError> {
    let invalid = |e| CoreError::InvalidProfile(format!("fallback_eq {band:?}: {e:?}"));
    let kind = match band.kind {
        EqKind::Lowshelf => Type::LowShelf(band.gain),
        EqKind::Highshelf => Type::HighShelf(band.gain),
        EqKind::Peak => Type::PeakingEQ(band.gain),
    };
    let fs = Hertz::<f32>::from_hz(SAMPLE_RATE as f32).map_err(invalid)?;
    let f0 = Hertz::<f32>::from_hz(band.freq).map_err(invalid)?;
    Coefficients::<f32>::from_params(kind, fs, f0, band.q).map_err(invalid)
}

/// Magnitude response of a biquad at `hz`, in dB.
pub fn magnitude_db(c: &Coefficients<f32>, hz: f32) -> f32 {
    let w = std::f64::consts::TAU * hz as f64 / SAMPLE_RATE as f64;
    let z1 = Complex64::from_polar(1.0, -w);
    let z2 = z1 * z1;
    let num = Complex64::new(c.b0 as f64, 0.0) + z1 * c.b1 as f64 + z2 * c.b2 as f64;
    let den = Complex64::new(1.0, 0.0) + z1 * c.a1 as f64 + z2 * c.a2 as f64;
    (20.0 * (num.norm() / den.norm()).log10()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{EqBand, EqKind};
    use approx::assert_abs_diff_eq;

    fn profile(eq: Vec<EqBand>, preamp_db: f32) -> Profile {
        let mut p = Profile::from_json(
            r#"{"id":"t","name":"T","match":{},"model":"m.swm","strength_db":6.0}"#,
        )
        .unwrap();
        p.fallback_eq = eq;
        p.preamp_db = preamp_db;
        p
    }

    #[test]
    fn peak_magnitude_equals_gain_at_centre() {
        let band = EqBand {
            kind: EqKind::Peak,
            freq: 2500.0,
            gain: 3.0,
            q: 1.0,
        };
        let c = eq_coefficients(&band).unwrap();
        assert_abs_diff_eq!(magnitude_db(&c, 2500.0), 3.0, epsilon = 1e-3);
    }

    #[test]
    fn lowshelf_magnitude_equals_gain_near_dc() {
        let band = EqBand {
            kind: EqKind::Lowshelf,
            freq: 120.0,
            gain: -4.0,
            q: 0.7,
        };
        let c = eq_coefficients(&band).unwrap();
        assert_abs_diff_eq!(magnitude_db(&c, 1.0), -4.0, epsilon = 1e-2);
    }

    #[test]
    fn empty_eq_is_just_preamp() {
        let bands = ErbBands::new();
        let mask = StaticEqMask::from_profile(&profile(vec![], -4.0), &bands).unwrap();
        assert!(mask.gains_db().iter().all(|&g| g == -4.0));
    }

    #[test]
    fn peak_at_band_centre_sets_that_band() {
        let bands = ErbBands::new();
        let freq = bands.centres_hz()[10];
        let eq = vec![EqBand {
            kind: EqKind::Peak,
            freq,
            gain: 6.0,
            q: 1.0,
        }];
        let mask = StaticEqMask::from_profile(&profile(eq, -1.0), &bands).unwrap();
        assert_abs_diff_eq!(mask.gains_db()[10], 5.0, epsilon = 1e-2);
    }

    #[test]
    fn next_mask_copies_static_gains_and_unity_is_zero() {
        let bands = ErbBands::new();
        let mut mask = StaticEqMask::from_profile(&profile(vec![], -2.0), &bands).unwrap();
        let mut g = [9.0; NUM_BANDS];
        mask.next_mask(&[], &mut g);
        assert_eq!(g, [-2.0; NUM_BANDS]);
        UnityMask.next_mask(&[], &mut g);
        assert_eq!(g, [0.0; NUM_BANDS]);
    }
}
