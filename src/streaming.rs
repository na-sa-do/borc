use std::cell::RefCell;
use std::collections::VecDeque;
use std::str::from_utf8 as str_from_utf8;

use crate::DecodeError;

fn read_be_u16(input: &[u8]) -> u16 {
	u16::from_be_bytes(input.split_at(2).0.try_into().unwrap())
}

fn read_be_u32(input: &[u8]) -> u32 {
	u32::from_be_bytes(input.split_at(4).0.try_into().unwrap())
}

fn read_be_u64(input: &[u8]) -> u64 {
	u64::from_be_bytes(input.split_at(8).0.try_into().unwrap())
}

/// A streaming CBOR decoder with minimal logic.
///
/// [`StreamDecoder`] handles input buffering, retrying when new data arrives, and basic parsing.
/// It does not enforce higher-level rules, instead aiming to represent the input data as faithfully as possible.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
	// the usize is the number of bytes to drain from the buffer before doing anything else
	input_buffer: RefCell<(VecDeque<u8>, usize)>,
	pending: Vec<Pending>,
}

#[derive(Debug, Clone)]
enum Pending {
	Break,
}

impl StreamDecoder {
	pub fn new() -> Self {
		StreamDecoder {
			input_buffer: RefCell::new((VecDeque::with_capacity(128), 0)),
			pending: Vec::new(),
		}
	}

	/// Feed some data into the decoder.
	///
	/// The data provided will not be parsed until [`next_event`] is called.
	/// The return value is the total number of bytes in the internal buffer.
	pub fn feed(&mut self, data: impl Iterator<Item = u8>) -> usize {
		let input = self.input_buffer.get_mut();
		input.0.extend(data);
		input.0.len()
	}

