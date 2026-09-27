//! 32 bands spaced uniformly on the ERB-rate scale, and interpolation of band gains to FFT bins.

use crate::stft::{BINS, BIN_HZ};

pub const NUM_BANDS: usize = 32;
pub const MIN_HZ: f32 = 50.0;
pub const MAX_HZ: f32 = 20_000.0;

pub fn erb_rate(hz: f32) -> f32 {
    21.4 * (1.0 + 0.00437 * hz).log10()
}

pub fn erb_rate_to_hz(e: f32) -> f32 {
    (10f32.powf(e / 21.4) - 1.0) / 0.00437
}

pub struct ErbBands {
    centres_hz: [f32; NUM_BANDS],
    /// Per bin: lower neighbouring band and the weight of the upper one.
    interp: Vec<(usize, f32)>,
}

impl ErbBands {
    pub fn new() -> Self {
        let lo = erb_rate(MIN_HZ);
        let step = (erb_rate(MAX_HZ) - lo) / (NUM_BANDS - 1) as f32;
        let mut centres_hz = [0.0; NUM_BANDS];
        for (i, c) in centres_hz.iter_mut().enumerate() {
            *c = erb_rate_to_hz(lo + step * i as f32);
        }
        let last = (NUM_BANDS - 1) as f32;
        let interp = (0..BINS)
            .map(|k| {
                let pos = ((erb_rate(k as f32 * BIN_HZ) - lo) / step).clamp(0.0, last);
                let lower = (pos.floor() as usize).min(NUM_BANDS - 2);
                (lower, pos - lower as f32)
            })
            .collect();
        Self { centres_hz, interp }
    }

    pub fn centres_hz(&self) -> &[f32; NUM_BANDS] {
        &self.centres_hz
    }

    /// Linear interpolation on the ERB-rate axis; edge bands hold outside the range.
    pub fn interpolate_db(&self, band_db: &[f32; NUM_BANDS], bin_db: &mut [f32]) {
        for (out, &(i, w)) in bin_db.iter_mut().zip(&self.interp) {
            *out = band_db[i] * (1.0 - w) + band_db[i + 1] * w;
        }
    }
}

impl Default for ErbBands {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn erb_rate_round_trips() {
        for hz in [50.0, 1000.0, 20_000.0] {
            assert_relative_eq!(erb_rate_to_hz(erb_rate(hz)), hz, max_relative = 1e-4);
        }
    }

    #[test]
    fn centres_span_range_and_increase() {
        let bands = ErbBands::new();
        let c = bands.centres_hz();
        assert_relative_eq!(c[0], MIN_HZ, max_relative = 1e-3);
        assert_relative_eq!(c[NUM_BANDS - 1], MAX_HZ, max_relative = 1e-3);
        assert!(c.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn constant_bands_give_constant_bins() {
        let bands = ErbBands::new();
        let mut bins = [0.0; BINS];
        bands.interpolate_db(&[-3.0; NUM_BANDS], &mut bins);
        assert!(bins.iter().all(|&g| (g + 3.0).abs() < 1e-6));
    }

    #[test]
    fn increasing_bands_give_non_decreasing_bins_with_edge_hold() {
        let bands = ErbBands::new();
        let mut ramp = [0.0; NUM_BANDS];
        for (i, g) in ramp.iter_mut().enumerate() {
            *g = i as f32;
        }
        let mut bins = [0.0; BINS];
        bands.interpolate_db(&ramp, &mut bins);
        assert!(bins.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(bins[0], 0.0); // 0 Hz is below the first centre
        assert_eq!(bins[BINS - 1], (NUM_BANDS - 1) as f32); // 24 kHz is above the last
    }
}
