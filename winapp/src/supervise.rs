//! Restart policy for `stepwave run`'s self-supervision loop.
//!
//! Task Scheduler's `RestartOnFailure` (see `install::task_xml`) only covers the process
//! failing to *start* at logon; once it is running, nothing restarts it if it crashes. So
//! `stepwave run` supervises itself: it spawns a child `stepwave run ... --supervised` (the
//! actual app, see `app::run`), waits for it, and restarts it on an unexpected exit — except
//! when the exit is a clean shutdown (`0`) or another instance already owns the control pipe
//! (`ADDR_IN_USE_CODE`; retrying would just find that same live instance again). To avoid a
//! restart storm, it gives up after `MAX_RESTARTS` within any trailing `WINDOW`.
//!
//! `should_restart` holds the whole policy and is platform-neutral so it can be unit-tested
//! anywhere; the process-spawning loop around it is Windows-only.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Exit code `main` uses when `app::run` fails because another stepwave instance already owns
/// the control pipe (`AddrInUse` from `pipe::bind`).
pub const ADDR_IN_USE_CODE: i32 = 3;

/// At most this many restarts within any trailing `WINDOW`.
const MAX_RESTARTS: usize = 3;
const WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Restart,
    Stop,
}

/// Decide whether to restart after a child exited with `code` (`None` if it terminated without
/// one, e.g. killed). Never restarts on a clean exit (`0`) or `ADDR_IN_USE_CODE`. Otherwise
/// prunes `history` to the trailing `WINDOW` ending at `now` and restarts unless `MAX_RESTARTS`
/// already happened inside it; a restart records `now` in `history`.
pub fn should_restart(
    history: &mut VecDeque<Instant>,
    now: Instant,
    code: Option<i32>,
) -> Decision {
    if matches!(code, Some(0) | Some(ADDR_IN_USE_CODE)) {
        return Decision::Stop;
    }
    while history
        .front()
        .is_some_and(|&t| now.saturating_duration_since(t) >= WINDOW)
    {
        history.pop_front();
    }
    if history.len() >= MAX_RESTARTS {
        return Decision::Stop;
    }
    history.push_back(now);
    Decision::Restart
}

#[cfg(windows)]
mod spawn {
    use super::*;
    use std::ffi::OsString;
    use std::process::ExitCode;

    /// Run `current_exe() <args> --supervised` in a loop, restarting per `should_restart`.
    /// `args` is the process's own argv (without the program name), unchanged; the caller only
    /// reaches this when `--supervised` was not already among them.
    pub fn run_supervised(args: &[OsString]) -> ExitCode {
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("stepwave: locating stepwave.exe: {e}");
                return ExitCode::FAILURE;
            }
        };
        let mut history: VecDeque<Instant> = VecDeque::new();
        loop {
            let status = match std::process::Command::new(&exe)
                .args(args)
                .arg("--supervised")
                .status()
            {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("stepwave: spawning stepwave.exe: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let code = status.code();
            if code == Some(0) {
                return ExitCode::SUCCESS;
            }
            if code == Some(ADDR_IN_USE_CODE) {
                // The child already printed the reason; do not retry against the same
                // still-running instance.
                return ExitCode::from(ADDR_IN_USE_CODE as u8);
            }
            match should_restart(&mut history, Instant::now(), code) {
                Decision::Restart => {
                    eprintln!("stepwave: run exited with {status}; restarting");
                    std::thread::sleep(Duration::from_secs(1));
                }
                Decision::Stop => {
                    eprintln!(
                        "stepwave: run crashed {MAX_RESTARTS} times within {}s; giving up",
                        WINDOW.as_secs()
                    );
                    return ExitCode::FAILURE;
                }
            }
        }
    }
}

#[cfg(windows)]
pub use spawn::run_supervised;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarts_on_crash() {
        let mut history = VecDeque::new();
        let now = Instant::now();
        assert_eq!(
            should_restart(&mut history, now, Some(1)),
            Decision::Restart
        );
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn never_restarts_on_clean_exit_or_addr_in_use() {
        let mut history = VecDeque::new();
        let now = Instant::now();
        assert_eq!(should_restart(&mut history, now, Some(0)), Decision::Stop);
        assert!(history.is_empty(), "clean exit must not count as a crash");
        assert_eq!(
            should_restart(&mut history, now, Some(ADDR_IN_USE_CODE)),
            Decision::Stop
        );
        assert!(history.is_empty(), "AddrInUse must not count as a crash");
    }

    #[test]
    fn stops_after_max_restarts_in_the_window() {
        let mut history = VecDeque::new();
        let base = Instant::now();
        for i in 0..MAX_RESTARTS {
            let t = base + Duration::from_secs(i as u64);
            assert_eq!(
                should_restart(&mut history, t, Some(1)),
                Decision::Restart,
                "restart {i}"
            );
        }
        let t = base + Duration::from_secs(MAX_RESTARTS as u64);
        assert_eq!(should_restart(&mut history, t, Some(1)), Decision::Stop);
    }

    #[test]
    fn allows_again_after_the_window_passes() {
        let mut history = VecDeque::new();
        let base = Instant::now();
        for i in 0..MAX_RESTARTS {
            let t = base + Duration::from_secs(i as u64);
            should_restart(&mut history, t, Some(1));
        }
        let still_inside = base + WINDOW - Duration::from_millis(1);
        assert_eq!(
            should_restart(&mut history, still_inside, Some(1)),
            Decision::Stop,
            "just inside the window: still capped"
        );
        let after = base + WINDOW + Duration::from_secs(1);
        assert_eq!(
            should_restart(&mut history, after, Some(1)),
            Decision::Restart,
            "old crashes must have fallen out of the window"
        );
    }

    #[test]
    fn none_code_counts_as_a_crash() {
        let mut history = VecDeque::new();
        let now = Instant::now();
        assert_eq!(should_restart(&mut history, now, None), Decision::Restart);
    }
}