	/// Pull an event from the decoder.
	pub fn next_event(&mut self) -> Result<Option<StreamEvent>, DecodeError> {
		let input = self.input_buffer.get_mut();
		input.0.drain(0..input.1);
		input.1 = 0;
		if input.0.is_empty() {
			Ok(None)
		} else {
			let (event, size) = {
				let input = input.0.make_contiguous();
				let initial = input[0];
				let excess = &input[1..];
				let major = initial >> 5;
				let additional = initial & 0b11111;

				macro_rules! bounds_check {
					($bound:expr) => {
						if excess.len() < $bound {
							return Ok(None);
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
							28 | 29 | 30 => return Err(DecodeError::Malformed),
							31 => (None, 0),
							_ => unreachable!(),
						}
					};
				}

				match major {
					0 => {
						let (val, offset) = read_argument!();
						(
							StreamEvent::Unsigned(match val {
								Some(x) => x,
								None => return Err(DecodeError::Malformed),
							}),
							offset,
						)
					}
					1 => {
						let (val, offset) = read_argument!();
						(
							StreamEvent::Signed(match val {
								Some(x) => x,
								None => return Err(DecodeError::Malformed),
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
								(
									StreamEvent::ByteString(&excess[offset - 1..len + offset - 1]),
									len + offset,
								)
							}
							None => {
								self.pending.push(Pending::Break);
								(StreamEvent::UnknownLengthByteStringStart, 1)
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
								(
									StreamEvent::TextString(str_from_utf8(
										&excess[offset - 1..len + offset - 1],
									)?),
									len + offset,
								)
							}
							None => {
								self.pending.push(Pending::Break);
								(StreamEvent::UnknownLengthTextStringStart, 1)
							}
						}
					}
					4..=6 => todo!(),
					7 => match additional {
						0..=27 => todo!(),
						28..=30 => return Err(DecodeError::Malformed),
						31 => {
							match self.pending.pop() {
								Some(Pending::Break) => (),
								_ => return Err(DecodeError::Malformed),
							}
							(StreamEvent::Break, 1)
						}
						31..=u8::MAX => unreachable!(),
					},
					8..=u8::MAX => unreachable!(),
				}
			};
			input.1 = size;
			Ok(Some(event))
		}
	}

	/// Check whether it is possible to end the decoding now.
	pub fn ready_to_finish(&self) -> bool {
		let mut input = self.input_buffer.borrow_mut();
		// for some reason Rust doesn't like this if you get rid of the temporary
		let to_drain = input.1;
		input.0.drain(0..to_drain);
		input.1 = 0;

		if input.0.is_empty() && self.pending.is_empty() {
			return true;
		} else {
			return false;
		}
	}

	/// End the decoding.
	///
	/// This will report an error if there is excess data and the `ignore_excess` parameter is false.
	pub fn finish(self, ignore_excess: bool) -> Result<(), DecodeError> {
		if self.ready_to_finish() {
			Ok(())
		} else {
			todo!();
		}
	}
}

/// An event encountered while decoding CBOR.
#[derive(Debug, Clone)]
pub enum StreamEvent<'a> {
	/// An unsigned integer.
	Unsigned(u64),
	/// A signed integer in a slightly odd representation.
	///
	/// The actual value of the integer is -1 minus the provided value.
	/// Some integers that can be CBOR encoded underflow [`i64`].
	/// Use one of the `interpret_signed` associated functions if you don't care about that.
	Signed(u64),
	/// A byte string.
	ByteString(&'a [u8]),
	/// The start of a byte string whose length is unknown.
	///
	/// After this event come a series of `ByteString` events, followed by a `Break`.
	/// To get the true value of the byte string, concatenate the `ByteString` events together.
	UnknownLengthByteStringStart,
	/// A text string.
	TextString(&'a str),
	/// The start of a text string whose length is unknown.
	///
	/// After this event come a series of `TextString` events, followed by a `Break`.
	/// To get the true value of the text string, concatenate the `TextString` events together.
	UnknownLengthTextStringStart,
	/// The end of an unknown-length item.
	Break,
}

impl<'a> StreamEvent<'a> {
	/// Interpret a [`StreamEvent::Signed`] value.
	///
	/// # Overflow behavior
	///
	/// On overflow, this function will panic if overflow checks are enabled (default in debug mode)
	/// and wrap if overflow checks are disabled (default in release mode).
	pub fn interpret_signed(val: u64) -> i64 {
		-1 - (val as i64)
	}

	/// Interpret a [`StreamEvent::Signed`] value.
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

	/// Interpret a [`StreamEvent::Signed`] value.
	///
	/// # Overflow behavior
	///
	/// This function does not overflow, because it returns an [`i128`].
	pub fn interpret_signed_wide(val: u64) -> i128 {
		-1 - (val as i128)
	}
}

#[cfg(test)]
mod test {
	use super::*;

	macro_rules! decode_test {
		(match $decoder:ident: $in:expr => $out:pat if $cond:expr) => {
			match $decoder.next_event() {
				Ok($out) if $cond => (),
				other => panic!(concat!("{:X?} -> {:X?} instead of ", stringify!($out), " if ", stringify!($cond)), $in, other),
			}
		};
		(match $decoder:ident: $in:expr => $out:pat) => {
			decode_test!(match $decoder: $in => $out if true);
		};
		(match $decoder:ident: $out:pat if $cond:expr) => {
			match $decoder.next_event() {
				Ok($out) if $cond => (),
				other => panic!("? -> {:X?}", other),
			}
		};
		(match $decoder:ident: $out:pat) => {
			decode_test!(match $decoder: $out if true);
		};
		($in:expr => $out:pat if $cond:expr) => {
			let mut decoder = StreamDecoder::new();
			decoder.feed($in.into_iter());
			decode_test!(match decoder: $in => Some($out) if $cond);
			decoder.finish(false).unwrap();
		};
		($in:expr => $out:pat) => {
			decode_test!($in => $out if true);
		};
		(ref $in:expr => $out:pat if $cond:expr) => {
			let mut decoder = StreamDecoder::new();
			decoder.feed($in.into_iter().map(|x| *x));
			decode_test!(match decoder: $in => Some($out) if $cond);
			decoder.finish(false).unwrap();
		};
		(ref $in:expr => $out:pat) => {
			decode_test!(ref $in => $out if true);
		};
		(small $in:expr) => {
			let mut decoder = StreamDecoder::new();
			decoder.feed($in.into_iter());
			decode_test!(match decoder: $in => None);
			assert!(!decoder.ready_to_finish());
		};
		(small ref $in:expr) => {
			let mut decoder = StreamDecoder::new();
			decoder.feed($in.into_iter().map(|x| *x));
			decode_test!(match decoder: $in => None);
			assert!(!decoder.ready_to_finish());
		};
	}

	#[test]
	fn decode_uint_tiny() {
		for i1 in 0..=0x17u8 {
			decode_test!([i1] => StreamEvent::Unsigned(i2) if i2 == i1 as _);
		}
	}

	#[test]
	fn decode_uint_8bit() {
		decode_test!([0x18u8, 0x01] => StreamEvent::Unsigned(0x01));
	}

	#[test]
	fn decode_uint_8bit_bounds() {
		decode_test!(small ref b"\x18");
	}

	#[test]
	fn decode_uint_16bit() {
		decode_test!([0x19u8, 0x01, 0x02] => StreamEvent::Unsigned(0x0102));
	}

	#[test]
	fn decode_uint_16bit_bounds() {
		decode_test!(small ref b"\x19\x00");
	}

	#[test]
	fn decode_uint_32bit() {
		decode_test!([0x1Au8, 0x01, 0x02, 0x03, 0x04] => StreamEvent::Unsigned(0x01020304));
	}

	#[test]
	fn decode_uint_32bit_bounds() {
		decode_test!(small ref b"\x1A\x00\x00\x00");
	}

	#[test]
	fn decode_uint_64bit() {
		decode_test!([0x1Bu8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08] => StreamEvent::Unsigned(0x0102030405060708));
	}

	#[test]
	fn decode_uint_64bit_bounds() {
		decode_test!(small ref b"\x1B\x00\x00\x00\x00\x00\x00\x00");
	}

	#[test]
	fn decode_negint_tiny() {
		decode_test!([0x20u8] => StreamEvent::Signed(0));
		decode_test!([0x37u8] => StreamEvent::Signed(0x17));
	}

	#[test]
	fn decode_negint_8bit() {
		decode_test!([0x38, 0x01] => StreamEvent::Signed(0x01));
	}

	#[test]
	fn decode_negint_8bit_bounds() {
		decode_test!(small ref b"\x38");
	}

	#[test]
	fn decode_negint_16bit() {
		decode_test!([0x39, 0x01, 0x02] => StreamEvent::Signed(0x0102));
	}

	#[test]
	fn decode_negint_16bit_bounds() {
		decode_test!(small ref b"\x39\x00");
	}

	#[test]
	fn decode_negint_32bit() {
		decode_test!([0x3A, 0x01, 0x02, 0x03, 0x04] => StreamEvent::Signed(0x01020304));
	}

	#[test]
	fn decode_negint_32bit_bounds() {
		decode_test!(small ref b"\x3A\x00\x00\x00");
	}

	#[test]
	fn decode_negint_64bit() {
		decode_test!([0x3B, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08] => StreamEvent::Signed(0x0102030405060708));
	}

	#[test]
	fn decode_negint_64bit_bounds() {
		decode_test!(small ref b"\x3B\x00\x00\x00\x00\x00\x00\x00");
	}

	#[test]
	fn interpret_signed() {
		assert_eq!(StreamEvent::interpret_signed(0), -1);
		assert_eq!(StreamEvent::interpret_signed_checked(0), Some(-1));
		assert_eq!(StreamEvent::interpret_signed_checked(u64::MAX), None);
		assert_eq!(StreamEvent::interpret_signed_wide(0), -1);
		assert_eq!(
			StreamEvent::interpret_signed_wide(u64::MAX),
			-1 - u64::MAX as i128
		);
	}

	#[test]
	fn decode_bytes_tiny() {
		decode_test!([0x40] => StreamEvent::ByteString(bytes) if bytes.len() == 0);
		decode_test!(ref b"\x45Hello" => StreamEvent::ByteString(bytes) if bytes == b"Hello");
	}

	#[test]
	fn decode_bytes_8bit() {
		decode_test!(ref b"\x58\x04Halo" => StreamEvent::ByteString(bytes) if bytes == b"Halo");
	}

	#[test]
	fn decode_bytes_8bit_bounds() {
		decode_test!(small ref b"\x58");
		decode_test!(small ref b"\x58\x01");
	}

	#[test]
	fn decode_bytes_16bit() {
		decode_test!(ref b"\x59\x00\x07Goodbye" => StreamEvent::ByteString(bytes) if bytes == b"Goodbye");
	}

	#[test]
	fn decode_bytes_16bit_bounds() {
		decode_test!(small ref b"\x59\x00");
		decode_test!(small ref b"\x59\x00\x01");
	}

	#[test]
	fn decode_bytes_32bit() {
		decode_test!(ref b"\x5A\x00\x00\x00\x0DLong message!" => StreamEvent::ByteString(bytes) if bytes == b"Long message!");
	}

	#[test]
	fn decode_bytes_32bit_bounds() {
		decode_test!(small ref b"\x5A\x00\x00\x00");
		decode_test!(small ref b"\x5A\x00\x00\x00\x01");
	}

	#[test]
	fn decode_bytes_64bit() {
		decode_test!(ref b"\x5B\x00\x00\x00\x00\x00\x00\x00\x01?" => StreamEvent::ByteString(bytes) if bytes == b"?");
	}

	#[test]
	fn decode_bytes_64bit_bounds() {
		decode_test!(small ref b"\x5B\x00\x00\x00\x00\x00\x00\x00");
		decode_test!(small ref b"\x5B\x00\x00\x00\x00\x00\x00\x00\x01");
	}

	#[test]
	fn decode_bytes_segmented() {
		let mut decoder = StreamDecoder::new();
		decoder.feed(b"\x5F\x44abcd\x43efg\xFF".into_iter().map(|x| *x));
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthByteStringStart));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Some(StreamEvent::ByteString(b"abcd")));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Some(StreamEvent::ByteString(b"efg")));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Some(StreamEvent::Break));
		assert!(decoder.ready_to_finish());
	}

	#[test]
	fn decode_bytes_segmented_small() {
		let mut decoder = StreamDecoder::new();
		decoder.feed(b"\x5F\x44abcd".into_iter().map(|x| *x));
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthByteStringStart));
		decode_test!(match decoder: Some(StreamEvent::ByteString(b"abcd")));
		decode_test!(match decoder: None);
		assert!(!decoder.ready_to_finish());
	}

	#[test]
	fn decode_text_tiny() {
		decode_test!([0x60] => StreamEvent::TextString(text) if text.len() == 0);
		decode_test!(ref b"\x65Hello" => StreamEvent::TextString(text) if text == "Hello");
	}

	#[test]
	fn decode_text_8bit() {
		decode_test!(ref b"\x78\x04Halo" => StreamEvent::TextString(text) if text == "Halo");
	}

	#[test]
	fn decode_text_8bit_bounds() {
		decode_test!(small ref b"\x78");
		decode_test!(small ref b"\x78\x01");
	}

	#[test]
	fn decode_text_16bit() {
		decode_test!(ref b"\x79\x00\x07Goodbye" => StreamEvent::TextString(text) if text == "Goodbye");
	}

	#[test]
	fn decode_text_16bit_bounds() {
		decode_test!(small ref b"\x79\x00");
		decode_test!(small ref b"\x79\x00\x01");
	}

	#[test]
	fn decode_text_32bit() {
		decode_test!(ref b"\x7A\x00\x00\x00\x0DLong message!" => StreamEvent::TextString(text) if text == "Long message!");
	}

	#[test]
	fn decode_text_32bit_bounds() {
		decode_test!(small ref b"\x7A\x00\x00\x00");
		decode_test!(small ref b"\x7A\x00\x00\x00\x01");
	}

	#[test]
	fn decode_text_64bit() {
		decode_test!(ref b"\x7B\x00\x00\x00\x00\x00\x00\x00\x01?" => StreamEvent::TextString(text) if text == "?");
	}

	#[test]
	fn decode_text_64bit_bounds() {
		decode_test!(small ref b"\x7B\x00\x00\x00\x00\x00\x00\x00");
		decode_test!(small ref b"\x7B\x00\x00\x00\x00\x00\x00\x00\x01");
	}

	#[test]
	fn decode_text_segmented() {
		let mut decoder = StreamDecoder::new();
		decoder.feed(b"\x7F\x64abcd\x63efg\xFF".into_iter().map(|x| *x));
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthTextStringStart));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Some(StreamEvent::TextString("abcd")));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Some(StreamEvent::TextString("efg")));
		assert!(!decoder.ready_to_finish());
		decode_test!(match decoder: Some(StreamEvent::Break));
		assert!(decoder.ready_to_finish());
	}

	#[test]
	fn decode_text_segmented_small() {
		let mut decoder = StreamDecoder::new();
		decoder.feed(b"\x7F\x64abcd".into_iter().map(|x| *x));
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthTextStringStart));
		decode_test!(match decoder: Some(StreamEvent::TextString("abcd")));
		decode_test!(match decoder: None);
		assert!(!decoder.ready_to_finish());
	}
}
