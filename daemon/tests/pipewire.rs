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

/// Start `pw-record` capturing `target` as 16-bit PCM into `out`. Passes
/// `node.dont-fallback` so that if `target` is missing, `pw-record` fails
/// instead of silently falling back to the default source (e.g. the mic) —
/// confirmed by hand: without it, targeting a nonexistent node links to
/// `alsa_input....source` instead; with it, the stream stays unconnected.
fn record(target: &str, out: &Path) -> Child {
    Command::new("pw-record")
        .args(["--target", target, "--rate", "48000"])
        .args(["--channels", "2", "--format", "s16"])
        .args(["-P", "{ node.dont-fallback = true }"])
        .arg(out)
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// Where `pw-record`'s left input is linked from, if anywhere.
fn pw_record_source() -> Option<String> {
    let out = Command::new("pw-link").arg("-l").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    while let Some(l) = lines.next() {
        if l.trim() == "pw-record:input_FL" {
            return lines
                .next()
                .map(|n| n.trim().trim_start_matches("|<-").trim().to_string());
        }
    }
    None
}

/// Read 16-bit PCM samples out of a WAV file's `data` chunk. Tolerant of a
/// header whose declared size was never patched (a writer stopped with
/// SIGKILL rather than closed cleanly): falls back to "rest of the file".
fn read_wav_i16(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF", "not a RIFF file");
    assert_eq!(&bytes[8..12], b"WAVE", "not a WAVE file");
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let declared = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        if id == b"data" {
            let available = bytes.len() - start;
            let size = if declared == 0 || declared > available {
                available
            } else {
                declared
            };
            return bytes[start..start + size]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b))
                .collect();
        }
        pos = start + declared + (declared % 2);
    }
    panic!("no data chunk found in {}", path.display());
}

/// RMS level of 16-bit PCM samples, in dBFS.
fn rms_dbfs(samples: &[i16]) -> f64 {
    let sum_sq: f64 = samples.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    let rms = (sum_sq / samples.len() as f64).sqrt();
    20.0 * (rms / f64::from(i16::MAX)).log10()
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

#[test]
#[ignore = "needs a running PipeWire session"]
fn routed_audio_reaches_the_output() {
    let (dir, wav, sock) = setup();
    let _d = daemon(dir.path(), &sock);
    wait_for("daemon socket", || sock.exists());
    sleep(Duration::from_millis(500));
    let _p = play(&wav);
    wait_for("routing into stepwave", || {
        pw_play_target().is_some_and(|t| t.starts_with("stepwave:"))
    });

    // `stepwave-output` is a plain playback stream (not a device), but
    // `pw-record --target` links to it directly just as well; confirmed by
    // hand against this machine's PipeWire 1.6.8 / WirePlumber 0.5.17
    // (`pw-link -l` shows `pw-record:input_FL/FR` linked from
    // `stepwave-output:output_FL/FR` while recording).
    let out = dir.path().join("out.wav");
    let mut rec = record("stepwave-output", &out);
    // Guard against silently recording the default source (e.g. the mic) if
    // `stepwave-output` were ever missing: require the exact link before
    // trusting anything this records.
    wait_for("pw-record linked to stepwave-output", || {
        pw_record_source().as_deref() == Some("stepwave-output:output_FL")
    });
    sleep(Duration::from_millis(1500));
    // SIGTERM, not the `Kill` guard's SIGKILL: `pw-record` handles it by
    // closing the WAV with a correct data-chunk size before exiting.
    let _ = Command::new("kill").arg(rec.id().to_string()).status();
    let _ = rec.wait();

    let samples = read_wav_i16(&out);
    assert!(
        !samples.is_empty(),
        "recorded no samples from stepwave-output"
    );
    let db = rms_dbfs(&samples);
    // The sine is mixed at about -43 dBFS RMS; the static EQ shifts that by a
    // few dB either way. -60 dBFS is well below that, so this only fails if
    // audio never actually reached the playback stream (e.g. silence from an
    // underrunning or desynchronised ring).
    assert!(db > -60.0, "recording is too quiet ({db:.1} dBFS)");
}
