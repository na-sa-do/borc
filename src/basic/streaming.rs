//! A streaming implementation of the CBOR basic data model.
//!
//! This module allows you to parse and generate CBOR very efficiently,
//! at the expense of being somewhat difficult to actually use.
//! It models CBOR as a series of [`Event`]s, which are not always full data items.
//! In this way, it is comparable to SAX in the XML world.

use crate::errors::{DecodeError, EncodeError};
use std::{
	borrow::Cow,
	cell::RefCell,
	io::{Read, Write},
	num::NonZeroUsize,
};

fn read_be_u16(input: &[u8]) -> u16 {
	let mut bytes = [0u8; 2];
	bytes.copy_from_slice(&input[..2]);
	u16::from_be_bytes(bytes)
}

fn read_be_u32(input: &[u8]) -> u32 {
	let mut bytes = [0u8; 4];
	bytes.copy_from_slice(&input[..4]);
	u32::from_be_bytes(bytes)
}

fn read_be_u64(input: &[u8]) -> u64 {
	let mut bytes = [0u8; 8];
	bytes.copy_from_slice(&input[..8]);
	u64::from_be_bytes(bytes)
}

/// A streaming decoder for the CBOR basic data model.
#[derive(Debug, Clone)]
pub struct Decoder<T: Read> {
	source: RefCell<T>,
	input_buffer: Vec<u8>,
	pending: Vec<Pending>,
}

#[derive(Debug, Clone)]
enum Pending {
	Break,
	Array(u64),
	Map(u64, bool),
	UnknownLengthMap(bool),
	Tag,
}

