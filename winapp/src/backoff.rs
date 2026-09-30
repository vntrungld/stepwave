//! Capture reconnect backoff policy for `wasapi_io::capture_thread`.
//!
//! A capture session can fail two very different ways: the endpoint was never found (VB-Cable
//! missing at boot) or the stream started fine and later broke (device invalidated, a 2 s
//! stall). The first case grows the backoff exponentially so a persistently absent device is
//! polled less often; the second must not inherit that grown backoff — game audio has already
//! been silent since the failure, so retrying at the accumulated multi-second delay would add
//! up to `MAX` more seconds of dead audio for what is often a transient blip. `next` is the pure
//! decision so it can be unit-tested without WASAPI; `wasapi_io` is the only caller.

use std::time::Duration;

/// Backoff before the first retry, and what a session that actually started resets to.
pub const INITIAL: Duration = Duration::from_millis(250);
/// Upper bound for the backoff while the device stays unfound.
const MAX: Duration = Duration::from_secs(5);

/// Decide how long to sleep before retrying a failed capture session, and what `current`
/// should become for the next failure. `started` is whether the stream actually started
/// (`client.start_stream()` succeeded) before this session failed.
///
/// If it started, the device was working until the failure: sleep only `INITIAL` and reset the
/// backoff to `INITIAL`, so a later never-found-device failure starts growing from scratch
/// rather than from wherever this session's crash happened to be. If it never started, sleep
/// (and grow) `current` as before — an absent device should be polled less often over time, not
/// hammered.
pub fn next(current: Duration, started: bool) -> (Duration, Duration) {
    if started {
        (INITIAL, INITIAL)
    } else {
        (current, (current * 2).min(MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_exponentially_up_to_the_cap_while_never_started() {
        let mut backoff = INITIAL;
        let mut slept = Vec::new();
        for _ in 0..6 {
            let (sleep, next_backoff) = next(backoff, false);
            slept.push(sleep);
            backoff = next_backoff;
        }
        assert_eq!(
            slept,
            [
                Duration::from_millis(250),
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(5), // capped
            ]
        );
        assert_eq!(backoff, Duration::from_secs(5), "stays at the cap");
    }

    #[test]
    fn a_started_session_resets_regardless_of_the_accumulated_backoff() {
        let grown = Duration::from_secs(5);
        let (sleep, next_backoff) = next(grown, true);
        assert_eq!(sleep, INITIAL, "no multi-second wait for a mid-stream drop");
        assert_eq!(
            next_backoff, INITIAL,
            "future never-found failures start over"
        );
    }

    #[test]
    fn never_started_at_the_initial_value_still_just_doubles() {
        let (sleep, next_backoff) = next(INITIAL, false);
        assert_eq!(sleep, INITIAL);
        assert_eq!(next_backoff, Duration::from_millis(500));
    }
}
