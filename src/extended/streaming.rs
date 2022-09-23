//! A streaming implementation of CBOR with extensions.
//!
//! This module allows you to parse and generate CBOR very efficiently,
//! at the expense of being somewhat difficult to actually use.
//! It models CBOR as a series of [`Event`]s, which are not always full data items.
//! In this way, it is comparable to SAX in the XML world.

use crate::{
	basic::streaming::{Decoder as BasicDecoder, Encoder as BasicEncoder, Event as BasicEvent},
	errors::{DecodeError, EncodeError},
	extended::{
		BignumDecodeStyle, DateTimeDecodeStyle, DateTimeEncodeStyle, DecodeExtensionConfig,
		EncodeExtensionConfig,
	},
};
use std::{
	borrow::Cow,
	collections::VecDeque,
	io::{Read, Write},
};

#[cfg(feature = "chrono")]
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
#[cfg(feature = "num-bigint")]
use num_bigint::{BigInt, Sign, ToBigInt};

/// An event encountered while decoding or encoding CBOR using a streaming extended implementation.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Event<'a> {
	/// An unsigned integer.
	Unsigned(u64),
	/// A signed integer in a slightly odd representation.
	///
	/// The actual value of the integer is -1 minus the provided value.
	/// Some integers that can be CBOR encoded underflow [`i64`].
	/// Use one of the `interpret_signed` associated functions to resolve this.
	Signed(u64),
	/// A byte string.
	ByteString(Cow<'a, [u8]>),
	/// The start of a byte string whose length is unknown.
	///
	/// After this event come a series of `ByteString` events, followed by a `Break`.
	/// To get the true value of the byte string, concatenate the `ByteString` events together.
	UnknownLengthByteString,
	/// A text string.
	TextString(Cow<'a, str>),
	/// The start of a text string whose length is unknown.
	///
	/// After this event come a series of `TextString` events, followed by a `Break`.
	/// To get the true value of the text string, concatenate the `TextString` events together.
	UnknownLengthTextString,
	/// The start of an array with a known length.
	Array(u64),
	/// The start of an array whose length is unknown.
	///
	/// After this event come a series of events representing the items in the array.
	/// The array ends at the matching `Break` event.
	UnknownLengthArray,
	/// The start of a map with a known length.
	///
	/// Note that the actual number of sub-items is _twice_ the length given.
	/// The first in each pair is a key, and the second is the value.
	Map(u64),
	/// The start of a map with an unknown length.
	UnknownLengthMap,
	/// Additional type information for the next CBOR item.
	UnrecognizedTag(u64),
	/// A CBOR simple value.
	///
	/// Most notably, simple values 20, 21, 22, and 23 represent false, true, null, and undefined, respectively.
	Simple(u8),
	/// A floating-point number.
	Float(f64),
	/// The end of an unknown-length item.
	Break,

	/// A date/time.
	///
	/// This corresponds to tags 0 and 1.
	/// and only appears if the [`Decoder::date_time_style`] extension is set to [`Chrono`](`DateTimeDecodeStyle::Chrono`).
	#[cfg(feature = "chrono")]
	ChronoDateTime(DateTime<FixedOffset>),
	/// A non-negative bignum.
	///
	/// This corresponds to tags 2 and 3,
	/// and only appears if the [`Decoder::bignum_style`] extension is set to [`Num`](`BignumDecodeStyle::Num`).
	#[cfg(feature = "num-bigint")]
	NumBigInt(BigInt),
}

impl Event<'_> {
	/// Convert this [`Event`] to an owned value.
	pub fn into_owned(self) -> Event<'static> {
		match self {
			Self::Unsigned(n) => Event::Unsigned(n),
			Self::Signed(n) => Event::Signed(n),
			Self::ByteString(b) => Event::ByteString(Cow::Owned(b.into_owned())),
			Self::UnknownLengthByteString => Event::UnknownLengthByteString,
			Self::TextString(t) => Event::TextString(Cow::Owned(t.into_owned())),
			Self::UnknownLengthTextString => Event::UnknownLengthTextString,
			Self::Array(l) => Event::Array(l),
			Self::UnknownLengthArray => Event::UnknownLengthArray,
			Self::Map(l) => Event::Map(l),
			Self::UnknownLengthMap => Event::UnknownLengthMap,
			Self::UnrecognizedTag(t) => Event::UnrecognizedTag(t),
			Self::Simple(s) => Event::Simple(s),
			Self::Float(f) => Event::Float(f),
			Self::Break => Event::Break,

			#[cfg(feature = "chrono")]
			Self::ChronoDateTime(dt) => Event::ChronoDateTime(dt),
			#[cfg(feature = "num-bigint")]
			Self::NumBigInt(big) => Event::NumBigInt(big),
		}
	}

	/// Interpret a [`Event::Signed`] value.
	///
	/// This is a convenience alias for [the basic `Event`'s `interpet_signed`](`BasicEvent::interpret_signed`).
	pub fn interpret_signed(val: u64) -> i64 {
		BasicEvent::interpret_signed(val)
	}

	/// Interpret a [`Event::Signed`] value.
	///
	/// This is a convenience alias for [the basic `Event`'s `interpret_signed_checked`](`BasicEvent::interpret_signed_checked`).
	pub fn interpret_signed_checked(val: u64) -> Option<i64> {
		BasicEvent::interpret_signed_checked(val)
	}

	/// Interpret a [`Event::Signed`] value.
	///
	/// This is a convenience alias for [the basic `Event`'s `interpret_signed_wide`](`BasicEvent::interpret_signed_wide`).
	pub fn interpret_signed_wide(val: u64) -> i128 {
		BasicEvent::interpret_signed_wide(val)
	}

	/// Create a [`Event::Signed`] or [`Event::Unsigned`] value.
	///
	/// This is a convenience alias for [the basic `Event`'s `create_signed`](`BasicEvent::create_signed`),
	/// except that it returns an extended [`Event`] instead.
	pub fn create_signed(val: i64) -> Event<'static> {
		match BasicEvent::create_signed(val) {
			BasicEvent::Unsigned(n) => Event::Unsigned(n),
			BasicEvent::Signed(n) => Event::Signed(n),
			_ => unreachable!(),
		}
	}

	/// Create a [`Event::Signed`] or [`Event::Unsigned`] value.
	///
	/// This is a convenience alias for [the basic `Event`'s `create_signed_wide`](`BasicEvent::create_signed_wide`),
	/// except that it returns an extended [`Event`] instead.
	pub fn create_signed_wide(val: i128) -> Option<Event<'static>> {
		match BasicEvent::create_signed_wide(val) {
			Some(BasicEvent::Unsigned(n)) => Some(Event::Unsigned(n)),
			Some(BasicEvent::Signed(n)) => Some(Event::Signed(n)),
			Some(_) => unreachable!(),
			None => None,
		}
	}
}