#[derive(Debug)]
enum TryNextEventOutcome {
	GotEvent(Event<'static>),
	Error(DecodeError),
	Needs(NonZeroUsize),
}

impl<T: Read> Decoder<T> {
	pub fn new(source: T) -> Self {
		Decoder {
			source: RefCell::new(source),
			input_buffer: Vec::with_capacity(128),
			pending: Vec::new(),
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
		use TryNextEventOutcome::*;
		loop {
			match self.try_next_event() {
				GotEvent(e) => return Ok(e),
				Error(e) => return Err(e),
				Needs(n) => self.extend_input_buffer(n)?,
			}
		}
	}

	fn extend_input_buffer(&mut self, by: NonZeroUsize) -> Result<(), DecodeError> {
		let by = by.into();
		let orig_len = self.input_buffer.len();
		self.input_buffer.reserve(by);
		for _ in 0..by {
			// 0xFF is used because encountering a string of them at the wrong time will usually cause an InvalidBreak,
			// whereas encountering a string of (for example) zeros will be interpreted as valid.
			self.input_buffer.push(0xFF);
		}
		let buf = &mut self.input_buffer[orig_len..];
		match self.source.borrow_mut().read_exact(buf) {
			Ok(()) => (),
			Err(e) => {
				if e.kind() == std::io::ErrorKind::UnexpectedEof {
					return Err(DecodeError::Insufficient);
				} else {
					return Err(DecodeError::IoError(e));
				}
			}
		}
		Ok(())
	}

	fn try_next_event(&mut self) -> TryNextEventOutcome {
		use TryNextEventOutcome::*;
		let input = &mut self.input_buffer;
		if input.is_empty() {
			Needs(1.try_into().unwrap())
		} else {
			let (event, size) = {
				let initial = input[0];
				let excess = &input[1..];
				let major = initial >> 5;
				let additional = initial & 0b11111;

				macro_rules! bounds_check {
					($bound:expr) => {
						match ($bound as usize)
							.checked_sub(excess.len())
							.unwrap_or(0)
							.try_into()
						{
							Ok(n) => return Needs(n),
							Err(_) => (),
						}
					};
				}

				macro_rules! read_argument {
					() => {
						match additional {
							n if n < 24 => (Some(n as u64), 1),
							24 => {
								bounds_check!(1);
								(Some(excess[0] as _), 2)
							}
							25 => {
								bounds_check!(2);
								(Some(read_be_u16(excess) as _), 3)
							}
							26 => {
								bounds_check!(4);
								(Some(read_be_u32(excess) as _), 5)
							}
							27 => {
								bounds_check!(8);
								(Some(read_be_u64(excess) as _), 9)
							}
							28 | 29 | 30 => return Error(DecodeError::Malformed),
							31 => (None, 0),
							_ => unreachable!(),
						}
					};
				}

				let mut pop_pending = false;
				match self.pending.last_mut() {
					Some(Pending::Array(ref mut n)) => {
						*n -= 1;
						if *n == 0 {
							pop_pending = true;
						}
					}
					Some(Pending::Map(ref mut n, ref mut can_stop)) => {
						*can_stop = !*can_stop;
						if *can_stop {
							*n -= 1;
							if *n == 0 {
								pop_pending = true;
							}
						}
					}
					Some(Pending::UnknownLengthMap(ref mut can_stop)) => {
						*can_stop = !*can_stop;
					}
					Some(Pending::Tag) => {
						pop_pending = true;
					}
					Some(Pending::Break) | None => (),
				}
				if pop_pending {
					self.pending.pop();
				}

				match major {
					0 => {
						let (val, offset) = read_argument!();
						(
							Event::Unsigned(match val {
								Some(x) => x,
								None => return Error(DecodeError::Malformed),
							}),
							offset,
						)
					}
					1 => {
						let (val, offset) = read_argument!();
						(
							Event::Signed(match val {
								Some(x) => x,
								None => return Error(DecodeError::Malformed),
							}),
							offset,
						)
					}
					2 => {
						let (val, offset) = read_argument!();
						match val {
							Some(len) => {
								let len = len as usize;
								// remember that offset includes the initial
								bounds_check!(len + offset - 1);
								let contents = excess[offset - 1..len + offset - 1].to_owned();
								(Event::ByteString(Cow::Owned(contents)), len + offset)
							}
							None => {
								self.pending.push(Pending::Break);
								(Event::UnknownLengthByteString, 1)
							}
						}
					}
					3 => {
						let (val, offset) = read_argument!();
						match val {
							Some(len) => {
								let len = len as usize;
								// remember that offset includes the initial
								bounds_check!(len + offset - 1);
								let contents = excess[offset - 1..len + offset - 1].to_owned();
								match String::from_utf8(contents) {
									Ok(s) => (Event::TextString(Cow::Owned(s)), len + offset),
									Err(e) => return Error(e.into()),
								}
							}
							None => {
								self.pending.push(Pending::Break);
								(Event::UnknownLengthTextString, 1)
							}
						}
					}
					4 => {
						let (val, offset) = read_argument!();
						match val {
							Some(len) => {
								if len > 0 {
									self.pending.push(Pending::Array(len));
								}
								(Event::Array(len), offset)
							}
							None => {
								self.pending.push(Pending::Break);
								(Event::UnknownLengthArray, 1)
							}
						}
					}
					5 => {
						let (val, offset) = read_argument!();
						match val {
							Some(len) => {
								if len > 0 {
									self.pending.push(Pending::Map(len, true));
								}
								(Event::Map(len), offset)
							}
							None => {
								self.pending.push(Pending::UnknownLengthMap(true));
								(Event::UnknownLengthMap, 1)
							}
						}
					}
					6 => {
						let (val, offset) = read_argument!();
						match val {
							Some(tag) => {
								self.pending.push(Pending::Tag);
								(Event::Tag(tag), offset)
							}
							None => {
								return Error(DecodeError::Malformed);
							}
						}
					}
					7 => match additional {
						n @ 0..=23 => (Event::Simple(n), 1),
						24 => {
							bounds_check!(1);
							match excess[0] {
								0..=23 => return Error(DecodeError::Malformed),
								n => (Event::Simple(n), 2),
							}
						}
						25 => {
							bounds_check!(2);
							let mut bytes = [0u8; 2];
							bytes.copy_from_slice(&excess[..2]);
							(Event::Float(half::f16::from_be_bytes(bytes).into()), 3)
						}
						26 => {
							bounds_check!(4);
							let mut bytes = [0u8; 4];
							bytes.copy_from_slice(&excess[..4]);
							(Event::Float(f32::from_be_bytes(bytes).into()), 5)
						}
						27 => {
							bounds_check!(8);
							let mut bytes = [0u8; 8];
							bytes.copy_from_slice(&excess[..8]);
							(Event::Float(f64::from_be_bytes(bytes)), 9)
						}
						28..=30 => return Error(DecodeError::Malformed),
						31 => {
							match self.pending.pop() {
								// This is false because it's already been flipped for this item.
								Some(Pending::Break) | Some(Pending::UnknownLengthMap(false)) => (),
								_ => return Error(DecodeError::Malformed),
							}
							(Event::Break, 1)
						}
						32..=u8::MAX => unreachable!(),
					},
					8..=u8::MAX => unreachable!(),
				}
			};
			input.drain(0..size);
			GotEvent(event)
		}
	}

	/// Check whether it is possible to end the decoding now.
	///
	/// If this returns true, it means cutting off the CBOR now results in a complete object, _and_ there is no extra data in the internal buffer.
	/// There can be extra data in the internal buffer if a partial CBOR event has just been read.
	pub fn ready_to_finish(&self) -> bool {
		self.pending.is_empty() && self.input_buffer.is_empty()
	}

	/// End the decoding.
	///
	/// This is [checked](`Self::ready_to_finish`) and will return [`DecodeError::Insufficient`] if the CBOR is incomplete.
	/// If you've performed the check already, try [`Self::force_finish`].
	pub fn finish(self) -> Result<T, DecodeError> {
		if self.ready_to_finish() {
			Ok(self.source.into_inner())
		} else {
			Err(DecodeError::Insufficient)
		}
	}

	/// End the decoding, without checking whether the decoder is finished or not.
	///
	/// This does not return the original reader,
	/// but the reader returned behaves as if it were the original reader.
	/// (The discrepancy is because [`Decoder`] contains an internal buffer.
	/// Rest assured it behaves as if this buffer were not used.)
	pub fn force_finish(self) -> impl Read {
		std::io::Cursor::new(self.input_buffer).chain(self.source.into_inner())
	}
}

/// An event encountered while decoding or encoding CBOR using a streaming basic implementation.
#[derive(Debug, Clone)]
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
	Tag(u64),
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
			Self::Tag(t) => Event::Tag(t),
			Self::Simple(s) => Event::Simple(s),
			Self::Float(f) => Event::Float(f),
			Self::Break => Event::Break,
		}
	}

	/// Interpret a [`Event::Signed`] value.
	///
	/// # Overflow behavior
	///
	/// On overflow, this function will panic if overflow checks are enabled (default in debug mode)
	/// and wrap if overflow checks are disabled (default in release mode).
	pub fn interpret_signed(val: u64) -> i64 {
		-1 - (val as i64)
	}

	/// Interpret a [`Event::Signed`] value.
	///
	/// # Overflow behavior
	///
	/// On overflow, this function will return [`None`].
	pub fn interpret_signed_checked(val: u64) -> Option<i64> {
		match val {
			n if n < i64::MAX as u64 => Some(-1 - (n as i64)),
			_ => None,
		}
	}

	/// Interpret a [`Event::Signed`] value.
	///
	/// # Overflow behavior
	///
	/// This function does not overflow, because it returns an [`i128`].
	pub fn interpret_signed_wide(val: u64) -> i128 {
		-1 - (val as i128)
	}

	/// Create a [`Event::Signed`] or [`Event::Unsigned`] value.
	pub fn create_signed(val: i64) -> Event<'static> {
		if val.is_negative() {
			Event::Signed(val.abs_diff(-1))
		} else {
			Event::Unsigned(val as _)
		}
	}

	/// Create a [`Event::Signed`] or [`Event::Unsigned`] value.
	///
	/// Because this takes an [`i128`], it can express all the numbers CBOR can encode.
	/// However, some [`i128`]s cannot be encoded in basic CBOR integers.
	/// In this case, it will return [`None`].
	pub fn create_signed_wide(val: i128) -> Option<Event<'static>> {
		if val.is_negative() {
			match val.abs_diff(-1).try_into() {
				Ok(val) => Some(Event::Signed(val)),
				Err(_) => None,
			}
		} else {
			Some(Event::Unsigned(val as _))
		}
	}
}

