//! stepwave core DSP: STFT, ERB band mask, gain smoothing and limiting.

pub mod erb;
pub mod error;
pub mod features;
pub mod gru;
pub mod limiter;
pub mod mask;
pub mod processor;
pub mod profile;
pub mod select;
pub mod smoother;
pub mod stft;
pub mod swm;
pub mod testing;

pub use error::CoreError;
pub use processor::Processor;
pub use profile::Profile;
pub use swm::SwmModel;