/// A streaming decoder for CBOR with extensions.
#[derive(Debug, Clone)]
pub struct Decoder<T: Read> {
	basic: BasicDecoder<T>,
	config: DecodeExtensionConfig,
	// Queue of fake events to be returned before any more processing takes place.
	queue: VecDeque<Event<'static>>,
}

include!("forward_config_accessors.in.rs");

impl<T: Read> Decoder<T> {
	pub(crate) fn new_from_config(basic: BasicDecoder<T>, config: DecodeExtensionConfig) -> Self {
		Self {
			basic,
			config,
			queue: VecDeque::new(),
		}
	}

	pub fn new_from_basic_decoder(basic: BasicDecoder<T>) -> Self {
		Self::new_from_config(basic, Default::default())
	}

	pub fn new(source: T) -> Self {
		Self::new_from_basic_decoder(BasicDecoder::new(source))
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

	// Read a byte string, which may be unknown-length; non-byte-strings are malformed.
	// This is a convenience function for the extended decoders.
	pub(crate) fn read_byte_string(&mut self) -> Result<Cow<[u8]>, DecodeError> {
		self.basic.read_byte_string()
	}

	// Read the body of an unknown-length byte string.
	// This is a convenience function for the extended decoders.
	pub(crate) fn read_unknown_length_byte_string_body(
		&mut self,
	) -> Result<Cow<[u8]>, DecodeError> {
		self.basic.read_unknown_length_byte_string_body()
	}

	// Read a text string, which may be unknown-length; non-text-strings are malformed.
	// This is a convenience function for the extended decoders.
	pub(crate) fn read_text_string(&mut self) -> Result<Cow<str>, DecodeError> {
		self.basic.read_text_string()
	}

	// Read the body of an unknown-length text string.
	// This is a convenience function for the extended decoders.
	pub(crate) fn read_unknown_length_text_string_body(&mut self) -> Result<Cow<str>, DecodeError> {
		self.basic.read_unknown_length_text_string_body()
	}

	fn do_bignum(&mut self, is_negative: bool) -> Result<Event, DecodeError> {
		use BignumDecodeStyle as BignumStyle;

		let raw_bytes = self.read_byte_string()?.into_owned();
		if raw_bytes.len() == 0 {
			return Ok(Event::Unsigned(0));
		}
		let starting_idx = {
			let mut starting_idx = raw_bytes.len() - 1;
			for (idx, &byte) in raw_bytes.iter().enumerate() {
				if byte != 0 {
					starting_idx = idx;
					break;
				}
			}
			starting_idx
		};
		let real_bytes = &raw_bytes[starting_idx..];

		let number = match real_bytes.len() {
			0 => 0,
			1..=7 => {
				let mut value = 0u64;
				for byte in real_bytes.iter() {
					value <<= 8;
					value += *byte as u64;
				}
				value
			}
			_ => match self.config.bignum_style {
				BignumStyle::Convert => {
					// this converts unknown-length byte strings to known-length ones, but oh well
					self.queue
						.push_back(Event::ByteString(Cow::Owned(raw_bytes)));
					return Ok(Event::UnrecognizedTag(if is_negative { 3 } else { 2 }));
				}
				BignumStyle::ForceConvert => return Err(DecodeError::OversizedBignum),
				#[cfg(feature = "num-bigint")]
				BignumStyle::Num => {
					return Ok(Event::NumBigInt(match is_negative {
						false => BigInt::from_bytes_be(Sign::Plus, real_bytes),
						true => {
							(-1).to_bigint().unwrap()
								- BigInt::from_bytes_be(Sign::Plus, real_bytes)
						}
					}))
				}
			},
		};

		if is_negative {
			Ok(Event::Signed(number))
		} else {
			Ok(Event::Unsigned(number))
		}
	}

	/// Pull an event from the decoder.
	///
	/// Note that the resulting event does not, at present, actually borrow the decoder.
	/// At the moment, the decoder isn't zero-copy.
	/// Even though [`Event`] supports borrowing the contents of byte- and text-strings,
	/// they are never borrowed in decoding, only in encoding.
	/// However, `next_event` is typed as if it were zero-copy for forward compatibility.
	pub fn next_event(&mut self) -> Result<Event, DecodeError> {
		if let Some(event) = self.queue.pop_front() {
			return Ok(event);
		}

		use DateTimeDecodeStyle as DateTimeStyle;

		Ok(match self.basic.next_event()?.into_owned() {
			BasicEvent::Unsigned(n) => Event::Unsigned(n),
			BasicEvent::Signed(n) => Event::Signed(n),
			BasicEvent::ByteString(b) => Event::ByteString(b),
			BasicEvent::UnknownLengthByteString => Event::UnknownLengthByteString,
			BasicEvent::TextString(t) => Event::TextString(t),
			BasicEvent::UnknownLengthTextString => Event::UnknownLengthTextString,
			BasicEvent::Array(len) => Event::Array(len),
			BasicEvent::UnknownLengthArray => Event::UnknownLengthArray,
			BasicEvent::Map(len) => Event::Map(len),
			BasicEvent::UnknownLengthMap => Event::UnknownLengthMap,
			BasicEvent::Simple(s) => Event::Simple(s),
			BasicEvent::Float(f) => Event::Float(f),
			BasicEvent::Break => Event::Break,

			BasicEvent::Tag(tag) => match tag {
				0 => match self.config.date_time_style {
					DateTimeStyle::None => Event::UnrecognizedTag(0),
					#[cfg(feature = "chrono")]
					DateTimeStyle::Chrono => match self.read_text_string() {
						Ok(t) => Event::ChronoDateTime(DateTime::parse_from_rfc3339(&t)?),
						Err(DecodeError::Malformed) => return Err(DecodeError::TagInvalid(0)),
						Err(e) => return Err(e),
					},
				},
				1 => match self.config.date_time_style {
					DateTimeStyle::None => Event::UnrecognizedTag(1),
					#[cfg(feature = "chrono")]
					DateTimeStyle::Chrono => match self.basic.next_event()? {
						BasicEvent::Unsigned(n) => {
							let time: i64 = n.try_into().map_err(|_| DecodeError::TagInvalid(1))?;
							Event::ChronoDateTime(Utc.timestamp(time, 0).into())
						}
						BasicEvent::Signed(n) => match BasicEvent::interpret_signed_checked(n) {
							Some(time) => Event::ChronoDateTime(Utc.timestamp(time, 0).into()),
							None => return Err(DecodeError::TagInvalid(1)),
						},
						BasicEvent::Float(f) => {
							let seconds = (f - f.fract()) as i64;
							let nanos = (f.fract() * 1_000_000_000f64) as i64;
							Event::ChronoDateTime(
								Utc.timestamp_nanos(seconds * 1_000_000_000 + nanos).into(),
							)
						}
						_ => return Err(DecodeError::TagInvalid(0)),
					},
				},
				2 => self.do_bignum(false)?,
				3 => self.do_bignum(true)?,
				_ => Event::UnrecognizedTag(tag),
			},
		})
	}

	/// Check whether it is possible to end the decoding now.
	///
	/// See [the basic counterpart](`crate::basic::streaming::Decoder::ready_to_finish`) for details.
	pub fn ready_to_finish(&self) -> bool {
		self.basic.ready_to_finish()
	}

	/// End the decoding.
	///
	/// This is [checked](`Self::ready_to_finish`) and will return [`DecodeError::Insufficient`] if the CBOR is incomplete.
	/// If you've performed the check already, try [`Self::force_finish`].
	pub fn finish(self) -> Result<T, DecodeError> {
		self.basic.finish()
	}

	/// End the decoding, without checking whether the decoder is finished or not.
	///
	/// See [the basic counterpart](`crate::basic::streaming::Decoder::force_finish`) for details.
	pub fn force_finish(self) -> impl Read {
		self.basic.force_finish()
	}
}

/// A streaming encoder for CBOR with extensions.
#[derive(Debug, Clone)]
pub struct Encoder<T: Write> {
	dest: BasicEncoder<T>,
	config: EncodeExtensionConfig,
}

impl<T: Write> Encoder<T> {
	fn new_from_config(dest: BasicEncoder<T>, config: EncodeExtensionConfig) -> Self {
		Self { dest, config }
	}

