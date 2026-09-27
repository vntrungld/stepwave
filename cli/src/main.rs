//! Offline stepwave processing: `stepwave-cli in.wav out.wav --profile cs2`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Parser;
use stepwave_core::mask::UnityMask;
use stepwave_core::stft::{HOP, SAMPLE_RATE};
use stepwave_core::{Processor, Profile};

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
    /// Run the pipeline with a unity mask, for A/B listening
    #[arg(long)]
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

    let mut processor = if args.bypass {
        Processor::with_mask(Box::new(UnityMask), SAMPLE_RATE)?
    } else {
        Processor::new(&profile, SAMPLE_RATE)?
    };

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
