# M5: Linux daemon — native PipeWire node, per-app routing, CLI control

Date: 2026-09-28
Status: approved in chat, awaiting written-spec review

## Goal

Run the stepwave pipeline in real time on Linux, for daily play and for A/B testing on the
dev box. It must be good enough to leave on while playing: it routes only the game's
stream, never glitches on toggles, and never silences the game.

## Decisions (from brainstorming)

| Decision | Choice | Reason |
|---|---|---|
| Purpose | Daily play on Linux and live A/B testing | User answer "both" |
| Control | CLI (`stepwave on/off/toggle/...`), bound to KDE global shortcuts by the user | Fastest to build; tray UI is deferred to M6 and shared with Windows |
| Hosting | One Rust daemon that owns a native PipeWire node (crate `pipewire` 0.10) | See "Rejected alternatives" |
| Plugin format on Linux | None | The LV2 plugin in the original CLAUDE.md plan is dropped; Windows keeps VST3 (M6) |

### Rejected alternatives

- **LV2 plugin + filter-chain + separate routing daemon.** The Rust `lv2` crate has been
  unmaintained since 2021. LV2 has no string control ports, so per-game model and profile
  switching would need a filter-chain reload. A routing daemon is needed anyway.
- **LADSPA plugin + filter-chain.** It has the same limitations, with fewer features.

## Architecture

```
game (cs2) ──stream──► [stepwave sink] ──► Processor (core) ──► default output device
other apps ──────────────────────────────────────────────────► default output device

stepwave CLI ──Unix socket──► daemon ──(lock-free slot)──► audio thread
```

New crate `daemon/` builds a binary named `stepwave`:

- `stepwave daemon [--profiles <dir>] [--mode <m>]` runs the service. A systemd user unit,
  `daemon/stepwave.service`, is shipped with it.
- `stepwave status | on | off | toggle | mode <auto|model|eq|bypass> | strength <dB> |
  profile <id> | reload [--json]` are the client commands.

### Components

Each component has one job and is testable on its own. Only `node` touches PipeWire's
data thread.

1. **`node` — audio node.**
   - Creates a virtual sink named `stepwave` (`media.class = Audio/Sink`, stereo,
     48 kHz, f32).
   - Plays the processed audio to the current default output and follows default-device
     changes.
   - The process callback calls `Processor::process` on each block.
   - It reports 960 samples of latency to PipeWire.
   - Real-time rules from CLAUDE.md apply: no allocation, locks, logging, syscalls or
     panics in the callback.
   - **Risk:** exposing a sink and a playback side through `pipewire-rs` is unproven here.
     The implementation plan starts with a spike task that proves this mechanism
     (capture-sink plus playback stream, as `module-filter-chain` and `module-loopback`
     do). If the spike fails, the fallback is `libpipewire-module-loopback`, loaded by the
     daemon, with the DSP running in a `pw_filter` between its two ends.
2. **`router` — per-app routing.**
   - Watches the registry for nodes with `media.class = Stream/Output/Audio`.
   - When a stream's `application.process.binary` matches a profile's `match.linux`
     entry, it sets the stream's `target.object` to the `stepwave` node through the
     `default` metadata. WirePlumber then relinks it.
   - Other streams are never touched.
   - The matching decision is a pure function of the stream properties and the profiles.
3. **`engine` — state and processor swaps.**
   - Holds the active profile, mode, strength, on/off state and the fallback reason.
   - On any change it builds a new `Processor` off the audio thread and publishes it
     through a lock-free single-slot handoff.
   - The audio thread crossfades old → new over 10 ms (480 samples, linear), then returns
     the old processor through a second slot so it is dropped off the audio thread.
4. **`control` — Unix socket.**
   - Listens at `$XDG_RUNTIME_DIR/stepwave.sock`. The protocol is one JSON object per
     line in each direction.
5. **`cli` — client subcommands.**
   - Formats the reply for humans, or passes it through with `--json`.

### Behaviour rules

- **`off` is bypass through a `UnityMask` processor,** not unrouting. Latency stays at
  20 ms, so toggling causes no time jump, and A/B comparisons are level- and
  time-aligned.
- **Profile selection:**
  - A matching stream appearing selects its profile.
  - With several matching streams, the most recently appeared one wins.
  - When no stream matches, the current profile is kept, so the processor is not rebuilt
    needlessly.
  - `stepwave profile <id>` pins a profile until `stepwave profile --auto`.
