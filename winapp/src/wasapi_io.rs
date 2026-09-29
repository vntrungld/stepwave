//! WASAPI capture (VB-Cable's "CABLE Output") and render (the default output device) loops.
//! Windows only. Each loop runs on its own thread registered with MMCSS "Pro Audio", opens
//! its endpoint in shared, event-driven mode as 48 kHz float stereo (the Windows audio engine
//! converts from the device's own format), and reopens with a backoff when the device goes
//! away. The per-packet paths reuse preallocated buffers: no allocation, locks or logging.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use stepwave_host::audio::AudioCore;
use wasapi::{BufferFlags, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

use crate::backoff;
use crate::devices;
use crate::drift;

/// Friendly-name fragment that identifies VB-Cable's capture endpoint.
pub const CABLE_CAPTURE: &str = "CABLE Output";
const RATE: usize = 48_000;
const CHANNELS: usize = 2;
const BYTES_PER_FRAME: usize = 4 * CHANNELS;

/// Device state shared with the control side (status reporting and device-change signals).
#[derive(Default)]
pub struct Shared {
    pub stop: AtomicBool,
    /// Set by the capture loop: the open capture endpoint's name.
    pub capture_name: Mutex<Option<String>>,
    /// Set by the render loop: the open render endpoint's name.
    pub render_name: Mutex<Option<String>>,
    /// Why the capture side is not running (e.g. VB-Cable missing), for `status`.
    pub capture_error: Mutex<Option<String>>,
    /// Why the render side is not running (e.g. the default output is CABLE Input), for
    /// `status`.
    pub render_error: Mutex<Option<String>>,
    /// Negotiated capture rate, 0 while closed.
    pub capture_rate: AtomicU32,
    /// Bumped by the main thread when the default render device changes.
    pub render_generation: AtomicU64,
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn format() -> WaveFormat {
    WaveFormat::new(32, 32, &SampleType::Float, RATE, CHANNELS, None)
}

/// Register the calling thread with MMCSS "Pro Audio" (best effort).
fn boost_thread() {
    use windows::core::w;
    use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
    let mut index = 0u32;
    // SAFETY: valid, NUL-terminated task name and a valid out-pointer.
    let _ = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut index) };
}

fn set(slot: &Mutex<Option<String>>, value: Option<String>) {
    if let Ok(mut g) = slot.lock() {
        *g = value;
    }
}

/// Convert a WASAPI device period (100 ns units) to whole frames at `RATE`, at least 1.
fn period_frames(hns: i64) -> u32 {
    ((hns * RATE as i64) / 10_000_000).max(1) as u32
}

fn find_capture(enumerator: &DeviceEnumerator) -> Res<Option<(wasapi::Device, String)>> {
    let devices = enumerator.get_device_collection(&Direction::Capture)?;
    for i in 0..devices.get_nbr_devices()? {
        let device = devices.get_device_at_index(i)?;
        let name = device.get_friendlyname()?;
        if name.contains(CABLE_CAPTURE) {
            return Ok(Some((device, name)));
        }
    }
    Ok(None)
}

/// Run the capture side until `shared.stop`: VB-Cable → `AudioCore` → drift ring.
pub fn capture_thread(shared: Arc<Shared>, mut core: AudioCore, mut ring: drift::Capture) {
    let _ = wasapi::initialize_mta().ok();
    boost_thread();
    let mut backoff_dur = backoff::INITIAL;
    while !shared.stop.load(Relaxed) {
        let mut started = false;
        match capture_session(&shared, &mut core, &mut ring, &mut started) {
            Ok(()) => backoff_dur = backoff::INITIAL,
            Err(e) => {
                set(&shared.capture_error, Some(e.to_string()));
                // A session that actually started ran until a mid-stream fault, not because the
                // device was never found; back off only briefly and forget any backoff grown
                // while the device was missing earlier (see `backoff`'s module doc comment).
                let (sleep, next) = backoff::next(backoff_dur, started);
                std::thread::sleep(sleep);
                backoff_dur = next;
            }
        }
        shared.capture_rate.store(0, Relaxed);
        set(&shared.capture_name, None);
    }
}

