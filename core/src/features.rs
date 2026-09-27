//! Offline feature extraction for training: per-hop ERB band energies of the mid signal.
//! Uses the same STFT framing as `Processor`, so training sees the plugin's features exactly.
//! This is not a real-time API: it allocates its result.

use realfft::RealFftPlanner;

use crate::erb::{ErbBands, NUM_BANDS};
use crate::stft::{Complex32, StftChannel, BINS, HOP};

pub struct FeatureExtractor {
    left: StftChannel,
    right: StftChannel,
    bands: ErbBands,
    mid: Vec<Complex32>,
}

impl FeatureExtractor {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::new();
        Self {
            left: StftChannel::new(&mut planner),
            right: StftChannel::new(&mut planner),
            bands: ErbBands::new(),
            mid: vec![Complex32::new(0.0, 0.0); BINS],
        }
    }

    /// One row of band energies (dB) per complete hop of `min(left.len(), right.len())` samples.
    pub fn band_energies_db(&mut self, left: &[f32], right: &[f32]) -> Vec<[f32; NUM_BANDS]> {
        self.left.reset();
        self.right.reset();
        let n = left.len().min(right.len());
        let (l_hops, _) = left[..n].as_chunks::<HOP>();
        let (r_hops, _) = right[..n].as_chunks::<HOP>();
        let mut rows = Vec::with_capacity(l_hops.len());
        for (l, r) in l_hops.iter().zip(r_hops) {
            self.left.analyze(l);
            self.right.analyze(r);
            for ((m, a), b) in self
                .mid
                .iter_mut()
                .zip(self.left.spectrum())
                .zip(self.right.spectrum())
            {
                *m = (*a + *b) * 0.5;
            }
            let mut row = [0.0; NUM_BANDS];
            self.bands.band_energies_db(&self.mid, &mut row);
            rows.push(row);
        }
        rows
    }
}

impl Default for FeatureExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{sine, white_noise};

    #[test]
    fn one_row_per_complete_hop() {
        let x = white_noise(1, HOP * 7 + 100, 0.3);
        let rows = FeatureExtractor::new().band_energies_db(&x, &x);
        assert_eq!(rows.len(), 7);
    }

    #[test]
    fn repeated_calls_are_identical() {
        let x = sine(1000.0, 0.5, 4800);
        let mut fx = FeatureExtractor::new();
        assert_eq!(fx.band_energies_db(&x, &x), fx.band_energies_db(&x, &x));
    }

    #[test]
    fn anti_phase_stereo_has_silent_mid() {
        let l = sine(1000.0, 0.5, 4800);
        let r: Vec<f32> = l.iter().map(|v| -v).collect();
        let rows = FeatureExtractor::new().band_energies_db(&l, &r);
        assert!(rows.iter().flatten().all(|v| *v < -90.0));
    }
}
