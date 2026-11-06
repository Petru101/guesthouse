//! A name-based GUID. The real one is made once at install, and any stable one does.

const OID_NAMESPACE: [u8; 16] =
    [0x6B, 0xA7, 0xB8, 0x12, 0x9D, 0xAD, 0x11, 0xD1, 0x80, 0xB4, 0x00, 0xC0, 0x4F, 0xD4, 0x30, 0xC8];
const SHA1_START: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
const SHA1_ROUND_CONSTANTS: [u32; 4] = [0x5A82_7999, 0x6ED9_EBA1, 0x8F1B_BCDC, 0xCA62_C1D6];
const BLOCK: usize = 64;

fn sha1_block(state: &mut [u32; 5], block: &[u8]) {
    let mut words = [0; 80];
    for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    for index in 16..80 {
        let mixed = words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16];
        words[index] = mixed.rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (index, word) in words.iter().enumerate() {
        let mix = match index / 20 {
            0 => (b & c) | (!b & d),
            2 => (b & c) | (b & d) | (c & d),
            _ => b ^ c ^ d,
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(mix)
            .wrapping_add(e)
            .wrapping_add(SHA1_ROUND_CONSTANTS[index / 20])
            .wrapping_add(*word);
        (e, d, c, b, a) = (d, c, b.rotate_left(30), a, next);
    }
    for (part, value) in state.iter_mut().zip([a, b, c, d, e]) {
        *part = part.wrapping_add(value);
    }
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % BLOCK != BLOCK - 8 {
        padded.push(0);
    }
    padded.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    let mut state = SHA1_START;
    padded.as_chunks::<BLOCK>().0.iter().for_each(|block| sha1_block(&mut state, block));
    let mut digest = [0; 20];
    for (bytes, part) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        bytes.copy_from_slice(&part.to_be_bytes());
    }
    digest
}

/// A version 5 UUID of `name`, in the usual text form, upper case, with braces.
pub fn name_based_guid(name: &str) -> String {
    let digest = sha1(&[&OID_NAMESPACE[..], name.as_bytes()].concat());
    let mut bytes: [u8; 16] = digest[..16].try_into().expect("sixteen bytes");
    bytes[6] = bytes[6] & 0x0F | 0x50;
    bytes[8] = bytes[8] & 0x3F | 0x80;
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    let part = |range: std::ops::Range<usize>| hex[range].concat();
    format!("{{{}-{}-{}-{}-{}}}", part(0..4), part(4..6), part(6..8), part(8..10), part(10..16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_the_standard_example() {
        let digest: String = sha1(b"abc").iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(digest, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn a_guid_is_stable_and_marked_version_5() {
        let guid = name_based_guid("seed");
        assert_eq!(guid, name_based_guid("seed"));
        assert_eq!(guid.len(), 38);
        assert_eq!(&guid[15..16], "5");
    }
}
