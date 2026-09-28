//! The audio-thread side: runs the current `Processor` on interleaved stereo blocks and
//! swaps in new processors without allocating, locking or clicking.
//!
//! A new processor arrives through a lock-free ring. It first runs in parallel for
//! `WARMUP` samples (its own latency, so its output is valid and time-aligned with the
//! old one), then the output crossfades linearly over `XFADE` samples. The old processor
//! goes back through a second ring so it is dropped off the audio thread.

use stepwave_core::Processor;

/// Samples a new processor runs in parallel before the crossfade: its latency.
pub const WARMUP: usize = 960;
/// Crossfade length (10 ms at 48 kHz).
pub const XFADE: usize = 480;
/// Largest block processed in one go; longer blocks are split.
pub const MAX_FRAMES: usize = 8192;

/// Control-thread end of the handoff.
pub struct Handoff {
    new_tx: rtrb::Producer<Box<Processor>>,
    old_rx: rtrb::Consumer<Box<Processor>>,
}

impl Handoff {
    /// Queue a processor for the audio thread. Gives it back if two are already queued.
    pub fn publish(&mut self, p: Box<Processor>) -> Result<(), Box<Processor>> {
        self.new_tx.push(p).map_err(|rtrb::PushError::Full(b)| b)
    }

    /// Drop processors the audio thread has retired. Returns how many were dropped.
    pub fn collect(&mut self) -> usize {
        let mut n = 0;
        while self.old_rx.pop().is_ok() {
            n += 1;
        }
        n
    }
}

pub struct AudioCore {
    current: Box<Processor>,
    incoming: Option<Box<Processor>>,
    /// Samples processed since `incoming` arrived.
    pos: usize,
    retired: Option<Box<Processor>>,
    new_rx: rtrb::Consumer<Box<Processor>>,
    old_tx: rtrb::Producer<Box<Processor>>,
    l: Vec<f32>,
    r: Vec<f32>,
    l2: Vec<f32>,
    r2: Vec<f32>,
}

/// Create the two ends. `initial` runs until something else is published.
pub fn channel(initial: Processor) -> (Handoff, AudioCore) {
    let (new_tx, new_rx) = rtrb::RingBuffer::new(2);
    let (old_tx, old_rx) = rtrb::RingBuffer::new(4);
    let core = AudioCore {
        current: Box::new(initial),
        incoming: None,
        pos: 0,
        retired: None,
        new_rx,
        old_tx,
        l: vec![0.0; MAX_FRAMES],
        r: vec![0.0; MAX_FRAMES],
        l2: vec![0.0; MAX_FRAMES],
        r2: vec![0.0; MAX_FRAMES],
    };
    (Handoff { new_tx, old_rx }, core)
}

impl AudioCore {
    /// Process interleaved stereo `[L, R, L, R, ...]` in place. Real-time safe.
    pub fn process_interleaved(&mut self, data: &mut [f32]) {
        for block in data.chunks_mut(MAX_FRAMES * 2) {
            self.process_block(block);
        }
    }

    fn process_block(&mut self, data: &mut [f32]) {
        if let Some(old) = self.retired.take() {
            if let Err(rtrb::PushError::Full(old)) = self.old_tx.push(old) {
                self.retired = Some(old);
            }
        }
        if self.incoming.is_none() {
            if let Ok(p) = self.new_rx.pop() {
                self.incoming = Some(p);
                self.pos = 0;
            }
        }

        let n = data.len() / 2;
        let (l, r) = (&mut self.l[..n], &mut self.r[..n]);
        for (i, f) in data.as_chunks::<2>().0.iter().enumerate() {
            l[i] = f[0];
            r[i] = f[1];
        }

        let Some(next) = self.incoming.as_mut() else {
            self.current.process(l, r);
            interleave(data, l, r);
            return;
        };

        let (l2, r2) = (&mut self.l2[..n], &mut self.r2[..n]);
        l2.copy_from_slice(l);
        r2.copy_from_slice(r);
        self.current.process(l, r);
        next.process(l2, r2);
        for i in 0..n {
            let t = self.pos + i;
            let w = if t < WARMUP {
                0.0
            } else if t < WARMUP + XFADE {
                (t - WARMUP) as f32 / XFADE as f32
            } else {
                1.0
            };
            l[i] += (l2[i] - l[i]) * w;
            r[i] += (r2[i] - r[i]) * w;
        }
        interleave(data, l, r);
        self.pos += n;

        if self.pos >= WARMUP + XFADE {
            let next = self.incoming.take().expect("checked above");
            let old = std::mem::replace(&mut self.current, next);
            if let Err(rtrb::PushError::Full(old)) = self.old_tx.push(old) {
                self.retired = Some(old);
            }
        }
    }
}

