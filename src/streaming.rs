use std::collections::VecDeque;

use crate::DecodeError;

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

/// A streaming CBOR decoder with minimal logic.
///
/// [`StreamDecoder`] handles input buffering, retrying when new data arrives, and basic parsing.
/// It does not enforce higher-level rules, instead aiming to represent the input data as faithfully as possible.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
	input_buffer: VecDeque<u8>,
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

impl StreamDecoder {
	pub fn new() -> Self {
		StreamDecoder {
			input_buffer: VecDeque::with_capacity(128),
			pending: Vec::new(),
		}
	}

	/// Feed some data into the decoder.
	///
	/// The data provided will not be parsed until [`next_event`] is called.
	/// The return value is the total number of bytes in the internal buffer.
	pub fn feed(&mut self, data: impl Iterator<Item = u8>) -> usize {
		self.input_buffer.extend(data);
		self.input_buffer.len()
	}

	/// Feed some data into the decoder.
	///
	/// The data provided will not be parsed until [`next_event`] is called.
	/// The return value is the total number of bytes in the internal buffer.
	pub fn feed_ref<'a>(&mut self, data: impl Iterator<Item = &'a u8>) -> usize {
		self.feed(data.map(|x| *x))
	}

	/// Feed some data into the decoder.
	///
	/// The data provided will not be parsed until [`next_event`] is called.
	/// The return value is the total number of bytes in the internal buffer.
	pub fn feed_slice(&mut self, data: &[u8]) -> usize {
		self.feed_ref(data.into_iter())
	}

	/// Pull an event from the decoder.
	pub fn next_event(&mut self) -> Result<Option<StreamEvent>, DecodeError> {
		if self.input_buffer.is_empty() {
			Ok(None)
		} else {
			let (event, size) = {
				let input = self.input_buffer.make_contiguous();
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
								let contents = excess[offset - 1..len + offset - 1].to_owned();
								(StreamEvent::ByteString(contents), len + offset)
							}
							None => {
								self.pending.push(Pending::Break);
								(StreamEvent::UnknownLengthByteString, 1)
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
								let contents = String::from_utf8(contents)?;
								(StreamEvent::TextString(contents), len + offset)
							}
							None => {
								self.pending.push(Pending::Break);
								(StreamEvent::UnknownLengthTextString, 1)
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
								(StreamEvent::Array(len), offset)
							}
							None => {
								self.pending.push(Pending::Break);
								(StreamEvent::UnknownLengthArray, 1)
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
								(StreamEvent::Map(len), offset)
							}
							None => {
								self.pending.push(Pending::UnknownLengthMap(true));
								(StreamEvent::UnknownLengthMap, 1)
							}
						}
					}
					6 => {
						let (val, offset) = read_argument!();
						match val {
							Some(tag) => {
								self.pending.push(Pending::Tag);
								(StreamEvent::Tag(tag), offset)
							}
							None => {
								return Err(DecodeError::Malformed);
							}
						}
					}
					7 => match additional {
						n @ 0..=23 => (StreamEvent::Simple(n), 1),
						24 => {
							bounds_check!(1);
							match excess[0] {
								0..=23 => return Err(DecodeError::Malformed),
								n => (StreamEvent::Simple(n), 2),
							}
						}
						#[cfg(not(feature = "half"))]
						25 => {
							return Err(DecodeError::NoHalfFloatSupport);
						}
						#[cfg(feature = "half")]
						25 => {
							bounds_check!(2);
							let mut bytes = [0u8; 2];
							bytes.copy_from_slice(&excess[..2]);
							(
								StreamEvent::Float(half::f16::from_be_bytes(bytes).into()),
								3,
							)
						}
						26 => {
							bounds_check!(4);
							let mut bytes = [0u8; 4];
							bytes.copy_from_slice(&excess[..4]);
							(StreamEvent::Float(f32::from_be_bytes(bytes).into()), 5)
						}
						27 => {
							bounds_check!(8);
							let mut bytes = [0u8; 8];
							bytes.copy_from_slice(&excess[..8]);
							(StreamEvent::Float(f64::from_be_bytes(bytes)), 9)
						}
						28..=30 => return Err(DecodeError::Malformed),
						31 => {
							match self.pending.pop() {
								// This is false because it's already been flipped for this item.
								Some(Pending::Break) | Some(Pending::UnknownLengthMap(false)) => (),
								_ => return Err(DecodeError::Malformed),
							}
							(StreamEvent::Break, 1)
						}
						32..=u8::MAX => unreachable!(),
					},
					8..=u8::MAX => unreachable!(),
				}
			};
			self.input_buffer.drain(0..size);
			Ok(Some(event))
		}
	}

	/// Check whether it is possible to end the decoding now.
	///
	/// This will report that it is not possible to end the decoding if there is excess data and the `ignore_excess` parameter is false.
	pub fn ready_to_finish(&self, ignore_excess: bool) -> bool {
		if (self.input_buffer.is_empty() || ignore_excess) && self.pending.is_empty() {
			return true;
		} else {
			return false;
		}
	}

	/// End the decoding.
	///
	/// This will return the [`StreamDecoder`] if there is excess data and the `ignore_excess` parameter is false.
	pub fn finish(self, ignore_excess: bool) -> Result<(), Self> {
		if self.ready_to_finish(ignore_excess) {
			Ok(())
		} else {
			Err(self)
		}
	}
}

