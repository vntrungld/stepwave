//! The PipeWire audio node: a capture stream that appears as the `stepwave` sink, and a
//! playback stream that follows the default output device. The capture callback runs
//! `AudioCore`; a lock-free ring carries the result to the playback callback. Both
//! callbacks run on PipeWire's real-time data thread (`RT_PROCESS`).

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::Arc;

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::pod::{Pod, Value};
use pw::stream::{StreamFlags, StreamListener, StreamRc};

use crate::audio::{AudioCore, MAX_FRAMES};

pub const SINK_NAME: &str = "stepwave";
const PLAYBACK_NAME: &str = "stepwave-output";
const CHANNELS: usize = 2;
const RATE: u32 = 48_000;
/// Ring between the two streams: 200 ms of stereo audio.
const RING_SAMPLES: usize = RATE as usize / 5 * CHANNELS;
/// Algorithmic latency reported to PipeWire, in samples.
const LATENCY_SAMPLES: i32 = 960;
/// Backlog the playback side lets build up before it starts discarding whole
/// frames (10 ms at 48 kHz), on top of whatever the current callback requests.
const TARGET_FRAMES: usize = 480;

/// Keeps both streams and their listeners alive; drop it to remove the sink.
///
/// Field order is drop order: the listeners must unhook before the streams
/// they listen on are destroyed. `pw_stream_destroy` frees memory a still-
/// registered listener's `spa_hook` list node points into; dropping the
/// stream fields first is a use-after-free (confirmed under valgrind, and it
/// fires on every reconnect and on any `connect` that fails after this
/// struct starts being built).
pub struct Node {
    _capture_listener: StreamListener<CaptureData>,
    _playback_listener: StreamListener<rtrb::Consumer<f32>>,
    _capture: StreamRc,
    _playback: StreamRc,
    /// Negotiated capture rate (0 until negotiated).
    pub rate: Arc<AtomicU32>,
}

pub struct CaptureData {
    core: AudioCore,
    ring: rtrb::Producer<f32>,
    rate: Arc<AtomicU32>,
    scratch: Vec<f32>,
}

/// How many whole frames the playback side should discard from `ring` before
/// filling this callback's output, so the backlog never exceeds
/// `TARGET_FRAMES + requested_frames`. Pure and allocation-free so it can be
/// unit-tested without a real ring; never returns more than fits in whole
/// frames, so it can never split one.
fn frames_to_discard(available_samples: usize, requested_frames: usize) -> usize {
    let available_frames = available_samples / CHANNELS;
    let target = TARGET_FRAMES + requested_frames;
    available_frames.saturating_sub(target)
}

/// Pop one stereo frame, keeping L/R aligned. Real-time safe (no allocation).
fn pop_frame(ring: &mut rtrb::Consumer<f32>) -> [f32; 2] {
    if ring.slots() >= 2 {
        let l = ring.pop().unwrap_or(0.0);
        let r = ring.pop().unwrap_or(0.0);
        [l, r]
    } else {
        // Underrun, or a lone sample stranded by a race with the capture side
        // (which only ever pushes whole frames): drop it so the ring stays
        // frame-aligned, and output silence for this frame.
        let _ = ring.pop();
        [0.0, 0.0]
    }
}

fn serialize(value: Value) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &value)
        .expect("static pod serialises")
        .0
        .into_inner()
}

fn format_pod() -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(CHANNELS as u32);
    let mut pos = [0u32; spa::param::audio::MAX_CHANNELS];
    pos[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    pos[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(pos);
    serialize(Value::Object(spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    }))
}

fn latency_pod() -> Vec<u8> {
    serialize(Value::Object(spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamProcessLatency.as_raw(),
        id: spa::param::ParamType::ProcessLatency.as_raw(),
        properties: vec![spa::pod::Property {
            key: spa::sys::SPA_PARAM_PROCESS_LATENCY_rate,
            flags: spa::pod::PropertyFlags::empty(),
            value: Value::Int(LATENCY_SAMPLES),
        }],
    }))
}