- **Sample rate:** the sink's stream format is fixed at 48 000 Hz; if the PipeWire graph
  runs at another rate, PipeWire's adapter converts at the stream boundary, so processing
  still runs at 48 kHz (the plugin itself never resamples). `status.graph_rate` reports
  the negotiated stream rate.
- **Model fallback:** the same order as the M4 CLI's `auto` mode. It tries the model,
  then the static EQ, then bypass. `status` carries the reason for each step down.
- **Manual moves are respected:** if the user moves a routed stream elsewhere (for
  example with pavucontrol), the router leaves that stream alone until it ends.

## Control protocol

Each request is one line of JSON. Examples:

```json
{"cmd":"status"}
{"cmd":"on"}
{"cmd":"off"}
{"cmd":"toggle"}
{"cmd":"mode","value":"eq"}
{"cmd":"strength","value":7.0}
{"cmd":"profile","value":"cs2"}
{"cmd":"profile","value":null}
{"cmd":"reload"}
```

`{"cmd":"profile","value":null}` returns profile selection to automatic.

Each reply is one line: `{"ok":true,"status":{...}}` or `{"ok":false,"error":"..."}`.

The `status` object contains:
- `enabled`, `mode`, `strength_db`
- `profile` (id, or null) and `profile_pinned`
- `processing`: `model`, `eq` or `bypass`, meaning what is actually running
- `fallback_reason` (string or null)
- `graph_rate`
- `routed_streams`: a list of `{id, binary}`

Validation:
- `strength` must be finite and in [0, 24].
- `mode` must be one of the four names.
- `profile` must name a loaded profile.

If the daemon is not running, the CLI prints
`daemon not running; start with: systemctl --user start stepwave` and exits with code 1.

## Configuration

- **Profiles directory:** `--profiles <dir>`, else `~/.config/stepwave/profiles/`.
- **Model path:** a profile's `model` is resolved relative to the parent of the profiles
  directory, as the M4 CLI does. For example, `~/.config/stepwave/profiles/cs2.json`
  resolves to `~/.config/stepwave/models/cs2.swm`.
- **Profile errors:** an invalid profile is skipped and logged; the others still load.
- **Runtime overrides:** changes made through the CLI (strength, mode, on/off, pin) live
  in memory only. Profiles on disk are the single source of truth; `reload` re-reads them.
- **No daemon config file:** the service unit passes flags.

## Error handling

| Situation | Behaviour |
|---|---|
| Model missing or corrupt | Static EQ, with the reason in `status` |
| Invalid profile JSON | That profile is skipped and logged |
| Graph rate is not 48 kHz | PipeWire converts at the stream boundary; processing continues |
| PipeWire restarts or the connection drops | Reconnect with backoff (0.25 s doubling to 5 s), recreate the node, re-route matching streams |
| Daemon crashes | The sink disappears and WirePlumber returns the stream to the default device. systemd restarts the daemon (`Restart=on-failure`) |
| Socket already in use by a live daemon | The second instance exits with an error |
| Stale socket file (no listener) | Removed, then bound |

Logging goes to stderr (journald under systemd), and only from non-audio threads.

## Testing

**Unit tests (CI, no PipeWire):**
- **Router matching:** match or no match by binary name, most-recent-wins, and the
  manual-move rule, all driven by a fake event sequence.
- **Control protocol:** parse and serialise every command. Out-of-range strength, unknown
  mode and unknown profile are rejected.
- **Engine swaps:** swapping processors mid-stream is continuous. A test signal crossing a
  swap has no sample-to-sample step above a bound derived from the signal. A counting
  allocator, as in M4's `no_alloc.rs`, proves the audio path does not allocate during a
  swap.
- **Profile discovery:** relative model paths resolve correctly, and an invalid profile is
  skipped while the others load.

**Integration tests (`#[ignore]`, need a running PipeWire; run with
`cargo test -p stepwave-daemon -- --ignored`):**
- A test client plays a sine as a stream whose `application.process.binary` matches a
  test profile. The test asserts the stream is linked to `stepwave` and the node's output
  carries signal.
- Stopping the daemon returns the stream to the default device.

**Manual checklist** (`docs/measurements/m5-checklist.md`), filled in by the user:
- CS2 on Linux is routed automatically.
- `toggle` makes no click.
- Browser and voice chat are unaffected.
- Switching the output device mid-game works.
- `systemctl --user restart pipewire` mid-game recovers.

## Out of scope

- Tray UI (M6).
- Persisting CLI changes to profiles.
- Sample rates other than 48 kHz.
- Windows (M6).
- Loudness-matching or preamp behaviour in model mode (a separate open decision).
