//! Python bindings to `stepwave-core` for training: the exact features the plugin computes.

use numpy::ndarray::Array2;
use numpy::{IntoPyArray, PyArray1, PyArray2, PyReadonlyArray1};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use stepwave_core::erb::{ErbBands, NUM_BANDS};
use stepwave_core::features::FeatureExtractor;
use stepwave_core::stft::{BINS, HOP, SAMPLE_RATE, WIN};

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

#[pymodule]
fn stepwave_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("SAMPLE_RATE", SAMPLE_RATE)?;
    m.add("WIN", WIN)?;
    m.add("HOP", HOP)?;
    m.add("BINS", BINS)?;
    m.add("NUM_BANDS", NUM_BANDS)?;
    m.add_function(wrap_pyfunction!(erb_centres_hz, m)?)?;
    m.add_function(wrap_pyfunction!(band_energies_db, m)?)?;
    Ok(())
}
