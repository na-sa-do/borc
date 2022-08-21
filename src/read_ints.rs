pub(crate) fn read_be_u16(input: &[u8]) -> u16 {
	let mut bytes = [0u8; 2];
	bytes.copy_from_slice(&input[..2]);
	u16::from_be_bytes(bytes)
}

pub(crate) fn read_be_u32(input: &[u8]) -> u32 {
	let mut bytes = [0u8; 4];
	bytes.copy_from_slice(&input[..4]);
	u32::from_be_bytes(bytes)
}

pub(crate) fn read_be_u64(input: &[u8]) -> u64 {
	let mut bytes = [0u8; 8];
	bytes.copy_from_slice(&input[..8]);
	u64::from_be_bytes(bytes)
}
