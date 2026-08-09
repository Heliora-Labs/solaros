// CRC32C (Castagnoli) -- used by ext4 metadata_csum.
// Reflected bitwise implementation; poly = 0x1EDC6F41 (reflected 0x82F63B78).
// Same semantics as the Linux kernel's crc32c(): no final xor, caller picks
// the initial value (usually 0xFFFF_FFFF).

const POLY: u32 = 0x82F6_3B78;

pub fn crc32c(init: u32, data: &[u8]) -> u32 {
    let mut c = init;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ POLY } else { c >> 1 };
        }
    }
    c
}
