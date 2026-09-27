//! `.swm` v1 model files (written by `training/model` M3 export): parse and validate.
//! Layout: b"SWM1", u32 LE version, u32 LE header length, JSON header, f32 LE tensors.
//! Loading allocates; it is never called from the audio path.

use std::path::Path;

use serde::Deserialize;

use crate::erb::NUM_BANDS;
use crate::stft::{HOP, SAMPLE_RATE};
use crate::CoreError;

pub const INPUTS: usize = 64;
pub const DENSE: usize = 64;
pub const HIDDEN: usize = 96;
/// Rows of a GRU weight matrix: r, z and n gates stacked.
pub const GATES: usize = 3 * HIDDEN;

const MAGIC: &[u8; 4] = b"SWM1";
const VERSION: u32 = 1;
const PREFIX: usize = 12;
const ARCH: &str = "dense-gru2-dense";
const GATE_ORDER: &str = "rzn";

/// Tensor names and shapes in payload order (PyTorch state_dict names).
const LAYOUT: [(&str, &[usize]); 12] = [
    ("inp.weight", &[DENSE, INPUTS]),
    ("inp.bias", &[DENSE]),
    ("gru1.weight_ih_l0", &[GATES, DENSE]),
    ("gru1.weight_hh_l0", &[GATES, HIDDEN]),
    ("gru1.bias_ih_l0", &[GATES]),
    ("gru1.bias_hh_l0", &[GATES]),
    ("gru2.weight_ih_l0", &[GATES, HIDDEN]),
    ("gru2.weight_hh_l0", &[GATES, HIDDEN]),
    ("gru2.bias_ih_l0", &[GATES]),
    ("gru2.bias_hh_l0", &[GATES]),
    ("out.weight", &[NUM_BANDS, HIDDEN]),
    ("out.bias", &[NUM_BANDS]),
];

/// One GRU layer's weights, row-major, gates stacked (r, z, n).
#[derive(Debug, Clone)]
pub struct GruWeights {
    pub w_ih: Vec<f32>,
    pub w_hh: Vec<f32>,
    pub b_ih: Vec<f32>,
    pub b_hh: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct SwmModel {
    strength_db: f32,
    gain_db_range: (f32, f32),
    mean: [f32; NUM_BANDS],
    std: [f32; NUM_BANDS],
    inp_weight: Vec<f32>,
    inp_bias: Vec<f32>,
    gru1: GruWeights,
    gru2: GruWeights,
    out_weight: Vec<f32>,
    out_bias: Vec<f32>,
}

#[derive(Deserialize)]
struct Header {
    arch: String,
    gate_order: String,
    sizes: Sizes,
    gain_db_range: Vec<f32>,
    sample_rate: u32,
    hop: usize,
    num_bands: usize,
    feature: Feature,
    strength_db: f32,
    tensors: Vec<TensorSpec>,
}

#[derive(Deserialize)]
struct Sizes {
    inputs: usize,
    dense: usize,
    hidden: usize,
    bands: usize,
}

#[derive(Deserialize)]
struct Feature {
    mean: Vec<f32>,
    std: Vec<f32>,
    /// Delta convention name; if present it must be the one `core` implements.
    #[serde(default)]
    delta: Option<String>,
}

/// The only `feature.delta` convention `ModelRunner` implements.
const DELTA_CONVENTION: &str = "raw_diff_over_std";

#[derive(Deserialize)]
struct TensorSpec {
    name: String,
    shape: Vec<usize>,
}

fn invalid(msg: impl Into<String>) -> CoreError {
    CoreError::InvalidModel(msg.into())
}

fn require(ok: bool, msg: impl FnOnce() -> String) -> Result<(), CoreError> {
    if ok {
        Ok(())
    } else {
        Err(invalid(msg()))
    }
}

fn bands(values: &[f32], field: &str) -> Result<[f32; NUM_BANDS], CoreError> {
    let arr: [f32; NUM_BANDS] = values.try_into().map_err(|_| {
        invalid(format!(
            "{field} has {} values, expected {NUM_BANDS}",
            values.len()
        ))
    })?;
    require(arr.iter().all(|v| v.is_finite()), || {
        format!("{field} has non-finite values")
    })?;
    Ok(arr)
}

impl Header {
    fn validate(&self) -> Result<(), CoreError> {
        require(self.arch == ARCH, || {
            format!("arch {:?}, expected {ARCH:?}", self.arch)
        })?;
        require(self.gate_order == GATE_ORDER, || {
            format!("gate_order {:?}, expected {GATE_ORDER:?}", self.gate_order)
        })?;
        let s = &self.sizes;
        require(
            (s.inputs, s.dense, s.hidden, s.bands) == (INPUTS, DENSE, HIDDEN, NUM_BANDS),
            || {
                format!(
                    "sizes {}/{}/{}/{}, expected {INPUTS}/{DENSE}/{HIDDEN}/{NUM_BANDS}",
                    s.inputs, s.dense, s.hidden, s.bands
                )
            },
        )?;
        require(self.sample_rate == SAMPLE_RATE, || {
            format!("sample_rate {}, expected {SAMPLE_RATE}", self.sample_rate)
        })?;
        require(self.hop == HOP, || {
            format!("hop {}, expected {HOP}", self.hop)
        })?;
        require(self.num_bands == NUM_BANDS, || {
            format!("num_bands {}, expected {NUM_BANDS}", self.num_bands)
        })?;
        require(
            self.gain_db_range.len() == 2
                && self.gain_db_range.iter().all(|v| v.is_finite())
                && self.gain_db_range[0] < self.gain_db_range[1]
                && self.gain_db_range[0] <= 0.0
                && 0.0 <= self.gain_db_range[1],
            || {
                format!(
                    "gain_db_range {:?} must be [low, high] with low <= 0 <= high \
                     (so strength 0 always yields unity gain)",
                    self.gain_db_range
                )
            },
        )?;
        require(
            self.strength_db.is_finite() && self.strength_db > 0.0,
            || format!("strength_db {} must be finite and > 0", self.strength_db),
        )?;
        if let Some(delta) = &self.feature.delta {
            require(delta == DELTA_CONVENTION, || {
                format!("feature.delta {delta:?}, expected {DELTA_CONVENTION:?}")
            })?;
        }
        let layout_ok = self.tensors.len() == LAYOUT.len()
            && self
                .tensors
                .iter()
                .zip(LAYOUT)
                .all(|(t, (name, shape))| t.name == name && t.shape == shape);
        require(layout_ok, || {
            "tensors list does not match the swm v1 layout".into()
        })
    }
}

/// Largest `.swm` file `load` will read. Real model files are a few hundred KB;
/// this guards against accidentally (or maliciously) pointing the loader at a
/// huge or unbounded file (e.g. a device node) before any bytes are parsed.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

impl SwmModel {
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        use std::io::Read;