fn interleave(data: &mut [f32], l: &[f32], r: &[f32]) {
    for (i, f) in data.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        f[0] = l[i];
        f[1] = r[i];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stepwave_core::mask::UnityMask;
    use stepwave_core::stft::SAMPLE_RATE;
    use stepwave_core::Profile;

    const CS2: &str = include_str!("../../profiles/cs2.json");

    fn unity() -> Processor {
        Processor::with_mask(Box::new(UnityMask), SAMPLE_RATE).unwrap()
    }

    fn eq() -> Processor {
        Processor::new(&Profile::from_json(CS2).unwrap(), SAMPLE_RATE).unwrap()
    }

    fn sine(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let v = 0.3 * (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / 48_000.0).sin();
                [v, v]
            })
            .collect()
    }

    fn run(core: &mut AudioCore, input: &[f32], block: usize) -> Vec<f32> {
        let mut out = input.to_vec();
        for chunk in out.chunks_mut(block * 2) {
            core.process_interleaved(chunk);
        }
        out
    }

    #[test]
    fn without_swaps_it_is_the_processor() {
        let input = sine(4800);
        let (_h, mut core) = channel(eq());
        let got = run(&mut core, &input, 256);
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = input
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| (f[0], f[1]))
            .unzip();
        eq().process(&mut l, &mut r);
        let want: Vec<f32> = l.iter().zip(&r).flat_map(|(&a, &b)| [a, b]).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn swap_crossfades_without_jumps_and_ends_on_the_new_processor() {
        let frames = 9600;
        let input = sine(frames);
        let (mut h, mut core) = channel(unity());
        let before = run(&mut core, &input[..4800 * 2], 256);
        assert!(h.publish(Box::new(eq())).is_ok());
        let after = run(&mut core, &input[4800 * 2..], 256);
        let out: Vec<f32> = before.into_iter().chain(after).collect();

        // Natural max step of this sine is 0.3 * 2π * 1000 / 48000 ≈ 0.039.
        let max_step = out
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| f[0])
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.05, "max step {max_step}");

        // Well after the transition the output matches the new processor alone.
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = input
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| (f[0], f[1]))
            .unzip();
        let mut solo = eq();
        solo.process(&mut l, &mut r);
        let tail = 4800 + WARMUP + XFADE + 960;
        for i in tail..frames {
            assert!((out[i * 2] - l[i]).abs() < 1e-3, "frame {i}");
        }
        // The retired processor comes back for dropping off the audio thread.
        assert_eq!(h.collect(), 1);
    }

    #[test]
    fn publish_refuses_when_two_are_queued() {
        let (mut h, _core) = channel(unity());
        assert!(h.publish(Box::new(unity())).is_ok());
        assert!(h.publish(Box::new(unity())).is_ok());
        assert!(h.publish(Box::new(unity())).is_err());
    }

    #[test]
    fn huge_blocks_are_split() {
        let input = sine(MAX_FRAMES * 2 + 100);
        let (_h, mut core) = channel(unity());
        let out = run(&mut core, &input, MAX_FRAMES * 2 + 100);
        assert_eq!(out.len(), input.len());
        assert!(out.iter().all(|v| v.is_finite()));
    }
}
