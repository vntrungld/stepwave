//! Python bindings to `stepwave-core` for training: the exact features the plugin computes.

use numpy::ndarray::Array2;
use numpy::{IntoPyArray, PyArray1, PyArray2, PyReadonlyArray1, PyReadonlyArray2};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use stepwave_core::erb::{ErbBands, NUM_BANDS};
use stepwave_core::features::FeatureExtractor;
use stepwave_core::mask::{ExternalMask, StaticEqMask};
use stepwave_core::smoother::GainSmoother;
use stepwave_core::stft::{BINS, HOP, SAMPLE_RATE, WIN};
use stepwave_core::{Processor, Profile};

/// Centre frequencies (Hz) of the 32 ERB bands.
#[pyfunction]
fn erb_centres_hz(py: Python<'_>) -> Bound<'_, PyArray1<f32>> {
    ErbBands::new().centres_hz().to_vec().into_pyarray(py)
}

/// Per-hop ERB band energies (dB) of the mid signal, shape (frames, 32).
#[pyfunction]
fn band_energies_db<'py>(
    py: Python<'py>,
    left: PyReadonlyArray1<'py, f32>,
    right: PyReadonlyArray1<'py, f32>,
) -> PyResult<Bound<'py, PyArray2<f32>>> {
    let l = left.as_slice()?;
    let r = right.as_slice()?;
    if l.len() != r.len() {
        return Err(PyValueError::new_err(
            "left and right must have the same length",
        ));
    }
    let rows = FeatureExtractor::new().band_energies_db(l, r);
    let flat: Vec<f32> = rows.iter().flatten().copied().collect();
    let array = Array2::from_shape_vec((rows.len(), NUM_BANDS), flat)
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(array.into_pyarray(py))
}

fn gain_rows(gains_db: &PyReadonlyArray2<'_, f32>) -> PyResult<Vec<[f32; NUM_BANDS]>> {
    let g = gains_db.as_array();
    if g.ncols() != NUM_BANDS {
        return Err(PyValueError::new_err(format!(
            "gains_db must have {NUM_BANDS} columns, got {}",
            g.ncols()
        )));
    }
    Ok(g.rows()
        .into_iter()
        .map(|row| {
            let mut out = [0.0; NUM_BANDS];
            for (o, v) in out.iter_mut().zip(row.iter()) {
                *o = *v;
            }
            out
        })
        .collect())
}

/// Left/right output buffers returned by [`apply_band_gains`].
type StereoOutput<'py> = (Bound<'py, PyArray1<f32>>, Bound<'py, PyArray1<f32>>);

/// Run the core pipeline with one given gain row per hop, latency compensated.
#[pyfunction]
#[pyo3(signature = (left, right, gains_db, limiter = true))]
fn apply_band_gains<'py>(
    py: Python<'py>,
    left: PyReadonlyArray1<'py, f32>,
    right: PyReadonlyArray1<'py, f32>,
    gains_db: PyReadonlyArray2<'py, f32>,
    limiter: bool,
) -> PyResult<StereoOutput<'py>> {
    let l = left.as_slice()?;
    let r = right.as_slice()?;
    if l.len() != r.len() {
        return Err(PyValueError::new_err(
            "left and right must have the same length",
        ));
    }
    let rows = gain_rows(&gains_db)?;
    let frames = l.len() / HOP;
    if rows.len() != frames {
        return Err(PyValueError::new_err(format!(
            "gains_db has {} rows, expected {frames} (one per complete hop)",
            rows.len()
        )));
    }
    let mut p = Processor::with_mask(Box::new(ExternalMask::new(rows)), SAMPLE_RATE)
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    p.set_limiter_enabled(limiter);
    let latency = p.latency_samples();
    let mut lo = l.to_vec();
    let mut ro = r.to_vec();
    lo.resize(l.len() + latency, 0.0);
    ro.resize(r.len() + latency, 0.0);
    p.process(&mut lo, &mut ro);
    Ok((
        lo.split_off(latency).into_pyarray(py),
        ro.split_off(latency).into_pyarray(py),
    ))
}

/// The core gain smoother applied to a (frames, 32) gain sequence.
#[pyfunction]
fn smooth_gains_db<'py>(
    py: Python<'py>,
    gains_db: PyReadonlyArray2<'py, f32>,
) -> PyResult<Bound<'py, PyArray2<f32>>> {
    let rows = gain_rows(&gains_db)?;
    let mut smoother = GainSmoother::new();
    let mut row_out = [0.0; NUM_BANDS];
    let mut flat = Vec::with_capacity(rows.len() * NUM_BANDS);
    for row in &rows {
        smoother.process(row, &mut row_out);
        flat.extend_from_slice(&row_out);
    }
    let array = Array2::from_shape_vec((rows.len(), NUM_BANDS), flat)
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(array.into_pyarray(py))
}

/// The 32 static-EQ band gains (dB) a profile's `fallback_eq` + `preamp_db` produce.
#[pyfunction]
fn static_eq_gains_db<'py>(
    py: Python<'py>,
    profile_json: &str,
) -> PyResult<Bound<'py, PyArray1<f32>>> {
    let profile =
        Profile::from_json(profile_json).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let mask = StaticEqMask::from_profile(&profile, &ErbBands::new())
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(mask.gains_db().to_vec().into_pyarray(py))
}

#[pymodule]
fn stepwave_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("SAMPLE_RATE", SAMPLE_RATE)?;
    m.add("WIN", WIN)?;
    m.add("HOP", HOP)?;
    m.add("BINS", BINS)?;
    m.add("NUM_BANDS", NUM_BANDS)?;
    m.add_function(wrap_pyfunction!(erb_centres_hz, m)?)?;
    m.add_function(wrap_pyfunction!(band_energies_db, m)?)?;
    m.add_function(wrap_pyfunction!(apply_band_gains, m)?)?;
    m.add_function(wrap_pyfunction!(smooth_gains_db, m)?)?;
    m.add_function(wrap_pyfunction!(static_eq_gains_db, m)?)?;
    Ok(())
}
