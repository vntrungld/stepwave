use std::path::Path;
use std::process::{Command, Output};

use stepwave_core::testing::white_noise;

const BIN: &str = env!("CARGO_BIN_EXE_stepwave-cli");
const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
const CS2: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../profiles/cs2.json");

fn write_wav(path: &Path, channels: u16, sample_rate: u32, bits: u16, frames: usize) {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: bits,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let scale = ((1i64 << (bits - 1)) - 1) as f32;
    for s in white_noise(5, frames * channels as usize, 0.3) {
        w.write_sample((s * scale) as i32).unwrap();
    }
    w.finalize().unwrap();
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
