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

/// All `pw-link -l` targets of the ports of `node` whose names start with
/// `port_prefix` (e.g. `("stepwave-output", "output_")`).
fn links_of(node: &str, port_prefix: &str) -> Vec<String> {
    let out = Command::new("pw-link").arg("-l").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let own = format!("{node}:{port_prefix}");
    let mut links = Vec::new();
    let mut in_node = false;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with("|->") || t.starts_with("|<-") {
            if in_node {
                links.push(t[3..].trim().to_string());
            }
        } else {
            in_node = t.starts_with(&own);
        }
    }
    links
}

/// Value of `key` in the `default` metadata (subject 0), if set.
fn default_metadata(key: &str) -> Option<String> {
    let out = Command::new("pw-metadata")
        .args(["-n", "default", "0", key])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains(&format!("key:'{key}'")))?;
    let value = line.split("value:'").nth(1)?;
    Some(value[..value.find("' type:")?].to_string())
}

/// Sets `default.configured.audio.sink` for the life of the guard and puts
/// the previous value back (or removes the key if it was unset) on drop, so
/// the user's default output is restored even if the test panics.
struct DefaultSink(Option<String>);
impl DefaultSink {
    fn set(name: &str) -> DefaultSink {
        let key = "default.configured.audio.sink";
        let guard = DefaultSink(default_metadata(key));
        let value = format!("{{\"name\":\"{name}\"}}");
        let ok = Command::new("pw-metadata")
            .args(["-n", "default", "0", key, &value, "Spa:String:JSON"])
            .stdout(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "could not set the default sink");
        guard
    }
}
impl Drop for DefaultSink {
    fn drop(&mut self) {
        let key = "default.configured.audio.sink";
        let mut cmd = Command::new("pw-metadata");
        cmd.args(["-n", "default"]);
        match &self.0 {
            Some(prev) => cmd.args(["0", key, prev, "Spa:String:JSON"]),
            None => cmd.args(["-d", "0", key]),
        };
        let _ = cmd.stdout(Stdio::null()).status();
    }
}

#[test]
#[ignore = "needs a running PipeWire session; briefly changes the default output"]
fn selecting_stepwave_as_default_output_does_not_loop() {
    let (dir, _wav, sock) = setup();
    let _d = daemon(dir.path(), &sock);
    wait_for("daemon socket", || sock.exists());
    wait_for("stepwave-output linked to a device", || {
        !links_of("stepwave-output", "output_").is_empty()
    });
    let _default = DefaultSink::set("stepwave");
    wait_for("WirePlumber to apply the new default", || {
        default_metadata("default.audio.sink").is_some_and(|v| v.contains("\"stepwave\""))
    });
    // Give WirePlumber time to (wrongly) re-link the output to the new default.
    sleep(Duration::from_millis(1500));
    let links = links_of("stepwave-output", "output_");
    assert!(
        !links.is_empty(),
        "stepwave-output should stay linked to a real device"
    );
    assert!(
        links.iter().all(|l| !l.starts_with("stepwave:")),
        "feedback loop: stepwave-output is linked into its own sink: {links:?}"
    );
}

