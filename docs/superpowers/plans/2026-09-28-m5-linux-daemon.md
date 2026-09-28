# M5 Linux Daemon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `stepwave` binary. It runs a native PipeWire sink that processes audio with `core`, routes only matching game streams into it, and is controlled from the CLI over a Unix socket.

**Architecture:**
- New workspace crate `daemon/` (package `stepwave-daemon`, binary `stepwave`).
- Pure modules have no PipeWire dependency and are unit-tested in CI: `protocol`, `profiles`, `router`, `audio`, `engine`, `control`.
- Thin PipeWire glue: `node` (a capture stream acting as the sink, plus a playback stream), `graph` (registry and metadata routing) and `daemon` (main-loop wiring and reconnect). The glue is covered by `#[ignore]` integration tests against the live session.
- Mode selection and fallback move from the offline CLI into `core::select`, so the CLI and the daemon share them.

**Tech Stack:** Rust 1.98 stable, edition 2021. New crates: `pipewire` 0.10 (feature `v0_3_49`), `rtrb` 0.4, `serde`/`serde_json`, `clap` 4, `anyhow`, `tempfile` (dev). PipeWire 1.6 and WirePlumber 0.5 on the dev box.

**Spec:** `docs/superpowers/specs/2026-09-28-m5-linux-daemon-design.md`

All code in this plan was compiled, linted and tested in a scratch clone before it was written down:
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -D warnings` are clean.
- The whole workspace test suite passes.
- The 2 PipeWire integration tests pass against the live session.
- Manual end-to-end runs covered: routing a `pw-play` stream, `status`, `toggle`, rejecting strength 30, SIGKILL fallback, and restarting over a stale socket.

**PipeWire facts verified on the dev box (they shape the code; do not "fix" them):**
- `application.process.binary` is on the **client** info. It is not on the stream node or the node's registry global, which only carry `client.id`. The client's info can arrive **after** the stream node appears.
- `pw-play` is a symlink to `pw-cat`, so its binary is `pw-cat`.
- Setting `target.object` = `"stepwave"` (the sink's node name, with no type) in the `default` metadata makes WirePlumber move the stream.
- When the sink disappears (daemon exits or is killed), WirePlumber returns the stream to the default device, and nothing is persisted.
- The safe `pipewire` crate has no `pw_filter` wrapper. The sink is therefore a capture stream with `media.class = Audio/Sink`, and a separate playback stream carries the output. A lock-free ring connects the two, and both run with `RT_PROCESS`.
- `Buffer::requested()` needs the crate feature `v0_3_49`.
- A fresh `Processor` outputs silence for its first 960 samples (its latency). A swap must therefore run the new processor in parallel for 960 samples before crossfading.

## Global Constraints

- Rust stable, edition 2021.
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` must pass after every task. The toolchain's clippy rejects `chunks_exact(<const>)`: use `as_chunks::<N>()` instead.
- Real-time rules (CLAUDE.md rule 2): the PipeWire process callbacks and `AudioCore::process_interleaved` never allocate, lock, log, make syscalls or panic. Everything is allocated in constructors. Processors are built off the audio thread, arrive through `rtrb`, and are dropped off the audio thread.
- Stereo image: one mask computed from mid, identical gains on L and R. This comes from `core::Processor` and must not change.
- Sample rate 48 000 Hz. If the negotiated capture rate is not 48 000, the audio passes through unprocessed.
- Reported latency is 960 samples. Swaps use a 960-sample warm-up, then a 480-sample linear crossfade.
- The sink's node name is `stepwave`, and the playback stream is `stepwave-output`.
- Control socket: `$XDG_RUNTIME_DIR/stepwave.sock` by default. The protocol is one JSON object per line.
- Strength is valid in [0, 24] dB (`core::profile::MAX_GAIN_DB`).
- Profiles directory: `--profiles <dir>`, else `~/.config/stepwave/profiles`. A model path is resolved relative to the parent of the profiles directory.
- Test data is synthetic only. PipeWire integration tests are `#[ignore]`.
- Commits follow conventional commits (`feat(daemon): …`) on `main`. Every commit message ends with the trailer line `Co-Authored-By: Claude <noreply@anthropic.com>`.

## Review Focus

- **Client info arriving after the stream node:** the stream is still routed. Covered in Task 3 (`routes_when_client_info_arrives_late`).
- **Many control changes in a row** (rapid `toggle` presses, more than the 2-slot queue holds): the latest state still reaches the audio thread. Covered in Task 5 (`rapid_changes_never_lose_the_latest_processor`).
- **Model file missing when a game is detected:** processing falls back to the static EQ with a reason, never silence. Covered in Task 5 (`missing_model_falls_back_to_eq_with_a_reason`) and Task 7 (`matching_stream_is_routed_and_returns_when_daemon_stops`, where `processing == "eq"`).
- **A stale socket file left by a crashed daemon:** the next daemon replaces it, while a live daemon is refused. Covered in Task 6 (`not_running_and_stale_socket`).
- **A PipeWire block larger than the preallocated scratch** (quantum above 8192): it is split, not truncated, and no panic occurs. Covered in Task 4 (`huge_blocks_are_split`).

## File Map

| File | Responsibility |
|---|---|
| `core/src/select.rs` | `Mode`, `Processing`, `Built`, `build()`, `resolve_model_path()`: shared processor selection and fallback |
| `cli/src/main.rs` | Offline CLI, now using `core::select` |
| `daemon/Cargo.toml` | Crate `stepwave-daemon`, binary `stepwave` |
| `daemon/src/lib.rs` | Module list |
| `daemon/src/protocol.rs` | `Request`, `Response`, `Status`, `ModeArg`: JSON line protocol and validation |
| `daemon/src/profiles.rs` | `ProfileSet`: load the directory, skip invalid profiles, build the binary → profile table |
| `daemon/src/router.rs` | `Router`: pure routing decisions (`Event` → `Action`) |
| `daemon/src/audio.rs` | `AudioCore` / `Handoff`: audio-thread processing and lock-free swaps with crossfade |
| `daemon/src/engine.rs` | `Engine`: control state → processor builds and publishes |
| `daemon/src/control.rs` | Unix socket server and client |
| `daemon/src/node.rs` | PipeWire capture sink, playback stream and ring |
| `daemon/src/graph.rs` | PipeWire registry and metadata glue for `Router` |
| `daemon/src/daemon.rs` | `run()`: main loop, control dispatch, housekeeping timer, reconnect |
| `daemon/src/main.rs` | `stepwave` CLI (daemon and client subcommands) |
| `daemon/tests/no_alloc.rs` | Allocation counter over a processor swap |
| `daemon/tests/pipewire.rs` | `#[ignore]` live-session tests |
| `daemon/stepwave.service` | systemd user unit |
| `docs/measurements/m5-checklist.md` | Manual checklist |

---

### Task 1: `core::select`, shared processor selection

**Files:**
- Create: `core/src/select.rs`
- Modify: `core/src/lib.rs` (add `pub mod select;` after `pub mod profile;`)
- Modify: `cli/src/main.rs` (use `core::select`; delete its `resolve_model` and `build_processor`)
- Test: unit tests in `core/src/select.rs`; the existing `cli/tests/cli.rs` must still pass unchanged

**Interfaces:**
- Consumes: `Processor::{new, with_model, with_mask}`, `SwmModel::load`, `mask::UnityMask`, `stft::SAMPLE_RATE`, `CoreError`, `Profile` (all existing).
- Produces:
  - `stepwave_core::select::Mode { Auto, Model, Eq, Bypass }`
  - `select::Processing { Model, Eq, Bypass }` with `as_str() -> &'static str`, returning `"model"`, `"eq"` or `"bypass"`
  - `select::Built { processor: Processor, processing: Processing, fallback_reason: Option<String> }`
  - `select::resolve_model_path(&Profile, profile_path: &Path) -> PathBuf`
  - `select::build(mode: Mode, profile: &Profile, model_path: &Path, strict_model: bool) -> Result<Built, CoreError>`. With `Model` and `strict_model = true`, a load failure is `Err`. Otherwise a failure falls back to the static EQ, with the reason `"model <path> unusable (<err>); using static EQ"`.

- [ ] **Step 1: Write the failing tests.** Add `pub mod select;` to `core/src/lib.rs`, then create `core/src/select.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const CS2: &str = include_str!("../../profiles/cs2.json");
    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/model_random.swm"
    );

    fn profile() -> Profile {
        Profile::from_json(CS2).unwrap()
    }

    #[test]
    fn model_path_is_relative_to_profiles_parent() {
        let p = profile();
        assert_eq!(
            resolve_model_path(&p, Path::new("/home/u/.config/stepwave/profiles/cs2.json")),
            Path::new("/home/u/.config/stepwave/models/cs2.swm")
        );
    }

    #[test]
    fn absolute_model_path_is_kept() {
        let mut p = profile();
        p.model = "/opt/m.swm".into();
        assert_eq!(
            resolve_model_path(&p, Path::new("/x/profiles/cs2.json")),
            Path::new("/opt/m.swm")
        );
    }

    #[test]
    fn each_mode_builds_what_it_says() {
        let p = profile();
        let ok = Path::new(FIXTURE);
        assert_eq!(
            build(Mode::Model, &p, ok, true).unwrap().processing,
            Processing::Model
        );
        assert_eq!(
            build(Mode::Auto, &p, ok, true).unwrap().processing,
            Processing::Model
        );
        assert_eq!(
            build(Mode::Eq, &p, ok, true).unwrap().processing,
            Processing::Eq
        );
        assert_eq!(
            build(Mode::Bypass, &p, ok, true).unwrap().processing,
            Processing::Bypass
        );
    }

    #[test]
    fn missing_model_falls_back_except_strict_model() {
        let p = profile();
        let missing = Path::new("/nonexistent/m.swm");
        let auto = build(Mode::Auto, &p, missing, true).unwrap();
        assert_eq!(auto.processing, Processing::Eq);
        assert!(auto.fallback_reason.unwrap().contains("/nonexistent/m.swm"));
        let lenient = build(Mode::Model, &p, missing, false).unwrap();
        assert_eq!(lenient.processing, Processing::Eq);
        assert!(lenient.fallback_reason.is_some());
        assert!(build(Mode::Model, &p, missing, true).is_err());
    }
}
```

- [ ] **Step 2: Run to verify failure.** Run `cargo test -p stepwave-core --lib select`. Expected: compile errors (`cannot find function build`, `resolve_model_path`).

- [ ] **Step 3: Implement.** Put this above the test module:

```rust
//! Choosing and building a `Processor` for a profile: shared by the CLI and the daemon.
//! Runs off the audio thread (it allocates and reads model files).

use std::path::{Path, PathBuf};

use crate::mask::UnityMask;
use crate::stft::SAMPLE_RATE;
use crate::swm::SwmModel;
use crate::{CoreError, Processor, Profile};

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Model if it loads, else static EQ.
    Auto,
    /// Model; see [`build`] for what happens when it cannot load.
    Model,
    /// The profile's static EQ.
    Eq,
    /// Unity gain (same latency as the other modes).
    Bypass,
}

/// What is actually running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Processing {
    Model,
    Eq,
    Bypass,
}

impl Processing {
    pub fn as_str(self) -> &'static str {
        match self {
            Processing::Model => "model",
            Processing::Eq => "eq",
            Processing::Bypass => "bypass",
        }
    }
}

pub struct Built {
    pub processor: Processor,
    pub processing: Processing,
    /// Why the result is not what `mode` asked for (e.g. the model failed to load).
    pub fallback_reason: Option<String>,
}

/// A profile's `model` path: absolute as-is, else relative to the parent of the directory
/// holding the profile (`<root>/profiles/x.json` → `<root>/<model>`).
pub fn resolve_model_path(profile: &Profile, profile_path: &Path) -> PathBuf {
    let model = Path::new(&profile.model);
    if model.is_absolute() {
        return model.to_path_buf();
    }
    let root = profile_path
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."));
    root.join(model)
}

/// Build a processor. `Auto` falls back to static EQ when the model cannot load. `Model`
/// is strict when `strict_model` is true (the offline CLI) and falls back like `Auto`
/// otherwise (the daemon, which must never go silent).
pub fn build(
    mode: Mode,
    profile: &Profile,
    model_path: &Path,
    strict_model: bool,
) -> Result<Built, CoreError> {
    let eq = |reason: Option<String>| -> Result<Built, CoreError> {
        Ok(Built {
            processor: Processor::new(profile, SAMPLE_RATE)?,
            processing: Processing::Eq,
            fallback_reason: reason,
        })
    };
    match mode {
        Mode::Bypass => Ok(Built {
            processor: Processor::with_mask(Box::new(UnityMask), SAMPLE_RATE)?,
            processing: Processing::Bypass,
            fallback_reason: None,
        }),
        Mode::Eq => eq(None),
        Mode::Model | Mode::Auto => match SwmModel::load(model_path) {
            Ok(model) => Ok(Built {
                processor: Processor::with_model(profile, &model, SAMPLE_RATE)?,
                processing: Processing::Model,
                fallback_reason: None,
            }),
            Err(err) if mode == Mode::Model && strict_model => Err(err),
            Err(err) => eq(Some(format!(
                "model {} unusable ({err}); using static EQ",
                model_path.display()
            ))),
        },
    }
}
```

