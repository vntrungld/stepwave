//! Clock-drift compensation between the capture device (VB-Cable) and the render device.
//!
//! The capture side pushes whole stereo frames into a lock-free ring. The render side pulls
//! fixed chunks through `rubato::Slip`, which occasionally inserts or drops one frame (hidden
//! by a short crossfade) so that the long-run rates match. A PI controller steers the slip
//! ratio from a smoothed ring fill toward a target fill.
//!
//! **Target fill.** Capture arrives in device-period packets and render asks for device-period
//! buffers. Because the two clocks differ, their phases slide through each other; now and then
//! two render periods fall between two capture packets (or the reverse). To never underrun, the
//! ring must hold about two packets plus one chunk: `2 · max(capture_period, render_period) +
//! CHUNK`, at least `MIN_TARGET`. `wasapi_io` declares both periods once per session, in frames,
//! through `Capture::set_period` / `Render::set_period`; once both are known they alone drive
//! the target. Until then — in tests, and before the first declared period arrives — the target
//! falls back to the largest packet or request actually observed. The declared periods, once
//! known, must win outright rather than blend with observed sizes: WASAPI's first render
//! request after `start_stream` is the whole endpoint buffer, and any late wakeup asks for
//! about two periods, so folding those observed sizes into the target would double it and leave
//! it doubled for the rest of the session. With 10 ms shared-mode periods the target is ≈ 21 ms;
//! with 3 ms periods ≈ 7 ms.
//!
//! **Priming and resync.** The render side outputs silence until the ring reaches the target,
//! so playback starts at the target latency. If the ring runs dry (a capture stall) it primes
//! again; if it overfills past `RESYNC_FACTOR · target` (a render stall) it drops the excess.
//! Both count in `resyncs`. The learned clock ratio (the integral term) survives a resync.
//!
//! Real-time safe: `Capture::push` and `Render::render` never allocate, lock or panic.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Adjustable, FixedAsync, Resampler, Slip};

pub const CHANNELS: usize = 2;
/// Render-side processing chunk, in output frames.
pub const CHUNK: usize = 64;
/// Lower bound for the target fill, in frames.
pub const MIN_TARGET: usize = 256;
/// The ring is resynced when it holds more than this many targets.
pub const RESYNC_FACTOR: usize = 4;
/// Largest correction the controller may request, in ppm.
pub const MAX_PPM: f64 = 1000.0;
/// Ring capacity, in frames (1 s).
pub const RING_FRAMES: usize = 48_000;

// Loop design. A correction of `p` ppm changes the fill by `0.048 · p` frames per second, so
// the plant is an integrator with gain 0.048. KP = 2 gives a ~10 s time constant (slow enough
// that the packet sawtooth left after smoothing moves the ratio by only a few ppm); KI removes
// the steady-state offset a constant clock difference would otherwise leave (≈ 30 s).
/// Proportional gain: ppm per frame of fill error.
const KP: f64 = 2.0;
/// Integral gain: ppm per (frame · second) of fill error.
const KI: f64 = 0.07;
/// Per-chunk smoothing of the fill measurement: a ~0.5 s time constant (64-frame chunks)
/// averages out the sawtooth of packet-sized capture pushes.
const FILL_ALPHA: f64 = 0.0027;

/// Shared, lock-free view of the drift state for `status`.
#[derive(Default)]
pub struct DriftStats {
    fill_frames: AtomicU32,
    target_frames: AtomicU32,
    max_packet: AtomicU32,
    /// Declared capture device period, in frames. 0 = not yet declared.
    capture_period: AtomicU32,
    /// Declared render device period, in frames. 0 = not yet declared.
    render_period: AtomicU32,
    ppm_bits: AtomicU64,
    resyncs: AtomicU64,
}

impl DriftStats {
    /// Frames in the ring at the last render chunk.
    pub fn fill_frames(&self) -> u32 {
        self.fill_frames.load(Relaxed)
    }
    /// Current target fill, in frames.
    pub fn target_frames(&self) -> u32 {
        self.target_frames.load(Relaxed)
    }
    /// Current correction, in ppm (negative: the render side consumes less input).
    pub fn ppm(&self) -> f64 {
        f64::from_bits(self.ppm_bits.load(Relaxed))
    }
    /// Times the ring was primed again or trimmed after leaving its safe range.
    pub fn resyncs(&self) -> u64 {
        self.resyncs.load(Relaxed)
    }
}

