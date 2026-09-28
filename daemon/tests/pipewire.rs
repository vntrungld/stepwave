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
