//! CBOR done right.
//! 
//! borc is an implementation of [CBOR, the Concise Binary Object Representation](https://cbor.io),
//! as defined in [RFC 8949](https://www.rfc-editor.org/rfc/rfc8949.html).
//! CBOR is a simple but powerful data format whose potential has so far gone underused,
//! especially in the Rust space (due to the dominance of Serde, which does not and probably cannot support its full feature set).
//! borc aims to be a one-stop shop for CBOR and CBOR-based protocols.
//! 
//! This crate provides the following interfaces:
//! 
//! - [`basic::streaming`], a streaming encoder/decoder pair that is lightweight but tricky to use
//! - [`basic::tree`], a tree-based encoder/decoder pair that is easier to use but holds the entire CBOR structure in memory at once
//! - [`extended::streaming`], a streaming encoder/decoder pair with special handling of some tags
//! 
//! The obviously missing `extended::tree` is not yet implemented.
//! 
//! For details on the extensions currently implemented, check the documentation for the [`extended`] module.

pub mod basic;
pub mod errors;
pub mod extended;