- [ ] **Step 4: Switch the CLI to it.** Replace `cli/src/main.rs` with the file below. Behaviour is unchanged: auto fallback still prints a `warning: model ... using static EQ` line, and strict `--mode model` still fails with `loading model <path>`.

```rust
//! Offline stepwave processing: `stepwave-cli in.wav out.wav --profile cs2 --mode auto`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Parser;
use stepwave_core::select::{self, build, resolve_model_path};
use stepwave_core::stft::{HOP, SAMPLE_RATE};
use stepwave_core::Profile;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Mode {
    /// Use the profile's model if it loads, else the static EQ (with a warning)
    Auto,
    /// Use the model; fail if it cannot be loaded
    Model,
    /// Use the profile's static EQ
    Eq,
    /// Unity mask, for A/B listening
    Bypass,
}

#[derive(Parser)]
#[command(
    name = "stepwave-cli",
    version,
    about = "Process a 48 kHz WAV file with stepwave"
)]
struct Args {
    /// Input WAV (48 kHz, mono or stereo, 16/24-bit int or 32-bit float)
    input: PathBuf,
    /// Output WAV (48 kHz stereo, 32-bit float)
    output: PathBuf,
    /// Profile id (looked up as profiles/<id>.json) or a path to a profile JSON
    #[arg(long)]
    profile: String,
    /// Processing mode (default: auto)
    #[arg(long, value_enum)]
    mode: Option<Mode>,
    /// Model file (default: the profile's `model`, relative to the profile's repo root)
    #[arg(long)]
    model: Option<PathBuf>,
    /// Same as `--mode bypass`
    #[arg(long, conflicts_with = "mode")]
    bypass: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let profile_path = resolve_profile(&args.profile);
    let json = std::fs::read_to_string(&profile_path)
        .with_context(|| format!("reading profile {}", profile_path.display()))?;
    let profile = Profile::from_json(&json)
        .with_context(|| format!("parsing profile {}", profile_path.display()))?;

    let (mut left, mut right) = read_wav(&args.input)?;
    let len = left.len();
    let peak_in = peak_dbfs(left.iter().chain(&right));

    let mode = if args.bypass {
        Mode::Bypass
    } else {
        args.mode.unwrap_or(Mode::Auto)
    };
    let model_path = args
        .model
        .clone()
        .unwrap_or_else(|| resolve_model_path(&profile, &profile_path));
    let built = build(mode.into(), &profile, &model_path, true)
        .with_context(|| format!("loading model {}", model_path.display()))?;
    if let Some(reason) = &built.fallback_reason {
        eprintln!("warning: {reason}");
    }
    let used = built.processing.as_str();
    let mut processor = built.processor;

    // Feed `latency` samples of silence so the tail comes out, then drop the head.
    let latency = processor.latency_samples();
    left.resize(len + latency, 0.0);
    right.resize(len + latency, 0.0);
    let start = Instant::now();
    for (l, r) in left.chunks_mut(4096).zip(right.chunks_mut(4096)) {
        processor.process(l, r);
    }
    let elapsed = start.elapsed();

    write_wav(&args.output, &left[latency..], &right[latency..])?;

    let frames = ((len + latency) / HOP).max(1);
    let peak_out = peak_dbfs(left[latency..].iter().chain(&right[latency..]));
    println!("peak in {peak_in:.1} dBFS, out {peak_out:.1} dBFS");
    println!(
        "{:.1} µs per 10 ms frame ({frames} frames)",
        elapsed.as_secs_f64() * 1e6 / frames as f64
    );
    println!("mode: {used}");
    Ok(())
}

fn resolve_profile(arg: &str) -> PathBuf {
    let path = Path::new(arg);
    if path.extension().is_some_and(|e| e == "json") || path.is_file() {
        path.to_path_buf()
    } else {
        Path::new("profiles").join(format!("{arg}.json"))
    }
}

/// A relative `model` in a profile resolves against the parent of the profile's directory
/// (`<root>/profiles/x.json` → `<root>/<model>`).
impl From<Mode> for select::Mode {
    fn from(m: Mode) -> Self {
        match m {
            Mode::Auto => select::Mode::Auto,
            Mode::Model => select::Mode::Model,
            Mode::Eq => select::Mode::Eq,
            Mode::Bypass => select::Mode::Bypass,
        }
    }
}

fn read_wav(path: &Path) -> Result<(Vec<f32>, Vec<f32>)> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    if spec.sample_rate != SAMPLE_RATE {
        bail!(
            "{}: sample rate is {} Hz; only 48000 Hz is supported (resample first)",
            path.display(),
            spec.sample_rate
        );
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .with_context(|| format!("reading samples from {}", path.display()))?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .with_context(|| format!("reading samples from {}", path.display()))?
        }
    };
    match spec.channels {
        1 => Ok((samples.clone(), samples)),
        2 => Ok(samples
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| (f[0], f[1]))
            .unzip()),
        n => bail!(
            "{}: {n} channels; only mono or stereo is supported",
            path.display()
        ),
    }
}

fn write_wav(path: &Path, left: &[f32], right: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("creating {}", path.display()))?;
    for (&l, &r) in left.iter().zip(right) {
        writer
            .write_sample(l)
            .with_context(|| format!("writing samples to {}", path.display()))?;
        writer
            .write_sample(r)
            .with_context(|| format!("writing samples to {}", path.display()))?;
    }
    writer
        .finalize()
        .with_context(|| format!("finalizing {}", path.display()))?;
    Ok(())
}

fn peak_dbfs<'a>(x: impl Iterator<Item = &'a f32>) -> f32 {
    20.0 * x.fold(0.0f32, |m, v| m.max(v.abs())).log10()
}
```

- [ ] **Step 5: Verify.** Run `cargo test -p stepwave-core --lib select && cargo test -p stepwave-cli && cargo clippy --workspace --all-targets -- -D warnings`. Expected: 4 select tests pass, 13 CLI tests pass, clippy is clean.

- [ ] **Step 6: Commit**

```bash
git add core/src/select.rs core/src/lib.rs cli/src/main.rs
git commit -m "refactor(core): share processor selection and fallback in core::select

Move the CLI's mode handling (auto/model/eq/bypass, static-EQ fallback
when the model cannot load) and model-path resolution into
core::select so the M5 daemon reuses them. The daemon passes
strict_model = false so a broken model never silences a game; the
CLI keeps failing hard on --mode model.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: Daemon crate, control protocol and profiles

**Files:**
- Modify: `Cargo.toml` (root): `members = ["core", "cli", "daemon", "bindings/python"]`
- Modify: `.github/workflows/ci.yml`: in the `rust` job, add the apt step shown below
- Create: `daemon/Cargo.toml`, `daemon/src/lib.rs`, `daemon/src/main.rs` (a stub for now), `daemon/src/protocol.rs`, `daemon/src/profiles.rs`
- Test: unit tests in `protocol.rs` and `profiles.rs`

**Interfaces:**
- Consumes: `select::{Mode, resolve_model_path}`, `profile::MAX_GAIN_DB`, `Profile` (and its `match_rules.linux: Vec<String>`).
- Produces:
  - `protocol::ModeArg { Auto, Model, Eq, Bypass }`: serde lowercase, `as_str()`, `From<ModeArg> for select::Mode`
  - `protocol::Request { Status, On, Off, Toggle, Mode(ModeArg), Strength(f32), Profile(Option<String>), Reload }`: serde `tag = "cmd", content = "value"`, lowercase
  - `Request::parse(&str) -> Result<Request, String>` and `Request::validate(&self) -> Result<(), String>`
  - `protocol::RoutedStream { id: u32, binary: String }`
  - `protocol::Status { enabled, mode: ModeArg, strength_db: Option<f32>, profile: Option<String>, profile_pinned, processing: String, fallback_reason: Option<String>, graph_rate: u32, routed_streams: Vec<RoutedStream> }`
  - `protocol::Response { ok, status: Option<Status>, error: Option<String> }`, with `Response::ok(Status)`, `Response::err(msg)` and `to_line() -> String` (ends in `\n`, omits `None` fields)
  - `profiles::LoadedProfile { profile: Profile, model_path: PathBuf }`
  - `profiles::ProfileSet::load(dir: &Path) -> (ProfileSet, Vec<String>)`, plus `get(&str) -> Option<&LoadedProfile>`, `is_empty()` and `matches() -> HashMap<String, String>` (binary → profile id)

- [ ] **Step 1: Scaffold.**

Root `Cargo.toml` members line:
```toml
members = ["core", "cli", "daemon", "bindings/python"]
```

`daemon/Cargo.toml`:
```toml
[package]
name = "stepwave-daemon"
description = "Linux stepwave daemon: PipeWire sink running core, per-app routing, CLI control"
version.workspace = true
edition.workspace = true

[[bin]]
name = "stepwave"
path = "src/main.rs"

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
pipewire = { version = "0.10", features = ["v0_3_49"] }
rtrb = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
stepwave-core = { path = "../core" }

[dev-dependencies]
tempfile = "3"
```

`daemon/src/main.rs` (stub until Task 7):
```rust
fn main() {}
```

`daemon/src/lib.rs` (Tasks 3–7 each add their module line):
```rust
//! stepwave Linux daemon: PipeWire sink running `core`, per-app routing, CLI control.

pub mod profiles;
pub mod protocol;
```

In `.github/workflows/ci.yml`, job `rust`, insert this step right after `- uses: Swatinem/rust-cache@v2` and before `- run: cargo fmt --all --check`:
```yaml
      # pipewire-sys (daemon crate) needs the PipeWire headers and libclang for bindgen.
      - run: sudo apt-get update && sudo apt-get install -y libpipewire-0.3-dev libclang-dev
```

- [ ] **Step 2: Write the failing tests.** Create `daemon/src/protocol.rs` and `daemon/src/profiles.rs` each containing only its test module:

`protocol.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_command() {
        let cases = [
            (r#"{"cmd":"status"}"#, Request::Status),
            (r#"{"cmd":"on"}"#, Request::On),
            (r#"{"cmd":"off"}"#, Request::Off),
            (r#"{"cmd":"toggle"}"#, Request::Toggle),
            (r#"{"cmd":"mode","value":"eq"}"#, Request::Mode(ModeArg::Eq)),
            (r#"{"cmd":"strength","value":7.0}"#, Request::Strength(7.0)),
            (
                r#"{"cmd":"profile","value":"cs2"}"#,
                Request::Profile(Some("cs2".into())),
            ),
            (r#"{"cmd":"profile","value":null}"#, Request::Profile(None)),
            (r#"{"cmd":"reload"}"#, Request::Reload),
        ];
        for (line, want) in cases {
            assert_eq!(Request::parse(line).unwrap(), want, "{line}");
        }
    }

    #[test]
    fn requests_round_trip() {
        for req in [
            Request::Toggle,
            Request::Mode(ModeArg::Bypass),
            Request::Strength(0.0),
            Request::Profile(None),
        ] {
            let line = serde_json::to_string(&req).unwrap();
            assert_eq!(Request::parse(&line).unwrap(), req);
        }
    }

    #[test]
    fn rejects_bad_requests() {
        for line in [
            r#"{"cmd":"strength","value":-1}"#,
            r#"{"cmd":"strength","value":30}"#,
            r#"{"cmd":"mode","value":"loud"}"#,
            r#"{"cmd":"explode"}"#,
            "not json",
        ] {
            assert!(Request::parse(line).is_err(), "{line}");
        }
    }

    #[test]
    fn response_is_one_line_and_omits_empty_fields() {
        let line = Response::err("nope").to_line();
        assert_eq!(line, "{\"ok\":false,\"error\":\"nope\"}\n");
    }
}
```

`profiles.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    const CS2: &str = include_str!("../../profiles/cs2.json");

    #[test]
    fn loads_valid_skips_invalid_and_resolves_models() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("profiles");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("cs2.json"), CS2).unwrap();
        std::fs::write(dir.join("broken.json"), "{ nope").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let (set, warnings) = ProfileSet::load(&dir);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("broken.json"));
        let cs2 = set.get("cs2").unwrap();
        assert_eq!(cs2.model_path, root.path().join("models/cs2.swm"));
        let m = set.matches();
        assert_eq!(m.get("cs2").map(String::as_str), Some("cs2"));
        assert_eq!(m.get("firefox"), None);
    }

    #[test]
    fn missing_dir_is_a_warning_not_a_panic() {
        let (set, warnings) = ProfileSet::load(Path::new("/nonexistent/profiles"));
        assert!(set.is_empty());
        assert_eq!(warnings.len(), 1);
    }
}
```

- [ ] **Step 3: Run to verify failure.** Run `cargo test -p stepwave-daemon --lib`. Expected: compile errors (`cannot find type Request`, `ProfileSet`). The first build also compiles `pipewire-sys`, which needs `libpipewire` headers and `libclang`; both are present on the dev box.

- [ ] **Step 4: Implement.** Put this above the test module of `protocol.rs`:

```rust
//! Control protocol: one JSON object per line in each direction.

use serde::{Deserialize, Serialize};
use stepwave_core::profile::MAX_GAIN_DB;
use stepwave_core::select;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeArg {
    Auto,
    Model,
    Eq,
    Bypass,
}

impl ModeArg {
    pub fn as_str(self) -> &'static str {
        match self {
            ModeArg::Auto => "auto",
            ModeArg::Model => "model",
            ModeArg::Eq => "eq",
            ModeArg::Bypass => "bypass",
        }
    }
}

impl From<ModeArg> for select::Mode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Auto => select::Mode::Auto,
            ModeArg::Model => select::Mode::Model,
            ModeArg::Eq => select::Mode::Eq,
            ModeArg::Bypass => select::Mode::Bypass,
        }
    }
}

