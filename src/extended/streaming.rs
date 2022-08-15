//! A streaming implementation of CBOR with extensions.
//!
//! This module allows you to parse and generate CBOR very efficiently,
//! at the expense of being somewhat difficult to actually use.
//! It models CBOR as a series of [`Event`]s, which are not always full data items.
//! In this way, it is comparable to SAX in the XML world.

use std::{borrow::Cow, io::Read};

use crate::{
	basic::streaming::{Decoder as BasicDecoder, Event as BasicEvent},
	errors::DecodeError,
};

/// An event encountered while decoding or encoding CBOR using a streaming extended implementation.
#[derive(Debug, Clone)]
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
}

impl Event<'_> {
	/// Convert this [`Event`] to an owned value.
	pub fn into_owned(self) -> Event<'static> {
		let new = match self {
			Self::Unsigned(n) => Self::Unsigned(n),
			Self::Signed(n) => Self::Signed(n),
			Self::ByteString(b) => Self::ByteString(Cow::Owned(b.into_owned())),
			Self::UnknownLengthByteString => Self::UnknownLengthByteString,
			Self::TextString(t) => Self::TextString(Cow::Owned(t.into_owned())),
			Self::UnknownLengthTextString => Self::UnknownLengthTextString,
			Self::Array(l) => Self::Array(l),
			Self::UnknownLengthArray => Self::UnknownLengthArray,
			Self::Map(l) => Self::Map(l),
			Self::UnknownLengthMap => Self::UnknownLengthMap,
			Self::UnrecognizedTag(t) => Self::UnrecognizedTag(t),
			Self::Simple(s) => Self::Simple(s),
			Self::Float(f) => Self::Float(f),
			Self::Break => Self::Break,
		};
		unsafe { std::mem::transmute(new) }
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
}

impl<T: Read> Decoder<T> {
	pub fn new_from_basic_decoder(basic: BasicDecoder<T>) -> Self {
		Self { basic }
	}

	pub fn new(source: T) -> Self {
		Self::new_from_basic_decoder(BasicDecoder::new(source))
	}

	/// Pull an event from the decoder.
	///
	/// Note that the resulting event does not, at present, actually borrow the decoder.
	/// At the moment, the decoder isn't zero-copy.
	/// Even though [`Event`] supports borrowing the contents of byte- and text-strings,
	/// they are never borrowed in decoding, only in encoding.
	/// However, `next_event` is typed as if it were zero-copy for forward compatibility.
	pub fn next_event(&mut self) -> Result<Event, DecodeError> {
		Ok(match self.basic.next_event()? {
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
