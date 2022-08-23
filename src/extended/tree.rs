//! A tree-based implementation of CBOR with extensions.
//!
//! This module parses CBOR into a large data structure which can be explored at will.
//! It is much easier to use than a streaming implementation,
//! but moderately less performant and with much higher memory requirements.
//! It is comparable to DOM in the XML world.

use super::{
	streaming::{Decoder as StreamingDecoder, Encoder as StreamingEncoder, Event},
	BignumDecodeStyle, DateTimeDecodeStyle, DateTimeEncodeStyle, DecodeExtensionConfig,
	EncodeExtensionConfig,
};
use crate::errors::{DecodeError, EncodeError};
#[cfg(feature = "chrono")]
use chrono::{DateTime, FixedOffset};
#[cfg(feature = "num-bigint")]
use num_bigint::BigInt;
use std::{
	borrow::Cow,
	io::{Read, Write},
};

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
	/// and only appears if the [`Decoder::date_time_style`] extension is set to [`Chrono`](`DateTimeDecodeStyle::Chrono`).
	#[cfg(feature = "chrono")]
	ChronoDateTime(DateTime<FixedOffset>),
	/// A bignum.
	///
	/// This corresponds to tags 2 and 3,
	/// and only appears if the [`Decoder::bignum_style`] extension is set to [`Num`](`BignumDecodeStyle::Num`).
	#[cfg(feature = "num-bigint")]
	NumBigInt(BigInt),
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
		set_date_time_style,
		"the way date-times are decoded."
	);

	forward_config_accessors!(
		BignumDecodeStyle,
		bignum_style,
		bignum_style_mut,
		set_bignum_style,
		"the way bignums are decoded."
	);

	/// Parse some CBOR.
	///
	/// This is just a shortcut for [`Self::decode_from_stream`]
	/// which constructs the [`streaming::Decoder`](`StreamingDecoder`) for you
	/// and converts [`None`]s into [`DecodeError::Malformed`]s.
	pub fn decode(&mut self, source: impl Read) -> Result<Item, DecodeError> {
		match self.decode_from_stream(&mut StreamingDecoder::new(source)) {
			Ok(Some(item)) => Ok(item),
			Ok(None) => Err(DecodeError::Malformed),
			Err(e) => Err(e),
		}
	}

	/// Parse some CBOR from a provided streaming decoder.
	///
	/// If this returns `Ok(None)`, it means that the first thing it encountered was a break (`0xFF`).
	/// This may or may not be acceptable depending on the situation,
	/// so `decode_from_stream` doesn't count it as a failure.
	pub fn decode_from_stream(
		&mut self,
		decoder: &mut StreamingDecoder<impl Read>,
	) -> Result<Option<Item>, DecodeError> {
		Ok(Some(match decoder.next_event()? {
			Event::Unsigned(n) => Item::Unsigned(n),
			Event::Signed(n) => Item::Signed(n),
			Event::ByteString(b) => Item::ByteString(b.into_owned()),
			Event::UnknownLengthByteString => {
				Item::ByteString(decoder.read_unknown_length_byte_string_body()?.into_owned())
			}
			Event::TextString(val) => Item::TextString(val.into_owned()),
			Event::UnknownLengthTextString => {
				Item::TextString(decoder.read_unknown_length_text_string_body()?.into_owned())
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
			#[cfg(feature = "num-bigint")]
			Event::NumBigInt(n) => Item::NumBigInt(n),
		}))
	}
}

/// A tree-walking encoder for CBOR with extensions.
#[derive(Debug, Clone, Default)]
pub struct Encoder {
	config: EncodeExtensionConfig,
}

impl Encoder {
	pub fn new() -> Self {
		Default::default()
	}

	forward_config_accessors!(
		DateTimeEncodeStyle,
		date_time_style,
		date_time_style_mut,
		set_date_time_style,
		"the way date-times are encoded."
	);

	/// Encode some CBOR.
	///
	/// This is just a shortcut for [`Self::encode_to_stream`] which constructs the [`streaming::Encoder`](`crate::basic::streaming::Encoder`) for you.
	pub fn encode(&mut self, cbor: &Item, dest: impl Write) -> Result<(), EncodeError> {
		self.encode_to_stream(cbor, &mut StreamingEncoder::new(dest))
	}

