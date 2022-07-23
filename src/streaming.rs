use std::collections::VecDeque;

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
    input_buffer: VecDeque<u8>,
	ignore_before_next_event: usize,
}

impl StreamDecoder {
    pub fn new() -> Self {
        StreamDecoder {
            input_buffer: VecDeque::with_capacity(128),
			ignore_before_next_event: 0,
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

    /// Pull an event from the decoder.
    pub fn next_event(&mut self) -> Result<Option<StreamEvent>, DecodeError> {
		self.input_buffer.drain(0..self.ignore_before_next_event);
        if self.input_buffer.is_empty() {
            Ok(None)
        } else {
            let (event, size) = {
            	let input = self.input_buffer.make_contiguous();
				match input[0] {
					n if n <= 0x17 => (StreamEvent::Unsigned(n as _), 1),
					0x18 => (StreamEvent::Unsigned(input[1] as _), 2),
					0x19 => (StreamEvent::Unsigned(read_be_u16(&input[1..]) as _), 3),
					0x1A => (StreamEvent::Unsigned(read_be_u32(&input[1..]) as _), 5),
					0x1B => (StreamEvent::Unsigned(read_be_u64(&input[1..]) as _), 9),
					0x1C..=0x1F => return Err(DecodeError::Malformed),
					n if n <= 0x37 => (StreamEvent::Signed((n - 0x20) as _), 1),
					0x38 => (StreamEvent::Signed(input[1] as _), 2),
					0x39 => (StreamEvent::Signed(read_be_u16(&input[1..]) as _), 3),
					0x3A => (StreamEvent::Signed(read_be_u32(&input[1..]) as _), 5),
					0x3B => (StreamEvent::Signed(read_be_u64(&input[1..]) as _), 9),
					0x3C..=0x3F => return Err(DecodeError::Malformed),
					_ => todo!(),
				}
			};
			self.ignore_before_next_event = size;
			Ok(Some(event))
        }
    }

    /// End the decoding.
    ///
    /// This will report an error if there is excess data and the `ignore_excess` parameter is false.
    pub fn finish(mut self, ignore_excess: bool) -> Result<(), DecodeError> {
		self.input_buffer.drain(0..self.ignore_before_next_event);
        if self.input_buffer.is_empty() {
			Ok(())
		} else {
			todo!();
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
		($in:expr => $out:pat if $cond:expr) => {
			let mut decoder = StreamDecoder::new();
			decoder.feed($in.into_iter());
			match decoder.next_event() {
				Ok(Some($out)) if $cond => (),
				other => panic!("{:?} -> {:?}", $in, other),
			}
			decoder.finish(false).unwrap();
		};
		($in:expr => $out:pat) => {
			decode_test!($in => $out if true);
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
	fn decode_uint_16bit() {
		decode_test!([0x19u8, 0x01, 0x02] => StreamEvent::Unsigned(0x0102));
	}

	#[test]
	fn decode_uint_32bit() {
		decode_test!([0x1Au8, 0x01, 0x02, 0x03, 0x04] => StreamEvent::Unsigned(0x01020304));
	}

	#[test]
	fn decode_uint_64bit() {
		decode_test!([0x1Bu8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08] => StreamEvent::Unsigned(0x0102030405060708));
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
	fn decode_negint_16bit() {
		decode_test!([0x39, 0x01, 0x02] => StreamEvent::Signed(0x0102));
	}

	#[test]
	fn decode_negint_32bit() {
		decode_test!([0x3A, 0x01, 0x02, 0x03, 0x04] => StreamEvent::Signed(0x01020304));
	}

	#[test]
	fn decode_negint_64bit() {
		decode_test!([0x3B, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08] => StreamEvent::Signed(0x0102030405060708));
	}

	#[test]
	fn interpret_signed() {
		assert_eq!(StreamEvent::interpret_signed(0), -1);
		assert_eq!(StreamEvent::interpret_signed_checked(0), Some(-1));
		assert_eq!(StreamEvent::interpret_signed_checked(u64::MAX), None);
		assert_eq!(StreamEvent::interpret_signed_wide(0), -1);
		assert_eq!(StreamEvent::interpret_signed_wide(u64::MAX), -1 - u64::MAX as i128);
	}
}