fn capture_session(
    shared: &Shared,
    core: &mut AudioCore,
    ring: &mut drift::Capture,
    started: &mut bool,
) -> Res<()> {
    let enumerator = DeviceEnumerator::new()?;
    let Some((device, name)) = find_capture(&enumerator)? else {
        return Err(format!("{CABLE_CAPTURE} not found — install VB-Cable").into());
    };
    let mut client = device.get_iaudioclient()?;
    let (default_period, _min) = client.get_device_period()?;
    // Declare the device period to the drift compensator before the stream starts, so the ring
    // target is derived from it rather than from whatever size WASAPI's first request happens
    // to be (see `drift`'s module doc comment).
    ring.set_period(period_frames(default_period));
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: default_period,
    };
    client.initialize_client(&format(), &Direction::Capture, &mode)?;
    let event = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    let frames = client.get_buffer_size()? as usize;
    let mut bytes = vec![0u8; frames * BYTES_PER_FRAME];
    let mut samples = vec![0f32; frames * CHANNELS];
    client.start_stream()?;
    // Set once per session (not per packet): the RT loop below never touches this flag.
    *started = true;
    set(&shared.capture_name, Some(name));
    set(&shared.capture_error, None);
    shared.capture_rate.store(RATE as u32, Relaxed);

    while !shared.stop.load(Relaxed) {
        if event.wait_for_event(2000).is_err() {
            // No packets for 2 s: VB-Cable delivers silence while idle, so this means trouble.
            return Err("capture stalled".into());
        }
        loop {
            let packet = capture.get_next_packet_size()?.unwrap_or(0) as usize;
            if packet == 0 {
                break;
            }
            let packet = packet.min(frames);
            let (read, info) = capture.read_from_device(&mut bytes[..packet * BYTES_PER_FRAME])?;
            let n = read as usize * CHANNELS;
            if info.flags.silent {
                samples[..n].fill(0.0);
            } else {
                for (s, b) in samples[..n]
                    .iter_mut()
                    .zip(bytes[..n * 4].as_chunks::<4>().0)
                {
                    *s = f32::from_le_bytes(*b);
                }
            }
            core.process_interleaved(&mut samples[..n]);
            ring.push(&samples[..n]);
        }
    }
    client.stop_stream()?;
    Ok(())
}

/// Run the render side until `shared.stop`: drift ring → default output device. Reopens when
/// the default device changes (`render_generation`) or the device fails.
pub fn render_thread(shared: Arc<Shared>, mut ring: drift::Render) {
    let _ = wasapi::initialize_mta().ok();
    boost_thread();
    while !shared.stop.load(Relaxed) {
        let generation = shared.render_generation.load(Relaxed);
        let result = render_session(&shared, &mut ring, generation);
        set(&shared.render_name, None);
        ring.reset();
        if let Err(e) = result {
            set(&shared.render_error, Some(e.to_string()));
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn render_session(shared: &Shared, ring: &mut drift::Render, generation: u64) -> Res<()> {
    let enumerator = DeviceEnumerator::new()?;
    let device = enumerator.get_default_device(&Direction::Render)?;
    let name = device.get_friendlyname()?;
    if devices::is_virtual_cable(&name) {
        // Rendering into CABLE Input would feed our own output back into the capture side.
        // Retry every second: a new default device bumps `render_generation` and is picked up.
        return Err(
            "default output is CABLE Input — set your headset/speakers as the Default Device"
                .into(),
        );
    }
    let mut client = device.get_iaudioclient()?;
    let (default_period, _min) = client.get_device_period()?;
    // See `capture_session`: declare the period before `start_stream`.
    ring.set_period(period_frames(default_period));
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: default_period,
    };
    client.initialize_client(&format(), &Direction::Render, &mode)?;
    let event = client.set_get_eventhandle()?;
    let render = client.get_audiorenderclient()?;
    let frames = client.get_buffer_size()? as usize;
    let mut samples = vec![0f32; frames * CHANNELS];
    let mut bytes = vec![0u8; frames * BYTES_PER_FRAME];
    // Pre-fill the endpoint buffer with silence before starting, so the first event asks for
    // about one period instead of the whole buffer; together with the trim when priming
    // completes (see `drift`) this keeps the ring at its target after a (re)open.
    let prefill = (client.get_available_space_in_frames()? as usize).min(frames);
    render.write_to_device(
        prefill,
        &bytes[..prefill * BYTES_PER_FRAME],
        Some(BufferFlags {
            silent: true,
            ..BufferFlags::none()
        }),
    )?;
    client.start_stream()?;
    set(&shared.render_name, Some(name));
    set(&shared.render_error, None);

    while !shared.stop.load(Relaxed) && shared.render_generation.load(Relaxed) == generation {
        if event.wait_for_event(1000).is_err() {
            return Err("render stalled".into());
        }
        let space = (client.get_available_space_in_frames()? as usize).min(frames);
        if space == 0 {
            continue;
        }
        let n = space * CHANNELS;
        ring.render(&mut samples[..n]);
        for (b, s) in bytes[..n * 4]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(&samples[..n])
        {
            *b = s.to_le_bytes();
        }
        render.write_to_device(space, &bytes[..n * 4], None)?;
    }
    client.stop_stream()?;
    Ok(())
}

/// Id of the current default render device, for change detection on the main thread.
pub fn default_render_id() -> Option<String> {
    let enumerator = DeviceEnumerator::new().ok()?;
    let device = enumerator.get_default_device(&Direction::Render).ok()?;
    device.get_id().ok()
}
