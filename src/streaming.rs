use std::{
	cell::RefCell,
	collections::VecDeque,
	io::{Read, Write},
	num::NonZeroUsize,
};

use crate::{DecodeError, EncodeError};

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
pub struct StreamDecoder<T: Read> {
	source: RefCell<T>,
	input_buffer: RefCell<VecDeque<u8>>,
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
	Event(StreamEvent),
	Error(DecodeError),
	Needs(NonZeroUsize),
}

impl<T: Read> StreamDecoder<T> {
	pub fn new(source: T) -> Self {
		StreamDecoder {
			source: RefCell::new(source),
			input_buffer: RefCell::new(VecDeque::with_capacity(128)),
			pending: Vec::new(),
		}
	}

	/// Pull an event from the decoder.
	pub fn next_event(&mut self) -> Result<StreamEvent, DecodeError> {
		use TryNextEventOutcome::*;
		self.input_buffer.get_mut().make_contiguous();
		loop {
			match self.try_next_event() {
				Event(e) => return Ok(e),
				Error(e) => return Err(e),
				Needs(n) => self.extend_input_buffer(n)?,
			}
		}
	}

	fn extend_input_buffer(&self, by: NonZeroUsize) -> Result<(), DecodeError> {
		let mut buf = Vec::with_capacity(by.into());
		buf.resize(by.into(), 0);
		match self.source.borrow_mut().read_exact(&mut buf) {
			Ok(()) => (),
			Err(e) => {
				if e.kind() == std::io::ErrorKind::UnexpectedEof {
					return Err(DecodeError::Insufficient);
				} else {
					return Err(DecodeError::IoError(e));
				}
			}
		}
		self.input_buffer.borrow_mut().extend(buf.into_iter());
		Ok(())
	}