	/// Encode some CBOR to a provided streaming encoder.
	pub fn encode_to_stream(
		&mut self,
		cbor: &Item,
		encoder: &mut StreamingEncoder<impl Write>,
	) -> Result<(), EncodeError> {
		match cbor {
			Item::Unsigned(n) => encoder.feed_event(Event::Unsigned(*n)),
			Item::Signed(n) => encoder.feed_event(Event::Signed(*n)),
			Item::Float(f) => encoder.feed_event(Event::Float(*f)),
			Item::ByteString(bytes) => {
				encoder.feed_event(Event::ByteString(Cow::Borrowed(&*bytes)))
			}
			Item::TextString(text) => encoder.feed_event(Event::TextString(Cow::Borrowed(&*text))),
			Item::Array(arr) => {
				encoder.feed_event(Event::Array(
					arr.len().try_into().expect("I'm on a 128-bit system? Wow."),
				))?;
				for item in arr.iter() {
					self.encode_to_stream(item, encoder)?;
				}
				Ok(())
			}
			Item::Map(map) => {
				encoder.feed_event(Event::Map(
					map.len().try_into().expect("I'm on a 128-bit system? Wow."),
				))?;
				for (key, val) in map.iter() {
					self.encode_to_stream(key, encoder)?;
					self.encode_to_stream(val, encoder)?;
				}
				Ok(())
			}
			Item::UnrecognizedTag(tag, val) => {
				encoder.feed_event(Event::UnrecognizedTag(*tag))?;
				self.encode_to_stream(val, encoder)
			}
			Item::Simple(n) => encoder.feed_event(Event::Simple(*n)),

			#[cfg(feature = "chrono")]
			Item::ChronoDateTime(dt) => encoder.feed_event(Event::ChronoDateTime(*dt)),
			#[cfg(feature = "num-bigint")]
			Item::NumBigInt(n) => encoder.feed_event(Event::NumBigInt(n.clone())),
		}
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use std::io::Cursor;

	macro_rules! decode_test {
		($in:expr => $out:pat if $guard:expr) => {
			let input = $in;
			match Decoder::new().decode(Cursor::new(&input)) {
				$out if $guard => (),
				other => panic!("{:X?} => {:?}", input, other),
			}
		};
		($in:expr => $out:pat) => {
			decode_test!($in => $out if true);
		}
	}

	macro_rules! encode_test {
		($in:expr => $out:expr) => {
			let input = $in;
			let output = $out;
			let mut buf = Vec::with_capacity(output.len());
			Encoder::new()
				.encode(&input, Cursor::new(&mut buf))
				.unwrap();
			assert_eq!(buf, output, "{:?} => {:X?}", input, buf);
		};
	}

	#[test]
	fn decode_bytes_segmented() {
		decode_test!(b"\x5F\x42ab\x42cd\xFF" => Ok(Item::ByteString(b)) if b == b"abcd");
		decode_test!(b"\x5F\xFF" => Ok(Item::ByteString(b)) if b == b"");
	}

	#[test]
	fn decode_bytes_segmented_wrong() {
		decode_test!(b"\x5F\x00" => Err(DecodeError::Malformed));
	}

	#[test]
	fn decode_text_segmented() {
		decode_test!(b"\x7F\x62ab\x62cd\xFF" => Ok(Item::TextString(t)) if t == "abcd");
		decode_test!(b"\x7F\xFF" => Ok(Item::TextString(t)) if t == "");
	}

	#[test]
	fn decode_text_segmented_wrong() {
		decode_test!(b"\x7F\x00" => Err(DecodeError::Malformed));
	}

	#[test]
	fn decode_array() {
		decode_test!(b"\x80" => Ok(Item::Array(v)) if v.is_empty());
		decode_test!(b"\x84\x00\x01\x02\x03" => Ok(Item::Array(v)) if v == [0,1,2,3].map(|x| Item::Unsigned(x)));
	}

	#[test]
	fn decode_array_segmented() {
		decode_test!(b"\x9F\xFF" => Ok(Item::Array(v)) if v.is_empty());
		decode_test!(b"\x9F\x00\x00\xFF" => Ok(Item::Array(v)) if v == vec![Item::Unsigned(0); 2]);
	}

	#[test]
	fn encode_array() {
		encode_test!(
			Item::Array(vec![
				Item::Unsigned(0),
				Item::Unsigned(1),
			])
			=> b"\x82\x00\x01"
		);
	}

	#[test]
	fn decode_map() {
		decode_test!(b"\xA0" => Ok(Item::Map(m)) if m.is_empty());
		decode_test!(b"\xA1\x00\x01" => Ok(Item::Map(m)) if m == [(Item::Unsigned(0), Item::Unsigned(1))]);
	}

	#[test]
	fn decode_map_segmented() {
		decode_test!(b"\xBF\xFF" => Ok(Item::Map(m)) if m.is_empty());
		decode_test!(b"\xBF\x00\x01\xFF" => Ok(Item::Map(m)) if m == [(Item::Unsigned(0), Item::Unsigned(1))]);
	}

	#[test]
	fn decode_map_wrong() {
		decode_test!(b"\xA1\x00" => Err(DecodeError::Insufficient));
		decode_test!(b"\xBF\x00\xFF" => Err(DecodeError::Malformed));
	}

	#[test]
	fn encode_map() {
		encode_test!(
			Item::Map(vec![
				(Item::Unsigned(0), Item::Unsigned(1)),
				(Item::Unsigned(2), Item::Unsigned(3)),
			])
			=> b"\xA2\x00\x01\x02\x03"
		);
	}
}
