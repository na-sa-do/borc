//! A tree-based implementation of the CBOR basic data model.
//!
//! This module parses CBOR into a large data structure which can be explored at will.
//! It is much easier to use than a streaming implementation,
//! but moderately less performant and with much higher memory requirements.
//! It is comparable to DOM in the XML world.

use crate::{
	basic::streaming::{Decoder as StreamingDecoder, Event},
	errors::DecodeError,
};
use std::io::Read;

/// An item in the CBOR basic data model.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
	/// An unsigned integer.
	Unsigned(u64),
	/// A signed integer in a slightly odd representation.
	///
	/// The actual value of the integer is -1 minus the provided value.
	/// Some integers that can be CBOR encoded underflow [`i64`].
	/// Use one of the `interpret_signed` associated functions to resolve this.
	Signed(u64),
	/// A floating-point number.
	Float(f64),
	/// A byte string.
	ByteString(Vec<u8>),
	/// A text string.
	TextString(String),
	/// An array.
	Array(Vec<Item>),
	/// A map.
	///
	/// This uses a [`Vec`] as its actual implementation because [`Item`] can implement neither [`Ord`] nor [`Hash`] (nor even [`Eq`]).
	Map(Vec<(Item, Item)>),
	/// A tagged item.
	Tag(u64, Box<Item>),
	/// A CBOR simple value.
	Simple(u8),
}

impl Item {
	/// Interpret a [`Item::Signed`] value.
	///
	/// This is a convenience alias for [`super::streaming::Event::interpret_signed`].
	pub fn interpret_signed(val: u64) -> i64 {
		super::streaming::Event::interpret_signed(val)
	}

	/// Interpret a [`Item::Signed`] value.
	///
	/// This is a convenience alias for [`super::streaming::Event::interpret_signed_checked`].
	pub fn interpret_signed_checked(val: u64) -> Option<i64> {
		super::streaming::Event::interpret_signed_checked(val)
	}

	/// Interpret a [`Item::Signed`] value.
	///
	/// This is a convenience alias for [`super::streaming::Event::interpret_signed_wide`].
	pub fn interpret_signed_wide(val: u64) -> i128 {
		super::streaming::Event::interpret_signed_wide(val)
	}

	/// Create a [`Item::Signed`] or [`Item::Unsigned`] value.
	///
	/// This is a convenience alias for [`super::streaming::Event::create_signed`],
	/// except that it returns an [`Item`] instead.
	pub fn create_signed(val: i64) -> Item {
		match super::streaming::Event::create_signed(val) {
			super::streaming::Event::Unsigned(n) => Self::Unsigned(n),
			super::streaming::Event::Signed(n) => Self::Signed(n),
			_ => unreachable!(),
		}
	}

	/// Create a [`Item::Signed`] or [`Item::Unsigned`] value.
	///
	/// This is a convenience alias for [`super::streaming::Event::create_signed_wide`],
	/// except that it returns an [`Item`] instead.
	pub fn create_signed_wide(val: i128) -> Option<Item> {
		match super::streaming::Event::create_signed_wide(val) {
			Some(super::streaming::Event::Unsigned(n)) => Some(Self::Unsigned(n)),
			Some(super::streaming::Event::Signed(n)) => Some(Self::Signed(n)),
			Some(_) => unreachable!(),
			None => None,
		}
	}
}

/// A tree-building decoder for the CBOR basic data model.
#[derive(Debug, Clone, Default)]
pub struct Decoder {}

impl Decoder {
	pub fn new() -> Self {
		Default::default()
	}

	/// Parse some CBOR.
	pub fn decode(self, source: impl Read) -> Result<Item, DecodeError> {
		match self.decode_inner(&mut StreamingDecoder::new(source)) {
			Ok(Some(item)) => Ok(item),
			Ok(None) => Err(DecodeError::InvalidBreak),
			Err(e) => Err(e),
		}
	}

	fn decode_inner(
		&self,
		decoder: &mut StreamingDecoder<impl Read>,
	) -> Result<Option<Item>, DecodeError> {
		Ok(Some(match decoder.next_event()? {
			Event::Unsigned(val) => Item::Unsigned(val),
			Event::Signed(val) => Item::Signed(val),
			Event::ByteString(val) => Item::ByteString(val),
			Event::UnknownLengthByteString => {
				let mut buffer: Vec<u8>;
				match decoder.next_event()? {
					Event::ByteString(b) => buffer = b,
					Event::Break => buffer = Vec::new(),
					_ => return Err(DecodeError::Malformed),
				}
				loop {
					match decoder.next_event()? {
						Event::ByteString(b) => buffer.extend_from_slice(&b),
						Event::Break => return Ok(Some(Item::ByteString(buffer))),
						_ => return Err(DecodeError::Malformed),
					}
				}
			}
			Event::TextString(val) => Item::TextString(val),
			Event::UnknownLengthTextString => {
				let mut buffer: String;
				match decoder.next_event()? {
					Event::TextString(b) => buffer = b,
					Event::Break => buffer = String::new(),
					_ => return Err(DecodeError::Malformed),
				}
				loop {
					match decoder.next_event()? {
						Event::TextString(b) => {
							let mut buffer2 = buffer.into_bytes();
							buffer2.extend_from_slice(b.as_bytes());
							// Safe because they were strings just a moment ago.
							// Concatenating UTF-8 strings always produces valid UTF-8.
							buffer = unsafe { String::from_utf8_unchecked(buffer2) };
						}
						Event::Break => return Ok(Some(Item::TextString(buffer))),
						_ => return Err(DecodeError::Malformed),
					}
				}
			}
			Event::Array(len) => todo!(),
			Event::UnknownLengthArray => todo!(),
			Event::Map(len) => todo!(),
			Event::UnknownLengthMap => todo!(),
			Event::Tag(tag) => todo!(),
			Event::Simple(val) => Item::Simple(val),
			Event::Float(val) => Item::Float(val),
			Event::Break => return Ok(None),
		}))
	}
}

#[cfg(test)]
mod test {
	use super::*;

	macro_rules! decode_test {
		($in:expr => $out:pat if $guard:expr) => {
			let input = $in;
			match Decoder::new().decode(std::io::Cursor::new(&input)) {
				$out if $guard => (),
				other => panic!("{:X?} => {:?}", input, other),
			}
		};
		($in:expr => $out:pat) => {
			decode_test!($in => $out if true);
		}
	}

	#[test]
	fn decode_bytes_segmented() {
		decode_test!(b"\x5F\x42ab\x42cd\xFF" => Ok(Item::ByteString(b)) if b == b"abcd");
	}

	#[test]
	fn decode_bytes_segmented_wrong() {
		decode_test!(b"\x5F\x00" => Err(DecodeError::Malformed));
	}

	#[test]
	fn decode_text_segmented() {
		decode_test!(b"\x7F\x62ab\x62cd\xFF" => Ok(Item::TextString(t)) if t == "abcd");
	}

	#[test]
	fn decode_text_segmented_wrong() {
		decode_test!(b"\x7F\x00" => Err(DecodeError::Malformed));
	}
}
