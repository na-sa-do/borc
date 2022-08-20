//! A tree-based implementation of CBOR with extensions.
//!
//! This module parses CBOR into a large data structure which can be explored at will.
//! It is much easier to use than a streaming implementation,
//! but moderately less performant and with much higher memory requirements.
//! It is comparable to DOM in the XML world.

use super::{
	streaming::{Decoder as StreamingDecoder, Event},
	DateTimeDecodeStyle, DecodeExtensionConfig,
};
use crate::errors::DecodeError;
#[cfg(feature = "chrono")]
use chrono::{DateTime, FixedOffset};
use std::io::Read;

/// An item in an extended CBOR data model.
// TODO: implement tags with unknown semantics somehow
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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
	/// A tagged item whose semantics are unknown.
	UnrecognizedTag(u64, Box<Item>),
	/// A CBOR simple value.
	Simple(u8),

	/// A date/time.
	///
	/// This corresponds to tags 0 and 1.
	///  and only appears if the [`Decoder::date_time_style`] extension is set to [`Chrono`](`DateTimeDecodeStyle::Chrono`).
	#[cfg(feature = "chrono")]
	ChronoDateTime(DateTime<FixedOffset>),
}

impl Item {
	/// Interpret a [`Item::Signed`] value.
	///
	/// This is a convenience alias for [`crate::basic::streaming::Event::interpret_signed`].
	pub fn interpret_signed(val: u64) -> i64 {
		Event::interpret_signed(val)
	}

	/// Interpret a [`Item::Signed`] value.
	///
	/// This is a convenience alias for [`crate::basic::streaming::Event::interpret_signed_checked`].
	pub fn interpret_signed_checked(val: u64) -> Option<i64> {
		Event::interpret_signed_checked(val)
	}

	/// Interpret a [`Item::Signed`] value.
	///
	/// This is a convenience alias for [`crate::basic::streaming::Event::interpret_signed_wide`].
	pub fn interpret_signed_wide(val: u64) -> i128 {
		Event::interpret_signed_wide(val)
	}

	/// Create a [`Item::Signed`] or [`Item::Unsigned`] value.
	///
	/// This is a convenience alias for [`crate::basic::streaming::Event::create_signed`],
	/// except that it returns an extended [`Item`] instead.
	pub fn create_signed(val: i64) -> Item {
		match Event::create_signed(val) {
			Event::Unsigned(n) => Self::Unsigned(n),
			Event::Signed(n) => Self::Signed(n),
			_ => unreachable!(),
		}
	}

	/// Create a [`Item::Signed`] or [`Item::Unsigned`] value.
	///
	/// This is a convenience alias for [`crate::basic::streaming::Event::create_signed_wide`],
	/// except that it returns an extended [`Item`] instead.
	pub fn create_signed_wide(val: i128) -> Option<Item> {
		match Event::create_signed_wide(val) {
			Some(Event::Unsigned(n)) => Some(Self::Unsigned(n)),
			Some(Event::Signed(n)) => Some(Self::Signed(n)),
			Some(_) => unreachable!(),
			None => None,
		}
	}
}

include!("forward_config_accessors.in.rs");

/// A tree-building decoder for CBOR with extensions.
#[derive(Debug, Clone, Default)]
pub struct Decoder {
	config: DecodeExtensionConfig,
}

impl Decoder {
	pub fn new() -> Self {
		Default::default()
	}

	forward_config_accessors!(
		DateTimeDecodeStyle,
		date_time_style,
		date_time_style_mut,
		set_date_time_style
	);

	/// Parse some CBOR.
	///
	/// This is just a shortcut for [`Self::decode_from_stream`] which constructs the [`streaming::Decoder`](`StreamingDecoder`) for you.
	pub fn decode(&mut self, source: impl Read) -> Result<Item, DecodeError> {
		match self.decode_from_stream(&mut StreamingDecoder::new(source)) {
			Ok(Some(item)) => Ok(item),
			Ok(None) => Err(DecodeError::Malformed),
			Err(e) => Err(e),
		}
	}

	/// Parse some CBOR from a provided streaming decoder.
	pub fn decode_from_stream(
		&mut self,
		decoder: &mut StreamingDecoder<impl Read>,
	) -> Result<Option<Item>, DecodeError> {
		Ok(Some(match decoder.next_event()? {
			Event::Unsigned(n) => Item::Unsigned(n),
			Event::Signed(n) => Item::Signed(n),
			Event::ByteString(b) => Item::ByteString(b.into_owned()),
			Event::UnknownLengthByteString => {
				let mut buffer: Vec<u8>;
				match decoder.next_event()? {
					Event::ByteString(b) => buffer = b.into_owned(),
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
			Event::TextString(val) => Item::TextString(val.into_owned()),
			Event::UnknownLengthTextString => {
				let mut buffer: String;
				match decoder.next_event()? {
					Event::TextString(b) => buffer = b.into_owned(),
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
			Event::Array(len) => {
				let mut arr = Vec::with_capacity(len.try_into().unwrap_or(usize::MAX));
				for _ in 0..len {
					match self.decode_from_stream(decoder)? {
						None => return Err(DecodeError::Malformed),
						Some(item) => arr.push(item),
					}
				}
				assert_eq!(arr.len(), len as _);
				Item::Array(arr)
			}
			Event::UnknownLengthArray => {
				let mut arr = Vec::new();
				loop {
					match self.decode_from_stream(decoder)? {
						None => break,
						Some(item) => arr.push(item),
					}
				}
				Item::Array(arr)
			}
			Event::Map(len) => {
				let mut map = Vec::with_capacity(len.try_into().unwrap_or(usize::MAX));
				for _ in 0..len {
					let key = match self.decode_from_stream(decoder)? {
						None => return Err(DecodeError::Malformed),
						Some(item) => item,
					};
					let val = match self.decode_from_stream(decoder)? {
						None => return Err(DecodeError::Malformed),
						Some(item) => item,
					};
					map.push((key, val));
				}
				Item::Map(map)
			}
			Event::UnknownLengthMap => {
				let mut map = Vec::new();
				loop {
					let key = match self.decode_from_stream(decoder)? {
						None => break,
						Some(item) => item,
					};
					let val = match self.decode_from_stream(decoder)? {
						None => return Err(DecodeError::Malformed),
						Some(item) => item,
					};
					map.push((key, val));
				}
				Item::Map(map)
			}
			Event::UnrecognizedTag(tag) => match self.decode_from_stream(decoder) {
				Ok(Some(value)) => Item::UnrecognizedTag(tag, Box::new(value)),
				Ok(None) => return Err(DecodeError::Malformed),
				Err(e) => return Err(e),
			},
			Event::Simple(val) => Item::Simple(val),
			Event::Float(val) => Item::Float(val),
			Event::Break => return Ok(None),

			#[cfg(feature = "chrono")]
			Event::ChronoDateTime(dt) => Item::ChronoDateTime(dt),
		}))
	}
}