/// A client request. `{"cmd":"profile","value":null}` returns profile selection to auto.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "value", rename_all = "lowercase")]
pub enum Request {
    Status,
    On,
    Off,
    Toggle,
    Mode(ModeArg),
    Strength(f32),
    Profile(Option<String>),
    Reload,
}

impl Request {
    /// Checks that do not need daemon state (profile names are checked by the daemon).
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Request::Strength(v) if !v.is_finite() || !(0.0..=MAX_GAIN_DB).contains(v) => {
                Err(format!("strength {v} must be in [0, {MAX_GAIN_DB}] dB"))
            }
            _ => Ok(()),
        }
    }

    pub fn parse(line: &str) -> Result<Request, String> {
        let req: Request =
            serde_json::from_str(line.trim()).map_err(|e| format!("bad request: {e}"))?;
        req.validate()?;
        Ok(req)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutedStream {
    pub id: u32,
    pub binary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub enabled: bool,
    pub mode: ModeArg,
    pub strength_db: Option<f32>,
    pub profile: Option<String>,
    pub profile_pinned: bool,
    /// What is actually running: "model", "eq" or "bypass".
    pub processing: String,
    pub fallback_reason: Option<String>,
    /// Negotiated sample rate of the sink, 0 until the stream is connected.
    pub graph_rate: u32,
    pub routed_streams: Vec<RoutedStream>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(status: Status) -> Self {
        Response {
            ok: true,
            status: Some(status),
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Response {
            ok: false,
            status: None,
            error: Some(msg.into()),
        }
    }

    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).expect("Response always serialises");
        s.push('\n');
        s
    }
}
```

And this above the test module of `profiles.rs`:

```rust
//! Loading the profiles directory. Invalid profiles are skipped with a warning.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use stepwave_core::select::resolve_model_path;
use stepwave_core::Profile;

#[derive(Debug, Clone)]
pub struct LoadedProfile {
    pub profile: Profile,
    pub model_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct ProfileSet {
    by_id: BTreeMap<String, LoadedProfile>,
}

impl ProfileSet {
    /// Reads every `*.json` in `dir`. Returns the set and one warning per skipped file.
    pub fn load(dir: &Path) -> (ProfileSet, Vec<String>) {
        let mut set = ProfileSet::default();
        let mut warnings = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                warnings.push(format!("profiles dir {}: {e}", dir.display()));
                return (set, warnings);
            }
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        for path in paths {
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|json| Profile::from_json(&json).map_err(|e| e.to_string()));
            match parsed {
                Ok(profile) => {
                    let model_path = resolve_model_path(&profile, &path);
                    set.by_id.insert(
                        profile.id.clone(),
                        LoadedProfile {
                            profile,
                            model_path,
                        },
                    );
                }
                Err(e) => warnings.push(format!("skipping profile {}: {e}", path.display())),
            }
        }
        (set, warnings)
    }

    pub fn get(&self, id: &str) -> Option<&LoadedProfile> {
        self.by_id.get(id)
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Process binary name -> profile id, from every profile's `match.linux`.
    pub fn matches(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        for p in self.by_id.values() {
            for b in &p.profile.match_rules.linux {
                m.insert(b.clone(), p.profile.id.clone());
            }
        }
        m
    }
}
```

- [ ] **Step 5: Verify.** Run `cargo test -p stepwave-daemon --lib && cargo clippy --workspace --all-targets -- -D warnings`. Expected: 6 tests pass (4 protocol, 2 profiles), clippy is clean.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock .github/workflows/ci.yml daemon
git commit -m "feat(daemon): add stepwave-daemon crate with control protocol and profiles

JSON-line control protocol (status/on/off/toggle/mode/strength/
profile/reload) with validation, and a profiles loader that skips
invalid files and maps process binaries to profiles. CI installs the
PipeWire headers the new crate's pipewire dependency needs.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: Router, pure routing decisions

**Files:**
- Create: `daemon/src/router.rs`
- Modify: `daemon/src/lib.rs` (add `pub mod router;`, keep modules alphabetical)
- Test: unit tests in `router.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `router::Event { Client { id: u32, binary: String }, ClientRemoved { id: u32 }, Stream { id: u32, client: u32 }, StreamRemoved { id: u32 }, Target { stream: u32, target: Option<String> } }`
  - `router::Action { Route { stream: u32 }, SelectProfile { id: String } }`
  - `Router::new(sink_name: &str, matches: HashMap<String, String>)`, plus `handle(Event) -> Vec<Action>`, `set_matches(HashMap<String, String>)` and `routed() -> Vec<(u32, String)>` (sorted by stream id)
- Rules:
  - A stream is routed once, when both its client binary and a match are known, in either arrival order.
  - A routed stream whose `target.object` is set to anything other than the sink name becomes "manual" and is never routed again until it is removed.
  - Our own echo of `target.object = sink_name` is not a manual move.

- [ ] **Step 1: Write the failing tests.** Add `pub mod router;` to `lib.rs`, then create `daemon/src/router.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn router() -> Router {
        let matches = [("cs2", "cs2"), ("pubg", "pubg")]
            .into_iter()
            .map(|(b, p)| (b.to_string(), p.to_string()))
            .collect();
        Router::new("stepwave", matches)
    }

    fn client(id: u32, binary: &str) -> Event {
        Event::Client {
            id,
            binary: binary.into(),
        }
    }

    #[test]
    fn routes_matching_stream_when_client_known_first() {
        let mut r = router();
        assert!(r.handle(client(10, "cs2")).is_empty());
        assert_eq!(
            r.handle(Event::Stream { id: 50, client: 10 }),
            vec![
                Action::Route { stream: 50 },
                Action::SelectProfile { id: "cs2".into() }
            ]
        );
        assert_eq!(r.routed(), vec![(50, "cs2".to_string())]);
    }

    #[test]
    fn routes_when_client_info_arrives_late() {
        let mut r = router();
        assert!(r.handle(Event::Stream { id: 50, client: 10 }).is_empty());
        assert_eq!(
            r.handle(client(10, "cs2")),
            vec![
                Action::Route { stream: 50 },
                Action::SelectProfile { id: "cs2".into() }
            ]
        );
    }

    #[test]
    fn ignores_non_matching_streams_and_routes_once() {
        let mut r = router();
        r.handle(client(10, "firefox"));
        assert!(r.handle(Event::Stream { id: 50, client: 10 }).is_empty());
        r.handle(client(11, "cs2"));
        assert_eq!(r.handle(Event::Stream { id: 51, client: 11 }).len(), 2);
        // client info repeated: no second route
        assert!(r.handle(client(11, "cs2")).is_empty());
        assert_eq!(r.routed(), vec![(51, "cs2".to_string())]);
    }

    #[test]
    fn most_recent_matching_stream_selects_profile() {
        let mut r = router();
        r.handle(client(10, "cs2"));
        r.handle(client(11, "pubg"));
        r.handle(Event::Stream { id: 50, client: 10 });
        let actions = r.handle(Event::Stream { id: 51, client: 11 });
        assert_eq!(actions[1], Action::SelectProfile { id: "pubg".into() });
    }

    #[test]
    fn manual_move_is_respected_until_stream_ends() {
        let mut r = router();
        r.handle(client(10, "cs2"));
        r.handle(Event::Stream { id: 50, client: 10 });
        // our own write echoes back: not manual
        r.handle(Event::Target {
            stream: 50,
            target: Some("stepwave".into()),
        });
        assert_eq!(r.routed().len(), 1);
        // user moves it to the speakers
        r.handle(Event::Target {
            stream: 50,
            target: Some("alsa_output.x".into()),
        });
        assert!(r.routed().is_empty());
        assert!(r.handle(client(10, "cs2")).is_empty());
        // stream ends; a new stream from the same game is routed again
        r.handle(Event::StreamRemoved { id: 50 });
        assert_eq!(r.handle(Event::Stream { id: 52, client: 10 }).len(), 2);
    }

    #[test]
    fn reload_changes_future_matches_only() {
        let mut r = router();
        r.handle(client(10, "cs2"));
        r.handle(Event::Stream { id: 50, client: 10 });
        r.set_matches(HashMap::new());
        assert_eq!(r.routed().len(), 1);
        r.handle(client(11, "cs2"));
        assert!(r.handle(Event::Stream { id: 51, client: 11 }).is_empty());
    }
}
```

- [ ] **Step 2: Run to verify failure.** Run `cargo test -p stepwave-daemon --lib router`. Expected: compile errors (`cannot find type Router`).

- [ ] **Step 3: Implement.** Put this above the test module:

```rust
//! Per-app routing decisions, independent of PipeWire. The PipeWire glue (`graph.rs`)
//! feeds events in and applies the returned actions.
//!
//! A stream's process binary lives on its *client* (`application.process.binary`), not
//! on the node, and the client's info can arrive after the stream node appears; both
//! orders are handled.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A client's info arrived (or changed).
    Client {
        id: u32,
        binary: String,
    },
    ClientRemoved {
        id: u32,
    },
    /// An output audio stream node appeared.
    Stream {
        id: u32,
        client: u32,
    },
    StreamRemoved {
        id: u32,
    },
    /// Someone set (or cleared) `target.object` for a stream in the default metadata.
    Target {
        stream: u32,
        target: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Set the stream's `target.object` to the stepwave sink.
    Route { stream: u32 },
    /// A newly routed stream selects its profile (most recent wins).
    SelectProfile { id: String },
}

#[derive(Debug, Clone)]
struct StreamInfo {
    client: u32,
    /// We have routed it.
    routed: bool,
    /// The user moved it elsewhere after we routed it: leave it alone until it ends.
    manual: bool,
}

pub struct Router {
    sink_name: String,
    /// binary -> profile id
    matches: HashMap<String, String>,
    clients: HashMap<u32, String>,
    streams: HashMap<u32, StreamInfo>,
}

impl Router {
    pub fn new(sink_name: &str, matches: HashMap<String, String>) -> Self {
        Router {
            sink_name: sink_name.to_string(),
            matches,
            clients: HashMap::new(),
            streams: HashMap::new(),
        }
    }

    /// Replace the binary -> profile table (after `reload`). Already-routed streams stay.
    pub fn set_matches(&mut self, matches: HashMap<String, String>) {
        self.matches = matches;
    }

    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        match event {
            Event::Client { id, binary } => {
                self.clients.insert(id, binary);
                let waiting: Vec<u32> = self
                    .streams
                    .iter()
                    .filter(|(_, s)| s.client == id && !s.routed && !s.manual)
                    .map(|(&sid, _)| sid)
                    .collect();
                waiting
                    .into_iter()
                    .flat_map(|sid| self.try_route(sid))
                    .collect()
            }
            Event::ClientRemoved { id } => {
                self.clients.remove(&id);
                Vec::new()
            }
            Event::Stream { id, client } => {
                self.streams.insert(
                    id,
                    StreamInfo {
                        client,
                        routed: false,
                        manual: false,
                    },
                );
                self.try_route(id)
            }
            Event::StreamRemoved { id } => {
                self.streams.remove(&id);
                Vec::new()
            }
            Event::Target { stream, target } => {
                if let Some(s) = self.streams.get_mut(&stream) {
                    if s.routed && target.as_deref() != Some(self.sink_name.as_str()) {
                        s.manual = true;
                    }
                }
                Vec::new()
            }
        }
    }

    fn try_route(&mut self, stream: u32) -> Vec<Action> {
        let Some(info) = self.streams.get(&stream) else {
            return Vec::new();
        };
        if info.routed || info.manual {
            return Vec::new();
        }
        let Some(binary) = self.clients.get(&info.client) else {
            return Vec::new();
        };
        let Some(profile) = self.matches.get(binary).cloned() else {
            return Vec::new();
        };
        if let Some(s) = self.streams.get_mut(&stream) {
            s.routed = true;
        }
        vec![
            Action::Route { stream },
            Action::SelectProfile { id: profile },
        ]
    }

    /// Streams currently routed into the sink (not manually moved away), sorted by id.
    pub fn routed(&self) -> Vec<(u32, String)> {
        let mut v: Vec<(u32, String)> = self
            .streams
            .iter()
            .filter(|(_, s)| s.routed && !s.manual)
            .map(|(&id, s)| (id, self.clients.get(&s.client).cloned().unwrap_or_default()))
            .collect();
        v.sort();
        v
    }
}
```

