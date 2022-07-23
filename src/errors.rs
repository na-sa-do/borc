use thiserror::Error;

/// Errors that can occur when decoding CBOR.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DecodeError {
    #[error("malformed CBOR")]
    Malformed,
    #[error("incomplete or excess data")]
    IncompleteOrExcess,
}