	pub fn new_from_basic_encoder(dest: BasicEncoder<T>) -> Self {
		Self::new_from_config(dest, Default::default())
	}

	pub fn new(dest: T) -> Self {
		Self::new_from_basic_encoder(BasicEncoder::new(dest))
	}

	forward_config_accessors!(
		DateTimeEncodeStyle,
		date_time_style,
		date_time_style_mut,
		set_date_time_style,
		"the way date-times are encoded."
	);

	/// Feed an event to the encoder.
	pub fn feed_event(&mut self, event: Event) -> Result<(), EncodeError> {
		let basic_event = match event {
			Event::Unsigned(n) => BasicEvent::Unsigned(n),
			Event::Signed(n) => BasicEvent::Signed(n),
			Event::ByteString(b) => BasicEvent::ByteString(b),
			Event::UnknownLengthByteString => BasicEvent::UnknownLengthByteString,
			Event::TextString(t) => BasicEvent::TextString(t),
			Event::UnknownLengthTextString => BasicEvent::UnknownLengthTextString,
			Event::Array(len) => BasicEvent::Array(len),
			Event::UnknownLengthArray => BasicEvent::UnknownLengthArray,
			Event::Map(len) => BasicEvent::Map(len),
			Event::UnknownLengthMap => BasicEvent::UnknownLengthMap,
			Event::UnrecognizedTag(t) => BasicEvent::Tag(t),
			Event::Simple(s) => BasicEvent::Simple(s),
			Event::Float(f) => BasicEvent::Float(f),
			Event::Break => BasicEvent::Break,

			#[cfg(feature = "chrono")]
			Event::ChronoDateTime(dt) => match self.config.date_time_style {
				DateTimeEncodeStyle::PreferText => {
					self.dest.feed_event(BasicEvent::Tag(0))?;
					BasicEvent::TextString(Cow::Owned(
						dt.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
					))
				}
				DateTimeEncodeStyle::PreferNumeric => {
					self.dest.feed_event(BasicEvent::Tag(1))?;
					match dt.timestamp_subsec_nanos() {
						0 => BasicEvent::create_signed(dt.timestamp()),
						_ => BasicEvent::Float(
							dt.timestamp() as f64
								+ (dt.timestamp_subsec_nanos() as f64) / 1_000_000_000f64,
						),
					}
				}
			},
			#[cfg(feature = "num-bigint")]
			Event::NumBigInt(mut n) => {
				if n == BigInt::from(0i32) {
					BasicEvent::Unsigned(0)
				} else if BigInt::from(0i32) < n && n <= BigInt::from(u64::MAX) {
					BasicEvent::Unsigned(n.try_into().unwrap())
				} else if -BigInt::from(u64::MAX) - 1i32 <= n && n < BigInt::from(0) {
					let value = if cfg!(debug_assertions) {
						let digits = (n + 1i32).magnitude().iter_u64_digits().collect::<Vec<_>>();
						assert!(digits.len() <= 1);
						digits.get(0).map(|x| *x).unwrap_or(0)
					} else {
						n.magnitude().iter_u64_digits().next().unwrap() - 1
					};
					BasicEvent::Signed(value)
				} else {
					self.dest
						.feed_event(BasicEvent::Tag(if n.sign() == Sign::Minus {
							n = n.magnitude().to_bigint().unwrap() - 1;
							3
						} else {
							2
						}))?;
					BasicEvent::ByteString(Cow::Owned(n.to_bytes_be().1))
				}
			}
		};
		self.dest.feed_event(basic_event)
	}

