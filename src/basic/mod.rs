//! Implementations of the CBOR basic data model.
//! 
//! For an overview of the basic data model, see [RFC 8949 section 2](https://www.rfc-editor.org/rfc/rfc8949.html#name-cbor-data-models).
//! The gist is that the basic data model represents only simple data types and structures,
//! but ignores details like exactly how many bytes are used to contain an integer.
//! However, it does distinguish between integers and floating-point numbers, even if their value is the same.

pub mod streaming;
