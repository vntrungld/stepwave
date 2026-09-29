//! `stepwave run` on Windows: wires the control pipe, the engine, the capture/render threads
//! and the main loop (engine housekeeping, default-device change detection).

use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use stepwave_host::audio;
use stepwave_host::engine::{initial_processor, Engine};
use stepwave_host::protocol::{IoStatus, ModeArg, Request, Response};

use crate::drift::{self, DriftStats};
use crate::pipe;
use crate::wasapi_io::{self, Shared};

/// Profile used until the user pins another one.
pub const DEFAULT_PROFILE: &str = "cs2";

pub struct Options {
    pub profiles_dir: PathBuf,
    pub mode: ModeArg,
    pub pipe: String,
}

fn io_status(shared: &Shared, drift: &DriftStats) -> IoStatus {
    let name = |m: &Mutex<Option<String>>| m.lock().ok().and_then(|g| g.clone());
    IoStatus {
        capture_device: name(&shared.capture_name),
        render_device: name(&shared.render_name),
        ring_fill_frames: drift.fill_frames(),
        drift_ppm: drift.ppm(),
        resyncs: drift.resyncs(),
    }
}

/// Handle one control request against the shared engine and fill in the Windows fields.
fn handle(engine: &Mutex<Engine>, shared: &Shared, drift: &DriftStats, req: Request) -> Response {
    let Ok(mut engine) = engine.lock() else {
        return Response::err("internal error: engine lock poisoned");
    };
    // `profile --auto` has no game detection to fall back on in M6a: keep the current profile
    // and only clear the pin.
    let keep = matches!(req, Request::Profile(None)).then(|| engine.status(0, &[]).profile);
    let rate = shared.capture_rate.load(Relaxed);
    let mut response = engine.handle(req, rate, &[]);
    if let Some(Some(id)) = keep {
        engine.select_profile(&id);
        response = Response::ok(engine.status(rate, &[]));
    }
    if let Some(status) = response.status.as_mut() {
        status.io = Some(io_status(shared, drift));
        if let Some(err) = shared.capture_error.lock().ok().and_then(|g| g.clone()) {
            status.fallback_reason = Some(match status.fallback_reason.take() {
                Some(r) => format!("{err}; {r}"),
                None => err,
            });
        }
    }
    response
}

pub fn run(opts: Options) -> Result<()> {
    let listener =
        pipe::bind(&opts.pipe).with_context(|| format!("opening control pipe '{}'", opts.pipe))?;

    let (handoff, audio_core) = audio::channel(initial_processor());
    let (mut engine, warnings) = Engine::new(&opts.profiles_dir, opts.mode, handoff);
    for w in warnings {
        eprintln!("stepwave: {w}");
    }
    if let Some(id) = engine.default_profile(DEFAULT_PROFILE) {
        engine.select_profile(&id);
    } else {
        eprintln!(
            "stepwave: no profiles in {}; running in bypass",
            opts.profiles_dir.display()
        );
    }
    let engine = Arc::new(Mutex::new(engine));

    let (capture_ring, render_ring, drift_stats) = drift::channel();
    let shared = Arc::new(Shared::default());

    let (e, s, d) = (engine.clone(), shared.clone(), drift_stats.clone());
    pipe::serve(listener, move |req| handle(&e, &s, &d, req));

    let s = shared.clone();
    let capture = std::thread::Builder::new()
        .name("stepwave-capture".into())
        .spawn(move || wasapi_io::capture_thread(s, audio_core, capture_ring))?;
    let s = shared.clone();
    let render = std::thread::Builder::new()
        .name("stepwave-render".into())
        .spawn(move || wasapi_io::render_thread(s, render_ring))?;

    let _ = wasapi::initialize_mta().ok();
    let mut default_id = wasapi_io::default_render_id();
    let mut last_device_check = Instant::now();
    eprintln!("stepwave: running; pipe '{}'", opts.pipe);
    while !capture.is_finished() && !render.is_finished() {
        std::thread::sleep(Duration::from_millis(250));
        if let Ok(mut e) = engine.lock() {
            e.tick();
        }
        if last_device_check.elapsed() >= Duration::from_secs(1) {
            last_device_check = Instant::now();
            let id = wasapi_io::default_render_id();
            if id != default_id {
                default_id = id;
                shared.render_generation.fetch_add(1, Relaxed);
            }
        }
    }
    shared.stop.store(true, Relaxed);
    anyhow::bail!("an audio thread exited unexpectedly")
}