/// A streaming encoder for the CBOR basic data model.
#[derive(Debug, Clone)]
pub struct Encoder<T: Write> {
	dest: T,
	pending: Vec<Pending>,
}

impl<T: Write> Encoder<T> {
	pub fn new(dest: T) -> Self {
		Encoder {
			dest,
			pending: Vec::new(),
		}
	}

	pub fn feed_event(&mut self, event: Event) -> Result<(), EncodeError> {
		macro_rules! write_initial_and_argument {
			($major:expr, $argument:expr) => {
				let major: u8 = $major << 5;
				match $argument {
					n if n <= 0x17 => {
						self.dest.write_all(&[major | n as u8])?;
					}
					n if n <= u8::MAX as _ => {
						self.dest.write_all(&[major | 0x18, n as u8])?;
					}
					n if n <= u16::MAX as _ => {
						self.dest.write_all(&[major | 0x19])?;
						self.dest.write_all(&u16::to_be_bytes(n as _))?;
					}
					n if n <= u32::MAX as _ => {
						self.dest.write_all(&[major | 0x1A])?;
						self.dest.write_all(&u32::to_be_bytes(n as _))?;
					}
					n => {
						self.dest.write_all(&[major | 0x1B])?;
						self.dest.write_all(&u64::to_be_bytes(n))?;
					}
				}
			};
		}

		let mut pop_pending = false;
		match self.pending.last_mut() {
			Some(Pending::Array(ref mut n)) => {
				*n -= 1;
				pop_pending = *n == 0;
			}
			Some(Pending::Map(ref mut n, ref mut can_stop)) => {
				*can_stop = match *can_stop {
					true => false,
					false => {
						*n -= 1;
						pop_pending = *n == 0;
						true
					}
				};
			}
			Some(Pending::UnknownLengthMap(ref mut can_stop)) => {
				*can_stop = !*can_stop;
			}
			Some(Pending::Tag) => {
				pop_pending = true;
			}
			Some(Pending::Break) | None => (),
		}
		if pop_pending {
			self.pending.pop();
		}

		match event {
			Event::Unsigned(n) => {
				write_initial_and_argument!(0, n);
			}
			Event::Signed(n) => {
				write_initial_and_argument!(1, n);
			}
			Event::ByteString(bytes) => {
				write_initial_and_argument!(2, bytes.len() as _);
				self.dest.write_all(&bytes)?;
			}
			Event::UnknownLengthByteString => {
				self.dest.write_all(&[0x5F])?;
				self.pending.push(Pending::Break);
			}
			Event::TextString(text) => {
				write_initial_and_argument!(3, text.len() as _);
				self.dest.write_all(text.as_bytes())?;
			}
			Event::UnknownLengthTextString => {
				self.dest.write_all(&[0x7F])?;
				self.pending.push(Pending::Break);
			}
			Event::Array(n) => {
				write_initial_and_argument!(4, n);
				self.pending.push(Pending::Array(n));
			}
			Event::UnknownLengthArray => {
				self.dest.write_all(&[0x9F])?;
				self.pending.push(Pending::Break);
			}
			Event::Map(n) => {
				write_initial_and_argument!(5, n);
				self.pending.push(Pending::Map(n, true));
			}
			Event::UnknownLengthMap => {
				self.dest.write_all(&[0xBF])?;
				self.pending.push(Pending::UnknownLengthMap(true));
			}
			Event::Tag(n) => {
				write_initial_and_argument!(6, n);
				self.pending.push(Pending::Tag);
			}
			Event::Float(n64) => {
				let n32 = n64 as f32;
				if n32 as f64 == n64 {
					let n16 = half::f16::from_f64(n64);
					if n16.to_f64() == n64 {
						self.dest.write_all(&[0xF9])?;
						self.dest.write_all(&n16.to_be_bytes())?;
					} else {
						self.dest.write_all(&[0xFA])?;
						self.dest.write_all(&n32.to_be_bytes())?;
					}
				} else {
					self.dest.write_all(&[0xFB])?;
					self.dest.write_all(&n64.to_be_bytes())?;
				}
			}
			Event::Simple(n) => {
				// The CBOR spec requires that simple values 0-24 be encoded as a single byte,
				// and simple values 25-255 be encoded as two bytes.
				// Why this is required when overlong arguments are otherwise legal is a mystery to me,
				// but in any case, we always generate the shortest encoding anyway, so it's fine.
				//
				// Also, since n is a u8, it'll never exceed 255, so we can just do this:
				write_initial_and_argument!(7, n as _);
				// and not worry about accidentally generating the prefix to a float.
			}
			Event::Break => match self.pending.pop() {
				Some(Pending::Break | Pending::UnknownLengthMap(false)) => {
					self.dest.write_all(&[0xFF])?
				}
				_ => return Err(EncodeError::InvalidBreak),
			},
		}

		Ok(())
	}

