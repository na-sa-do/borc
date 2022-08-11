//! A tree-based implementation of the CBOR basic data model.
//!
//! This module parses CBOR into a large data structure which can be explored at will.
//! It is much easier to use than a streaming implementation,
//! but moderately less performant and with much higher memory requirements.
//! It is comparable to DOM in the XML world.

use crate::errors::DecodeError;
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
	/// This is a convenience alias for [`super::streaming::Event::interpreset_signed_wide`].
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

	/// Parse some CBOR.
	pub fn decode(source: impl Read) -> Result<Self, DecodeError> {
		use super::streaming::{Decoder, Event};
		let mut decoder = Decoder::new(source);
		Ok(match decoder.next_event()? {
			Event::Unsigned(val) => Item::Unsigned(val),
			Event::Signed(val) => Item::Signed(val),
			Event::ByteString(val) => Item::ByteString(val),
			Event::UnknownLengthByteString => todo!(),
			Event::TextString(val) => Item::TextString(val),
			Event::UnknownLengthTextString => todo!(),
			Event::Array(len) => todo!(),
			Event::UnknownLengthArray => todo!(),
			Event::Map(len) => todo!(),
			Event::UnknownLengthMap => todo!(),
			Event::Tag(tag) => todo!(),
			Event::Simple(val) => Item::Simple(val),
			Event::Float(val) => Item::Float(val),
			Event::Break => todo!(),
		})
	}
}