fn target_for(max_packet: usize) -> usize {
    (2 * max_packet + CHUNK).max(MIN_TARGET)
}

/// Capture end: push interleaved stereo frames.
pub struct Capture {
    tx: rtrb::Producer<f32>,
    stats: Arc<DriftStats>,
}

impl Capture {
    /// Push whole frames; frames that do not fit are dropped whole. Returns frames pushed.
    pub fn push(&mut self, interleaved: &[f32]) -> usize {
        let frames = (interleaved.len() / CHANNELS) as u32;
        self.stats.max_packet.fetch_max(frames, Relaxed);
        let mut pushed = 0;
        for frame in interleaved.as_chunks::<CHANNELS>().0 {
            if self.tx.slots() < CHANNELS {
                break;
            }
            for &v in frame {
                let _ = self.tx.push(v);
            }
            pushed += 1;
        }
        pushed
    }

    /// Declare the capture device's period, in frames, for the render-target calculation.
    /// `wasapi_io` calls this once per session, from the WASAPI buffer/period size. Once both
    /// this and `Render::set_period` have been called, the declared periods alone drive the
    /// ring target — see the module doc comment.
    pub fn set_period(&mut self, frames: u32) {
        self.stats.capture_period.store(frames, Relaxed);
    }
}

/// Render end: fill output buffers at the render device's pace.
pub struct Render {
    rx: rtrb::Consumer<f32>,
    slip: Slip<f32>,
    input: Vec<f32>,
    chunk: Vec<f32>,
    chunk_pos: usize,
    target: usize,
    fill_smooth: f64,
    integral: f64,
    priming: bool,
    stats: Arc<DriftStats>,
}

/// Create both ends and the shared stats.
pub fn channel() -> (Capture, Render, Arc<DriftStats>) {
    let (tx, rx) = rtrb::RingBuffer::new(RING_FRAMES * CHANNELS);
    let slip = Slip::<f32>::new(CHUNK, CHANNELS, FixedAsync::Output).expect("valid Slip config");
    let input = vec![0.0; slip.input_frames_max() * CHANNELS];
    let stats = Arc::new(DriftStats::default());
    stats.target_frames.store(MIN_TARGET as u32, Relaxed);
    let render = Render {
        rx,
        slip,
        input,
        chunk: vec![0.0; CHUNK * CHANNELS],
        chunk_pos: CHUNK * CHANNELS, // empty: produce on first call
        target: MIN_TARGET,
        fill_smooth: MIN_TARGET as f64,
        integral: 0.0,
        priming: true,
        stats: stats.clone(),
    };
    let capture = Capture {
        tx,
        stats: stats.clone(),
    };
    (capture, render, stats)
}

impl Render {
    /// Declare the render device's period, in frames; see `Capture::set_period`.
    pub fn set_period(&mut self, frames: u32) {
        self.stats.render_period.store(frames, Relaxed);
    }

    /// Fill `out` (interleaved stereo) completely. Real-time safe.
    pub fn render(&mut self, out: &mut [f32]) {
        let cap_period = self.stats.capture_period.load(Relaxed);
        let ren_period = self.stats.render_period.load(Relaxed);
        let target = if cap_period > 0 && ren_period > 0 {
            // Both periods declared: they alone drive the target. Observed request sizes
            // (e.g. the oversized first WASAPI buffer) must not be folded in — see the module
            // doc comment.
            target_for(cap_period.max(ren_period) as usize)
        } else {
            let request = (out.len() / CHANNELS) as u32;
            let max_packet = self
                .stats
                .max_packet
                .fetch_max(request, Relaxed)
                .max(request);
            target_for(max_packet as usize)
        };
        if target != self.target {
            self.target = target;
            self.stats.target_frames.store(target as u32, Relaxed);
        }
        let mut written = 0;
        while written < out.len() {
            if self.chunk_pos == self.chunk.len() {
                self.produce_chunk();
            }
            let n = (out.len() - written).min(self.chunk.len() - self.chunk_pos);
            out[written..written + n]
                .copy_from_slice(&self.chunk[self.chunk_pos..self.chunk_pos + n]);
            written += n;
            self.chunk_pos += n;
        }
    }

