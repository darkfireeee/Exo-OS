//! Endianness conversion utilities for filesystem operations

/// Convert little-endian to CPU native format
#[inline]
pub const fn le_to_cpu_u16(val: u16) -> u16 {
    u16::from_le(val)
}

#[inline]
pub const fn le_to_cpu_u32(val: u32) -> u32 {
    u32::from_le(val)
}

#[inline]
pub const fn le_to_cpu_u64(val: u64) -> u64 {
    u64::from_le(val)
}

/// Convert CPU native format to little-endian
#[inline]
pub const fn cpu_to_le_u16(val: u16) -> u16 {
    val.to_le()
}

#[inline]
pub const fn cpu_to_le_u32(val: u32) -> u32 {
    val.to_le()
}

#[inline]
pub const fn cpu_to_le_u64(val: u64) -> u64 {
    val.to_le()
}

/// Convert big-endian to CPU native format
#[inline]
pub const fn be_to_cpu_u16(val: u16) -> u16 {
    u16::from_be(val)
}

#[inline]
pub const fn be_to_cpu_u32(val: u32) -> u32 {
    u32::from_be(val)
}

#[inline]
pub const fn be_to_cpu_u64(val: u64) -> u64 {
    u64::from_be(val)
}

/// Convert CPU native format to big-endian
#[inline]
pub const fn cpu_to_be_u16(val: u16) -> u16 {
    val.to_be()
}

#[inline]
pub const fn cpu_to_be_u32(val: u32) -> u32 {
    val.to_be()
}

#[inline]
pub const fn cpu_to_be_u64(val: u64) -> u64 {
    val.to_be()
}

/// Read little-endian u32 from byte slice
#[inline]
pub fn read_le_u32(bytes: &[u8]) -> u32 {
    let mut arr = [0u8; 4];
    arr.copy_from_slice(&bytes[..4]);
    u32::from_le_bytes(arr)
}

/// Read little-endian u64 from byte slice
#[inline]
pub fn read_le_u64(bytes: &[u8]) -> u64 {
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(arr)
}

/// Write little-endian u32 to byte slice
#[inline]
pub fn write_le_u32(bytes: &mut [u8], val: u32) {
    bytes[..4].copy_from_slice(&val.to_le_bytes());
}

/// Write little-endian u64 to byte slice
#[inline]
pub fn write_le_u64(bytes: &mut [u8], val: u64) {
    bytes[..8].copy_from_slice(&val.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_le_conversions() {
        let val: u32 = 0x12345678;
        let le = cpu_to_le_u32(val);
        let back = le_to_cpu_u32(le);
        assert_eq!(val, back);
    }

    #[test]
    fn test_read_write_le() {
        let val: u32 = 0xDEADBEEF;
        let mut buf = [0u8; 4];
        write_le_u32(&mut buf, val);
        let read = read_le_u32(&buf);
        assert_eq!(val, read);
    }
}