        let mut file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        if len > MAX_FILE_BYTES {
            return Err(invalid(format!(
                "model file is {len} bytes, exceeds the {MAX_FILE_BYTES}-byte (16 MiB) limit"
            )));
        }
        // Read one byte past the cap: a race that grows the file after the metadata
        // check above still gets caught here, without ever buffering the whole thing.
        let mut bytes = Vec::with_capacity(len as usize);
        (&mut file)
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(invalid(format!(
                "model file exceeds the {MAX_FILE_BYTES}-byte (16 MiB) limit"
            )));
        }
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.get(..4) != Some(MAGIC.as_slice()) {
            let got = &bytes[..bytes.len().min(4)];
            return Err(invalid(format!("bad magic {got:?}, expected {MAGIC:?}")));
        }
        let word = |at: usize| {
            bytes
                .get(at..at + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .ok_or_else(|| invalid("truncated before the header"))
        };
        let version = word(4)?;
        require(version == VERSION, || {
            format!("format version {version}, expected {VERSION}")
        })?;
        let head_end = PREFIX
            .checked_add(word(8)? as usize)
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| invalid("header truncated"))?;
        let header: Header = serde_json::from_slice(&bytes[PREFIX..head_end])
            .map_err(|e| invalid(format!("header is not valid JSON: {e}")))?;
        header.validate()?;
        let mean = bands(&header.feature.mean, "feature.mean")?;
        let std = bands(&header.feature.std, "feature.std")?;
        require(std.iter().all(|v| *v > 0.0), || {
            "feature.std values must be > 0".into()
        })?;

        let payload = &bytes[head_end..];
        let expected: usize = LAYOUT
            .iter()
            .map(|(_, shape)| shape.iter().product::<usize>() * 4)
            .sum();
        require(payload.len() >= expected, || {
            format!(
                "payload truncated: {} bytes, expected {expected}",
                payload.len()
            )
        })?;
        require(payload.len() == expected, || {
            format!(
                "{} trailing bytes after the payload",
                payload.len() - expected
            )
        })?;

        let mut values = payload
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b));
        let mut tensors = Vec::with_capacity(LAYOUT.len());
        for (name, shape) in LAYOUT {
            let t: Vec<f32> = values.by_ref().take(shape.iter().product()).collect();
            require(t.iter().all(|v| v.is_finite()), || {
                format!("tensor {name} has non-finite values")
            })?;
            tensors.push(t);
        }
        let mut t = tensors.into_iter();
        let mut next = || t.next().unwrap_or_default();
        Ok(Self {
            strength_db: header.strength_db,
            gain_db_range: (header.gain_db_range[0], header.gain_db_range[1]),
            mean,
            std,
            inp_weight: next(),
            inp_bias: next(),
            gru1: GruWeights {
                w_ih: next(),
                w_hh: next(),
                b_ih: next(),
                b_hh: next(),
            },
            gru2: GruWeights {
                w_ih: next(),
                w_hh: next(),
                b_ih: next(),
                b_hh: next(),
            },
            out_weight: next(),
            out_bias: next(),
        })
    }

    pub fn strength_db(&self) -> f32 {
        self.strength_db
    }
    pub fn gain_db_range(&self) -> (f32, f32) {
        self.gain_db_range
    }
    pub fn mean(&self) -> &[f32; NUM_BANDS] {
        &self.mean
    }
    pub fn std(&self) -> &[f32; NUM_BANDS] {
        &self.std
    }
    pub fn inp_weight(&self) -> &[f32] {
        &self.inp_weight
    }
    pub fn inp_bias(&self) -> &[f32] {
        &self.inp_bias
    }
    pub fn gru1(&self) -> &GruWeights {
        &self.gru1
    }
    pub fn gru2(&self) -> &GruWeights {
        &self.gru2
    }
    pub fn out_weight(&self) -> &[f32] {
        &self.out_weight
    }
    pub fn out_bias(&self) -> &[f32] {
        &self.out_bias
    }
}