	pub fn ready_to_finish(&self) -> bool {
		self.dest.ready_to_finish()
	}
}

#[cfg(test)]
#[allow(unused)]
mod test {
	use super::*;
	#[cfg(feature = "chrono")]
	use chrono::{TimeZone, Utc};
	use std::io::Cursor;

	#[cfg(feature = "chrono")]
	#[test]
	fn decode_chrono_text_datetime() {
		assert_eq!(
			Decoder::new(Cursor::new(b"\xC0\x741990-12-31T12:34:56Z"))
				.set_date_time_style(crate::extended::DateTimeDecodeStyle::Chrono)
				.next_event()
				.unwrap(),
			Event::ChronoDateTime(Utc.ymd(1990, 12, 31).and_hms(12, 34, 56).into())
		);
	}

	#[cfg(feature = "chrono")]
	#[test]
	fn decode_chrono_text_datetime_unknown_length() {
		assert_eq!(
			Decoder::new(Cursor::new(b"\xC0\x7F\x741990-12-31T12:34:56Z\xFF"))
				.set_date_time_style(crate::extended::DateTimeDecodeStyle::Chrono)
				.next_event()
				.unwrap(),
			Event::ChronoDateTime(Utc.ymd(1990, 12, 31).and_hms(12, 34, 56).into())
		);
	}

