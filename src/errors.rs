use std::string::FromUtf8Error;
use thiserror::Error;

/// Errors that can occur when decoding CBOR.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DecodeError {
	#[error("malformed CBOR")]
	Malformed,
	#[error("excess data")]
	Excess,
	#[error("insufficient data")]
	Insufficient,
	#[error("invalid UTF-8: {0}")]
	InvalidUtf8(#[from] FromUtf8Error),
	#[error("{0}")]
	IoError(#[from] std::io::Error),
}

/// Errors that can occur when encoding CBOR.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EncodeError {
	#[error("excess data")]
	Excess,
	#[error("insufficient data")]
	Insufficient,
	#[error("{0}")]
	IoError(#[from] std::io::Error),
	#[error("break at invalid time")]
	InvalidBreak,
}