- [ ] **Step 4: Verify.** Run `cargo test -p stepwave-daemon --lib router && cargo clippy --workspace --all-targets -- -D warnings`. Expected: 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add daemon/src/router.rs daemon/src/lib.rs
git commit -m "feat(daemon): add per-app routing decisions

Pure event -> action state machine: route a stream once its client's
process binary matches a profile (client info may arrive after the
stream), select that profile (most recent wins), and leave streams the
user moved elsewhere alone until they end.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: `AudioCore`, lock-free processor swaps with crossfade

**Files:**
- Create: `daemon/src/audio.rs`, `daemon/tests/no_alloc.rs`
- Modify: `daemon/src/lib.rs` (add `pub mod audio;`)
- Test: unit tests in `audio.rs`, plus `daemon/tests/no_alloc.rs`

**Interfaces:**
- Consumes: `stepwave_core::Processor` (`process(&mut self, left: &mut [f32], right: &mut [f32])`, latency 960).
- Produces:
  - `audio::{WARMUP = 960, XFADE = 480, MAX_FRAMES = 8192}`
  - `audio::channel(initial: Processor) -> (Handoff, AudioCore)`
  - `Handoff::publish(&mut self, Box<Processor>) -> Result<(), Box<Processor>>`, which fails when 2 processors are already queued
  - `Handoff::collect(&mut self) -> usize`, which drops retired processors
  - `AudioCore::process_interleaved(&mut self, data: &mut [f32])`: stereo `[L, R, …]` in place, real-time safe, splits blocks above `MAX_FRAMES`

- [ ] **Step 1: Write the failing tests.** Add `pub mod audio;` to `lib.rs`. Create `daemon/src/audio.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use stepwave_core::mask::UnityMask;
    use stepwave_core::stft::SAMPLE_RATE;
    use stepwave_core::Profile;

    const CS2: &str = include_str!("../../profiles/cs2.json");

    fn unity() -> Processor {
        Processor::with_mask(Box::new(UnityMask), SAMPLE_RATE).unwrap()
    }

    fn eq() -> Processor {
        Processor::new(&Profile::from_json(CS2).unwrap(), SAMPLE_RATE).unwrap()
    }

    fn sine(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let v = 0.3 * (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / 48_000.0).sin();
                [v, v]
            })
            .collect()
    }

    fn run(core: &mut AudioCore, input: &[f32], block: usize) -> Vec<f32> {
        let mut out = input.to_vec();
        for chunk in out.chunks_mut(block * 2) {
            core.process_interleaved(chunk);
        }
        out
    }

    #[test]
    fn without_swaps_it_is_the_processor() {
        let input = sine(4800);
        let (_h, mut core) = channel(eq());
        let got = run(&mut core, &input, 256);
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = input
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| (f[0], f[1]))
            .unzip();
        eq().process(&mut l, &mut r);
        let want: Vec<f32> = l.iter().zip(&r).flat_map(|(&a, &b)| [a, b]).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn swap_crossfades_without_jumps_and_ends_on_the_new_processor() {
        let frames = 9600;
        let input = sine(frames);
        let (mut h, mut core) = channel(unity());
        let before = run(&mut core, &input[..4800 * 2], 256);
        assert!(h.publish(Box::new(eq())).is_ok());
        let after = run(&mut core, &input[4800 * 2..], 256);
        let out: Vec<f32> = before.into_iter().chain(after).collect();

        // Natural max step of this sine is 0.3 * 2π * 1000 / 48000 ≈ 0.039.
        let max_step = out
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| f[0])
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.05, "max step {max_step}");

        // Well after the transition the output matches the new processor alone.
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = input
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| (f[0], f[1]))
            .unzip();
        let mut solo = eq();
        solo.process(&mut l, &mut r);
        let tail = 4800 + WARMUP + XFADE + 960;
        for i in tail..frames {
            assert!((out[i * 2] - l[i]).abs() < 1e-3, "frame {i}");
        }
        // The retired processor comes back for dropping off the audio thread.
        assert_eq!(h.collect(), 1);
    }

    #[test]
    fn publish_refuses_when_two_are_queued() {
        let (mut h, _core) = channel(unity());
        assert!(h.publish(Box::new(unity())).is_ok());
        assert!(h.publish(Box::new(unity())).is_ok());
        assert!(h.publish(Box::new(unity())).is_err());
    }

    #[test]
    fn huge_blocks_are_split() {
        let input = sine(MAX_FRAMES * 2 + 100);
        let (_h, mut core) = channel(unity());
        let out = run(&mut core, &input, MAX_FRAMES * 2 + 100);
        assert_eq!(out.len(), input.len());
        assert!(out.iter().all(|v| v.is_finite()));
    }
}
```

Create `daemon/tests/no_alloc.rs`:

```rust
//! The audio path must not allocate, including while swapping processors.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use stepwave_core::stft::SAMPLE_RATE;
use stepwave_core::{Processor, Profile};
use stepwave_daemon::audio::{channel, WARMUP, XFADE};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn allocs() -> usize {
    ALLOCS.with(|c| c.get())
}

#[test]
fn audio_core_swaps_without_allocating() {
    let profile = Profile::from_json(include_str!("../../profiles/cs2.json")).unwrap();
    let (mut handoff, mut core) = channel(Processor::new(&profile, SAMPLE_RATE).unwrap());
    let mut block = vec![0.1f32; 256 * 2];

    // Warm up (first calls may touch lazily initialised state in dependencies).
    for _ in 0..10 {
        core.process_interleaved(&mut block);
    }

    // Built and published on "the control thread" — allocation allowed here.
    assert!(handoff
        .publish(Box::new(Processor::new(&profile, SAMPLE_RATE).unwrap()))
        .is_ok());

    let before = allocs();
    for _ in 0..((WARMUP + XFADE) / 256 + 20) {
        core.process_interleaved(&mut block);
    }
    assert_eq!(allocs() - before, 0, "audio path allocated during a swap");
    assert_eq!(handoff.collect(), 1);
}
```

- [ ] **Step 2: Run to verify failure.** Run `cargo test -p stepwave-daemon`. Expected: compile errors (`cannot find function channel`).

- [ ] **Step 3: Implement.** Put this above the test module of `audio.rs`:

```rust
//! The audio-thread side: runs the current `Processor` on interleaved stereo blocks and
//! swaps in new processors without allocating, locking or clicking.
//!
//! A new processor arrives through a lock-free ring. It first runs in parallel for
//! `WARMUP` samples (its own latency, so its output is valid and time-aligned with the
//! old one), then the output crossfades linearly over `XFADE` samples. The old processor
//! goes back through a second ring so it is dropped off the audio thread.

use stepwave_core::Processor;

/// Samples a new processor runs in parallel before the crossfade: its latency.
pub const WARMUP: usize = 960;
/// Crossfade length (10 ms at 48 kHz).
pub const XFADE: usize = 480;
/// Largest block processed in one go; longer blocks are split.
pub const MAX_FRAMES: usize = 8192;

/// Control-thread end of the handoff.
pub struct Handoff {
    new_tx: rtrb::Producer<Box<Processor>>,
    old_rx: rtrb::Consumer<Box<Processor>>,
}

impl Handoff {
    /// Queue a processor for the audio thread. Gives it back if two are already queued.
    pub fn publish(&mut self, p: Box<Processor>) -> Result<(), Box<Processor>> {
        self.new_tx.push(p).map_err(|rtrb::PushError::Full(b)| b)
    }

    /// Drop processors the audio thread has retired. Returns how many were dropped.
    pub fn collect(&mut self) -> usize {
        let mut n = 0;
        while self.old_rx.pop().is_ok() {
            n += 1;
        }
        n
    }
}

pub struct AudioCore {
    current: Box<Processor>,
    incoming: Option<Box<Processor>>,
    /// Samples processed since `incoming` arrived.
    pos: usize,
    retired: Option<Box<Processor>>,
    new_rx: rtrb::Consumer<Box<Processor>>,
    old_tx: rtrb::Producer<Box<Processor>>,
    l: Vec<f32>,
    r: Vec<f32>,
    l2: Vec<f32>,
    r2: Vec<f32>,
}

/// Create the two ends. `initial` runs until something else is published.
pub fn channel(initial: Processor) -> (Handoff, AudioCore) {
    let (new_tx, new_rx) = rtrb::RingBuffer::new(2);
    let (old_tx, old_rx) = rtrb::RingBuffer::new(4);
    let core = AudioCore {
        current: Box::new(initial),
        incoming: None,
        pos: 0,
        retired: None,
        new_rx,
        old_tx,
        l: vec![0.0; MAX_FRAMES],
        r: vec![0.0; MAX_FRAMES],
        l2: vec![0.0; MAX_FRAMES],
        r2: vec![0.0; MAX_FRAMES],
    };
    (Handoff { new_tx, old_rx }, core)
}

impl AudioCore {
    /// Process interleaved stereo `[L, R, L, R, ...]` in place. Real-time safe.
    pub fn process_interleaved(&mut self, data: &mut [f32]) {
        for block in data.chunks_mut(MAX_FRAMES * 2) {
            self.process_block(block);
        }
    }

    fn process_block(&mut self, data: &mut [f32]) {
        if let Some(old) = self.retired.take() {
            if let Err(rtrb::PushError::Full(old)) = self.old_tx.push(old) {
                self.retired = Some(old);
            }
        }
        if self.incoming.is_none() {
            if let Ok(p) = self.new_rx.pop() {
                self.incoming = Some(p);
                self.pos = 0;
            }
        }

        let n = data.len() / 2;
        let (l, r) = (&mut self.l[..n], &mut self.r[..n]);
        for (i, f) in data.as_chunks::<2>().0.iter().enumerate() {
            l[i] = f[0];
            r[i] = f[1];
        }

        let Some(next) = self.incoming.as_mut() else {
            self.current.process(l, r);
            interleave(data, l, r);
            return;
        };

        let (l2, r2) = (&mut self.l2[..n], &mut self.r2[..n]);
        l2.copy_from_slice(l);
        r2.copy_from_slice(r);
        self.current.process(l, r);
        next.process(l2, r2);
        for i in 0..n {
            let t = self.pos + i;
            let w = if t < WARMUP {
                0.0
            } else if t < WARMUP + XFADE {
                (t - WARMUP) as f32 / XFADE as f32
            } else {
                1.0
            };
            l[i] += (l2[i] - l[i]) * w;
            r[i] += (r2[i] - r[i]) * w;
        }
        interleave(data, l, r);
        self.pos += n;

        if self.pos >= WARMUP + XFADE {
            let next = self.incoming.take().expect("checked above");
            let old = std::mem::replace(&mut self.current, next);
            if let Err(rtrb::PushError::Full(old)) = self.old_tx.push(old) {
                self.retired = Some(old);
            }
        }
    }
}

fn interleave(data: &mut [f32], l: &[f32], r: &[f32]) {
    for (i, f) in data.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        f[0] = l[i];
        f[1] = r[i];
    }
}
```