	#[cfg(feature = "chrono")]
	#[test]
	fn decode_chrono_numeric_datetime() {
		assert_eq!(
			Decoder::new(Cursor::new(b"\xC1\x04"))
				.set_date_time_style(crate::extended::DateTimeDecodeStyle::Chrono)
				.next_event()
				.unwrap(),
			Event::ChronoDateTime(Utc.ymd(1970, 01, 01).and_hms(00, 00, 04).into())
		);
	}

	#[cfg(feature = "chrono")]
	#[test]
	fn decode_chrono_numeric_datetime_signed() {
		assert_eq!(
			Decoder::new(Cursor::new(b"\xC1\x20"))
				.set_date_time_style(crate::extended::DateTimeDecodeStyle::Chrono)
				.next_event()
				.unwrap(),
			Event::ChronoDateTime(Utc.ymd(1969, 12, 31).and_hms(23, 59, 59).into())
		);
	}

	#[cfg(feature = "chrono")]
	#[test]
	fn decode_chrono_numeric_datetime_fractional() {
		assert_eq!(
			Decoder::new(Cursor::new(b"\xC1\xFA\x3F\xA0\x00\x00"))
				.set_date_time_style(crate::extended::DateTimeDecodeStyle::Chrono)
				.next_event()
				.unwrap(),
			Event::ChronoDateTime(Utc.ymd(1970, 01, 01).and_hms_milli(00, 00, 01, 250).into())
		);
	}

