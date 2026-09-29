//! stepwave Linux daemon: PipeWire sink running `core`, per-app routing, CLI control.

pub use stepwave_host::{audio, engine, profiles, protocol};

pub mod control;
pub mod daemon;
pub mod graph;
pub mod node;
pub mod router;
