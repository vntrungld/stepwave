# M6a: Windows app — VB-Cable capture, WASAPI render, CLI control

Date: 2026-09-29
Status: approved in chat, awaiting written-spec review

## Goal

Run the stepwave pipeline on Windows for CS2. Processing covers **only the game's audio**:
CS2 outputs to the VB-Cable virtual device, and `stepwave.exe` captures that audio,
processes it with `core`, and plays it on the real output device. Control is a CLI with the
same commands as the Linux daemon. Premier and Competitive come first. Use on FACEIT waits
for FACEIT Support to confirm the tool is allowed; see "Fair play".

M6b is out of scope here: tray UI, foreground-game detection, per-game profile switching,
and setting CS2's output device automatically.

## Decisions (from brainstorming)

| Decision | Choice | Reason |
|---|---|---|
| Hosting | Standalone app, VB-Cable in, WASAPI out | Processes only the game (like M5), is an ordinary user-mode program, reuses M5 code, and does not depend on Equalizer APO |
| Control | CLI `stepwave.exe status/on/off/toggle/mode/strength/profile/reload`, bound to a hotkey by the user | Same UX as M5; the base for the M6b tray app |
| Latency | Estimated ~50–80 ms total accepted (revised from 35–45 ms during planning; see "Latency"); to be measured | Only the game is processed, all in user mode |
| Game selection | The user sets CS2's output to "CABLE Input" once in Windows' Volume mixer | Windows remembers it; automating this is M6b |
| Profile | Fixed, default `cs2`, changed with `stepwave profile` | Foreground-game switching is M6b |

### Rejected alternatives

- **Equalizer APO + VST2 plugin.** This has the lowest latency. But the original Equalizer APO
  hosts VST2 only, `nih-plug` cannot build VST2, and the Rust VST2 crates are unmaintained.
  It also processes the whole device (every app) and runs our code inside `audiodg.exe`.
- **Equalizer APO fork with VST3.** It depends on a little-used third-party fork whose VST3
  support its author calls partial.
- **Own APO or own virtual-audio driver.** Too complex: registry and driver work, signing,
  fragility across Windows updates, and deeper contact with the audio stack than an
  anti-cheat-sensitive user wants.
- **Process-loopback capture API (Windows 10 2004+).** It copies the game's audio but does not
  stop the original from playing, so the user would hear both.

This supersedes the "VST3 plugin + Equalizer APO" plan in CLAUDE.md, which is updated to match.

## Architecture

```
CS2 ──(output device = "CABLE Input")──► VB-Cable
                                            │  "CABLE Output" (capture endpoint)
                                            ▼
stepwave.exe run:  capture thread ──► AudioCore (core::Processor) ──► ring ──► render thread
                                                                        │   adaptive resampler
                                                                        ▼
                                                             default render device (speakers/headset)
other apps (Discord, browser) ─────────────────────────────► default render device (untouched)

stepwave.exe <command> ──(named pipe \\.\pipe\stepwave, JSON lines)──► stepwave.exe run
```

### Crates

- **`host/` (new, platform-neutral):** M5's `audio` (`AudioCore`/`Handoff`), `engine`,
  `protocol` and `profiles` modules move here unchanged, with their tests. The Linux daemon
  depends on it. `Engine` gains nothing new; it is used with a pinned profile.
- **`daemon/` (Linux, M5):** keeps `node`, `graph`, `daemon`, `control` (Unix socket) and its
  CLI. It builds only on Linux: the crate uses `#![cfg(target_os = "linux")]`-gated modules,
  and the workspace builds it only when the target is Linux.
- **`winapp/` (new, Windows):** the binary `stepwave.exe`, with these modules:
  - `wasapi_io`: capture and render streams (crate `wasapi`), event-driven, shared mode.
  - `drift`: `rubato::Slip` (a drift-compensating frame slipper, no delay) plus a controller
    that keeps the ring fill at an adaptive target.
  - `pipe`: the control server and client over an `interprocess` local socket (a named pipe on
    Windows, a Unix socket elsewhere, so it is tested on Linux too), with the same JSON-line
    protocol and `ClientError` semantics as M5's `control`.
  - `install`: the Task Scheduler logon task.
  - `app`: wiring, device discovery, reconnect.
  - `main`: the CLI.

  Everything but `wasapi_io` and the Windows parts of `pipe` and `app` is platform-neutral and
  unit-tested on Linux.

### Data flow and threads