	#[cfg(feature = "chrono")]
	#[test]
	fn encode_chrono_text_datetime() {
		let mut buf = Vec::new();
		let mut enc = Encoder::new(Cursor::new(&mut buf));
		assert_eq!(
			enc.date_time_style(),
			&crate::extended::DateTimeEncodeStyle::PreferText
		);
		enc.feed_event(Event::ChronoDateTime(
			Utc.ymd(1990, 12, 31).and_hms(12, 34, 56).into(),
		));
		assert!(enc.ready_to_finish());
		drop(enc);
		assert_eq!(&buf, b"\xC0\x741990-12-31T12:34:56Z")
	}

	#[cfg(feature = "chrono")]
	#[test]
	fn encode_chrono_numeric_datetime() {
		let mut buf = Vec::new();
		let mut enc = Encoder::new(Cursor::new(&mut buf));
		enc.set_date_time_style(crate::extended::DateTimeEncodeStyle::PreferNumeric);
		enc.feed_event(Event::ChronoDateTime(
			Utc.ymd(1970, 01, 01).and_hms(00, 00, 04).into(),
		));
		assert!(enc.ready_to_finish());
		drop(enc);
		assert_eq!(&buf, b"\xC1\x04");
	}

	#[cfg(feature = "chrono")]
	#[test]
	fn encode_chrono_text_datetime_fractional() {
		let mut buf = Vec::new();
		let mut enc = Encoder::new(Cursor::new(&mut buf));
		assert_eq!(
			enc.date_time_style(),
			&crate::extended::DateTimeEncodeStyle::PreferText
		);
		enc.feed_event(Event::ChronoDateTime(
			Utc.ymd(1876, 4, 22).and_hms_milli(13, 22, 1, 500).into(),
		));
		assert!(enc.ready_to_finish());
		drop(enc);
		assert_eq!(&buf, b"\xC0\x78\x181876-04-22T13:22:01.500Z");
	}

	#[cfg(feature = "chrono")]
	#[test]
	fn encode_chrono_numeric_datetime_fractional() {
		let mut buf = Vec::new();
		let mut enc = Encoder::new(Cursor::new(&mut buf));
		enc.set_date_time_style(crate::extended::DateTimeEncodeStyle::PreferNumeric);
		enc.feed_event(Event::ChronoDateTime(
			Utc.ymd(1970, 01, 01).and_hms_milli(00, 00, 00, 500).into(),
		));
		assert!(enc.ready_to_finish());
		drop(enc);
		assert_eq!(&buf, b"\xC1\xF9\x38\x00");
	}

