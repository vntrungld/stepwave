use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid profile: {0}")]
    InvalidProfile(String),
    #[error("unsupported sample rate {0} Hz (only 48000 Hz is supported)")]
    UnsupportedSampleRate(u32),
    #[error("profile JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid model: {0}")]
    InvalidModel(String),
    #[error("model file: {0}")]
    Io(#[from] std::io::Error),
}
