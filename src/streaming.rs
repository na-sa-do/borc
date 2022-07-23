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
}

impl StreamDecoder {
    pub fn new() -> Self {
        StreamDecoder {
            input_buffer: VecDeque::with_capacity(128),
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
					_ => todo!(),
				}
			};
			self.input_buffer.drain(0..size);
			Ok(Some(event))
        }
    }

    /// End the decoding.
    ///
    /// This will report an error if there is excess data and the `ignore_excess` parameter is false.
    pub fn finish(self, ignore_excess: bool) -> Result<(), DecodeError> {
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
    Unsigned(u64),
}

#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn decode_uint_tiny() {
		for i1 in 0..=0x17u8 {
			let mut decoder = StreamDecoder::new();
			decoder.feed([i1].into_iter());
			match decoder.next_event() {
				Ok(Some(StreamEvent::Unsigned(i2))) if i2 == i1 as _ => (),
				other => panic!("{} -> {:?}", i1, other),
			}
			decoder.finish(false).unwrap();
		}
	}

	#[test]
	fn decode_uint_8bit() {
		for i1 in 0x0..=0xFFu8 {
			let mut decoder = StreamDecoder::new();
			decoder.feed([0x18, i1].into_iter());
			match decoder.next_event() {
				Ok(Some(StreamEvent::Unsigned(i2))) if i2 == i1 as _ => (),
				other => panic!("{} -> {:?}", i1, other),
			}
			decoder.finish(false).unwrap();
		}
	}

	#[test]
	fn decode_uint_16bit() {
		for i1 in 0x0..=0xFFu16 {
			for offset in [0, 8] {
				let i1 = i1 << offset;
				let mut decoder = StreamDecoder::new();
				decoder.feed([0x19].into_iter());
				decoder.feed(u16::to_be_bytes(i1).into_iter());
				match decoder.next_event() {
					Ok(Some(StreamEvent::Unsigned(i2))) if i2 == i1 as _ => (),
					other => panic!("{} -> {:?}", i1, other),
				}
				decoder.finish(false).unwrap();
			}
		}
	}

	#[test]
	fn decode_uint_32bit() {
		for i1 in 0x0..=0xFFu32 {
			for offset in [0, 8, 16, 24] {
				let i1 = i1 << offset;
				let mut decoder = StreamDecoder::new();
				decoder.feed([0x1A].into_iter());
				decoder.feed(u32::to_be_bytes(i1).into_iter());
				match decoder.next_event() {
					Ok(Some(StreamEvent::Unsigned(i2))) if i2 == i1 as _ => (),
					other => panic!("{} -> {:?}", i1, other),
				}
				decoder.finish(false).unwrap();
			}
		}
	}

	#[test]
	fn decode_uint_64bit() {
		for i1 in 0x0..=0xFFu64 {
			for offset in [0, 8, 16, 24, 32, 40, 48, 56] {
				let i1 = i1 << offset;
				let mut decoder = StreamDecoder::new();
				decoder.feed([0x1B].into_iter());
				decoder.feed(u64::to_be_bytes(i1).into_iter());
				match decoder.next_event() {
					Ok(Some(StreamEvent::Unsigned(i2))) if i2 == i1 as _ => (),
					other => panic!("{} -> {:?}", i1, other),
				}
				decoder.finish(false).unwrap();
			}
		}
	}
}