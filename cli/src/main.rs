//! Offline stepwave processing: `stepwave-cli in.wav out.wav --profile cs2 --mode auto`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Parser;
use stepwave_core::mask::UnityMask;
use stepwave_core::stft::{HOP, SAMPLE_RATE};
use stepwave_core::{Processor, Profile, SwmModel};

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
    let model_path = resolve_model(args.model.as_deref(), &profile, &profile_path);
    let (mut processor, used) = build_processor(mode, &profile, &model_path)?;

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
fn resolve_model(arg: Option<&Path>, profile: &Profile, profile_path: &Path) -> PathBuf {
    if let Some(path) = arg {
        return path.to_path_buf();
    }
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

fn build_processor(
    mode: Mode,
    profile: &Profile,
    model_path: &Path,
) -> Result<(Processor, &'static str)> {
    let eq = || Processor::new(profile, SAMPLE_RATE);
    Ok(match mode {
        Mode::Bypass => (
            Processor::with_mask(Box::new(UnityMask), SAMPLE_RATE)?,
            "bypass",
        ),
        Mode::Eq => (eq()?, "eq"),
        Mode::Model => {
            let model = SwmModel::load(model_path)
                .with_context(|| format!("loading model {}", model_path.display()))?;
            (
                Processor::with_model(profile, &model, SAMPLE_RATE)?,
                "model",
            )
        }
        Mode::Auto => match SwmModel::load(model_path) {
            Ok(model) => (
                Processor::with_model(profile, &model, SAMPLE_RATE)?,
                "model",
            ),
            Err(err) => {
                eprintln!(
                    "warning: model {} unusable ({err}); using static EQ",
                    model_path.display()
                );
                (eq()?, "eq")
            }
        },
    })
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
