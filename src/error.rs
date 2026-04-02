use thiserror::Error;

/// Core error type for the `eruditio` crate.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EruditioError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Format error: {0}")]
    Format(String),

    #[error("Parsing error: {0}")]
    Parse(String),

    #[error("Unsupported feature: {0}")]
    Unsupported(String),

    #[error("Missing metadata: {0}")]
    MissingMetadata(String),

    #[error("Invalid metadata: {0}")]
    InvalidMetadata(String),

    #[error("Compression error: {0}")]
    Compression(String),

    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("Conversion error: {0}")]
    Conversion(String),

    #[error("Validation error: {0}")]
    Validation(String),
}

/// A specialized `Result` type for the `eruditio` crate.
pub type Result<T> = std::result::Result<T, EruditioError>;
