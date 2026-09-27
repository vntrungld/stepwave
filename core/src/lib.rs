//! stepwave core DSP: STFT, ERB band mask, gain smoothing and limiting.

pub mod erb;
pub mod error;
pub mod profile;
pub mod smoother;
pub mod stft;
pub mod testing;

pub use error::CoreError;
pub use profile::Profile;
