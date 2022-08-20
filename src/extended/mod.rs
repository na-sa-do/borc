//! Implementations of CBOR with extensions.
//!
//! This module itself contains extension configuration types.
//! The CBOR encoder and decoder are in [`streaming`], like the [`basic`](`crate::basic`) API.
//!
//! At the moment, one extension is implemented:
//! dates and times using the `chrono` crate (requires the `chrono` feature).
//!
//! (We can't link to other crates here if they may or may not be compiled in, because if they aren't rustdoc gets confused.)

pub mod streaming;
pub mod tree;

macro_rules! config_accessors {
	($field:ident, $type:ty, $getter:ident, $mut_getter:ident, $setter:ident) => {
		pub fn $getter(&self) -> &$type {
			&self.$field
		}

		pub fn $mut_getter(&mut self) -> &mut $type {
			&mut self.$field
		}

		pub fn $setter(&mut self, value: $type) -> &mut Self {
			self.$field = value;
			self
		}
	};
}

#[derive(Debug, Clone, Default)]
pub struct DecodeExtensionConfig {
	date_time_style: DateTimeDecodeStyle,
}

impl DecodeExtensionConfig {
	config_accessors!(
		date_time_style,
		DateTimeDecodeStyle,
		date_time_style,
		date_time_style_mut,
		set_date_time_style
	);
}

#[derive(Debug, Clone, Default)]
pub struct EncodeExtensionConfig {
	date_time_style: DateTimeEncodeStyle,
}

impl EncodeExtensionConfig {
	config_accessors!(
		date_time_style,
		DateTimeEncodeStyle,
		date_time_style,
		date_time_style_mut,
		set_date_time_style
	);
}

/// How to decode datetimes.
///
/// This type is used instead of a simple boolean so that we can add support for other timekeeping libraries in the future.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DateTimeDecodeStyle {
	/// Do not handle datetimes.
	None,
	/// Use the [`chrono`] crate to handle datetimes.
	///
	/// This results in the use of the [`ChronoDateTime`](`streaming::Event::ChronoDateTime`) variant to handle datetimes.
	#[cfg(feature = "chrono")]
	Chrono,
}

impl Default for DateTimeDecodeStyle {
	/// Return [`DateTimeDecodeStyle::None`].
	fn default() -> Self {
		Self::None
	}
}

/// How to encode datetimes.
///
/// When decoding, both styles are supported equally.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DateTimeEncodeStyle {
	/// Prefer textual datetimes (CBOR tag 0).
	PreferText,
	/// Prefer numeric datetimes (CBOR tag 1).
	PreferNumeric,
}

impl Default for DateTimeEncodeStyle {
	/// Return [`DateTimeEncodeStyle::PreferText`].
	///
	/// borc uses textual datetime encoding by default because it handles dates before the UNIX epoch more robustly.
	fn default() -> Self {
		Self::PreferText
	}
}