/// Clicks (one sample at -12 dBFS on both channels) every 250 ms after 500 ms
/// of silence, as a 16-bit stereo WAV.
fn write_clicks(path: &Path, seconds: f32) {
    let frames = (48_000.0 * seconds) as usize;
    let clicks: Vec<usize> = (24_000..frames - 12_000).step_by(12_000).collect();
    let mut data = vec![0u8; frames * 4];
    for &c in &clicks {
        let v = 8192i16.to_le_bytes();
        data[c * 4..c * 4 + 2].copy_from_slice(&v);
        data[c * 4 + 2..c * 4 + 4].copy_from_slice(&v);
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

fn link(from: &str, to: &str) {
    let ok = Command::new("pw-link")
        .args([from, to])
        .status()
        .unwrap()
        .success();
    assert!(ok, "pw-link {from} {to} failed");
}

#[test]
#[ignore = "needs a running PipeWire session"]
fn output_delay_is_the_reported_latency() {
    let (dir, _wav, sock) = setup();
    let clicks = dir.path().join("clicks.wav");
    write_clicks(&clicks, 3.0);
    // Bypass: a unity mask with the same 960-sample latency as every mode, so
    // the click comes out unfiltered and easy to locate.
    let _d = Kill(
        Command::new(env!("CARGO_BIN_EXE_stepwave"))
            .args(["--socket"])
            .arg(&sock)
            .args(["daemon", "--mode", "bypass", "--profiles"])
            .arg(dir.path().join("profiles"))
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for("daemon socket", || sock.exists());
    sleep(Duration::from_millis(500));

    // One recorder, two channels, linked by hand: FL = what pw-play sends into
    // stepwave, FR = what stepwave-output plays. Both are recorded on the same
    // clock, so their offset is the added delay (plus the async hop explained
    // below).
    let out = dir.path().join("delay.wav");
    let rec = Kill(
        Command::new("pw-record")
            .args(["--rate", "48000", "--channels", "2", "--format", "s16"])
            .args(["-P", "{ node.autoconnect = false }"])
            .arg(&out)
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for("pw-record ports", || {
        links_of("pw-record", "input_").is_empty()
            && String::from_utf8_lossy(&Command::new("pw-link").arg("-i").output().unwrap().stdout)
                .contains("pw-record:input_FR")
    });
    link("stepwave-output:output_FL", "pw-record:input_FR");
    let p = Kill(
        Command::new("pw-play")
            .args(["--target", "stepwave"])
            .arg(&clicks)
            .spawn()
            .unwrap(),
    );
    wait_for("pw-play ports", || {
        String::from_utf8_lossy(&Command::new("pw-link").arg("-o").output().unwrap().stdout)
            .contains("pw-play:output_FL")
    });
    link("pw-play:output_FL", "pw-record:input_FL");
    sleep(Duration::from_millis(500));
    let quantum = driver_quantum_of("stepwave").expect("stepwave has a driver");
    let mut p = p;
    let _ = p.0.wait();
    sleep(Duration::from_millis(300));
    // SIGTERM so pw-record closes the WAV cleanly; the guard only reaps it.
    let mut rec = rec;
    let _ = Command::new("kill").arg(rec.0.id().to_string()).status();
    let _ = rec.0.wait();

    let samples = read_wav_i16(&out);
    let (dry, wet): (Vec<i16>, Vec<i16>) = samples
        .as_chunks::<2>()
        .0
        .iter()
        .map(|f| (f[0], f[1]))
        .unzip();
    // For each click in the dry channel, the loudest wet sample within the
    // next 100 ms is its processed copy.
    let mut delays = Vec::new();
    let mut i = 0;
    while i < dry.len() {
        if dry[i].unsigned_abs() > 4000 {
            let end = (i + 4800).min(wet.len());
            if let Some((d, _)) = wet[i..end]
                .iter()
                .enumerate()
                .max_by_key(|(_, v)| v.unsigned_abs())
            {
                delays.push(d);
            }
            i += 6000;
        } else {
            i += 1;
        }
    }
    assert!(delays.len() >= 4, "found too few clicks: {delays:?}");
    delays.sort_unstable();
    let median = delays[delays.len() / 2];
    // `pw-play` and `pw-record` are async nodes (PipeWire >= 1.2): each async
    // link hands over the previous cycle's buffer. The dry path has one such
    // hop (pw-play -> pw-record), the wet path two (pw-play -> stepwave,
    // stepwave-output -> pw-record), so the recording shows one quantum more
    // than stepwave itself adds. A real device is the driver and has no async
    // hop, so that quantum is not part of stepwave's latency.
    let added = median.saturating_sub(quantum);
    eprintln!(
        "measured delays (samples): {delays:?}; median {median}; quantum {quantum}; \
         added by stepwave {added}"
    );
    assert!(
        added.abs_diff(960) <= 48,
        "stepwave adds {added} samples (median {median} - quantum {quantum}); \
         expected the reported 960"
    );
}

/// The current quantum of the driver that `follower` runs under, from
/// `pw-top` (drivers are listed with their followers below them, prefixed
/// with `+`/`=`).
fn driver_quantum_of(follower: &str) -> Option<usize> {
    let out = Command::new("pw-top")
        .args(["-b", "-n", "2"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let mut driver_quantum = None;
    let mut found = None;
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 || cols[0] == "S" {
            continue;
        }
        let name = cols.last().copied().unwrap_or_default();
        let is_follower = line.contains(" + ") || line.contains(" = ");
        if !is_follower {
            driver_quantum = cols[2].parse::<usize>().ok().filter(|&q| q > 0);
        } else if name == follower {
            // Later samples (the second `-n` iteration) overwrite earlier ones.
            found = driver_quantum;
        }
    }
    found
}