/// An event encountered while decoding CBOR.
#[derive(Debug, Clone)]
pub enum StreamEvent {
	/// An unsigned integer.
	Unsigned(u64),
	/// A signed integer in a slightly odd representation.
	///
	/// The actual value of the integer is -1 minus the provided value.
	/// Some integers that can be CBOR encoded underflow [`i64`].
	/// Use one of the `interpret_signed` associated functions if you don't care about that.
	Signed(u64),
	/// A byte string.
	ByteString(Vec<u8>),
	/// The start of a byte string whose length is unknown.
	///
	/// After this event come a series of `ByteString` events, followed by a `Break`.
	/// To get the true value of the byte string, concatenate the `ByteString` events together.
	UnknownLengthByteString,
	/// A text string.
	TextString(String),
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

impl StreamEvent {
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
			decoder.feed_ref($in.into_iter());
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
			assert!(!decoder.ready_to_finish(false));
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
	fn decode_negint() {
		decode_test!([0x20u8] => StreamEvent::Signed(0));
		decode_test!([0x37u8] => StreamEvent::Signed(0x17));
		decode_test!([0x38, 0x01] => StreamEvent::Signed(0x01));
		decode_test!([0x39, 0x01, 0x02] => StreamEvent::Signed(0x0102));
		decode_test!([0x3A, 0x01, 0x02, 0x03, 0x04] => StreamEvent::Signed(0x01020304));
		decode_test!([0x3B, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08] => StreamEvent::Signed(0x0102030405060708));
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
	fn decode_bytes() {
		decode_test!([0x40] => StreamEvent::ByteString(x) if x == b"");
		decode_test!(ref b"\x45Hello" => StreamEvent::ByteString(x) if x == b"Hello");
		decode_test!(ref b"\x58\x04Halo" => StreamEvent::ByteString(x) if x == b"Halo");
		decode_test!(ref b"\x59\x00\x07Goodbye" => StreamEvent::ByteString(x) if x == b"Goodbye");
		decode_test!(ref b"\x5A\x00\x00\x00\x0DLong message!" => StreamEvent::ByteString(x) if x == b"Long message!");
		decode_test!(ref b"\x5B\x00\x00\x00\x00\x00\x00\x00\x01?" => StreamEvent::ByteString(x) if x == b"?");
	}

	#[test]
	fn decode_bytes_segmented() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_ref(b"\x5F\x44abcd\x43efg\xFF".into_iter());
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthByteString));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(StreamEvent::ByteString(x)) if x == b"abcd");
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(StreamEvent::ByteString(x)) if x == b"efg");
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(StreamEvent::Break));
		assert!(decoder.ready_to_finish(false));
	}

	#[test]
	fn decode_bytes_segmented_small() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_ref(b"\x5F\x44abcd".into_iter());
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthByteString));
		decode_test!(match decoder: Some(StreamEvent::ByteString(x)) if x == b"abcd");
		decode_test!(match decoder: None);
		assert!(!decoder.ready_to_finish(false));
	}

	#[test]
	fn decode_text() {
		decode_test!([0x60] => StreamEvent::TextString(x) if x == "");
		decode_test!(ref b"\x65Hello" => StreamEvent::TextString(x) if x == "Hello");
		decode_test!(ref b"\x78\x04Halo" => StreamEvent::TextString(x) if x == "Halo");
		decode_test!(ref b"\x79\x00\x07Goodbye" => StreamEvent::TextString(x) if x == "Goodbye");
		decode_test!(ref b"\x7A\x00\x00\x00\x0DLong message!" => StreamEvent::TextString(x) if x == "Long message!");
		decode_test!(ref b"\x7B\x00\x00\x00\x00\x00\x00\x00\x01?" => StreamEvent::TextString(x) if x == "?");
	}

	#[test]
	fn decode_text_64bit_bounds() {
		decode_test!(small ref b"\x7B\x00\x00\x00\x00\x00\x00\x00");
		decode_test!(small ref b"\x7B\x00\x00\x00\x00\x00\x00\x00\x01");
	}

	#[test]
	fn decode_text_segmented() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_ref(b"\x7F\x64abcd\x63efg\xFF".into_iter());
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthTextString));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(StreamEvent::TextString(x)) if x == "abcd");
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(StreamEvent::TextString(x)) if x == "efg");
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(StreamEvent::Break));
		assert!(decoder.ready_to_finish(false));
	}

	#[test]
	fn decode_text_segmented_small() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_ref(b"\x7F\x64abcd".into_iter());
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthTextString));
		decode_test!(match decoder: Some(StreamEvent::TextString(x)) if x == "abcd");
		decode_test!(match decoder: None);
		assert!(!decoder.ready_to_finish(false));
	}

	#[test]
	fn decode_text_invalid() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_ref(b"\x62\xFF\xFF".into_iter());
		match decoder.next_event() {
			Err(DecodeError::InvalidUtf8(_)) => (),
			_ => panic!("accepted invalid UTF-8"),
		}
	}

	#[test]
	fn decode_array() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_slice(b"\x84\0\x01\x02\x03");
		decode_test!(match decoder: Some(StreamEvent::Array(4)));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(decoder.ready_to_finish(false));

		let mut decoder = StreamDecoder::new();
		decoder.feed_slice(b"\x80");
		decode_test!(match decoder: Some(StreamEvent::Array(0)));
		assert!(decoder.ready_to_finish(false));
	}

	#[test]
	fn decode_array_segmented() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_slice(b"\x9F\x00\x00\x00\xFF");
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthArray));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(StreamEvent::Break));
		assert!(decoder.ready_to_finish(false));
	}

	#[test]
	fn decode_map() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_slice(b"\xA2\x01\x02\x03\x04");
		decode_test!(match decoder: Some(StreamEvent::Map(2)));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(decoder.ready_to_finish(false));

		let mut decoder = StreamDecoder::new();
		decoder.feed_slice(b"\xA0");
		decode_test!(match decoder: Some(StreamEvent::Map(0)));
		assert!(decoder.ready_to_finish(false));
	}

	#[test]
	fn decode_map_segmented() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_slice(b"\xBF\x00\x00\x00\x00\xFF");
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthMap));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(StreamEvent::Break));
		assert!(decoder.ready_to_finish(false));
	}

	#[test]
	fn decode_map_segmented_odd() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_slice(b"\xBF\x00\xFF");
		decode_test!(match decoder: Some(StreamEvent::UnknownLengthMap));
		decode_test!(match decoder: Some(_));
		assert!(!decoder.ready_to_finish(false));
	}

	#[test]
	fn decode_tag() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_slice(b"\xC1\x00");
		decode_test!(match decoder: Some(StreamEvent::Tag(1)));
		assert!(!decoder.ready_to_finish(false));
		decode_test!(match decoder: Some(_));
		assert!(decoder.ready_to_finish(false));
	}

	#[test]
	fn decode_simple_tiny() {
		for n in 0..=23 {
			decode_test!([0xE0 | n] => StreamEvent::Simple(x) if x == n);
		}
	}

	#[test]
	fn decode_simple_8bit() {
		for n in 24..=255 {
			decode_test!([0xF8, n] => StreamEvent::Simple(x) if x == n);
		}
	}

	#[test]
	fn decode_float_64bit() {
		decode_test!(ref b"\xFB\x7F\xF0\x00\x00\x00\x00\x00\x00" => StreamEvent::Float(n) if n == f64::INFINITY);
	}

	#[test]
	fn decode_float_32bit() {
		decode_test!(ref b"\xFA\x3F\x80\x00\x00" => StreamEvent::Float(n) if n == 1.0);
	}

	#[cfg(not(feature = "half"))]
	#[test]
	fn decode_float_16bit() {
		let mut decoder = StreamDecoder::new();
		decoder.feed_slice(b"\xF9\x00\x00");
		match decoder.next_event() {
			Err(DecodeError::NoHalfFloatSupport) => (),
			x => panic!("got {:?} when decoding a half-float", x),
		}
	}

	#[cfg(feature = "half")]
	#[test]
	fn decode_float_16bit() {
		decode_test!(ref b"\xF9\x00\x00" => StreamEvent::Float(n) if n == 0.0);
	}
}
