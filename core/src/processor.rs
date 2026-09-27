//! The real-time pipeline: STFT → mask → smoothing → shared L/R gain → iSTFT → limiter.

use realfft::RealFftPlanner;

use crate::erb::{ErbBands, NUM_BANDS};
use crate::limiter::Limiter;
use crate::mask::{MaskSource, StaticEqMask};
use crate::smoother::GainSmoother;
use crate::stft::{Complex32, StftChannel, BINS, HOP, SAMPLE_RATE, WIN};
use crate::{CoreError, Profile};

pub struct Processor {
    left: StftChannel,
    right: StftChannel,
    bands: ErbBands,
    mask: Box<dyn MaskSource>,
    smoother: GainSmoother,
    limiter: Limiter,
    mid: Vec<Complex32>,
    raw_db: [f32; NUM_BANDS],
    smooth_db: [f32; NUM_BANDS],
    bin_db: Vec<f32>,
    bin_gain: Vec<f32>,
    in_l: [f32; HOP],
    in_r: [f32; HOP],
    out_l: [f32; HOP],
    out_r: [f32; HOP],
    pos: usize,
}

impl Processor {
    /// Static-EQ processor from a profile's `fallback_eq`.
    pub fn new(profile: &Profile, sample_rate: u32) -> Result<Self, CoreError> {
        let mask = StaticEqMask::from_profile(profile, &ErbBands::new())?;
        Self::with_mask(Box::new(mask), sample_rate)
    }

    pub fn with_mask(mask: Box<dyn MaskSource>, sample_rate: u32) -> Result<Self, CoreError> {
        if sample_rate != SAMPLE_RATE {
            return Err(CoreError::UnsupportedSampleRate(sample_rate));
        }
        let mut planner = RealFftPlanner::new();
        Ok(Self {
            left: StftChannel::new(&mut planner),
            right: StftChannel::new(&mut planner),
            bands: ErbBands::new(),
            mask,
            smoother: GainSmoother::new(),
            limiter: Limiter::new(),
            mid: vec![Complex32::new(0.0, 0.0); BINS],
            raw_db: [0.0; NUM_BANDS],
            smooth_db: [0.0; NUM_BANDS],
            bin_db: vec![0.0; BINS],
            bin_gain: vec![1.0; BINS],
            in_l: [0.0; HOP],
            in_r: [0.0; HOP],
            out_l: [0.0; HOP],
            out_r: [0.0; HOP],
            pos: 0,
        })
    }

    /// One hop of input buffering plus one hop of overlap-add.
    pub fn latency_samples(&self) -> usize {
        WIN
    }

    /// Process in place. Never allocates, locks or panics.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        for (l, r) in left[..n].iter_mut().zip(right[..n].iter_mut()) {
            self.in_l[self.pos] = *l;
            self.in_r[self.pos] = *r;
            let (mut ol, mut or) = (self.out_l[self.pos], self.out_r[self.pos]);
            self.limiter.process(&mut ol, &mut or);
            *l = ol;
            *r = or;
            self.pos += 1;
            if self.pos == HOP {
                self.pos = 0;
                self.process_frame();
            }
        }
    }

    pub fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.smoother.reset();
        self.limiter.reset();
        self.in_l.fill(0.0);
        self.in_r.fill(0.0);
        self.out_l.fill(0.0);
        self.out_r.fill(0.0);
        self.pos = 0;
    }

    fn process_frame(&mut self) {
        self.left.analyze(&self.in_l);
        self.right.analyze(&self.in_r);
        // The FFT is linear, so the mid spectrum is the mean of the L and R spectra.
        for ((m, a), b) in self
            .mid
            .iter_mut()
            .zip(self.left.spectrum())
            .zip(self.right.spectrum())
        {
            *m = (*a + *b) * 0.5;
        }

        self.mask.next_mask(&self.mid, &mut self.raw_db);
        if self.raw_db.iter().all(|g| g.is_finite()) {
            self.smoother.process(&self.raw_db, &mut self.smooth_db);
            self.bands.interpolate_db(&self.smooth_db, &mut self.bin_db);
            for (g, &d) in self.bin_gain.iter_mut().zip(&self.bin_db) {
                *g = 10f32.powf(d / 20.0);
            }
        } else {
            // Bad mask: pass this frame at unity and keep the smoother state clean.
            self.bin_gain.fill(1.0);
        }

        // Stereo image: the SAME gains go to both channels.
        for (s, &g) in self.left.spectrum_mut().iter_mut().zip(&self.bin_gain) {
            *s *= g;
        }
        for (s, &g) in self.right.spectrum_mut().iter_mut().zip(&self.bin_gain) {
            *s *= g;
        }
        self.left.synthesize(&mut self.out_l);
        self.right.synthesize(&mut self.out_r);
    }
}
