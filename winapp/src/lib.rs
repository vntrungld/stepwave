//! stepwave for Windows: VB-Cable capture → core → WASAPI render, CLI over a named pipe.
//! `backoff`, `devices`, `drift`, `pipe`, `install`'s XML builder and `supervise`'s restart
//! policy are platform-neutral and tested everywhere; `wasapi_io` and `app` are Windows-only.

pub mod backoff;
pub mod devices;
pub mod drift;
pub mod install;
pub mod pipe;
pub mod supervise;

#[cfg(windows)]
pub mod app;
#[cfg(windows)]
pub mod wasapi_io;
