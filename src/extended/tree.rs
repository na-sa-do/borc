//! A tree-based implementation of CBOR with extensions.
//!
//! This module parses CBOR into a large data structure which can be explored at will.
//! It is much easier to use than a streaming implementation,
//! but moderately less performant and with much higher memory requirements.
//! It is comparable to DOM in the XML world.

use super::streaming::Event as StreamingEvent;

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
}

impl Item {
	/// Interpret a [`Item::Signed`] value.
	///
	/// This is a convenience alias for [`crate::basic::streaming::Event::interpret_signed`].
	pub fn interpret_signed(val: u64) -> i64 {
		StreamingEvent::interpret_signed(val)
	}

	/// Interpret a [`Item::Signed`] value.
	///
	/// This is a convenience alias for [`crate::basic::streaming::Event::interpret_signed_checked`].
	pub fn interpret_signed_checked(val: u64) -> Option<i64> {
		StreamingEvent::interpret_signed_checked(val)
	}

	/// Interpret a [`Item::Signed`] value.
	///
	/// This is a convenience alias for [`crate::basic::streaming::Event::interpret_signed_wide`].
	pub fn interpret_signed_wide(val: u64) -> i128 {
		StreamingEvent::interpret_signed_wide(val)
	}

	/// Create a [`Item::Signed`] or [`Item::Unsigned`] value.
	///
	/// This is a convenience alias for [`crate::basic::streaming::Event::create_signed`],
	/// except that it returns an extended [`Item`] instead.
	pub fn create_signed(val: i64) -> Item {
		match StreamingEvent::create_signed(val) {
			StreamingEvent::Unsigned(n) => Self::Unsigned(n),
			StreamingEvent::Signed(n) => Self::Signed(n),
			_ => unreachable!(),
		}
	}

	/// Create a [`Item::Signed`] or [`Item::Unsigned`] value.
	///
	/// This is a convenience alias for [`crate::basic::streaming::Event::create_signed_wide`],
	/// except that it returns an extended [`Item`] instead.
	pub fn create_signed_wide(val: i128) -> Option<Item> {
		match StreamingEvent::create_signed_wide(val) {
			Some(StreamingEvent::Unsigned(n)) => Some(Self::Unsigned(n)),
			Some(StreamingEvent::Signed(n)) => Some(Self::Signed(n)),
			Some(_) => unreachable!(),
			None => None,
		}
	}
}