- [ ] **Step 4: Verify.** Run `cargo test -p stepwave-daemon --lib audio && cargo test -p stepwave-daemon --test no_alloc && cargo clippy --workspace --all-targets -- -D warnings`. Expected: 4 audio tests and 1 no_alloc test pass. In particular, `swap_crossfades_without_jumps_and_ends_on_the_new_processor` checks for no step larger than 0.05, and that the output after the transition matches the new processor alone.

- [ ] **Step 5: Commit**

```bash
git add daemon/src/audio.rs daemon/src/lib.rs daemon/tests/no_alloc.rs
git commit -m "feat(daemon): swap processors on the audio thread without clicks

New processors arrive through an rtrb ring, run in parallel for their
960-sample latency, then crossfade linearly over 480 samples; retired
ones go back through a second ring to be dropped off the audio thread.
A counting-allocator test proves a swap does not allocate.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: `Engine`, control state to processors

**Files:**
- Create: `daemon/src/engine.rs`
- Modify: `daemon/src/lib.rs` (add `pub mod engine;`)
- Test: unit tests in `engine.rs`

**Interfaces:**
- Consumes: `audio::{channel, Handoff, AudioCore}` (Task 4), `profiles::ProfileSet` (Task 2), `protocol::{ModeArg, Request, Response, RoutedStream, Status}` (Task 2), `select::build` (Task 1), `mask::UnityMask`.
- Produces:
  - `engine::initial_processor() -> Processor` (unity, 48 kHz)
  - `Engine::new(profiles_dir: &Path, mode: ModeArg, handoff: Handoff) -> (Engine, Vec<String>)`
  - `matches() -> HashMap<String, String>`
  - `select_profile(&mut self, id: &str)`, which ignores unknown ids
  - `replace_handoff(&mut self, Handoff)`, which republishes the current state
  - `tick(&mut self)`, which collects retired processors and retries a pending publish
  - `handle(&mut self, req: Request, graph_rate: u32, routed: &[(u32, String)]) -> Response`
  - `status(&self, graph_rate: u32, routed: &[(u32, String)]) -> Status`
- Rules:
  - Active profile = pinned, else the last auto-selected one.
  - With no active profile: unity, with reason `"no game detected yet"`.
  - Off means `Bypass`.
  - A strength override replaces `profile.strength_db` before building.
  - Build with `select::build(mode, &profile, &model_path, false)`.
  - A processor is rebuilt only when (profile, effective mode, strength) changes, or after `reload`.
  - A publish that finds the queue full is kept as `pending` (the newest wins) and retried in `tick`.

- [ ] **Step 1: Write the failing tests.** Add `pub mod engine;` to `lib.rs`. Create `daemon/src/engine.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::channel;

    const CS2: &str = include_str!("../../profiles/cs2.json");
    const FIXTURE: &[u8] = include_bytes!("../../core/tests/fixtures/model_random.swm");

    /// `<tmp>/profiles/cs2.json` (+ `<tmp>/models/cs2.swm` when `with_model`).
    fn setup(with_model: bool) -> (tempfile::TempDir, Engine, crate::audio::AudioCore) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("profiles")).unwrap();
        std::fs::write(root.path().join("profiles/cs2.json"), CS2).unwrap();
        if with_model {
            std::fs::create_dir(root.path().join("models")).unwrap();
            std::fs::write(root.path().join("models/cs2.swm"), FIXTURE).unwrap();
        }
        let (handoff, core) = channel(initial_processor());
        let (engine, warnings) = Engine::new(&root.path().join("profiles"), ModeArg::Auto, handoff);
        assert!(warnings.is_empty(), "{warnings:?}");
        (root, engine, core)
    }

    fn st(e: &mut Engine, req: Request) -> Status {
        let r = e.handle(req, 48_000, &[]);
        assert!(r.ok, "{:?}", r.error);
        r.status.unwrap()
    }

    #[test]
    fn starts_in_bypass_until_a_game_is_seen() {
        let (_d, mut e, _c) = setup(true);
        let s = st(&mut e, Request::Status);
        assert_eq!(s.processing, "bypass");
        assert_eq!(s.profile, None);
        assert_eq!(e.matches().get("cs2").map(String::as_str), Some("cs2"));
    }

    #[test]
    fn selecting_a_profile_builds_the_model_and_toggle_bypasses() {
        let (_d, mut e, _c) = setup(true);
        e.select_profile("cs2");
        let s = st(&mut e, Request::Status);
        assert_eq!(
            (s.processing.as_str(), s.profile.as_deref()),
            ("model", Some("cs2"))
        );
        assert_eq!(s.strength_db, Some(7.0));
        let s = st(&mut e, Request::Toggle);
        assert!(!s.enabled);
        assert_eq!(s.processing, "bypass");
        let s = st(&mut e, Request::Toggle);
        assert_eq!(s.processing, "model");
    }

    #[test]
    fn missing_model_falls_back_to_eq_with_a_reason() {
        let (_d, mut e, _c) = setup(false);
        e.select_profile("cs2");
        let s = st(&mut e, Request::Mode(ModeArg::Model));
        assert_eq!(s.processing, "eq");
        assert!(s.fallback_reason.unwrap().contains("cs2.swm"));
    }

    #[test]
    fn strength_mode_and_pin_are_applied_and_validated() {
        let (_d, mut e, _c) = setup(true);
        let s = st(&mut e, Request::Profile(Some("cs2".into())));
        assert!(s.profile_pinned);
        let s = st(&mut e, Request::Strength(3.5));
        assert_eq!(s.strength_db, Some(3.5));
        let s = st(&mut e, Request::Mode(ModeArg::Eq));
        assert_eq!(s.processing, "eq");
        assert!(
            !e.handle(Request::Profile(Some("nope".into())), 48_000, &[])
                .ok
        );
        assert!(!e.handle(Request::Strength(99.0), 48_000, &[]).ok);
        let s = st(&mut e, Request::Profile(None));
        assert!(!s.profile_pinned);
    }

    #[test]
    fn reload_picks_up_new_profiles_and_drops_removed_ones() {
        let (d, mut e, _c) = setup(true);
        e.select_profile("cs2");
        std::fs::remove_file(d.path().join("profiles/cs2.json")).unwrap();
        let s = st(&mut e, Request::Reload);
        assert_eq!(s.profile, None);
        assert_eq!(s.processing, "bypass");
    }

    #[test]
    fn status_reports_routed_streams_and_rate() {
        let (_d, e, _c) = setup(true);
        let s = e.status(44_100, &[(7, "cs2".into())]);
        assert_eq!(s.graph_rate, 44_100);
        assert_eq!(
            s.routed_streams,
            vec![RoutedStream {
                id: 7,
                binary: "cs2".into()
            }]
        );
    }

    #[test]
    fn rapid_changes_never_lose_the_latest_processor() {
        let (_d, mut e, mut core) = setup(true);
        e.select_profile("cs2");
        for _ in 0..5 {
            st(&mut e, Request::Toggle);
        }
        // Drain the audio side, then tick: the pending (latest) processor gets published.
        let mut block = vec![0.0f32; 4096];
        for _ in 0..10 {
            core.process_interleaved(&mut block);
            e.tick();
        }
        assert!(e.pending.is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure.** Run `cargo test -p stepwave-daemon --lib engine`. Expected: compile errors (`cannot find type Engine`).

- [ ] **Step 3: Implement.** Put this above the test module:

```rust
//! Control-side state: which profile, mode, strength and on/off are wanted, and building
//! the matching `Processor` off the audio thread whenever that changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use stepwave_core::mask::UnityMask;
use stepwave_core::select;
use stepwave_core::stft::SAMPLE_RATE;
use stepwave_core::Processor;

use crate::audio::Handoff;
use crate::profiles::ProfileSet;
use crate::protocol::{ModeArg, Request, Response, RoutedStream, Status};

/// Everything a processor is built from; a new one is built only when this changes.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    profile: Option<String>,
    mode: ModeArg,
    strength: Option<f32>,
}

pub struct Engine {
    profiles_dir: PathBuf,
    profiles: ProfileSet,
    enabled: bool,
    mode: ModeArg,
    strength: Option<f32>,
    pinned: Option<String>,
    auto_profile: Option<String>,
    processing: String,
    fallback_reason: Option<String>,
    applied: Option<Key>,
    handoff: Handoff,
    pending: Option<Box<Processor>>,
}

/// The processor the audio thread starts with: unity, same latency as every other mode.
pub fn initial_processor() -> Processor {
    Processor::with_mask(Box::new(UnityMask), SAMPLE_RATE).expect("48 kHz is supported")
}

impl Engine {
    /// Returns the engine and the warnings from loading the profiles.
    pub fn new(profiles_dir: &Path, mode: ModeArg, handoff: Handoff) -> (Engine, Vec<String>) {
        let (profiles, warnings) = ProfileSet::load(profiles_dir);
        let mut engine = Engine {
            profiles_dir: profiles_dir.to_path_buf(),
            profiles,
            enabled: true,
            mode,
            strength: None,
            pinned: None,
            auto_profile: None,
            processing: "bypass".into(),
            fallback_reason: Some("no game detected yet".into()),
            applied: None,
            handoff,
            pending: None,
        };
        engine.apply();
        (engine, warnings)
    }

    /// binary -> profile id, for the router.
    pub fn matches(&self) -> HashMap<String, String> {
        self.profiles.matches()
    }

    /// The router saw a new matching stream.
    pub fn select_profile(&mut self, id: &str) {
        if self.profiles.get(id).is_some() {
            self.auto_profile = Some(id.to_string());
            self.apply();
        }
    }

    /// A new audio session (after a PipeWire reconnect): republish the current state.
    pub fn replace_handoff(&mut self, handoff: Handoff) {
        self.handoff = handoff;
        self.pending = None;
        self.applied = None;
        self.apply();
    }

    /// Periodic housekeeping: drop retired processors, retry a publish that found the
    /// queue full.
    pub fn tick(&mut self) {
        self.handoff.collect();
        if let Some(p) = self.pending.take() {
            if let Err(p) = self.handoff.publish(p) {
                self.pending = Some(p);
            }
        }
    }

    /// Handle one request. `graph_rate` and `routed` come from the PipeWire side.
    pub fn handle(&mut self, req: Request, graph_rate: u32, routed: &[(u32, String)]) -> Response {
        if let Err(e) = req.validate() {
            return Response::err(e);
        }
        match req {
            Request::Status => {}
            Request::On => self.enabled = true,
            Request::Off => self.enabled = false,
            Request::Toggle => self.enabled = !self.enabled,
            Request::Mode(m) => self.mode = m,
            Request::Strength(s) => self.strength = Some(s),
            Request::Profile(Some(id)) => {
                if self.profiles.get(&id).is_none() {
                    return Response::err(format!("unknown profile '{id}'"));
                }
                self.pinned = Some(id);
            }
            Request::Profile(None) => self.pinned = None,
            Request::Reload => {
                let (profiles, warnings) = ProfileSet::load(&self.profiles_dir);
                for w in warnings {
                    eprintln!("stepwave: {w}");
                }
                self.profiles = profiles;
                if self
                    .pinned
                    .as_ref()
                    .is_some_and(|id| self.profiles.get(id).is_none())
                {
                    self.pinned = None;
                }
                if self
                    .auto_profile
                    .as_ref()
                    .is_some_and(|id| self.profiles.get(id).is_none())
                {
                    self.auto_profile = None;
                }
                self.applied = None; // model files may have changed: rebuild
            }
        }
        self.apply();
        Response::ok(self.status(graph_rate, routed))
    }

    pub fn status(&self, graph_rate: u32, routed: &[(u32, String)]) -> Status {
        let profile = self.active_profile();
        let strength_db = self.strength.or_else(|| {
            profile
                .as_ref()
                .and_then(|id| self.profiles.get(id))
                .map(|p| p.profile.strength_db)
        });
        Status {
            enabled: self.enabled,
            mode: self.mode,
            strength_db,
            profile,
            profile_pinned: self.pinned.is_some(),
            processing: self.processing.clone(),
            fallback_reason: self.fallback_reason.clone(),
            graph_rate,
            routed_streams: routed
                .iter()
                .map(|(id, binary)| RoutedStream {
                    id: *id,
                    binary: binary.clone(),
                })
                .collect(),
        }
    }

    fn active_profile(&self) -> Option<String> {
        self.pinned.clone().or_else(|| self.auto_profile.clone())
    }

    fn apply(&mut self) {
        let key = Key {
            profile: self.active_profile(),
            mode: if self.enabled {
                self.mode
            } else {
                ModeArg::Bypass
            },
            strength: self.strength,
        };
        if self.applied.as_ref() == Some(&key) {
            return;
        }
        let (processor, processing, reason) = self.build(&key);
        self.processing = processing;
        self.fallback_reason = reason;
        self.applied = Some(key);
        // A newer processor supersedes one still waiting for queue space.
        self.pending = None;
        if let Err(p) = self.handoff.publish(Box::new(processor)) {
            self.pending = Some(p);
        }
    }

    fn build(&self, key: &Key) -> (Processor, String, Option<String>) {
        let unity = |reason: Option<String>| (initial_processor(), "bypass".to_string(), reason);
        let Some(id) = &key.profile else {
            return unity(Some("no game detected yet".into()));
        };
        let Some(loaded) = self.profiles.get(id) else {
            return unity(Some(format!("profile '{id}' not loaded")));
        };
        let mut profile = loaded.profile.clone();
        if let Some(s) = key.strength {
            profile.strength_db = s;
        }
        match select::build(key.mode.into(), &profile, &loaded.model_path, false) {
            Ok(b) => (
                b.processor,
                b.processing.as_str().to_string(),
                b.fallback_reason,
            ),
            Err(e) => unity(Some(format!("profile '{id}' unusable ({e}); bypassing"))),
        }
    }
}
```

- [ ] **Step 4: Verify.** Run `cargo test -p stepwave-daemon --lib engine && cargo clippy --workspace --all-targets -- -D warnings`. Expected: 7 tests pass. Note that `profiles/cs2.json` has `strength_db` 7.0.

- [ ] **Step 5: Commit**

```bash
git add daemon/src/engine.rs daemon/src/lib.rs
git commit -m "feat(daemon): add engine mapping control state to processors

Tracks on/off, mode, strength override and pinned/auto profile, builds
the matching processor off the audio thread (static-EQ fallback with a
reason when the model cannot load) and publishes it, keeping the newest
one pending when the swap queue is full.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: Control socket (server and client)

**Files:**
- Create: `daemon/src/control.rs`
- Modify: `daemon/src/lib.rs` (add `pub mod control;`)
- Test: unit tests in `control.rs`

**Interfaces:**
- Consumes: `protocol::{Request, Response}` (Task 2).
- Produces:
  - `control::default_socket_path() -> Result<PathBuf, String>`, which is `$XDG_RUNTIME_DIR/stepwave.sock`
  - `control::bind(&Path) -> io::Result<UnixListener>`: a live daemon gives `AddrInUse`; a stale file is removed
  - `control::serve(listener, handler: Fn(Request) -> Response + Send + 'static) -> JoinHandle<()>`
  - `control::request(&Path, &Request) -> Result<Response, ClientError>`
  - `ClientError { NotRunning, Io(io::Error), BadResponse(String) }`, whose `NotRunning` displays `daemon not running; start with: systemctl --user start stepwave`

- [ ] **Step 1: Write the failing tests.** Add `pub mod control;` to `lib.rs`. Create `daemon/src/control.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ModeArg, Status};

    fn status() -> Status {
        Status {
            enabled: true,
            mode: ModeArg::Auto,
            strength_db: Some(7.0),
            profile: Some("cs2".into()),
            profile_pinned: false,
            processing: "model".into(),
            fallback_reason: None,
            graph_rate: 48_000,
            routed_streams: vec![],
        }
    }

    #[test]
    fn round_trip_through_a_real_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let listener = bind(&path).unwrap();
        serve(listener, |req| match req {
            Request::Status => Response::ok(status()),
            _ => Response::err("only status"),
        });
        let r = request(&path, &Request::Status).unwrap();
        assert_eq!(r.status.unwrap().processing, "model");
        let r = request(&path, &Request::On).unwrap();
        assert_eq!(r.error.as_deref(), Some("only status"));
    }

    #[test]
    fn invalid_line_gets_an_error_reply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        serve(bind(&path).unwrap(), |_| Response::ok(status()));
        let mut conn = UnixStream::connect(&path).unwrap();
        conn.write_all(b"{\"cmd\":\"strength\",\"value\":99}\n")
            .unwrap();
        let mut reply = String::new();
        BufReader::new(conn).read_line(&mut reply).unwrap();
        assert!(reply.contains("\"ok\":false"), "{reply}");
    }

    #[test]
    fn not_running_and_stale_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        assert!(matches!(
            request(&path, &Request::Status),
            Err(ClientError::NotRunning)
        ));
        // A stale socket file (listener dropped) is replaced.
        drop(bind(&path).unwrap());
        assert!(path.exists());
        let _l = bind(&path).unwrap();
        // A live one is refused.
        assert_eq!(bind(&path).unwrap_err().kind(), io::ErrorKind::AddrInUse);
    }
}
```

- [ ] **Step 2: Run to verify failure.** Run `cargo test -p stepwave-daemon --lib control`. Expected: compile errors (`cannot find function bind`).

- [ ] **Step 3: Implement.** Put this above the test module:

```rust
//! Unix-socket control channel: the daemon serves, CLI subcommands connect.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::protocol::{Request, Response};

/// `$XDG_RUNTIME_DIR/stepwave.sock`.
pub fn default_socket_path() -> Result<PathBuf, String> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|d| PathBuf::from(d).join("stepwave.sock"))
        .ok_or_else(|| "XDG_RUNTIME_DIR is not set; pass --socket".to_string())
}

/// Bind the socket. A live daemon on it is an error; a stale file is removed.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("another stepwave daemon is running ({})", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    UnixListener::bind(path)
}

/// Serve requests on a background thread: one request line, one response line per
/// connection. `handler` is called on that thread.
pub fn serve<F>(listener: UnixListener, handler: F) -> std::thread::JoinHandle<()>
where
    F: Fn(Request) -> Response + Send + 'static,
{
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            if let Err(e) = handle_conn(conn, &handler) {
                eprintln!("stepwave: control connection: {e}");
            }
        }
    })
}

fn handle_conn<F: Fn(Request) -> Response>(conn: UnixStream, handler: &F) -> io::Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let response = match Request::parse(&line) {
        Ok(req) => handler(req),
        Err(e) => Response::err(e),
    };
    let mut conn = conn;
    conn.write_all(response.to_line().as_bytes())
}

#[derive(Debug)]
pub enum ClientError {
    /// Nothing is listening on the socket.
    NotRunning,
    Io(io::Error),
    BadResponse(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotRunning => {
                write!(
                    f,
                    "daemon not running; start with: systemctl --user start stepwave"
                )
            }
            ClientError::Io(e) => write!(f, "control socket: {e}"),
            ClientError::BadResponse(e) => write!(f, "bad response from daemon: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Send one request and wait for the response.
pub fn request(path: &Path, req: &Request) -> Result<Response, ClientError> {
    let mut conn = UnixStream::connect(path).map_err(|_| ClientError::NotRunning)?;
    conn.set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(ClientError::Io)?;
    let mut line = serde_json::to_string(req).expect("Request always serialises");
    line.push('\n');
    conn.write_all(line.as_bytes()).map_err(ClientError::Io)?;
    let mut reply = String::new();
    BufReader::new(conn)
        .read_line(&mut reply)
        .map_err(ClientError::Io)?;
    serde_json::from_str(reply.trim()).map_err(|e| ClientError::BadResponse(e.to_string()))
}
```

- [ ] **Step 4: Verify.** Run `cargo test -p stepwave-daemon --lib control && cargo clippy --workspace --all-targets -- -D warnings`. Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add daemon/src/control.rs daemon/src/lib.rs
git commit -m "feat(daemon): add Unix-socket control server and client

One JSON request line and one response line per connection; invalid
requests get an error reply, a stale socket file is replaced, and a
second daemon on a live socket is refused.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: PipeWire node, routing glue, daemon loop and `stepwave` CLI

**Files:**
- Create: `daemon/src/node.rs`, `daemon/src/graph.rs`, `daemon/src/daemon.rs`, `daemon/tests/pipewire.rs`
- Modify: `daemon/src/main.rs` (replace the stub), `daemon/src/lib.rs` (final version below)
- Test: `daemon/tests/pipewire.rs` (`#[ignore]`, needs the live PipeWire session), plus a manual smoke run

**Interfaces:**
- Consumes: everything from Tasks 2–6 with the signatures listed there.
- Produces:
  - `node::SINK_NAME = "stepwave"`
  - `node::create(&CoreRc, AudioCore) -> Result<Node, pw::Error>`, where `Node.rate: Arc<AtomicU32>` is the negotiated rate
  - `graph::create(&CoreRc, Router, on_select: Fn(&str)) -> Result<Graph, pw::Error>`, plus `Graph::routed()` and `Graph::set_matches()`
  - `daemon::Options { profiles_dir, mode: ModeArg, socket }` and `daemon::run(Options) -> anyhow::Result<()>`
  - The `stepwave` binary: `stepwave [--socket P] [--json] daemon|status|on|off|toggle|mode M|strength DB|profile ID|profile --auto|reload`

Design notes (why the code looks like this):
- `node`: the capture stream has `media.class = Audio/Sink` and `node.name = stepwave`. Its process callback decodes f32 LE into preallocated scratch, runs `AudioCore` (only when the negotiated rate is 48 000) and pushes into a 200 ms `rtrb` ring. The playback stream (`stepwave-output`, which follows the default device) pops from the ring and writes silence on underrun. `ProcessLatency` of 960 samples is advertised with `update_params`.
- `graph`: binds every `Client` and reads `application.process.binary` from its info. Streams are output-audio `Node` globals carrying `client.id`. It binds the `default` metadata to route streams and to observe `target.object` changes. Routes decided before the metadata is bound are queued.
- `daemon`: one `MainLoopRc` for the process lifetime. Control requests cross from the socket thread over `pw::channel` and are answered through `mpsc`. A 250 ms timer runs `Engine::tick`, and it connects or reconnects with backoff from 250 ms doubling to 5 s. The core error callback with `id == 0` marks the session lost. Each new session gets a fresh `AudioCore`/`Handoff` pair via `Engine::replace_handoff`, except the first, which uses the pair created at start-up.

- [ ] **Step 1: Write the integration tests** (they cannot pass before the binary exists). Create `daemon/tests/pipewire.rs`:

```rust
//! Integration tests against the running PipeWire + WirePlumber session.
//! Ignored by default (CI has no PipeWire); run with
//! `cargo test -p stepwave-daemon --test pipewire -- --ignored --test-threads=1`.
//! They play a quiet (-40 dBFS) sine through `pw-play` for a few seconds.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const CS2: &str = include_str!("../../profiles/cs2.json");

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `<tmp>/profiles/test.json` matching `pw-cat` (the binary behind `pw-play`), no model.
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut profile: serde_json::Value = serde_json::from_str(CS2).unwrap();
    profile["id"] = "test".into();
    profile["match"] = serde_json::json!({ "linux": ["pw-cat"], "windows": [] });
    std::fs::create_dir(dir.path().join("profiles")).unwrap();
    std::fs::write(dir.path().join("profiles/test.json"), profile.to_string()).unwrap();
    let wav = dir.path().join("sine.wav");
    write_sine(&wav, 6.0);
    let sock = dir.path().join("s.sock");
    (dir, wav, sock)
}

fn write_sine(path: &Path, seconds: f32) {
    let frames = (48_000.0 * seconds) as usize;
    let mut data = Vec::with_capacity(frames * 4);
    for n in 0..frames {
        let v = (328.0 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48_000.0).sin()) as i16;
        data.extend_from_slice(&v.to_le_bytes());
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, 0x0002_0001, 48_000, 48_000 * 4, 0x0010_0004] {
        wav.extend_from_slice(&v.to_le_bytes());
    }
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav).unwrap();
}

fn daemon(dir: &Path, sock: &Path) -> Kill {
    Kill(
        Command::new(env!("CARGO_BIN_EXE_stepwave"))
            .args(["--socket"])
            .arg(sock)
            .args(["daemon", "--profiles"])
            .arg(dir.join("profiles"))
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

fn play(wav: &Path) -> Kill {
    Kill(Command::new("pw-play").arg(wav).spawn().unwrap())
}

/// Where `pw-play`'s left output is linked, if anywhere.
fn pw_play_target() -> Option<String> {
    let out = Command::new("pw-link").arg("-l").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    while let Some(l) = lines.next() {
        if l.trim() == "pw-play:output_FL" {
            return lines
                .next()
                .map(|n| n.trim().trim_start_matches("|->").trim().to_string());
        }
    }
    None
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        sleep(Duration::from_millis(100));
    }
}

fn status(sock: &Path) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_stepwave"))
        .arg("--socket")
        .arg(sock)
        .args(["--json", "status"])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
#[ignore = "needs a running PipeWire session"]
fn matching_stream_is_routed_and_returns_when_daemon_stops() {
    let (dir, wav, sock) = setup();
    let d = daemon(dir.path(), &sock);
    wait_for("daemon socket", || sock.exists());
    sleep(Duration::from_millis(500));
    let _p = play(&wav);
    wait_for("routing into stepwave", || {
        pw_play_target().is_some_and(|t| t.starts_with("stepwave:"))
    });
    let s = status(&sock);
    assert_eq!(s["status"]["profile"], "test");
    assert_eq!(s["status"]["graph_rate"], 48_000);
    assert_eq!(s["status"]["routed_streams"][0]["binary"], "pw-cat");
    // No model file: the daemon must fall back to the static EQ, not go silent.
    assert_eq!(s["status"]["processing"], "eq");

    drop(d);
    wait_for("fallback to the default device", || {
        pw_play_target().is_some_and(|t| !t.starts_with("stepwave:"))
    });
}

#[test]
#[ignore = "needs a running PipeWire session"]
fn toggle_keeps_the_stream_routed() {
    let (dir, wav, sock) = setup();
    let _d = daemon(dir.path(), &sock);
    wait_for("daemon socket", || sock.exists());
    sleep(Duration::from_millis(500));
    let _p = play(&wav);
    wait_for("routing into stepwave", || {
        pw_play_target().is_some_and(|t| t.starts_with("stepwave:"))
    });
    let toggled = Command::new(env!("CARGO_BIN_EXE_stepwave"))
        .arg("--socket")
        .arg(&sock)
        .args(["--json", "toggle"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&toggled.stdout).unwrap();
    assert_eq!(v["status"]["processing"], "bypass");
    assert!(pw_play_target().is_some_and(|t| t.starts_with("stepwave:")));
}
```

- [ ] **Step 2: Run to verify failure.** Run `cargo test -p stepwave-daemon --test pipewire -- --ignored --test-threads=1`. Expected: both tests FAIL, because the stub binary exits at once and the socket never appears ("timed out waiting for daemon socket").

- [ ] **Step 3: Implement the node.** Create `daemon/src/node.rs`:

```rust
//! The PipeWire audio node: a capture stream that appears as the `stepwave` sink, and a
//! playback stream that follows the default output device. The capture callback runs
//! `AudioCore`; a lock-free ring carries the result to the playback callback. Both
//! callbacks run on PipeWire's real-time data thread (`RT_PROCESS`).

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::Arc;

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::pod::{Pod, Value};
use pw::stream::{StreamFlags, StreamListener, StreamRc};

use crate::audio::{AudioCore, MAX_FRAMES};

pub const SINK_NAME: &str = "stepwave";
const PLAYBACK_NAME: &str = "stepwave-output";
const CHANNELS: usize = 2;
const RATE: u32 = 48_000;
/// Ring between the two streams: 200 ms of stereo audio.
const RING_SAMPLES: usize = RATE as usize / 5 * CHANNELS;
/// Algorithmic latency reported to PipeWire, in samples.
const LATENCY_SAMPLES: i32 = 960;

/// Keeps both streams and their listeners alive; drop it to remove the sink.
pub struct Node {
    _capture: StreamRc,
    _playback: StreamRc,
    _capture_listener: StreamListener<CaptureData>,
    _playback_listener: StreamListener<rtrb::Consumer<f32>>,
    /// Negotiated capture rate (0 until negotiated).
    pub rate: Arc<AtomicU32>,
}

pub struct CaptureData {
    core: AudioCore,
    ring: rtrb::Producer<f32>,
    rate: Arc<AtomicU32>,
    scratch: Vec<f32>,
}

fn serialize(value: Value) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &value)
        .expect("static pod serialises")
        .0
        .into_inner()
}

fn format_pod() -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(CHANNELS as u32);
    let mut pos = [0u32; spa::param::audio::MAX_CHANNELS];
    pos[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    pos[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(pos);
    serialize(Value::Object(spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    }))
}

fn latency_pod() -> Vec<u8> {
    serialize(Value::Object(spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamProcessLatency.as_raw(),
        id: spa::param::ParamType::ProcessLatency.as_raw(),
        properties: vec![spa::pod::Property {
            key: spa::sys::SPA_PARAM_PROCESS_LATENCY_rate,
            flags: spa::pod::PropertyFlags::empty(),
            value: Value::Int(LATENCY_SAMPLES),
        }],
    }))
}

/// Create the sink and its output. `audio` is moved onto the data thread.
pub fn create(core: &pw::core::CoreRc, audio: AudioCore) -> Result<Node, pw::Error> {
    let rate = Arc::new(AtomicU32::new(0));
    let (ring_tx, ring_rx) = rtrb::RingBuffer::<f32>::new(RING_SAMPLES);

    let capture = StreamRc::new(
        core.clone(),
        SINK_NAME,
        properties! {
            *pw::keys::MEDIA_CLASS => "Audio/Sink",
            *pw::keys::NODE_NAME => SINK_NAME,
            *pw::keys::NODE_DESCRIPTION => "stepwave (footstep enhancer)",
            "audio.position" => "[ FL FR ]",
            "node.latency" => "480/48000",
        },
    )?;
    let capture_listener = capture
        .add_local_listener_with_user_data(CaptureData {
            core: audio,
            ring: ring_tx,
            rate: rate.clone(),
            scratch: vec![0.0; MAX_FRAMES * CHANNELS],
        })
        .param_changed(|_, data, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = spa::param::audio::AudioInfoRaw::new();
            if info.parse(param).is_ok() {
                data.rate.store(info.rate(), Relaxed);
            }
        })
        .process(|stream, data| {
            let Some(mut buf) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buf.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let offset = d.chunk().offset() as usize;
            let size = d.chunk().size() as usize;
            let Some(bytes) = d.data() else { return };
            let Some(bytes) = bytes.get(offset..offset + size) else {
                return;
            };
            let processing = data.rate.load(Relaxed) == RATE;
            for frame_bytes in bytes.chunks(data.scratch.len() * 4) {
                let n = frame_bytes.len() / 4;
                let block = &mut data.scratch[..n];
                for (v, b) in block.iter_mut().zip(frame_bytes.as_chunks::<4>().0) {
                    *v = f32::from_le_bytes(*b);
                }
                if processing {
                    data.core.process_interleaved(block);
                }
                for &v in block.iter() {
                    // Full ring: playback is not draining; drop rather than block.
                    let _ = data.ring.push(v);
                }
            }
        })
        .register()?;

    let playback = StreamRc::new(
        core.clone(),
        PLAYBACK_NAME,
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Game",
            *pw::keys::NODE_NAME => PLAYBACK_NAME,
            *pw::keys::NODE_DESCRIPTION => "stepwave output",
            "audio.position" => "[ FL FR ]",
            "node.latency" => "480/48000",
        },
    )?;
    let playback_listener = playback
        .add_local_listener_with_user_data(ring_rx)
        .process(|stream, ring| {
            let Some(mut buf) = stream.dequeue_buffer() else {
                return;
            };
            let requested = buf.requested() as usize;
            let datas = buf.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let mut frames = 0;
            if let Some(bytes) = d.data() {
                let max = bytes.len() / (4 * CHANNELS);
                frames = if requested == 0 {
                    max
                } else {
                    requested.min(max)
                };
                for b in bytes[..frames * CHANNELS * 4].as_chunks_mut::<4>().0 {
                    // Underrun (nothing captured yet): silence.
                    let v = ring.pop().unwrap_or(0.0);
                    b.copy_from_slice(&v.to_le_bytes());
                }
            }
            let chunk = d.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = (4 * CHANNELS) as i32;
            *chunk.size_mut() = (frames * CHANNELS * 4) as u32;
        })
        .register()?;

    let flags = StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS;
    let (fmt_in, fmt_out, lat) = (format_pod(), format_pod(), latency_pod());
    capture.connect(
        spa::utils::Direction::Input,
        None,
        flags,
        &mut [Pod::from_bytes(&fmt_in).expect("valid pod")],
    )?;
    capture.update_params(&mut [Pod::from_bytes(&lat).expect("valid pod")])?;
    playback.connect(
        spa::utils::Direction::Output,
        None,
        flags,
        &mut [Pod::from_bytes(&fmt_out).expect("valid pod")],
    )?;

    Ok(Node {
        _capture: capture,
        _playback: playback,
        _capture_listener: capture_listener,
        _playback_listener: playback_listener,
        rate,
    })
}
```

- [ ] **Step 4: Implement the routing glue.** Create `daemon/src/graph.rs`:

```rust
//! PipeWire glue for routing: turns registry and metadata events into `router::Event`s and
//! applies the resulting actions. Runs on the main loop thread only.
//!
//! Facts this relies on (checked against PipeWire 1.6 / WirePlumber 0.5):
//! - `application.process.binary` is on the *client* info, not on the stream node or its
//!   registry global; the node's global carries `client.id`.
//! - Setting `target.object` = the sink's node name in the `default` metadata makes
//!   WirePlumber move the stream; when the sink disappears the stream returns to the
//!   default device, and nothing is persisted.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pipewire as pw;
use pw::client::{Client, ClientListener};
use pw::metadata::{Metadata, MetadataListener};
use pw::registry::{Listener as RegistryListener, RegistryRc};
use pw::types::ObjectType;

use crate::node::SINK_NAME;
use crate::router::{Action, Event, Router};

type OnSelect = Box<dyn Fn(&str)>;

struct State {
    router: Router,
    metadata: Option<(Metadata, MetadataListener)>,
    clients: HashMap<u32, (Client, ClientListener)>,
    /// Routes decided before the `default` metadata was bound.
    pending: Vec<u32>,
    on_select: OnSelect,
}

pub struct Graph {
    _registry: RegistryRc,
    _listener: RegistryListener,
    state: Rc<RefCell<State>>,
}

impl Graph {
    /// Streams currently routed into the sink: `(node id, process binary)`.
    pub fn routed(&self) -> Vec<(u32, String)> {
        self.state.borrow().router.routed()
    }

    /// New binary -> profile table (after a reload).
    pub fn set_matches(&self, matches: HashMap<String, String>) {
        self.state.borrow_mut().router.set_matches(matches);
    }
}

/// Feed one event through the router and apply its actions.
fn dispatch(state: &Rc<RefCell<State>>, event: Event) {
    let actions = state.borrow_mut().router.handle(event);
    for action in actions {
        match action {
            Action::Route { stream } => {
                let st = state.borrow();
                if let Some((md, _)) = st.metadata.as_ref() {
                    md.set_property(stream, "target.object", None, Some(SINK_NAME));
                } else {
                    drop(st);
                    state.borrow_mut().pending.push(stream);
                }
            }
            Action::SelectProfile { id } => (state.borrow().on_select)(&id),
        }
    }
}

pub fn create(
    core: &pw::core::CoreRc,
    router: Router,
    on_select: impl Fn(&str) + 'static,
) -> Result<Graph, pw::Error> {
    let registry = core.get_registry_rc()?;
    let state = Rc::new(RefCell::new(State {
        router,
        metadata: None,
        clients: HashMap::new(),
        pending: Vec::new(),
        on_select: Box::new(on_select),
    }));

    let reg_weak = registry.downgrade();
    let st_global = state.clone();
    let st_remove = state.clone();
    let listener = registry
        .add_listener_local()
        .global(move |g| {
            let Some(props) = g.props else { return };
            let Some(reg) = reg_weak.upgrade() else {
                return;
            };
            match g.type_ {
                ObjectType::Client => {
                    let Ok(client) = reg.bind::<Client, _>(g) else {
                        return;
                    };
                    let id = g.id;
                    let st = st_global.clone();
                    let l = client
                        .add_listener_local()
                        .info(move |info| {
                            let binary = info
                                .props()
                                .and_then(|p| p.get("application.process.binary"))
                                .map(str::to_string);
                            if let Some(binary) = binary {
                                dispatch(&st, Event::Client { id, binary });
                            }
                        })
                        .register();
                    st_global.borrow_mut().clients.insert(id, (client, l));
                }
                ObjectType::Node if props.get("media.class") == Some("Stream/Output/Audio") => {
                    if let Some(client) = props.get("client.id").and_then(|c| c.parse().ok()) {
                        dispatch(&st_global, Event::Stream { id: g.id, client });
                    }
                }
                ObjectType::Metadata if props.get("metadata.name") == Some("default") => {
                    let Ok(md) = reg.bind::<Metadata, _>(g) else {
                        return;
                    };
                    let st = st_global.clone();
                    let l = md
                        .add_listener_local()
                        .property(move |subject, key, _type, value| {
                            if key == Some("target.object") {
                                let target = value.map(str::to_string);
                                dispatch(
                                    &st,
                                    Event::Target {
                                        stream: subject,
                                        target,
                                    },
                                );
                            }
                            0
                        })
                        .register();
                    let pending = std::mem::take(&mut st_global.borrow_mut().pending);
                    for stream in pending {
                        md.set_property(stream, "target.object", None, Some(SINK_NAME));
                    }
                    st_global.borrow_mut().metadata = Some((md, l));
                }
                _ => {}
            }
        })
        .global_remove(move |id| {
            st_remove.borrow_mut().clients.remove(&id);
            dispatch(&st_remove, Event::StreamRemoved { id });
            dispatch(&st_remove, Event::ClientRemoved { id });
        })
        .register();

    Ok(Graph {
        _registry: registry,
        _listener: listener,
        state,
    })
}
```

- [ ] **Step 5: Implement the daemon loop.** Create `daemon/src/daemon.rs`:

```rust
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
```

- [ ] **Step 6: Implement the CLI.** Replace `daemon/src/main.rs`:

```rust
//! `stepwave`: the Linux daemon and its control commands.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use stepwave_daemon::control::{self, ClientError};
use stepwave_daemon::daemon::{self, Options};
use stepwave_daemon::protocol::{ModeArg, Request, Response, Status};

#[derive(Parser)]
#[command(
    name = "stepwave",
    version,
    about = "stepwave footstep enhancer for PipeWire"
)]
struct Cli {
    /// Control socket (default: $XDG_RUNTIME_DIR/stepwave.sock)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Print the daemon's raw JSON reply
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon (normally started by systemd)
    Daemon {
        /// Profiles directory (default: ~/.config/stepwave/profiles)
        #[arg(long)]
        profiles: Option<PathBuf>,
        /// Initial mode
        #[arg(long, value_enum, default_value = "auto")]
        mode: CliMode,
    },
    /// Show what the daemon is doing
    Status,
    /// Enable processing
    On,
    /// Bypass processing (latency unchanged, for fair A/B)
    Off,
    /// Flip on/off (bind this to a global shortcut)
    Toggle,
    /// Processing mode
    Mode {
        #[arg(value_enum)]
        mode: CliMode,
    },
    /// Override the active profile's strength (dB, 0..24) until reload/restart
    Strength { db: f32 },
    /// Pin a profile, or `--auto` to follow the detected game again
    Profile {
        #[arg(required_unless_present = "auto")]
        id: Option<String>,
        #[arg(long, conflicts_with = "id")]
        auto: bool,
    },
    /// Re-read profiles and models from disk
    Reload,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum CliMode {
    Auto,
    Model,
    Eq,
    Bypass,
}

impl From<CliMode> for ModeArg {
    fn from(m: CliMode) -> Self {
        match m {
            CliMode::Auto => ModeArg::Auto,
            CliMode::Model => ModeArg::Model,
            CliMode::Eq => ModeArg::Eq,
            CliMode::Bypass => ModeArg::Bypass,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let socket = match cli
        .socket
        .clone()
        .map(Ok)
        .unwrap_or_else(control::default_socket_path)
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("stepwave: {e}");
            return ExitCode::FAILURE;
        }
    };
    let request = match cli.command {
        Command::Daemon { profiles, mode } => {
            let profiles_dir = profiles.unwrap_or_else(default_profiles_dir);
            let opts = Options {
                profiles_dir,
                mode: mode.into(),
                socket,
            };
            return match daemon::run(opts) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("stepwave: {e:#}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Status => Request::Status,
        Command::On => Request::On,
        Command::Off => Request::Off,
        Command::Toggle => Request::Toggle,
        Command::Mode { mode } => Request::Mode(mode.into()),
        Command::Strength { db } => Request::Strength(db),
        Command::Profile { id, .. } => Request::Profile(id),
        Command::Reload => Request::Reload,
    };
    match control::request(&socket, &request) {
        Ok(resp) => print_response(&resp, cli.json),
        Err(e @ ClientError::NotRunning) | Err(e) => {
            eprintln!("stepwave: {e}");
            ExitCode::FAILURE
        }
    }
}

fn default_profiles_dir() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    config.join("stepwave/profiles")
}

fn print_response(resp: &Response, json: bool) -> ExitCode {
    if json {
        print!("{}", resp.to_line());
    } else if let Some(err) = &resp.error {
        eprintln!("stepwave: {err}");
    } else if let Some(status) = &resp.status {
        print!("{}", format_status(status));
    }
    if resp.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn format_status(s: &Status) -> String {
    let mut out = format!(
        "enabled:    {}\nprofile:    {}{}\nmode:       {} (running: {})\nstrength:   {}\n",
        if s.enabled { "on" } else { "off (bypass)" },
        s.profile.as_deref().unwrap_or("-"),
        if s.profile_pinned { " (pinned)" } else { "" },
        s.mode.as_str(),
        s.processing,
        s.strength_db
            .map_or("-".to_string(), |v| format!("{v:.1} dB")),
    );
    if let Some(reason) = &s.fallback_reason {
        out.push_str(&format!("note:       {reason}\n"));
    }
    out.push_str(&format!(
        "rate:       {}\n",
        if s.graph_rate == 0 {
            "not negotiated yet (no audio)".to_string()
        } else {
            format!("{} Hz", s.graph_rate)
        }
    ));
    if s.routed_streams.is_empty() {
        out.push_str("routed:     none\n");
    } else {
        for r in &s.routed_streams {
            out.push_str(&format!("routed:     {} (node {})\n", r.binary, r.id));
        }
    }
    out
}
```

Final `daemon/src/lib.rs`:

```rust
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
```

- [ ] **Step 7: Verify.**
  1. Run `cargo test -p stepwave-daemon --test pipewire -- --ignored --test-threads=1`. Expected: 2 tests pass (about 2 s; a quiet −40 dBFS 440 Hz tone plays briefly).
  2. Run `cargo test --workspace && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`. Expected: all pass; the 2 PipeWire tests show as ignored.

- [ ] **Step 8: Manual smoke run.** Run these from the repo root, with the real `profiles/` and `models/`:

```bash
cargo run -q -p stepwave-daemon -- --socket /tmp/sw.sock daemon --profiles profiles &
sleep 1
cargo run -q -p stepwave-daemon -- --socket /tmp/sw.sock status      # profile -, running: bypass, "no game detected yet"
cargo run -q -p stepwave-daemon -- --socket /tmp/sw.sock profile cs2 # running: model
cargo run -q -p stepwave-daemon -- --socket /tmp/sw.sock toggle      # enabled: off (bypass)
cargo run -q -p stepwave-daemon -- --socket /tmp/sw.sock strength 30 # error, exit 1
kill %1
```

Record the outputs in the report.

- [ ] **Step 9: Commit**

```bash
git add daemon
git commit -m "feat(daemon): run core as a PipeWire sink with per-app routing

The stepwave sink is a capture stream (media.class Audio/Sink) whose
RT callback runs AudioCore; a lock-free ring feeds a playback stream
that follows the default device. Registry/metadata glue routes streams
whose client binary matches a profile via target.object, and the
daemon loop serves the control socket, ticks the engine and reconnects
to PipeWire with backoff. Adds the stepwave CLI and ignored live-session
integration tests.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: Service unit, checklist and docs

**Files:**
- Create: `daemon/stepwave.service`, `docs/measurements/m5-checklist.md`
- Modify: `README.md` (intro sentence and a new "Linux daemon (M5)" section), `CLAUDE.md` (roadmap: tick M0, M1, M2, M4 and M5; leave M3 unticked because real-recording A/B is still pending)
- Test: none (docs only). Verify that `systemd-analyze --user verify daemon/stepwave.service` prints no errors (a warning that the binary is missing is fine if not installed).

- [ ] **Step 1: Service unit.** Create `daemon/stepwave.service`:

```ini
# systemd user unit for the stepwave daemon.
# Install:
#   cargo install --path daemon
#   mkdir -p ~/.config/stepwave/profiles ~/.config/stepwave/models ~/.config/systemd/user
#   cp profiles/*.json ~/.config/stepwave/profiles/ && cp models/*.swm ~/.config/stepwave/models/
#   cp daemon/stepwave.service ~/.config/systemd/user/
#   systemctl --user daemon-reload && systemctl --user enable --now stepwave
[Unit]
Description=stepwave footstep enhancer (PipeWire)
After=pipewire.service wireplumber.service
Wants=pipewire.service

[Service]
ExecStart=%h/.cargo/bin/stepwave daemon
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
```

- [ ] **Step 2: Checklist.** Create `docs/measurements/m5-checklist.md`:

```markdown
# M5 manual checklist (Linux daemon)

Run on the dev box with the daemon installed as a user service
(`systemctl --user status stepwave` is active). Tick each item and note anything odd.

| # | Check | How | Result |
|---|---|---|---|
| 1 | CS2 is routed automatically | Start CS2; `stepwave status` shows `routed: cs2`, `running: model` | |
| 2 | Other apps are untouched | Play a browser video and join voice chat; `pw-link -l` shows them on the device, not on `stepwave` | |
| 3 | `toggle` makes no click | Bind `stepwave toggle` to a KDE global shortcut; toggle repeatedly during footsteps | |
| 4 | Strength and mode apply live | `stepwave strength 4`, `stepwave mode eq`, `stepwave mode auto` while playing | |
| 5 | Output device switch | Switch the default output (e.g. speakers → headphones) mid-game; audio follows | |
| 6 | PipeWire restart recovers | `systemctl --user restart pipewire wireplumber` mid-game; within ~5 s the game is routed again | |
| 7 | Daemon crash is harmless | `systemctl --user kill -s KILL stepwave`; game audio continues on the device; systemd restarts the daemon | |
| 8 | Manual move is respected | Move the game stream to the device in pavucontrol; the daemon does not pull it back until the game restarts | |
| 9 | Latency feels unchanged | Play a round; no perceptible delay between action and sound | |
```

- [ ] **Step 3: README.** Make two edits.

(a) In the intro, change `or an LV2 plugin in PipeWire (Linux)` to `or a native PipeWire daemon (Linux)`.

(b) Insert this section directly before `## Training data (M2)`:

~~~markdown
## Linux daemon (M5)

`stepwave` creates a PipeWire sink named `stepwave`, moves **only** matching game streams
into it (by process binary, from each profile's `match.linux`) and plays the processed
audio on your default output. Other apps are never touched; if the daemon stops, the game
falls back to the device by itself.

```sh
cargo install --path daemon
mkdir -p ~/.config/stepwave/profiles ~/.config/stepwave/models ~/.config/systemd/user
cp profiles/*.json ~/.config/stepwave/profiles/ && cp models/*.swm ~/.config/stepwave/models/
cp daemon/stepwave.service ~/.config/systemd/user/
systemctl --user daemon-reload && systemctl --user enable --now stepwave

stepwave status                 # what is routed and running
stepwave toggle                 # A/B: processing <-> bypass (bind to a KDE shortcut)
stepwave mode auto|model|eq|bypass
stepwave strength 7             # dB, until reload/restart
stepwave profile cs2            # pin a profile (e.g. to test with a video); --auto to unpin
stepwave reload                 # re-read profiles and models
```
~~~

- [ ] **Step 4: CLAUDE.md roadmap.** Change `- [ ] M0:`, `- [ ] M1:`, `- [ ] M2:`, `- [ ] M4:` and `- [ ] M5:` to `- [x]`.

- [ ] **Step 5: Commit**

```bash
git add daemon/stepwave.service docs/measurements/m5-checklist.md README.md CLAUDE.md
git commit -m "docs(daemon): add systemd unit, M5 checklist and usage docs

User service unit with Restart=on-failure, a manual checklist for the
behaviours only a live game session can show (device switch, PipeWire
restart, manual moves, clicks), README install/usage section, and the
roadmap updated (M3's real-recording A/B is still open).

Co-Authored-By: Claude <noreply@anthropic.com>"
```