    fn fill(&self) -> usize {
        self.rx.slots() / CHANNELS
    }

    /// Drop `frames` whole frames from the ring in O(1). Callers only ever pass `frames <=
    /// fill()`, so the chunk is always available; allocation-free (`f32` has no destructor to
    /// run, so `commit_all` is just pointer arithmetic).
    fn discard(&mut self, frames: usize) {
        if let Ok(chunk) = self.rx.read_chunk(frames * CHANNELS) {
            chunk.commit_all();
        }
    }

    fn produce_chunk(&mut self) {
        self.chunk_pos = 0;
        let fill = self.fill();
        if fill > RESYNC_FACTOR * self.target {
            self.discard(fill - self.target);
            self.fill_smooth = self.target as f64;
            self.stats.resyncs.fetch_add(1, Relaxed);
        }
        let fill = self.fill();
        self.stats.fill_frames.store(fill as u32, Relaxed);

        if self.priming {
            if fill < self.target {
                self.chunk.fill(0.0);
                return;
            }
            self.priming = false;
            self.fill_smooth = fill as f64;
        }

        // PI control on the smoothed fill: too full → consume faster (ratio < 1).
        self.fill_smooth += (fill as f64 - self.fill_smooth) * FILL_ALPHA;
        let err = self.fill_smooth - self.target as f64;
        let dt = CHUNK as f64 / 48_000.0;
        let ppm_unclamped = KP * err + KI * (self.integral + err * dt);
        if ppm_unclamped.abs() < MAX_PPM {
            self.integral += err * dt; // anti-windup: only integrate while unsaturated
        }
        let ppm = ppm_unclamped.clamp(-MAX_PPM, MAX_PPM);
        self.stats.ppm_bits.store(ppm.to_bits(), Relaxed);
        let _ = self.slip.set_resample_ratio(1.0 - ppm * 1e-6, false);

        let need = self.slip.input_frames_next();
        if fill < need {
            // Ran dry (capture stalled): silence, and prime again before playing.
            self.chunk.fill(0.0);
            self.priming = true;
            self.stats.resyncs.fetch_add(1, Relaxed);
            return;
        }
        for v in self.input[..need * CHANNELS].iter_mut() {
            *v = self.rx.pop().unwrap_or(0.0);
        }
        let (Ok(inp), Ok(mut outp)) = (
            InterleavedSlice::new(&self.input[..need * CHANNELS], CHANNELS, need),
            InterleavedSlice::new_mut(&mut self.chunk[..], CHANNELS, CHUNK),
        ) else {
            self.chunk.fill(0.0);
            return;
        };
        if self
            .slip
            .process_into_buffer(&inp, &mut outp, None)
            .is_err()
        {
            self.chunk.fill(0.0);
        }
    }

