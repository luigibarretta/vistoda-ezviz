//! CRC-32/MPEG-2 for PSI sections: polynomial 0x04C11DB7, no reflection,
//! initial value 0xFFFFFFFF, no final XOR.

pub(in crate::transport) fn crc32_mpeg2(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x04c1_1db7
            };
        }
    }
    crc
}
