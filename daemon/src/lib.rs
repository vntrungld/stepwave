//! stepwave Linux daemon: PipeWire sink running `core`, per-app routing, CLI control.

pub mod audio;
pub mod control;
pub mod daemon;
pub mod engine;
pub mod graph;
pub mod node;
pub mod profiles;
pub mod protocol;
pub mod router;