    /// Start over after a device change: forget any observed packet/request sizes (so the
    /// observed-size fallback target starts over at `MIN_TARGET` rather than keeping a stale
    /// high-water mark), drop the backlog to that target, forget the learned clock ratio (the
    /// render clock changed) and prime again. Declared device periods (`set_period`) are left
    /// alone — the physical device period does not change across a reset.
    pub fn reset(&mut self) {
        self.stats.max_packet.store(0, Relaxed);
        self.target = MIN_TARGET;
        self.stats.target_frames.store(MIN_TARGET as u32, Relaxed);
        let fill = self.fill();
        if fill > self.target {
            self.discard(fill - self.target);
        }
        self.fill_smooth = self.target as f64;
        self.integral = 0.0;
        self.priming = true;
        self.chunk_pos = self.chunk.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    struct Sim {
        left: Vec<f32>,
        stats: Arc<DriftStats>,
        fills: Vec<usize>,
        ppms: Vec<f64>,
    }

    type Clock = (f64, usize); // (ppm offset, frames per period)
    type Window = Option<(f64, f64)>; // (start s, length s)

    /// Simulate `seconds` of audio with the given capture and render clocks. `ren_stall`
    /// freezes the render side and `cap_stall` the capture side for a window.
    fn simulate(seconds: f64, cap: Clock, ren: Clock, ren_stall: Window, cap_stall: Window) -> Sim {
        let (mut capture, mut render, stats) = channel();
        let cap_period = cap.1 as f64 / (RATE * (1.0 + cap.0 * 1e-6));
        let ren_period = ren.1 as f64 / (RATE * (1.0 + ren.0 * 1e-6));
        let (mut t_cap, mut t_ren) = (0.0f64, 0.0037f64);
        let mut phase = 0u64;
        let mut packet = vec![0.0f32; cap.1 * CHANNELS];
        let mut out = vec![0.0f32; ren.1 * CHANNELS];
        let (mut left, mut fills, mut ppms) = (Vec::new(), Vec::new(), Vec::new());
        let frozen = |w: Window, t: f64| w.is_some_and(|(s, l)| t >= s && t < s + l);
        while t_cap.min(t_ren) < seconds {
            if t_cap <= t_ren {
                if !frozen(cap_stall, t_cap) {
                    for f in packet.as_chunks_mut::<CHANNELS>().0 {
                        let x = 2.0 * std::f64::consts::PI * 1000.0 * phase as f64 / RATE;
                        *f = [(0.5 * x.sin()) as f32; CHANNELS];
                        phase += 1;
                    }
                    capture.push(&packet);
                }
                t_cap += cap_period;
            } else {
                if !frozen(ren_stall, t_ren) {
                    render.render(&mut out);
                    left.extend(out.as_chunks::<CHANNELS>().0.iter().map(|f| f[0]));
                    fills.push(render.fill());
                    ppms.push(stats.ppm());
                }
                t_ren += ren_period;
            }
        }
        Sim {
            left,
            stats,
            fills,
            ppms,
        }
    }

    fn max_step(x: &[f32]) -> f32 {
        x.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn tracks_drift_without_resyncs_or_clicks() {
        // 10 minutes each. The 1 kHz, 0.5-amplitude sine's natural max step is
        // 0.5·2π·1000/48000 ≈ 0.0654.
        let cases = [
            ((0.0, 480), (0.0, 480)),
            ((200.0, 480), (0.0, 480)),
            ((-200.0, 480), (0.0, 480)),
            ((0.0, 480), (200.0, 480)),
            ((0.0, 144), (-150.0, 441)),
        ];
        for (cap, ren) in cases {
            let sim = simulate(600.0, cap, ren, None, None);
            let tag = format!("cap {cap:?} ren {ren:?}");
            assert_eq!(sim.stats.resyncs(), 0, "{tag}");
            let settled = &sim.fills[sim.fills.len() / 60..]; // after the first 10 s
            assert!(settled.iter().all(|&f| f > 0), "{tag}: ran dry");
            let step = max_step(&sim.left[48_000..]);
            assert!(step < 0.08, "{tag}: step {step}");
            assert!(sim.stats.ppm().abs() < MAX_PPM, "{tag}: saturated");
        }
    }

    #[test]
    fn converges_on_the_clock_difference() {
        // Render clock 200 ppm faster than capture: once settled the render side consumes
        // ~200 ppm less input per output frame, i.e. ppm ≈ -200.
        // The instantaneous value wobbles with the packet phase, so average the last minute.
        let sim = simulate(240.0, (0.0, 480), (200.0, 480), None, None);
        let last_minute = &sim.ppms[sim.ppms.len() - 6000..];
        let mean = last_minute.iter().sum::<f64>() / last_minute.len() as f64;
        assert!((mean + 200.0).abs() < 20.0, "mean ppm {mean}");
    }

    #[test]
    fn target_adapts_to_packet_size() {
        let sim = simulate(2.0, (0.0, 480), (0.0, 480), None, None);
        assert_eq!(sim.stats.target_frames() as usize, 2 * 480 + CHUNK);
        let sim = simulate(2.0, (0.0, 96), (0.0, 96), None, None);
        assert_eq!(sim.stats.target_frames() as usize, MIN_TARGET);
    }

    #[test]
    fn a_render_stall_causes_one_resync() {
        let sim = simulate(20.0, (0.0, 480), (0.0, 480), Some((5.0, 0.5)), None);
        assert_eq!(sim.stats.resyncs(), 1);
    }

    #[test]
    fn a_capture_stall_primes_again_and_recovers() {
        let sim = simulate(20.0, (0.0, 480), (0.0, 480), None, Some((5.0, 0.3)));
        assert_eq!(sim.stats.resyncs(), 1);
        let tail = &sim.fills[sim.fills.len() - 100..];
        assert!(tail.iter().all(|&f| f > 0));
    }

    #[test]
    fn render_primes_to_the_target_before_playing() {
        let (mut cap, mut ren, stats) = channel();
        let mut out = vec![1.0f32; CHUNK * CHANNELS];
        // 100-frame packets and 64-frame requests: target = max(2·100 + 64, 256) = 264.
        for _ in 0..2 {
            cap.push(&vec![0.5f32; 100 * CHANNELS]);
        }
        cap.push(&vec![0.5f32; 63 * CHANNELS]);
        ren.render(&mut out);
        assert_eq!(stats.target_frames(), 264);
        assert!(
            out.iter().all(|&v| v == 0.0),
            "played before reaching the target"
        );
        cap.push(&[0.5f32; CHANNELS]);
        ren.render(&mut out);
        assert!(out.iter().all(|&v| v == 0.5));
        assert_eq!(stats.resyncs(), 0);
    }

    #[test]
    fn capture_drops_whole_frames_when_full() {
        let (mut cap, _ren, _s) = channel();
        let block = vec![0.25f32; 1000 * CHANNELS];
        let total: usize = (0..60).map(|_| cap.push(&block)).sum();
        assert_eq!(total, RING_FRAMES);
    }

    #[test]
    fn render_outputs_silence_until_audio_arrives() {
        let (_cap, mut ren, _s) = channel();
        let mut out = vec![1.0f32; 300 * CHANNELS];
        ren.render(&mut out);
        assert!(out.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn declared_periods_ignore_oversized_requests() {
        let (mut cap, mut ren, stats) = channel();
        cap.set_period(480);
        ren.set_period(480);
        let expected = 2 * 480 + CHUNK;

        // Prime past the target before the first (whole-endpoint-buffer) render request.
        cap.push(&vec![0.0f32; 480 * CHANNELS]);
        cap.push(&vec![0.0f32; 480 * CHANNELS]);
        cap.push(&vec![0.0f32; 480 * CHANNELS]);
        let mut first = vec![0.0f32; 1056 * CHANNELS];
        ren.render(&mut first);
        assert_eq!(stats.target_frames() as usize, expected);

        let mut out = vec![0.0f32; 480 * CHANNELS];
        let mut oversized = vec![0.0f32; 960 * CHANNELS];
        // Drive well over a second of audio (48 kHz / 480 frames per period).
        for i in 0..200u32 {
            cap.push(&vec![0.0f32; 480 * CHANNELS]);
            if i > 0 && i % 50 == 0 {
                // A late wakeup: capture also delivers the catch-up packet, so the ring
                // stays balanced; the render request is what must not move the target.
                cap.push(&vec![0.0f32; 480 * CHANNELS]);
                ren.render(&mut oversized);
            } else {
                ren.render(&mut out);
            }
            assert_eq!(stats.target_frames() as usize, expected, "iteration {i}");
        }
        assert_eq!(stats.resyncs(), 0);
    }

    #[test]
    fn reset_forgets_observed_sizes() {
        let (mut cap, mut ren, stats) = channel();
        let mut big_out = vec![0.0f32; 64 * CHANNELS];
        cap.push(&vec![0.0f32; 2000 * CHANNELS]);
        ren.render(&mut big_out);
        assert_eq!(stats.target_frames() as usize, 2 * 2000 + CHUNK);

        ren.reset();
        assert_eq!(stats.target_frames() as usize, MIN_TARGET);

        let mut out = vec![0.0f32; 64 * CHANNELS];
        for _ in 0..5 {
            cap.push(&vec![0.0f32; 100 * CHANNELS]);
            ren.render(&mut out);
        }
        assert_eq!(
            stats.target_frames() as usize,
            (2 * 100 + CHUNK).max(MIN_TARGET)
        );
    }
}
