use std::path::Path;
use std::process::{Command, Output};

use stepwave_core::testing::white_noise;

const BIN: &str = env!("CARGO_BIN_EXE_stepwave-cli");
const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
const CS2: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../profiles/cs2.json");

fn write_wav(path: &Path, channels: u16, sample_rate: u32, bits: u16, frames: usize) {
    write_wav_amplitude(path, channels, sample_rate, bits, frames, 0.3);
}

fn write_wav_amplitude(
    path: &Path,
    channels: u16,
    sample_rate: u32,
    bits: u16,
    frames: usize,
    amplitude: f32,
) {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: bits,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let scale = ((1i64 << (bits - 1)) - 1) as f32;
    for s in white_noise(5, frames * channels as usize, amplitude) {
        w.write_sample((s * scale) as i32).unwrap();
    }
    w.finalize().unwrap();
}

/// Parses "peak in X dBFS, out Y dBFS" from stdout, returning (X, Y).
fn parse_peaks(stdout: &str) -> (f32, f32) {
    let line = stdout
        .lines()
        .find(|l| l.starts_with("peak in"))
        .unwrap_or_else(|| panic!("no peak line in stdout: {stdout:?}"));
    let nums: Vec<f32> = line
        .split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    (nums[0], nums[1])
}

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .current_dir(REPO)
        .args(args)
        .output()
        .unwrap()
}

fn assert_ok(out: &Output) {
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn processes_stereo_16_bit_file() {
    let dir = tempfile::tempdir().unwrap();
    let (i, o) = (dir.path().join("in.wav"), dir.path().join("out.wav"));
    write_wav(&i, 2, 48_000, 16, 4_800);
    let out = run(&[i.to_str().unwrap(), o.to_str().unwrap(), "--profile", "cs2"]);
    assert_ok(&out);
    let reader = hound::WavReader::open(&o).unwrap();
    let s = reader.spec();
    assert_eq!(
        (
            s.channels,
            s.sample_rate,
            s.bits_per_sample,
            s.sample_format
        ),
        (2, 48_000, 32, hound::SampleFormat::Float)
    );
    assert_eq!(reader.duration(), 4_800);
    assert!(String::from_utf8_lossy(&out.stdout).contains("per 10 ms frame"));
}

#[test]
fn mono_24_bit_becomes_stereo() {
    let dir = tempfile::tempdir().unwrap();
    let (i, o) = (dir.path().join("in.wav"), dir.path().join("out.wav"));
    write_wav(&i, 1, 48_000, 24, 4_800);
    assert_ok(&run(&[
        i.to_str().unwrap(),
        o.to_str().unwrap(),
        "--profile",
        CS2,
    ]));
    let reader = hound::WavReader::open(&o).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.duration(), 4_800);
}

#[test]
fn handles_empty_and_very_short_files() {
    for frames in [0, 100] {
        let dir = tempfile::tempdir().unwrap();
        let (i, o) = (dir.path().join("in.wav"), dir.path().join("out.wav"));
        write_wav(&i, 2, 48_000, 16, frames);
        assert_ok(&run(&[
            i.to_str().unwrap(),
            o.to_str().unwrap(),
            "--profile",
            "cs2",
        ]));
        assert_eq!(
            hound::WavReader::open(&o).unwrap().duration() as usize,
            frames
        );
    }
}

#[test]
fn bypass_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (i, o) = (dir.path().join("in.wav"), dir.path().join("out.wav"));
    write_wav(&i, 2, 48_000, 16, 4_800);
    assert_ok(&run(&[
        i.to_str().unwrap(),
        o.to_str().unwrap(),
        "--profile",
        "cs2",
        "--bypass",
    ]));
}

#[test]
fn rejects_44_1_khz_input() {
    let dir = tempfile::tempdir().unwrap();
    let (i, o) = (dir.path().join("in.wav"), dir.path().join("out.wav"));
    write_wav(&i, 2, 44_100, 16, 4_410);
    let out = run(&[i.to_str().unwrap(), o.to_str().unwrap(), "--profile", "cs2"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("48000"));
}

#[test]
fn reports_input_peak_not_processed_output_peak() {
    // A near-full-scale input (peak ~0 dBFS) run through the cs2 profile: the
    // profile's fallback EQ + preamp never boosts a band above 0 dB, so the
    // processed output is measurably quieter than the input. `peak in` must
    // reflect the *original* samples, not the buffer after `Processor::process`
    // has overwritten it in place.
    let dir = tempfile::tempdir().unwrap();
    let (i, o) = (dir.path().join("in.wav"), dir.path().join("out.wav"));
    write_wav_amplitude(&i, 2, 48_000, 16, 4_800, 0.9999);
    let out = run(&[i.to_str().unwrap(), o.to_str().unwrap(), "--profile", "cs2"]);
    assert_ok(&out);
    let (peak_in, peak_out) = parse_peaks(&String::from_utf8_lossy(&out.stdout));
    assert!(
        peak_in > -0.5,
        "peak in should be near 0 dBFS, got {peak_in}"
    );
    assert!(
        peak_out <= -1.0,
        "peak out should be attenuated by the profile, got {peak_out}"
    );
}

#[test]
fn missing_profile_names_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let (i, o) = (dir.path().join("in.wav"), dir.path().join("out.wav"));
    write_wav(&i, 2, 48_000, 16, 480);
    let out = run(&[
        i.to_str().unwrap(),
        o.to_str().unwrap(),
        "--profile",
        "nope",
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("profiles/nope.json"));
}