	pub fn ready_to_finish(&self) -> bool {
		self.pending.is_empty()
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use std::io::Cursor;

	macro_rules! decode_test {
		(match $decoder:ident: $in:expr => $out:pat if $cond:expr) => {
			match $decoder.next_event() {
				$out if $cond => (),
				other => panic!(concat!("{:X?} -> {:X?} instead of ", stringify!($out), " if ", stringify!($cond)), $in, other),
			}
		};
		(match $decoder:ident: $in:expr => $out:pat) => {
			decode_test!(match $decoder: $in => $out if true);
		};
		(match $decoder:ident: $out:pat if $cond:expr) => {
			match $decoder.next_event() {
				$out if $cond => (),
				other => panic!("? -> {:X?}", other),
			}
		};
		(match $decoder:ident: $out:pat) => {
			decode_test!(match $decoder: $out if true);
		};
		($in:expr => $out:pat if $cond:expr) => {
			let mut decoder = Decoder::new(Cursor::new($in));
			decode_test!(match decoder: $in => $out if $cond);
			decoder.finish().unwrap();
		};
		($in:expr => $out:pat) => {
			decode_test!($in => $out if true);
		};
		(small $in:expr) => {
			let mut decoder = Decoder::new(Cursor::new($in));
			decode_test!(match decoder: $in => Err(DecodeError::Insufficient));
			assert!(!decoder.ready_to_finish());
		};
	}

	macro_rules! encode_test {
		($($in:expr),+ => $out:expr, check finish if $cond:expr, expecting $expect:expr; $event:ident) => {
			let mut buf = Vec::new();
			let mut encoder = Encoder::new(Cursor::new(&mut buf));
			for (idx, $event) in [$($in),+].into_iter().enumerate() {
				let check_finish = $cond;
				let expected = $expect;
				encoder.feed_event($event).unwrap();
				if check_finish {
					assert_eq!(encoder.ready_to_finish(), expected, "readiness to finish was not as expected after event #{}", idx);
				}
			}
			assert!(encoder.ready_to_finish());
			std::mem::drop(encoder);
			assert_eq!(buf, $out);
		};
		($($in:expr),+ => $out:expr, check finish expecting $expect:expr; $event:ident) => {
			encode_test!($($in),+ => $out, check finish if true, expecting $expect; $event);
		};
		($($in:expr),+ => $out:expr, check finish if $cond:expr; $event:ident) => {
			encode_test!($($in),+ => $out, check finish if $cond, expecting true; $event);
		};
		($($in:expr),+ => $out:expr) => {
			encode_test!($($in),+ => $out, check finish if false; event);
		};
		($($in:expr),+ => $out:expr, check finish) => {
			encode_test!($($in),+ => $out, check finish if true; event);
		};
	}

	#[test]
	fn decode_uint_tiny() {
		for i1 in 0..=0x17u8 {
			decode_test!([i1] => Ok(Event::Unsigned(i2)) if i2 == i1 as _);
		}
	}

	#[test]
	fn encode_uint_tiny() {
		for i in 0..=0x17u8 {
			encode_test!(Event::Unsigned(i as _) => [i]);
		}
	}

	#[test]
	fn decode_uint_8bit() {
		decode_test!([0x18u8, 0x01] => Ok(Event::Unsigned(0x01)));
	}

	#[test]
	fn decode_uint_8bit_bounds() {
		decode_test!(small b"\x18");
	}

	#[test]
	fn encode_uint_8bit() {
		encode_test!(Event::Unsigned(0x3F) => [0x18, 0x3F]);
	}

	#[test]
	fn decode_uint_16bit() {
		decode_test!([0x19u8, 0x01, 0x02] => Ok(Event::Unsigned(0x0102)));
	}

	#[test]
	fn decode_uint_16bit_bounds() {
		decode_test!(small b"\x19\x00");
	}

	#[test]
	fn encode_uint_16bit() {
		encode_test!(Event::Unsigned(0x1234) => [0x19, 0x12, 0x34]);
	}

	#[test]
	fn decode_uint_32bit() {
		decode_test!([0x1Au8, 0x01, 0x02, 0x03, 0x04] => Ok(Event::Unsigned(0x01020304)));
	}

	#[test]
	fn decode_uint_32bit_bounds() {
		decode_test!(small b"\x1A\x00\x00\x00");
	}

	#[test]
	fn encode_uint_32bit() {
		encode_test!(Event::Unsigned(0x12345678) => [0x1A, 0x12, 0x34, 0x56, 0x78]);
	}

	#[test]
	fn decode_uint_64bit() {
		decode_test!([0x1Bu8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08] => Ok(Event::Unsigned(0x0102030405060708)));
	}

	#[test]
	fn decode_uint_64bit_bounds() {
		decode_test!(small b"\x1B\x00\x00\x00\x00\x00\x00\x00");
	}

	#[test]
	fn encode_uint_64bit() {
		encode_test!(Event::Unsigned(0x123456789ABCDEF0) => [0x1B, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
	}

	#[test]
	fn decode_negint() {
		decode_test!([0x20u8] => Ok(Event::Signed(0)));
		decode_test!([0x37u8] => Ok(Event::Signed(0x17)));
		decode_test!([0x38, 0x01] => Ok(Event::Signed(0x01)));
		decode_test!([0x39, 0x01, 0x02] => Ok(Event::Signed(0x0102)));
		decode_test!([0x3A, 0x01, 0x02, 0x03, 0x04] => Ok(Event::Signed(0x01020304)));
		decode_test!([0x3B, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08] => Ok(Event::Signed(0x0102030405060708)));
		decode_test!(small b"\x3B\x00\x00\x00\x00\x00\x00\x00");
	}

	#[test]
	fn encode_negint() {
		encode_test!(Event::Signed(0) => [0x20]);
		encode_test!(Event::Signed(0x17) => [0x37]);
		encode_test!(Event::Signed(0xAA) => [0x38, 0xAA]);
		encode_test!(Event::Signed(0x0102) => [0x39, 0x01, 0x02]);
		encode_test!(Event::Signed(0x01020304) => [0x3A, 0x01, 0x02, 0x03, 0x04]);
		encode_test!(Event::Signed(0x010203040506) => [0x3B, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
	}

	#[test]
	fn interpret_signed() {
		assert_eq!(Event::interpret_signed(0), -1);
		assert_eq!(Event::interpret_signed_checked(0), Some(-1));
		assert_eq!(Event::interpret_signed_checked(u64::MAX), None);
		assert_eq!(Event::interpret_signed_wide(0), -1);
		assert_eq!(
			Event::interpret_signed_wide(u64::MAX),
			-1 - u64::MAX as i128
		);
	}

	#[test]
	fn create_signed() {
		assert!(matches!(Event::create_signed(0), Event::Unsigned(0)));
		assert!(matches!(Event::create_signed(22), Event::Unsigned(22)));
		assert!(matches!(Event::create_signed(-1), Event::Signed(0)));
		assert!(matches!(Event::create_signed(-22), Event::Signed(21)));
	}

	#[test]
	fn decode_bytes() {
		decode_test!([0x40] => Ok(Event::ByteString(Cow::Owned(x))) if x == b"");
		decode_test!(b"\x45Hello" => Ok(Event::ByteString(Cow::Owned(x))) if x == b"Hello");
		decode_test!(b"\x58\x04Halo" => Ok(Event::ByteString(Cow::Owned(x))) if x == b"Halo");
		decode_test!(b"\x59\x00\x07Goodbye" => Ok(Event::ByteString(Cow::Owned(x))) if x == b"Goodbye");
		decode_test!(b"\x5A\x00\x00\x00\x0DLong message!" => Ok(Event::ByteString(Cow::Owned(x))) if x == b"Long message!");
		decode_test!(b"\x5B\x00\x00\x00\x00\x00\x00\x00\x01?" => Ok(Event::ByteString(Cow::Owned(x))) if x == b"?");
	}

	#[test]
	fn encode_bytes() {
		encode_test!(Event::ByteString(Cow::Borrowed(b"")) => b"\x40");
		encode_test!(Event::ByteString(Cow::Borrowed(b"abcd")) => b"\x44abcd");

		macro_rules! test {
			($size:expr, $prefix:expr) => {
				let size: usize = $size;
				let prefix = $prefix;
				let input = {
					let mut it = Vec::with_capacity(size);
					it.resize(size, 0x0Fu8);
					it
				};
				let mut output = Vec::with_capacity(size + prefix.len());
				let mut encoder = Encoder::new(Cursor::new(&mut output));
				encoder
					.feed_event(Event::ByteString(Cow::Borrowed(&input)))
					.unwrap();
				assert_eq!(output[..prefix.len()], prefix);
				assert_eq!(output[prefix.len()..], input);
			};
		}

		test!(0x30, [0x58, 0x30]);
		test!(0x02FA, [0x59, 0x02, 0xFA]);
		test!(0x010000, [0x5A, 0x00, 0x01, 0x00, 0x00]);
		// Allocates about 8 GiB of memory! And iterates 4Gi times! Wow!
		test!(
			2usize.pow(32),
			[0x5B, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]
		);
	}

	#[test]
	fn decode_bytes_segmented() {
		let mut decoder = Decoder::new(Cursor::new(b"\x5F\x44abcd\x43efg\xFF"));
		decode_test!(match decoder: Ok(Event::UnknownLengthByteString));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(Event::ByteString(Cow::Owned(x))) if x == b"abcd");
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(Event::ByteString(Cow::Owned(x))) if x == b"efg");
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(Event::Break));
		assert!(decoder.ready_to_finish());
	}

	#[test]
	fn decode_bytes_segmented_small() {
		let mut decoder = Decoder::new(Cursor::new(b"\x5F\x44abcd"));
		decode_test!(match decoder: Ok(Event::UnknownLengthByteString));
		decode_test!(match decoder: Ok(Event::ByteString(Cow::Owned(x))) if x == b"abcd");
		decode_test!(match decoder: Err(DecodeError::Insufficient));
		assert!(!decoder.ready_to_finish());
	}

	#[test]
	fn encode_bytes_segmented() {
		encode_test!(
			Event::UnknownLengthByteString,
			Event::ByteString(Cow::Borrowed(b"abcd")),
			Event::ByteString(Cow::Borrowed(b"efg")),
			Event::Break
			=> b"\x5F\x44abcd\x43efg\xFF",
			check finish expecting matches!(event, Event::Break); event
		);
	}

	#[test]
	fn decode_text() {
		decode_test!([0x60] => Ok(Event::TextString(x)) if x == "");
		decode_test!(b"\x65Hello" => Ok(Event::TextString(x)) if x == "Hello");
		decode_test!(b"\x78\x04Halo" => Ok(Event::TextString(x)) if x == "Halo");
		decode_test!(b"\x79\x00\x07Goodbye" => Ok(Event::TextString(x)) if x == "Goodbye");
		decode_test!(b"\x7A\x00\x00\x00\x0DLong message!" => Ok(Event::TextString(x)) if x == "Long message!");
		decode_test!(b"\x7B\x00\x00\x00\x00\x00\x00\x00\x01?" => Ok(Event::TextString(x)) if x == "?");
	}

	#[test]
	fn decode_text_64bit_bounds() {
		decode_test!(small b"\x7B\x00\x00\x00\x00\x00\x00\x00");
		decode_test!(small b"\x7B\x00\x00\x00\x00\x00\x00\x00\x01");
	}

	#[test]
	fn encode_text() {
		encode_test!(Event::TextString(Cow::Borrowed("")) => b"\x60");
		encode_test!(Event::TextString(Cow::Borrowed("abcd")) => b"\x64abcd");

		macro_rules! test {
			($size:expr, $prefix:expr) => {
				let size: usize = $size;
				let prefix = $prefix;
				let input = {
					let mut it = Vec::with_capacity(size);
					it.resize(size, b"A"[0]);
					unsafe { String::from_utf8_unchecked(it) }
				};
				let mut output = Vec::with_capacity(size + prefix.len());
				let mut encoder = Encoder::new(Cursor::new(&mut output));
				encoder
					.feed_event(Event::TextString(Cow::Borrowed(&input)))
					.unwrap();
				assert_eq!(output[..prefix.len()], prefix);
				assert_eq!(output[prefix.len()..], input.into_bytes());
			};
		}

		test!(0x30, [0x78, 0x30]);
		test!(0x02FA, [0x79, 0x02, 0xFA]);
		test!(0x010000, [0x7A, 0x00, 0x01, 0x00, 0x00]);
		// Allocates about 8 GiB of memory! And iterates 4Gi times! Wowie wow wow!
		test!(
			2usize.pow(32),
			[0x7B, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]
		);
	}

	#[test]
	fn decode_text_segmented() {
		let mut decoder = Decoder::new(Cursor::new(b"\x7F\x64abcd\x63efg\xFF"));
		decode_test!(match decoder: Ok(Event::UnknownLengthTextString));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(Event::TextString(x)) if x == "abcd");
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(Event::TextString(x)) if x == "efg");
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(Event::Break));
		assert!(decoder.ready_to_finish());
	}

	#[test]
	fn decode_text_segmented_small() {
		let mut decoder = Decoder::new(Cursor::new(b"\x7F\x64abcd"));
		decode_test!(match decoder: Ok(Event::UnknownLengthTextString));
		decode_test!(match decoder: Ok(Event::TextString(x)) if x == "abcd");
		decode_test!(match decoder: Err(DecodeError::Insufficient));
		assert!(!decoder.ready_to_finish());
	}

	#[test]
	fn decode_text_invalid() {
		let mut decoder = Decoder::new(Cursor::new(b"\x62\xFF\xFF"));
		match decoder.next_event() {
			Err(DecodeError::InvalidUtf8(_)) => (),
			_ => panic!("accepted invalid UTF-8"),
		}
	}

	#[test]
	fn encode_text_segmented() {
		encode_test!(
			Event::UnknownLengthTextString,
			Event::TextString(Cow::Borrowed("abcd")),
			Event::TextString(Cow::Borrowed("efg")),
			Event::Break
			=> b"\x7F\x64abcd\x63efg\xFF",
			check finish expecting matches!(event, Event::Break); event
		);
	}

	#[test]
	fn decode_array() {
		let mut decoder = Decoder::new(Cursor::new(b"\x84\0\x01\x02\x03"));
		decode_test!(match decoder: Ok(Event::Array(4)));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(decoder.ready_to_finish());

		let mut decoder = Decoder::new(Cursor::new(b"\x80"));
		decode_test!(match decoder: Ok(Event::Array(0)));
		assert!(decoder.ready_to_finish());
	}

	#[test]
	fn encode_array() {
		encode_test!(
			Event::Array(3),
			Event::Unsigned(1),
			Event::Unsigned(2),
			Event::Unsigned(3)
			=> b"\x83\x01\x02\x03",
			check finish expecting matches!(event, Event::Unsigned(3)); event
		);
	}

	#[test]
	fn decode_array_segmented() {
		let mut decoder = Decoder::new(Cursor::new(b"\x9F\x00\x00\x00\xFF"));
		decode_test!(match decoder: Ok(Event::UnknownLengthArray));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(Event::Break));
		assert!(decoder.ready_to_finish());
	}

	#[test]
	fn encode_array_segmented() {
		encode_test!(
			Event::UnknownLengthArray,
			Event::Unsigned(1),
			Event::Unsigned(2),
			Event::Unsigned(3),
			Event::Break
			=> b"\x9F\x01\x02\x03\xFF",
			check finish expecting matches!(event, Event::Break); event
		);
	}

	#[test]
	fn decode_map() {
		let mut decoder = Decoder::new(Cursor::new(b"\xA2\x01\x02\x03\x04"));
		decode_test!(match decoder: Ok(Event::Map(2)));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(decoder.ready_to_finish());

		let mut decoder = Decoder::new(Cursor::new(b"\xA0"));
		decode_test!(match decoder: Ok(Event::Map(0)));
		assert!(decoder.ready_to_finish());
	}

	#[test]
	fn encode_map() {
		encode_test!(
			Event::Map(2),
			Event::Unsigned(0),
			Event::TextString(Cow::Borrowed("a")),
			Event::Unsigned(1),
			Event::TextString(Cow::Borrowed("b"))
			=> b"\xA2\x00\x61a\x01\x61b",
			check finish expecting matches!(event, Event::TextString(ref s) if s == "b"); event
		);
	}

	#[test]
	fn decode_map_segmented() {
		let mut decoder = Decoder::new(Cursor::new(b"\xBF\x00\x00\x00\x00\xFF"));
		decode_test!(match decoder: Ok(Event::UnknownLengthMap));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(Event::Break));
		assert!(decoder.ready_to_finish());
	}

	#[test]
	fn encode_map_segmented() {
		encode_test!(
			Event::UnknownLengthMap,
			Event::Unsigned(0),
			Event::TextString(Cow::Borrowed("a")),
			Event::Unsigned(1),
			Event::TextString(Cow::Borrowed("b")),
			Event::Break
			=> b"\xBF\x00\x61a\x01\x61b\xFF",
			check finish expecting matches!(event, Event::Break); event
		);
	}

	// TODO: Remove this test in favor of adding `check finish` features to decode_test!.
	#[test]
	fn decode_map_segmented_odd() {
		let mut decoder = Decoder::new(Cursor::new(b"\xBF\x00\xFF"));
		decode_test!(match decoder: Ok(Event::UnknownLengthMap));
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish());
	}

	#[test]
	fn decode_tag() {
		let mut decoder = Decoder::new(Cursor::new(b"\xC1\x00"));
		decode_test!(match decoder: Ok(Event::Tag(1)));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Ok(_));
		assert!(decoder.ready_to_finish());
	}

	#[test]
	fn encode_tag() {
		encode_test!(
			Event::Tag(1),
			Event::Unsigned(0)
			=> b"\xC1\x00",
			check finish expecting matches!(event, Event::Unsigned(_)); event
		);
	}

	#[test]
	fn decode_simple_tiny() {
		for n in 0..=23 {
			decode_test!([0xE0 | n] => Ok(Event::Simple(x)) if x == n);
		}
	}

	#[test]
	fn encode_simple_tiny() {
		for n in 0..=23 {
			encode_test!(Event::Simple(n) => [0xE0 | n]);
		}
	}

	#[test]
	fn decode_simple_8bit() {
		for n in 24..=255 {
			decode_test!([0xF8, n] => Ok(Event::Simple(x)) if x == n);
		}
	}

	#[test]
	fn encode_simple_8bit() {
		for n in 24..=255 {
			encode_test!(Event::Simple(n) => [0xF8, n]);
		}
	}

	#[test]
	fn decode_float_64bit() {
		decode_test!(b"\xFB\x7F\xF0\x00\x00\x00\x00\x00\x00" => Ok(Event::Float(n)) if n == f64::INFINITY);
	}

	#[test]
	fn encode_float_64bit() {
		encode_test!(Event::Float(1.0000000000000002f64) => b"\xFB\x3F\xF0\x00\x00\x00\x00\x00\x01");
	}

	#[test]
	fn decode_float_32bit() {
		decode_test!(b"\xFA\x3F\x80\x00\x00" => Ok(Event::Float(n)) if n == 1.0);
	}

	#[test]
	fn encode_float_32bit() {
		encode_test!(Event::Float(0.999999940395355225f32 as f64) => b"\xFA\x3F\x7F\xFF\xFF");
	}

	#[test]
	fn decode_float_16bit() {
		decode_test!(b"\xF9\x00\x00" => Ok(Event::Float(n)) if n == 0.0);
	}

	#[test]
	fn encode_float_16bit() {
		encode_test!(Event::Float(f64::INFINITY) => b"\xF9\x7C\x00");
	}
}
