//! `stepwave daemon`: wires the control socket, the engine, the audio node and the
//! routing glue onto one PipeWire main loop, and reconnects if PipeWire goes away.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use pipewire as pw;

use crate::audio;
use crate::control;
use crate::engine::{initial_processor, Engine};
use crate::graph::{self, Graph};
use crate::node::{self, Node};
use crate::protocol::{ModeArg, Request, Response};
use crate::router::Router;

pub struct Options {
    pub profiles_dir: PathBuf,
    pub mode: ModeArg,
    pub socket: PathBuf,
}

/// One connection to PipeWire. Dropping it removes the sink (streams fall back).
struct Session {
    // Field order is drop order: routing and node before the core and context.
    graph: Graph,
    node: Node,
    _core_listener: pw::core::Listener,
    _core: pw::core::CoreRc,
    _context: pw::context::ContextRc,
}

const TICK: Duration = Duration::from_millis(250);
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

type Req = (Request, mpsc::Sender<Response>);

pub fn run(opts: Options) -> Result<()> {
    pw::init();
    let listener = control::bind(&opts.socket)
        .with_context(|| format!("binding control socket {}", opts.socket.display()))?;

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;

    // The audio thread's end is created per session; the engine outlives sessions.
    let (handoff, first_core) = audio::channel(initial_processor());
    let (engine, warnings) = Engine::new(&opts.profiles_dir, opts.mode, handoff);
    for w in warnings {
        eprintln!("stepwave: {w}");
    }
    let engine = Rc::new(RefCell::new(engine));
    let session: Rc<RefCell<Option<Session>>> = Rc::new(RefCell::new(None));
    let spare_core: Rc<RefCell<Option<audio::AudioCore>>> = Rc::new(RefCell::new(Some(first_core)));
    let disconnected = Rc::new(RefCell::new(false));

    // Control requests arrive on the socket thread and are handled on the main loop.
    let (tx, rx) = pw::channel::channel::<Req>();
    control::serve(listener, move |req| {
        let (reply_tx, reply_rx) = mpsc::channel();
        if tx.send((req, reply_tx)).is_err() {
            return Response::err("daemon is shutting down");
        }
        reply_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| Response::err("daemon did not answer in time"))
    });
    let (eng, sess) = (engine.clone(), session.clone());
    let _rx = rx.attach(mainloop.loop_(), move |(req, reply): Req| {
        let reload = matches!(req, Request::Reload);
        let (rate, routed) = match sess.borrow().as_ref() {
            Some(s) => (s.node.rate.load(Relaxed), s.graph.routed()),
            None => (0, Vec::new()),
        };
        let response = eng.borrow_mut().handle(req, rate, &routed);
        if reload {
            if let Some(s) = sess.borrow().as_ref() {
                s.graph.set_matches(eng.borrow().matches());
            }
        }
        let _ = reply.send(response);
    });

    // Periodic housekeeping and (re)connection with exponential backoff.
    let backoff = Rc::new(RefCell::new((Instant::now(), BACKOFF_MIN)));
    let (ml, eng, sess, spare, disc) = (
        mainloop.clone(),
        engine.clone(),
        session.clone(),
        spare_core.clone(),
        disconnected.clone(),
    );
    let timer = mainloop.loop_().add_timer(move |_| {
        eng.borrow_mut().tick();
        if std::mem::take(&mut *disc.borrow_mut()) {
            eprintln!("stepwave: lost PipeWire connection; reconnecting");
            *sess.borrow_mut() = None;
        }
        if sess.borrow().is_some() {
            return;
        }
        let (next_at, delay) = *backoff.borrow();
        if Instant::now() < next_at {
            return;
        }
        let core = spare.borrow_mut().take().unwrap_or_else(|| {
            let (handoff, core) = audio::channel(initial_processor());
            eng.borrow_mut().replace_handoff(handoff);
            core
        });
        match connect(&ml, &eng, core, &disc) {
            Ok(s) => {
                eprintln!("stepwave: connected; sink '{}' is up", node::SINK_NAME);
                *sess.borrow_mut() = Some(s);
                *backoff.borrow_mut() = (Instant::now(), BACKOFF_MIN);
            }
            Err(e) => {
                eprintln!("stepwave: PipeWire unavailable ({e}); retrying in {delay:?}");
                *backoff.borrow_mut() = (Instant::now() + delay, (delay * 2).min(BACKOFF_MAX));
            }
        }
    });
    let _ = timer.update_timer(Some(Duration::from_millis(1)), Some(TICK));

    mainloop.run();
    Ok(())
}

fn connect(
    mainloop: &pw::main_loop::MainLoopRc,
    engine: &Rc<RefCell<Engine>>,
    audio_core: audio::AudioCore,
    disconnected: &Rc<RefCell<bool>>,
) -> Result<Session> {
    let context = pw::context::ContextRc::new(mainloop, None)?;
    let core = context.connect_rc(None)?;
    let disc = disconnected.clone();
    let core_listener = core
        .add_listener_local()
        .error(move |id, _seq, res, msg| {
            // id 0 is the core itself: the connection is gone (e.g. -EPIPE).
            if id == 0 {
                eprintln!("stepwave: PipeWire core error {res}: {msg}");
                *disc.borrow_mut() = true;
            }
        })
        .register();
    let node = node::create(&core, audio_core)?;
    let router = Router::new(node::SINK_NAME, engine.borrow().matches());
    let eng = Rc::downgrade(engine);
    let graph = graph::create(&core, router, move |id| {
        if let Some(e) = eng.upgrade() {
            e.borrow_mut().select_profile(id);
        }
    })?;
    Ok(Session {
        graph,
        node,
        _core_listener: core_listener,
        _core: core,
        _context: context,
    })
}