- **Capture thread.** It runs under the "Pro Audio" MMCSS class, opens "CABLE Output" as a
  WASAPI shared-mode event-driven capture stream (48 kHz, float32, stereo, with the Windows
  engine's automatic format conversion enabled) and waits on its event. For each packet it runs `AudioCore::process_interleaved` in place and pushes whole
  stereo frames into an `rtrb` ring (1 s capacity). If the ring is full it drops whole frames.
- **Render thread.** It runs under the same MMCSS class and opens the current default render
  endpoint in shared, event-driven mode. On each event it asks for the free frames, pulls
  frames from the ring through the drift resampler, and writes silence on underrun. Stereo
  goes to FL/FR; other channels of a multichannel device are written as zeros.
- **Real-time rules (CLAUDE.md rule 2) apply to both threads' loops:** no allocation, locks or
  logging, and no panics. Buffers and the resampler are allocated before the loop starts.
  Status is published through atomics.
- **Main thread.** It serves the control pipe, runs `Engine::tick` every 250 ms, watches device
  changes and restarts streams.

### Clock drift

VB-Cable and the real device run on independent clocks, which typically differ by 10 to
500 ppm. Left alone, the ring slowly fills (latency grows) or drains (underrun clicks).

- **Slip.** The render side pulls 64-frame chunks through `rubato::Slip` (fixed output). Now
  and then Slip inserts or drops one frame, hidden by a short crossfade. This is a
  drift-compensating copy rather than a filter, so it adds no delay.
- **Controller.** A PI controller sets Slip's ratio each chunk from a smoothed ring fill
  (0.5 s time constant). KP is 2 ppm/frame (about a 10 s loop), KI is 0.07. The ratio is
  clamped to ±1000 ppm with anti-windup.
- **Target fill.** Capture arrives in device-period packets and render asks for device-period
  buffers. Because the clocks differ, their phases slide through each other, and now and
  then two render periods fall between two capture packets. The ring must therefore hold
  about two periods plus a chunk. The target adapts to the largest packet or request seen:
  `max(2 · period + 64, 256)` frames, which is about 21 ms with 10 ms periods and about 7 ms
  with 3 ms periods.
- **Priming and resync.** The render side outputs silence until the ring reaches the target.
  If it runs dry (capture stall) it primes again. If it holds more than 4 × target (render
  stall) it drops the excess. Both count as `resyncs`. The learned clock ratio survives a
  resync.
- **Rule 5.** This is clock matching *outside the processing chain*, so it does not break
  CLAUDE.md rule 5: `core` always processes 48 kHz.

### Latency

The total is estimated at **~50–80 ms** (processing 20 ms + ring ~20 ms + capture/render
engine buffers); measure it with the M6a checklist (`docs/measurements/m6a-checklist.md`):
- 960 samples (20 ms) of algorithmic latency;
- the ring target, about 21 ms with Windows' default 10 ms shared-mode periods;
- the two devices' own engine buffers.

This is over CLAUDE.md's ≤ 20 ms budget. It was accepted in exchange for processing only the
game, all in user mode. (Planning measured, in simulation, that the ring needs two device
periods, so the earlier 35–45 ms estimate was too low.) `status` reports the ring fill, so the
real figure is visible. Exclusive mode and low-latency shared periods (IAudioClient3), which
could roughly halve the ring and engine buffers, are out of scope.

## Control

Commands, arguments and replies are identical to M5 (`host::protocol`):

```
stepwave run [--profiles <dir>] [--mode <m>]      # the long-running app
stepwave install | uninstall                      # Task Scheduler logon task, per user, no admin
stepwave status | on | off | toggle | mode <m> | strength <dB> | profile <id> | profile --auto | reload [--json]
```

- **`profile --auto`.** In M6a there is no game detection, so `profile --auto` keeps the current
  profile and only clears the pin. The default profile is `cs2`, or the first profile when
  `cs2.json` is absent.
- **Transport.** An `interprocess` namespaced local socket named `stepwave`, which is a named
  pipe on Windows. A second `run` finds a live instance answering and exits with an error.
  Read timeouts are best-effort, because Windows named pipes do not support them. Each
  connection runs on its own thread, so a silent client cannot block other requests.
- **Extra `status` fields** (added to `Status` as optional fields that M5 omits):
  `capture_device`, `render_device`, `ring_fill_frames`, `drift_ppm`, `resyncs`.
- **Status rules.**
  - `graph_rate` reports the capture stream's rate.
  - `routed_streams` is empty. On Windows the game is chosen in the Volume mixer, not by
    routing.
  - `processing` and `fallback_reason` behave as in M5.

## Files and configuration

Everything lives under `%LOCALAPPDATA%\stepwave\`: `stepwave.exe`, `profiles\*.json` and
`models\*.swm`. Model paths resolve relative to the parent of `profiles\`, as in M5. There is
no daemon config file; `stepwave install` writes the task with the flags given to it.

## Setup (user-facing, `docs/windows-setup.md`)

1. Install VB-Cable (free, signed driver) and reboot.
2. In the classic Sound Control Panel, set **CABLE Input** (Playback tab), **CABLE Output**
   (Recording tab) and your real output device to **2 channels, 24-bit, 48000 Hz**.
3. Copy `stepwave.exe`, `profiles\` and (optionally) `models\` to `%LOCALAPPDATA%\stepwave\`.
4. Run `stepwave install` so the app starts at logon, then `stepwave run` once now, or log out
   and back in.
5. Start CS2. In Settings → System → Sound → Volume mixer, set `cs2.exe`'s output device to
   **CABLE Input**. Windows remembers this.
6. Bind `stepwave toggle` to a hotkey (a Desktop shortcut with a shortcut key, or AutoHotkey).
7. Recovery if the game goes silent: run `stepwave status`, or set `cs2.exe` back to your real
   device in the Volume mixer.

## Error handling

| Situation | Behaviour |
|---|---|
| VB-Cable not installed or not found | `run` keeps running; `status` says `CABLE Output not found — install VB-Cable`; it retries with a 0.25 s → 5 s backoff |
| Model missing or corrupt | Static EQ with the reason in `status` (as in M5) |
| Capture or render device format is not 48 kHz float stereo | The streams are opened with the Windows engine's automatic conversion, so `core` always receives 48 kHz float stereo; `graph_rate` reports the stream rate. The setup guide still recommends 48 kHz to avoid extra conversion |
| Default render device changes, or the device is unplugged | The render stream reopens on the new default device; ring and drift controller reset |
| Capture device disappears | Capture reopens with a 0.25 s → 5 s backoff |
| Ring fill leaves the safe range | Resync (drop whole frames, or insert silence); `resyncs` increments |
| `stepwave run` exits or crashes | **CS2 goes silent** because its output is pinned to VB-Cable. `stepwave run` supervises itself and restarts the app (up to 3 times a minute); the setup guide gives the Volume-mixer recovery step |
| Second `run` | The pipe is in use: error and exit |

Unlike Linux, stopping the app does **not** return the game to the speakers. The guide and the
checklist say so plainly.

## Testing

**Unit tests (Linux, in CI):**
- `host` keeps every M5 test.
- **Drift controller.** Simulate capture and render clocks at 0 and ±200 ppm for 10 minutes
  of audio, including mismatched periods (144 vs 441 frames). Assert:
  - no resyncs, and the ring never runs dry after the first 10 s;
  - the mean correction converges to the clock difference (±20 ppm);
  - a 1 kHz sine passes through with no sample-to-sample step above its natural maximum
    plus a small margin.
- **Resync.** A 500 ms render stall causes exactly one resync. A 300 ms capture stall causes
  one re-prime, then recovery.
- **Target.** The target adapts to the packet size.
- **No allocation.** A counting-allocator test covers the render pull path (resampler plus
  ring), in the style of M4 and M5.
- **Pipe protocol.** Request and response framing over an in-memory stream, reusing M5's
  cases.

**CI:** a new `windows` job on `windows-latest` runs clippy and the tests of `core`, `host` and
`winapp`, builds `stepwave.exe` in release mode and uploads it as an artifact. There is no
Linux cross-compile. Everything except the WASAPI glue also builds and tests on Linux.

**Manual checklist** (`docs/measurements/m6a-checklist.md`), filled in by the user on Windows:
- CS2 is processed (`status` shows `running: model`) and Discord is unaffected.
- `toggle` makes no click.
- Headset/speaker switch mid-game.
- 30 minutes of play with `resyncs` at 0 and no drift in latency.
- Perceived latency.
- Recovery after `stepwave run` is stopped.
- Unplugging and replugging the output device.

## Fair play

stepwave does not read game memory, inject into processes or modify game files; it only
processes the audio output. Output-side footstep enhancement is nonetheless a grey area under
FACEIT's "unfair advantage" rule, and FACEIT banned players in 2020 for boosting footsteps
through a game-file exploit. **Do not use stepwave in FACEIT matches until FACEIT Support has
confirmed in writing that it is allowed.** Premier and Competitive (VAC) first.

## Out of scope

- Tray UI, foreground-game detection, per-game switching, automatic Volume-mixer routing (M6b).
- Exclusive mode.
- Sample rates other than 48 kHz.
- An installer.
- Equalizer APO integration.