	fn try_next_event(&mut self) -> TryNextEventOutcome {
		use TryNextEventOutcome::*;
		let input = self.input_buffer.get_mut();
		if input.is_empty() {
			Needs(1.try_into().unwrap())
		} else {
			let (event, size) = {
				let input = {
					let slices = input.as_slices();
					assert_eq!(
						slices.1.len(),
						0,
						"try_next_event called with non-contiguous buffer"
					);
					slices.0
				};
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
							StreamEvent::Unsigned(match val {
								Some(x) => x,
								None => return Error(DecodeError::Malformed),
							}),
							offset,
						)
					}
					1 => {
						let (val, offset) = read_argument!();
						(
							StreamEvent::Signed(match val {
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
								match String::from_utf8(contents) {
									Ok(s) => (StreamEvent::TextString(s), len + offset),
									Err(e) => return Error(e.into()),
								}
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
								return Error(DecodeError::Malformed);
							}
						}
					}
					7 => match additional {
						n @ 0..=23 => (StreamEvent::Simple(n), 1),
						24 => {
							bounds_check!(1);
							match excess[0] {
								0..=23 => return Error(DecodeError::Malformed),
								n => (StreamEvent::Simple(n), 2),
							}
						}
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
						28..=30 => return Error(DecodeError::Malformed),
						31 => {
							match self.pending.pop() {
								// This is false because it's already been flipped for this item.
								Some(Pending::Break) | Some(Pending::UnknownLengthMap(false)) => (),
								_ => return Error(DecodeError::Malformed),
							}
							(StreamEvent::Break, 1)
						}
						32..=u8::MAX => unreachable!(),
					},
					8..=u8::MAX => unreachable!(),
				}
			};
			input.drain(0..size);
			Event(event)
		}
	}

	/// Check whether it is possible to end the decoding now.
	///
	/// This will report that it is not possible to end the decoding if there is excess data and the `ignore_excess` parameter is false.
	///
	/// Note that this method needs to read from the buffer in order to determine whether any data remains.
	/// This is only the case if `ignore_excess` is false.
	pub fn ready_to_finish(&self, ignore_excess: bool) -> Result<bool, std::io::Error> {
		if ignore_excess {
			Ok(self.pending.is_empty())
		} else {
			if self.input_buffer.borrow().is_empty() {
				if let Err(e) = self.extend_input_buffer(1.try_into().unwrap()) {
					match e {
						DecodeError::Insufficient => Ok(self.pending.is_empty()),
						DecodeError::IoError(ioe) => Err(ioe),
						_ => unreachable!(),
					}
				} else {
					Ok(false)
				}
			} else {
				Ok(false)
			}
		}
	}

	/// End the decoding.
	///
	/// This will return the [`StreamDecoder`] if there is excess data and the `ignore_excess` parameter is false.
	pub fn finish(self, ignore_excess: bool) -> Result<Option<Self>, std::io::Error> {
		if self.ready_to_finish(ignore_excess)? {
			Ok(None)
		} else {
			Ok(Some(self))
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
	/// Use one of the `interpret_signed` associated functions to resolve this.
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

	/// Create a [`StreamEvent::Signed`] or [`StreamEvent::Unsigned`] value.
	pub fn create_signed(val: i64) -> StreamEvent {
		if val.is_negative() {
			StreamEvent::Signed(val.abs_diff(-1))
		} else {
			StreamEvent::Unsigned(val as _)
		}
	}

	/// Create a [`StreamEvent::Signed`] or [`StreamEvent::Unsigned`] value.
	///
	/// Because this takes an [`i128`], it can express all the numbers CBOR can encode.
	/// However, some [`i128`]s cannot be encoded in basic CBOR integers.
	/// In this case, it will return [`None`].
	pub fn create_signed_wide(val: i128) -> Option<StreamEvent> {
		if val.is_negative() {
			match val.abs_diff(-1).try_into() {
				Ok(val) => Some(StreamEvent::Signed(val)),
				Err(_) => None,
			}
		} else {
			Some(StreamEvent::Unsigned(val as _))
		}
	}
}

/// A streaming CBOR encoder.
pub struct StreamEncoder<T: Write> {
	dest: T,
	pending: Vec<Pending>,
}

impl<T: Write> StreamEncoder<T> {
	pub fn new(dest: T) -> Self {
		StreamEncoder {
			dest,
			pending: Vec::new(),
		}
	}

	pub fn feed_event(&mut self, event: StreamEvent) -> Result<(), EncodeError> {
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

		match event {
			StreamEvent::Unsigned(n) => {
				write_initial_and_argument!(0, n);
			}
			StreamEvent::Signed(n) => {
				write_initial_and_argument!(1, n);
			}
			StreamEvent::ByteString(bytes) => {
				write_initial_and_argument!(2, bytes.len() as _);
				self.dest.write_all(&bytes)?;
			}
			StreamEvent::UnknownLengthByteString => {
				self.dest.write_all(&[0x5F])?;
				self.pending.push(Pending::Break);
			}
			StreamEvent::TextString(_) => todo!(),
			StreamEvent::UnknownLengthTextString => todo!(),
			StreamEvent::Array(_) => todo!(),
			StreamEvent::UnknownLengthArray => todo!(),
			StreamEvent::Map(_) => todo!(),
			StreamEvent::UnknownLengthMap => todo!(),
			StreamEvent::Tag(_) => todo!(),
			StreamEvent::Float(_) => todo!(),
			StreamEvent::Simple(_) => todo!(),
			StreamEvent::Break => match self.pending.pop() {
				Some(Pending::Break) => self.dest.write_all(&[0xFF])?,
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
			let mut decoder = StreamDecoder::new(Cursor::new($in));
			decode_test!(match decoder: $in => $out if $cond);
			decoder.finish(false).unwrap();
		};
		($in:expr => $out:pat) => {
			decode_test!($in => $out if true);
		};
		(small $in:expr) => {
			let mut decoder = StreamDecoder::new(Cursor::new($in));
			decode_test!(match decoder: $in => Err(DecodeError::Insufficient));
			assert!(!decoder.ready_to_finish(false).unwrap());
		};
	}

	macro_rules! encode_test {
		($($in:expr),+ => $out:expr) => {
			let mut buf = Vec::new();
			let mut encoder = StreamEncoder::new(Cursor::new(&mut buf));
			for event in [$($in),+] {
				encoder.feed_event(event).unwrap();
			}
			std::mem::drop(encoder);
			assert_eq!(buf, $out);
		}
	}

	#[test]
	fn decode_uint_tiny() {
		for i1 in 0..=0x17u8 {
			decode_test!([i1] => Ok(StreamEvent::Unsigned(i2)) if i2 == i1 as _);
		}
	}

	#[test]
	fn encode_uint_tiny() {
		for i in 0..=0x17u8 {
			encode_test!(StreamEvent::Unsigned(i as _) => [i]);
		}
	}

	#[test]
	fn decode_uint_8bit() {
		decode_test!([0x18u8, 0x01] => Ok(StreamEvent::Unsigned(0x01)));
	}

	#[test]
	fn decode_uint_8bit_bounds() {
		decode_test!(small b"\x18");
	}

	#[test]
	fn encode_uint_8bit() {
		encode_test!(StreamEvent::Unsigned(0x3F) => [0x18, 0x3F]);
	}

	#[test]
	fn decode_uint_16bit() {
		decode_test!([0x19u8, 0x01, 0x02] => Ok(StreamEvent::Unsigned(0x0102)));
	}

	#[test]
	fn decode_uint_16bit_bounds() {
		decode_test!(small b"\x19\x00");
	}

	#[test]
	fn encode_uint_16bit() {
		encode_test!(StreamEvent::Unsigned(0x1234) => [0x19, 0x12, 0x34]);
	}

	#[test]
	fn decode_uint_32bit() {
		decode_test!([0x1Au8, 0x01, 0x02, 0x03, 0x04] => Ok(StreamEvent::Unsigned(0x01020304)));
	}

	#[test]
	fn decode_uint_32bit_bounds() {
		decode_test!(small b"\x1A\x00\x00\x00");
	}

	#[test]
	fn encode_uint_32bit() {
		encode_test!(StreamEvent::Unsigned(0x12345678) => [0x1A, 0x12, 0x34, 0x56, 0x78]);
	}

	#[test]
	fn decode_uint_64bit() {
		decode_test!([0x1Bu8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08] => Ok(StreamEvent::Unsigned(0x0102030405060708)));
	}

	#[test]
	fn decode_uint_64bit_bounds() {
		decode_test!(small b"\x1B\x00\x00\x00\x00\x00\x00\x00");
	}

	#[test]
	fn encode_uint_64bit() {
		encode_test!(StreamEvent::Unsigned(0x123456789ABCDEF0) => [0x1B, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
	}

	#[test]
	fn decode_negint() {
		decode_test!([0x20u8] => Ok(StreamEvent::Signed(0)));
		decode_test!([0x37u8] => Ok(StreamEvent::Signed(0x17)));
		decode_test!([0x38, 0x01] => Ok(StreamEvent::Signed(0x01)));
		decode_test!([0x39, 0x01, 0x02] => Ok(StreamEvent::Signed(0x0102)));
		decode_test!([0x3A, 0x01, 0x02, 0x03, 0x04] => Ok(StreamEvent::Signed(0x01020304)));
		decode_test!([0x3B, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08] => Ok(StreamEvent::Signed(0x0102030405060708)));
		decode_test!(small b"\x3B\x00\x00\x00\x00\x00\x00\x00");
	}

	#[test]
	fn encode_negint() {
		encode_test!(StreamEvent::Signed(0) => [0x20]);
		encode_test!(StreamEvent::Signed(0x17) => [0x37]);
		encode_test!(StreamEvent::Signed(0xAA) => [0x38, 0xAA]);
		encode_test!(StreamEvent::Signed(0x0102) => [0x39, 0x01, 0x02]);
		encode_test!(StreamEvent::Signed(0x01020304) => [0x3A, 0x01, 0x02, 0x03, 0x04]);
		encode_test!(StreamEvent::Signed(0x010203040506) => [0x3B, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
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
	fn create_signed() {
		assert!(matches!(
			StreamEvent::create_signed(0),
			StreamEvent::Unsigned(0)
		));
		assert!(matches!(
			StreamEvent::create_signed(22),
			StreamEvent::Unsigned(22)
		));
		assert!(matches!(
			StreamEvent::create_signed(-1),
			StreamEvent::Signed(0)
		));
		assert!(matches!(
			StreamEvent::create_signed(-22),
			StreamEvent::Signed(21)
		));
	}

	#[test]
	fn decode_bytes() {
		decode_test!([0x40] => Ok(StreamEvent::ByteString(x)) if x == b"");
		decode_test!(b"\x45Hello" => Ok(StreamEvent::ByteString(x)) if x == b"Hello");
		decode_test!(b"\x58\x04Halo" => Ok(StreamEvent::ByteString(x)) if x == b"Halo");
		decode_test!(b"\x59\x00\x07Goodbye" => Ok(StreamEvent::ByteString(x)) if x == b"Goodbye");
		decode_test!(b"\x5A\x00\x00\x00\x0DLong message!" => Ok(StreamEvent::ByteString(x)) if x == b"Long message!");
		decode_test!(b"\x5B\x00\x00\x00\x00\x00\x00\x00\x01?" => Ok(StreamEvent::ByteString(x)) if x == b"?");
	}

	#[test]
	fn encode_bytes() {
		encode_test!(StreamEvent::ByteString(b"".to_vec()) => b"\x40");
		encode_test!(StreamEvent::ByteString(b"abcd".to_vec()) => b"\x44abcd");

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
				let mut encoder = StreamEncoder::new(Cursor::new(&mut output));
				encoder
					.feed_event(StreamEvent::ByteString(input.clone()))
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
		let mut decoder = StreamDecoder::new(Cursor::new(b"\x5F\x44abcd\x43efg\xFF"));
		decode_test!(match decoder: Ok(StreamEvent::UnknownLengthByteString));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(StreamEvent::ByteString(x)) if x == b"abcd");
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(StreamEvent::ByteString(x)) if x == b"efg");
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(StreamEvent::Break));
		assert!(decoder.ready_to_finish(false).unwrap());
	}

	#[test]
	fn decode_bytes_segmented_small() {
		let mut decoder = StreamDecoder::new(Cursor::new(b"\x5F\x44abcd"));
		decode_test!(match decoder: Ok(StreamEvent::UnknownLengthByteString));
		decode_test!(match decoder: Ok(StreamEvent::ByteString(x)) if x == b"abcd");
		decode_test!(match decoder: Err(DecodeError::Insufficient));
		assert!(!decoder.ready_to_finish(false).unwrap());
	}

	#[test]
	fn encode_bytes_segmented() {
		let mut output = Vec::new();
		let mut encoder = StreamEncoder::new(Cursor::new(&mut output));
		encoder
			.feed_event(StreamEvent::UnknownLengthByteString)
			.unwrap();
		assert!(!encoder.ready_to_finish());
		encoder
			.feed_event(StreamEvent::ByteString(b"abcd".to_vec()))
			.unwrap();
		assert!(!encoder.ready_to_finish());
		encoder
			.feed_event(StreamEvent::ByteString(b"efg".to_vec()))
			.unwrap();
		assert!(!encoder.ready_to_finish());
		encoder.feed_event(StreamEvent::Break).unwrap();
		assert!(encoder.ready_to_finish());
		assert_eq!(output, b"\x5F\x44abcd\x43efg\xFF");
	}

	#[test]
	fn decode_text() {
		decode_test!([0x60] => Ok(StreamEvent::TextString(x)) if x == "");
		decode_test!(b"\x65Hello" => Ok(StreamEvent::TextString(x)) if x == "Hello");
		decode_test!(b"\x78\x04Halo" => Ok(StreamEvent::TextString(x)) if x == "Halo");
		decode_test!(b"\x79\x00\x07Goodbye" => Ok(StreamEvent::TextString(x)) if x == "Goodbye");
		decode_test!(b"\x7A\x00\x00\x00\x0DLong message!" => Ok(StreamEvent::TextString(x)) if x == "Long message!");
		decode_test!(b"\x7B\x00\x00\x00\x00\x00\x00\x00\x01?" => Ok(StreamEvent::TextString(x)) if x == "?");
	}

	#[test]
	fn decode_text_64bit_bounds() {
		decode_test!(small b"\x7B\x00\x00\x00\x00\x00\x00\x00");
		decode_test!(small b"\x7B\x00\x00\x00\x00\x00\x00\x00\x01");
	}

	#[test]
	fn decode_text_segmented() {
		let mut decoder = StreamDecoder::new(Cursor::new(b"\x7F\x64abcd\x63efg\xFF"));
		decode_test!(match decoder: Ok(StreamEvent::UnknownLengthTextString));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(StreamEvent::TextString(x)) if x == "abcd");
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(StreamEvent::TextString(x)) if x == "efg");
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(StreamEvent::Break));
		assert!(decoder.ready_to_finish(false).unwrap());
	}

	#[test]
	fn decode_text_segmented_small() {
		let mut decoder = StreamDecoder::new(Cursor::new(b"\x7F\x64abcd"));
		decode_test!(match decoder: Ok(StreamEvent::UnknownLengthTextString));
		decode_test!(match decoder: Ok(StreamEvent::TextString(x)) if x == "abcd");
		decode_test!(match decoder: Err(DecodeError::Insufficient));
		assert!(!decoder.ready_to_finish(false).unwrap());
	}

	#[test]
	fn decode_text_invalid() {
		let mut decoder = StreamDecoder::new(Cursor::new(b"\x62\xFF\xFF"));
		match decoder.next_event() {
			Err(DecodeError::InvalidUtf8(_)) => (),
			_ => panic!("accepted invalid UTF-8"),
		}
	}

	#[test]
	fn decode_array() {
		let mut decoder = StreamDecoder::new(Cursor::new(b"\x84\0\x01\x02\x03"));
		decode_test!(match decoder: Ok(StreamEvent::Array(4)));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(decoder.ready_to_finish(false).unwrap());

		let mut decoder = StreamDecoder::new(Cursor::new(b"\x80"));
		decode_test!(match decoder: Ok(StreamEvent::Array(0)));
		assert!(decoder.ready_to_finish(false).unwrap());
	}

	#[test]
	fn decode_array_segmented() {
		let mut decoder = StreamDecoder::new(Cursor::new(b"\x9F\x00\x00\x00\xFF"));
		decode_test!(match decoder: Ok(StreamEvent::UnknownLengthArray));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(StreamEvent::Break));
		assert!(decoder.ready_to_finish(false).unwrap());
	}

	#[test]
	fn decode_map() {
		let mut decoder = StreamDecoder::new(Cursor::new(b"\xA2\x01\x02\x03\x04"));
		decode_test!(match decoder: Ok(StreamEvent::Map(2)));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(decoder.ready_to_finish(false).unwrap());

		let mut decoder = StreamDecoder::new(Cursor::new(b"\xA0"));
		decode_test!(match decoder: Ok(StreamEvent::Map(0)));
		assert!(decoder.ready_to_finish(false).unwrap());
	}

	#[test]
	fn decode_map_segmented() {
		let mut decoder = StreamDecoder::new(Cursor::new(b"\xBF\x00\x00\x00\x00\xFF"));
		decode_test!(match decoder: Ok(StreamEvent::UnknownLengthMap));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(StreamEvent::Break));
		assert!(decoder.ready_to_finish(false).unwrap());
	}

	#[test]
	fn decode_map_segmented_odd() {
		let mut decoder = StreamDecoder::new(Cursor::new(b"\xBF\x00\xFF"));
		decode_test!(match decoder: Ok(StreamEvent::UnknownLengthMap));
		decode_test!(match decoder: Ok(_));
		assert!(!decoder.ready_to_finish(false).unwrap());
	}

	#[test]
	fn decode_tag() {
		let mut decoder = StreamDecoder::new(Cursor::new(b"\xC1\x00"));
		decode_test!(match decoder: Ok(StreamEvent::Tag(1)));
		assert!(!decoder.ready_to_finish(false).unwrap());
		decode_test!(match decoder: Ok(_));
		assert!(decoder.ready_to_finish(false).unwrap());
	}

	#[test]
	fn decode_simple_tiny() {
		for n in 0..=23 {
			decode_test!([0xE0 | n] => Ok(StreamEvent::Simple(x)) if x == n);
		}
	}

	#[test]
	fn decode_simple_8bit() {
		for n in 24..=255 {
			decode_test!([0xF8, n] => Ok(StreamEvent::Simple(x)) if x == n);
		}
	}

	#[test]
	fn decode_float_64bit() {
		decode_test!(b"\xFB\x7F\xF0\x00\x00\x00\x00\x00\x00" => Ok(StreamEvent::Float(n)) if n == f64::INFINITY);
	}

	#[test]
	fn decode_float_32bit() {
		decode_test!(b"\xFA\x3F\x80\x00\x00" => Ok(StreamEvent::Float(n)) if n == 1.0);
	}

	#[test]
	fn decode_float_16bit() {
		decode_test!(b"\xF9\x00\x00" => Ok(StreamEvent::Float(n)) if n == 0.0);
	}
}