	#[test]
	fn decode_bignum_convert() {
		let mut decoder = Decoder::new(Cursor::new(b"\xC2\x40"));
		assert_eq!(decoder.bignum_style(), &BignumDecodeStyle::Convert);
		assert_eq!(decoder.next_event().unwrap(), Event::Unsigned(0));

		let mut decoder = Decoder::new(Cursor::new(b"\xC2\x42\x01\x00"));
		assert_eq!(decoder.next_event().unwrap(), Event::Unsigned(256));

		let mut decoder = Decoder::new(Cursor::new(b"\xC2\x4A1234567890"));
		assert_eq!(decoder.next_event().unwrap(), Event::UnrecognizedTag(2));
		assert_eq!(
			decoder.next_event().unwrap(),
			Event::ByteString(Cow::Borrowed(b"1234567890"))
		);
		assert!(matches!(
			decoder.next_event(),
			Err(DecodeError::Insufficient)
		)); // to make sure the queue works
	}

	#[test]
	fn decode_bignum_force_convert() {
		let mut decoder = Decoder::new(Cursor::new(b"\xC2\x4A1234567890"));
		decoder.set_bignum_style(BignumDecodeStyle::ForceConvert);
		match decoder.next_event() {
			Err(DecodeError::OversizedBignum) => (),
			other => panic!("got {other:?}"),
		}
	}

	#[cfg(feature = "num-bigint")]
	#[test]
	fn decode_bignum_num() {
		use num_bigint::{Sign, ToBigInt};

		let mut decoder = Decoder::new(Cursor::new(b"\xC2\x4A1234567890"));
		decoder.set_bignum_style(BignumDecodeStyle::Num);
		assert_eq!(
			decoder.next_event().unwrap(),
			Event::NumBigInt(BigInt::from_bytes_be(num_bigint::Sign::Plus, b"1234567890"))
		);

		let mut decoder = Decoder::new(Cursor::new(b"\xC3\x4A1234567890"));
		decoder.set_bignum_style(BignumDecodeStyle::Num);
		assert_eq!(
			decoder.next_event().unwrap(),
			Event::NumBigInt(
				(-1).to_bigint().unwrap() - BigInt::from_bytes_be(Sign::Plus, b"1234567890")
			)
		);
	}

	#[cfg(feature = "num-bigint")]
	#[test]
	fn encode_bignum_num() {
		let mut buf = Vec::new();
		let mut encoder = Encoder::new(Cursor::new(&mut buf));
		encoder
			.feed_event(Event::NumBigInt(BigInt::from(0i32)))
			.unwrap();
		assert!(encoder.ready_to_finish());
		drop(encoder);
		assert_eq!(buf, b"\0");

		buf.clear();
		let mut encoder = Encoder::new(Cursor::new(&mut buf));
		encoder
			.feed_event(Event::NumBigInt(BigInt::from(u64::MAX) + 1))
			.unwrap();
		assert!(encoder.ready_to_finish());
		drop(encoder);
		assert_eq!(buf, b"\xC2\x49\x01\0\0\0\0\0\0\0\0");

		buf.clear();
		let mut encoder = Encoder::new(Cursor::new(&mut buf));
		encoder
			.feed_event(Event::NumBigInt(BigInt::from(-1i32)))
			.unwrap();
		assert!(encoder.ready_to_finish());
		drop(encoder);
		assert_eq!(buf, b"\x20");

		buf.clear();
		let mut encoder = Encoder::new(Cursor::new(&mut buf));
		encoder
			.feed_event(Event::NumBigInt(-BigInt::from(u64::MAX) - 1))
			.unwrap();
		assert!(encoder.ready_to_finish());
		drop(encoder);
		assert_eq!(buf, b"\x3B\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF");

		buf.clear();
		let mut encoder = Encoder::new(Cursor::new(&mut buf));
		encoder
			.feed_event(Event::NumBigInt(-BigInt::from(u64::MAX) - 2))
			.unwrap();
		assert!(encoder.ready_to_finish());
		drop(encoder);
		assert_eq!(buf, b"\xC3\x49\x01\0\0\0\0\0\0\0\0");
	}
}