/// Create the sink and its output. `audio` is moved onto the data thread.
pub fn create(core: &pw::core::CoreRc, audio: AudioCore) -> Result<Node, pw::Error> {
    let rate = Arc::new(AtomicU32::new(0));
    let (ring_tx, ring_rx) = rtrb::RingBuffer::<f32>::new(RING_SAMPLES);

    let capture = StreamRc::new(
        core.clone(),
        SINK_NAME,
        properties! {
            *pw::keys::MEDIA_CLASS => "Audio/Sink",
            *pw::keys::NODE_NAME => SINK_NAME,
            *pw::keys::NODE_DESCRIPTION => "stepwave (footstep enhancer)",
            "audio.position" => "[ FL FR ]",
            "node.latency" => "480/48000",
            // Same group as the playback stream: they share one driver, like
            // module-loopback/filter-chain do, instead of free-running clocks.
            "node.group" => "stepwave",
        },
    )?;
    let capture_listener = capture
        .add_local_listener_with_user_data(CaptureData {
            core: audio,
            ring: ring_tx,
            rate: rate.clone(),
            scratch: vec![0.0; MAX_FRAMES * CHANNELS],
        })
        .param_changed(|_, data, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = spa::param::audio::AudioInfoRaw::new();
            if info.parse(param).is_ok() {
                data.rate.store(info.rate(), Relaxed);
            }
        })
        .process(|stream, data| {
            let Some(mut buf) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buf.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let offset = d.chunk().offset() as usize;
            let size = d.chunk().size() as usize;
            let Some(bytes) = d.data() else { return };
            let Some(bytes) = bytes.get(offset..offset + size) else {
                return;
            };
            // Defensive only: the stream format is fixed at 48 kHz below (PipeWire's
            // adapter converts from any other graph rate at the stream boundary), so
            // once negotiated this should always be true.
            let processing = data.rate.load(Relaxed) == RATE;
            for frame_bytes in bytes.chunks(data.scratch.len() * 4) {
                let n = frame_bytes.len() / 4;
                let block = &mut data.scratch[..n];
                for (v, b) in block.iter_mut().zip(frame_bytes.as_chunks::<4>().0) {
                    *v = f32::from_le_bytes(*b);
                }
                if processing {
                    data.core.process_interleaved(block);
                }
                // Move whole stereo frames only: push a frame just when both its
                // samples fit, else drop it whole. Never push one channel of a
                // frame without the other, or the playback side desynchronises L/R.
                for frame in block.as_chunks::<2>().0 {
                    if data.ring.slots() >= 2 {
                        let _ = data.ring.push(frame[0]);
                        let _ = data.ring.push(frame[1]);
                    }
                }
            }
        })
        .register()?;

    let playback = StreamRc::new(
        core.clone(),
        PLAYBACK_NAME,
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Game",
            *pw::keys::NODE_NAME => PLAYBACK_NAME,
            *pw::keys::NODE_DESCRIPTION => "stepwave output",
            "audio.position" => "[ FL FR ]",
            "node.latency" => "480/48000",
            // Same group as the capture stream: see the comment there.
            "node.group" => "stepwave",
        },
    )?;
    let playback_listener = playback
        .add_local_listener_with_user_data(ring_rx)
        .process(|stream, ring| {
            let Some(mut buf) = stream.dequeue_buffer() else {
                return;
            };
            let requested = buf.requested() as usize;
            let datas = buf.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let mut frames = 0;
            if let Some(bytes) = d.data() {
                let max = bytes.len() / (4 * CHANNELS);
                frames = if requested == 0 {
                    max
                } else {
                    requested.min(max)
                };
                // Bound the delay: if capture has been filling faster than this
                // stream drains, drop whole frames of backlog before filling, so
                // latency cannot grow without bound.
                for _ in 0..frames_to_discard(ring.slots(), frames) {
                    pop_frame(ring);
                }
                // Pop whole stereo frames and write both channels together, so
                // L/R can never end up misaligned by one channel's underrun.
                let mut samples = bytes[..frames * CHANNELS * 4]
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut();
                for _ in 0..frames {
                    let [l, r] = pop_frame(ring);
                    if let Some(b) = samples.next() {
                        b.copy_from_slice(&l.to_le_bytes());
                    }
                    if let Some(b) = samples.next() {
                        b.copy_from_slice(&r.to_le_bytes());
                    }
                }
            }
            let chunk = d.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = (4 * CHANNELS) as i32;
            *chunk.size_mut() = (frames * CHANNELS * 4) as u32;
        })
        .register()?;

    let flags = StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS;
    let (fmt_in, fmt_out, lat) = (format_pod(), format_pod(), latency_pod());
    capture.connect(
        spa::utils::Direction::Input,
        None,
        flags,
        &mut [Pod::from_bytes(&fmt_in).expect("valid pod")],
    )?;
    capture.update_params(&mut [Pod::from_bytes(&lat).expect("valid pod")])?;
    playback.connect(
        spa::utils::Direction::Output,
        None,
        flags,
        &mut [Pod::from_bytes(&fmt_out).expect("valid pod")],
    )?;

    Ok(Node {
        _capture_listener: capture_listener,
        _playback_listener: playback_listener,
        _capture: capture,
        _playback: playback,
        rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_backlog_discards_nothing() {
        assert_eq!(frames_to_discard(0, 128), 0);
        // Exactly at target: still nothing to discard.
        assert_eq!(frames_to_discard((TARGET_FRAMES + 128) * CHANNELS, 128), 0);
    }

    #[test]
    fn big_backlog_trims_down_to_target_plus_requested() {
        let requested = 128;
        let available_frames = TARGET_FRAMES + 1000;
        let discard = frames_to_discard(available_frames * CHANNELS, requested);
        assert_eq!(available_frames - discard, TARGET_FRAMES + requested);
    }

    #[test]
    fn odd_sample_count_never_splits_a_frame() {
        // One stray sample beyond a whole number of frames (should not happen
        // in practice, since the capture side only ever pushes pairs, but the
        // function must still never claim more samples than are available).
        let available_samples = (TARGET_FRAMES + 1000) * CHANNELS + 1;
        let discard = frames_to_discard(available_samples, 128);
        assert!(discard * CHANNELS <= available_samples);
    }
}
