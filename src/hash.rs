//! Hashes the port needs, std only.
//!
//! * **SHA-1** — the golden corpus manifest and every `memory_digest` use SHA-1
//!   (`tools/recon/gen_corpus.py`: `hashlib.sha1(blob).hexdigest()`). The Rust side must
//!   reproduce those bytes exactly or the differential report is meaningless, so SHA-1 is
//!   implemented here against the FIPS 180-4 test vectors. SHA-1 is used only as a content
//!   fingerprint, never for security (nothing in this project touches secrets).
//! * **SHA-256** — the corpus manifest records the SHA-256 of `corpus.jsonl`, and the
//!   end-to-end test recomputes it so a hand-edited corpus cannot slip past CI.
//! * **FNV-1a** — a fast, stable, dependency-free hash for internal caches and the
//!   `--filter` matching in the differential runner. Unlike `std::collections::hash_map`'s
//!   default hasher it is deterministic across processes and platforms.
//!
//! Both are `#![forbid(unsafe_code)]`, allocation-free and total: any input length is fine.

/// SHA-1 of the concatenation of `chunks` (any byte slices).
#[must_use]
pub fn sha1(chunks: &[&[u8]]) -> Sha1Digest {
    let mut hasher = Sha1::new();
    for chunk in chunks {
        hasher.update(chunk);
    }
    hasher.finish()
}

/// Incremental SHA-1 hasher.
#[derive(Clone, Debug)]
pub struct Sha1 {
    state: [u32; 5],
    buffer: [u8; 64],
    buffered: usize,
    length_bits: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    /// A fresh hasher with the standard initial state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: [
                0x6745_2301,
                0xEFCD_AB89,
                0x98BA_DCFE,
                0x1032_5476,
                0xC3D2_E1F0,
            ],
            buffer: [0; 64],
            buffered: 0,
            length_bits: 0,
        }
    }

    /// Absorb bytes. Procesing is block-wise; leftover bytes wait in the buffer.
    pub fn update(&mut self, mut bytes: &[u8]) {
        self.length_bits = self
            .length_bits
            .wrapping_add(u64::try_from(bytes.len()).unwrap_or(0).wrapping_mul(8));

        // Top up a partial block first.
        if self.buffered > 0 {
            let room = 64 - self.buffered;
            let take = room.min(bytes.len());
            if let (Some(target), Some(source)) = (
                self.buffer.get_mut(self.buffered..self.buffered + take),
                bytes.get(..take),
            ) {
                target.copy_from_slice(source);
            }
            self.buffered += take;
            bytes = bytes.get(take..).unwrap_or(&[]);
            if self.buffered == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            }
        }

        // Whole blocks straight from the input.
        while bytes.len() >= 64 {
            let (block, rest) = bytes.split_at(64);
            if let Ok(chunk) = <&[u8; 64]>::try_from(block) {
                self.compress(chunk);
            }
            bytes = rest;
        }

        // Keep the tail.
        if !bytes.is_empty() {
            if let Some(target) = self.buffer.get_mut(..bytes.len()) {
                target.copy_from_slice(bytes);
            }
            self.buffered = bytes.len();
        }
    }

    /// Finish and return the 20-byte digest.
    #[must_use]
    pub fn finish(mut self) -> Sha1Digest {
        let length_bits = self.length_bits;
        // Padding: 0x80, zeros, then the 64-bit big-endian length.
        self.update(&[0x80]);
        while self.buffered != 56 {
            self.update(&[0]);
        }
        // `update` kept counting padding into the length; restore the true length.
        self.length_bits = length_bits;
        let mut block = self.buffer;
        if let Some(tail) = block.get_mut(56..64) {
            tail.copy_from_slice(&length_bits.to_be_bytes());
        }
        self.compress(&block);
        let mut out = [0u8; 20];
        for (index, word) in self.state.iter().enumerate() {
            let start = index * 4;
            if let Some(slot) = out.get_mut(start..start + 4) {
                slot.copy_from_slice(&word.to_be_bytes());
            }
        }
        Sha1Digest(out)
    }

    // `a`, `b`, `c`, `d`, `e` are the working variables of SHA-1 (FIPS 180-4, section 6.1.3);
    // renaming them would make the code harder to check against the specification.
    #[allow(clippy::many_single_char_names)]
    fn compress(&mut self, block: &[u8; 64]) {
        // 64 bytes go in 16 big-endian words; `chunks_exact(4)` makes each chunk exactly four
        // bytes, so the conversion cannot fail and no index is ever out of range.
        let mut words = [0u32; 80];
        for (slot, chunk) in words.iter_mut().take(16).zip(block.chunks_exact(4)) {
            if let Ok(bytes) = <[u8; 4]>::try_from(chunk) {
                *slot = u32::from_be_bytes(bytes);
            }
        }
        // The message schedule: each word mixes the ones 3, 8, 14 and 16 places back, which is
        // why `split_at_mut` hands the finished prefix out read-only.
        for index in 16..80 {
            let (finished, rest) = words.split_at_mut(index);
            match (rest.first_mut(), schedule_word(finished, index)) {
                (Some(slot), Some(word)) => *slot = word,
                _ => break,
            }
        }

        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (index, word) in words.iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        let adds = [a, b, c, d, e];
        for (state, add) in self.state.iter_mut().zip(adds) {
            *state = state.wrapping_add(add);
        }
    }
}

/// `(w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16]).rotate_left(1)` for the message schedule.
///
/// Returns `None` when `words` does not reach 16 places back — impossible for the loop above,
/// which only asks for `i >= 16`, but expressed as a `Option` so the caller never indexes.
fn schedule_word(words: &[u32], index: usize) -> Option<u32> {
    let reach = |offset: usize| {
        index
            .checked_sub(offset)
            .and_then(|at| words.get(at))
            .copied()
    };
    let w16 = reach(16)?;
    let w14 = reach(14)?;
    let w8 = reach(8)?;
    let w3 = reach(3)?;
    Some((w3 ^ w8 ^ w14 ^ w16).rotate_left(1))
}

/// A SHA-1 digest (20 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha1Digest(pub [u8; 20]);

impl Sha1Digest {
    /// Lowercase hex, like Python's `hexdigest()`.
    #[must_use]
    pub fn hex(&self) -> String {
        let mut out = String::with_capacity(40);
        for byte in self.0 {
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(byte & 0x0F), 16).unwrap_or('0'));
        }
        out
    }

    /// The raw bytes.
    #[must_use]
    pub const fn bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

impl std::fmt::Display for Sha1Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}

/// SHA-256 of the concatenation of `chunks` (any byte slices).
///
/// FIPS 180-4, implemented the same way as [`sha1`]: block-wise, allocation-free, total. Used
/// to verify `tests/golden/manifest.json` against the corpus it describes.
#[must_use]
pub fn sha256(chunks: &[&[u8]]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    for chunk in chunks {
        hasher.update(chunk);
    }
    hasher.finish()
}

const SHA256_INITIAL: [u32; 8] = [
    0x6A09_E667,
    0xBB67_AE85,
    0x3C6E_F372,
    0xA54F_F53A,
    0x510E_527F,
    0x9B05_688C,
    0x1F83_D9AB,
    0x5BE0_CD19,
];

const SHA256_ROUNDS: [u32; 64] = [
    0x428A_2F98,
    0x7137_4491,
    0xB5C0_FBCF,
    0xE9B5_DBA5,
    0x3956_C25B,
    0x59F1_11F1,
    0x923F_82A4,
    0xAB1C_5ED5,
    0xD807_AA98,
    0x1283_5B01,
    0x2431_85BE,
    0x550C_7DC3,
    0x72BE_5D74,
    0x80DE_B1FE,
    0x9BDC_06A7,
    0xC19B_F174,
    0xE49B_69C1,
    0xEFBE_4786,
    0x0FC1_9DC6,
    0x240C_A1CC,
    0x2DE9_2C6F,
    0x4A74_84AA,
    0x5CB0_A9DC,
    0x76F9_88DA,
    0x983E_5152,
    0xA831_C66D,
    0xB003_27C8,
    0xBF59_7FC7,
    0xC6E0_0BF3,
    0xD5A7_9147,
    0x06CA_6351,
    0x1429_2967,
    0x27B7_0A85,
    0x2E1B_2138,
    0x4D2C_6DFC,
    0x5338_0D13,
    0x650A_7354,
    0x766A_0ABB,
    0x81C2_C92E,
    0x9272_2C85,
    0xA2BF_E8A1,
    0xA81A_664B,
    0xC24B_8B70,
    0xC76C_51A3,
    0xD192_E819,
    0xD699_0624,
    0xF40E_3585,
    0x106A_A070,
    0x19A4_C116,
    0x1E37_6C08,
    0x2748_774C,
    0x34B0_BCB5,
    0x391C_0CB3,
    0x4ED8_AA4A,
    0x5B9C_CA4F,
    0x682E_6FF3,
    0x748F_82EE,
    0x78A5_636F,
    0x84C8_7814,
    0x8CC7_0208,
    0x90BE_FFFA,
    0xA450_6CEB,
    0xBEF9_A3F7,
    0xC671_78F2,
];

/// Incremental SHA-256 hasher.
#[derive(Clone, Debug)]
pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffered: usize,
    length_bits: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// A fresh hasher with the standard initial state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: SHA256_INITIAL,
            buffer: [0; 64],
            buffered: 0,
            length_bits: 0,
        }
    }

    /// Absorb bytes. Processing is block-wise; leftover bytes wait in the buffer.
    pub fn update(&mut self, mut bytes: &[u8]) {
        self.length_bits = self
            .length_bits
            .saturating_add((bytes.len() as u64).saturating_mul(8));
        if self.buffered > 0 {
            let wanted = 64 - self.buffered;
            let taken = wanted.min(bytes.len());
            if let (Some(slot), Some(head)) = (
                self.buffer.get_mut(self.buffered..self.buffered + taken),
                bytes.get(..taken),
            ) {
                slot.copy_from_slice(head);
            }
            self.buffered += taken;
            bytes = bytes.get(taken..).unwrap_or_default();
            if self.buffered < 64 {
                return;
            }
            let block = self.buffer;
            self.compress(&block);
            self.buffered = 0;
        }
        let mut blocks = bytes.chunks_exact(64);
        for block in &mut blocks {
            if let Ok(block) = <[u8; 64]>::try_from(block) {
                self.compress(&block);
            }
        }
        let rest = blocks.remainder();
        if let Some(slot) = self.buffer.get_mut(..rest.len()) {
            slot.copy_from_slice(rest);
        }
        self.buffered = rest.len();
    }

    /// Finish and return the 32-byte digest.
    #[must_use]
    pub fn finish(mut self) -> Sha256Digest {
        let length_bits = self.length_bits;
        self.update(&[0x80]);
        while self.buffered != 56 {
            self.update(&[0]);
        }
        self.length_bits = length_bits;
        let mut block = self.buffer;
        if let Some(tail) = block.get_mut(56..64) {
            tail.copy_from_slice(&length_bits.to_be_bytes());
        }
        self.compress(&block);
        let mut out = [0u8; 32];
        for (index, word) in self.state.iter().enumerate() {
            let start = index * 4;
            if let Some(slot) = out.get_mut(start..start + 4) {
                slot.copy_from_slice(&word.to_be_bytes());
            }
        }
        Sha256Digest(out)
    }

    // `a`..`h` are the eight working variables of SHA-256 (FIPS 180-4, 6.2.2).
    #[allow(clippy::many_single_char_names)]
    fn compress(&mut self, block: &[u8; 64]) {
        let mut words = [0u32; 64];
        for (slot, chunk) in words.iter_mut().take(16).zip(block.chunks_exact(4)) {
            if let Ok(bytes) = <[u8; 4]>::try_from(chunk) {
                *slot = u32::from_be_bytes(bytes);
            }
        }
        for index in 16..64 {
            let (finished, rest) = words.split_at_mut(index);
            match (rest.first_mut(), sha256_schedule_word(finished, index)) {
                (Some(slot), Some(word)) => *slot = word,
                _ => break,
            }
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for (index, word) in words.iter().enumerate() {
            let constant = SHA256_ROUNDS.get(index).copied().unwrap_or(0);
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(constant)
                .wrapping_add(*word);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (state, add) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *state = state.wrapping_add(add);
        }
    }
}

/// The SHA-256 message schedule for `words[index]`, `index >= 16`.
fn sha256_schedule_word(words: &[u32], index: usize) -> Option<u32> {
    let reach = |offset: usize| {
        index
            .checked_sub(offset)
            .and_then(|at| words.get(at))
            .copied()
    };
    let w15 = reach(15)?;
    let w2 = reach(2)?;
    let w16 = reach(16)?;
    let w7 = reach(7)?;
    let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
    let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
    Some(w16.wrapping_add(s0).wrapping_add(w7).wrapping_add(s1))
}

/// A SHA-256 digest (32 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest(pub [u8; 32]);

impl Sha256Digest {
    /// Lowercase hex, like Python's `hexdigest()`.
    #[must_use]
    pub fn hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(byte & 0x0F), 16).unwrap_or('0'));
        }
        out
    }

    /// The raw bytes.
    #[must_use]
    pub const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}

/// FNV-1a, 64-bit: fast, stable, good enough for internal caches and filters.
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// FNV-1a, 32-bit: used where a compact, stable key is enough.
#[must_use]
pub fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C_9DC5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;

    fn sha1_hex(bytes: &[u8]) -> String {
        sha1(&[bytes]).hex()
    }

    // ------------------------------------------------------------------ SHA-1 vectors

    #[test]
    fn sha1_matches_the_fips_180_4_vectors() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha1_matches_the_million_a_vector() {
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            sha1_hex(&million),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }

    #[test]
    fn sha1_handles_every_length_around_block_and_padding_boundaries() {
        // `hashlib.sha1(b"a" * n).hexdigest()` from CPython 3.11, for the lengths where a
        // block buffer or the 0x80/56-byte padding rules change behaviour.
        let expected: [(usize, &str); 14] = [
            (0, "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
            (1, "86f7e437faa5a7fce15d1ddcb9eaeaea377667b8"),
            (55, "c1c8bbdc22796e28c0e15163d20899b65621d65a"),
            (56, "c2db330f6083854c99d4b5bfb6e8f29f201be699"),
            (57, "f08f24908d682555111be7ff6f004e78283d989a"),
            (63, "03f09f5b158a7a8cdad920bddc29b81c18a551f5"),
            (64, "0098ba824b5c16427bd7a1122a5a442a25ec644d"),
            (65, "11655326c708d70319be2610e8a57d9a5b959d3b"),
            (119, "ee971065aaa017e0632a8ca6c77bb3bf8b1dfc56"),
            (120, "f34c1488385346a55709ba056ddd08280dd4c6d6"),
            (127, "89d95fa32ed44a7c610b7ee38517ddf57e0bb975"),
            (128, "ad5b3fdbcb526778c2839d2f151ea753995e26a0"),
            (130, "e1cd437ec3e8a60db34e1d150a4fc73882d83b41"),
            (129, "d96debf1bdcbc896e6c134ea76e8141f40d78536"),
        ];
        for (length, digest) in expected {
            let input = "a".repeat(length);
            assert_eq!(sha1_hex(input.as_bytes()), digest, "length {length}");
        }
    }

    #[test]
    fn sha1_agrees_between_one_shot_and_chunked_updates() {
        let text = b"PyBot is 640,000 lines of rules. ".repeat(40);
        let one_shot = sha1(&[&text]);
        for chunk in [1usize, 7, 63, 64, 65, 127, 1000] {
            let mut hasher = Sha1::new();
            let mut start = 0;
            while start < text.len() {
                let end = (start + chunk).min(text.len());
                hasher.update(&text[start..end]);
                start = end;
            }
            assert_eq!(hasher.finish(), one_shot, "chunk size {chunk}");
        }
    }

    #[test]
    fn sha1_of_a_real_corpus_line_matches_the_manifest() {
        // The manifest's sha256 is checked elsewhere; here we pin one sha1 against CPython:
        //   hashlib.sha1(b'hello').hexdigest()
        assert_eq!(
            sha1_hex(b"hello"),
            "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
        );
        // And the type of digest used by memory_digest: canonical JSON of an empty dict.
        assert_eq!(sha1_hex(b"{}"), "bf21a9e8fbc5a3846fb05b4fa0859e0917b2202f");
    }

    #[test]
    fn sha1_digest_displays_and_exposes_bytes() {
        let digest = sha1(&[b"abc"]);
        assert_eq!(digest.to_string(), digest.hex());
        assert_eq!(digest.bytes().len(), 20);
        assert_eq!(digest.bytes()[0], 0xA9);
        assert_eq!(digest.bytes()[19], 0x9D);
        assert_eq!(
            Sha1Digest([0; 20]).hex(),
            "0000000000000000000000000000000000000000"
        );
        assert_eq!(
            format!("{:?}", Sha1Digest([0xAB; 20])),
            "Sha1Digest([171, 171, 171, 171, 171, 171, 171, 171, 171, 171, 171, 171, 171, 171, 171, 171, 171, 171, 171, 171])"
        );
    }

    #[test]
    fn sha1_hashes_disjoint_chunks_as_their_concatenation() {
        let left = b"line one\n".as_slice();
        let right = b"line two\n\r\t\0".as_slice();
        let joined: Vec<u8> = left.iter().chain(right.iter()).copied().collect();
        assert_eq!(sha1(&[left, right]), sha1(&[&joined]));
        assert_eq!(sha1(&[]), sha1(&[b""]), "no chunks == one empty chunk");
    }

    // ------------------------------------------------------------------ SHA-256

    fn sha256_hex(bytes: &[u8]) -> String {
        sha256(&[bytes]).hex()
    }

    #[test]
    fn sha256_matches_the_fips_180_4_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            sha256_hex(b"The quick brown fox jumps over the lazy dog"),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(
            sha256_hex(b"{}"),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
    }

    #[test]
    fn sha256_matches_the_million_a_vector() {
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256_hex(&million),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn sha256_handles_every_length_around_the_block_boundary() {
        // 55, 56, 63, 64, 65 and 128 bytes are where padding bugs live. Python anchors:
        for (length, expected) in [
            (
                55,
                "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
            ),
            (
                56,
                "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
            ),
            (
                63,
                "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34",
            ),
            (
                64,
                "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
            ),
            (
                65,
                "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0",
            ),
            (
                128,
                "6836cf13bac400e9105071cd6af47084dfacad4e5e302c94bfed24e013afb73e",
            ),
        ] {
            let bytes = vec![b'a'; length];
            assert_eq!(sha256_hex(&bytes), expected, "{length} bytes of 'a'");
        }
    }

    #[test]
    fn sha256_hashes_disjoint_chunks_as_their_concatenation() {
        let left = b"line one\n".as_slice();
        let right = b"line two\n\r\t\0".as_slice();
        let joined: Vec<u8> = left.iter().chain(right.iter()).copied().collect();
        assert_eq!(sha256(&[left, right]), sha256(&[&joined]));
        assert_eq!(sha256(&[]), sha256(&[b""]));
    }

    #[test]
    fn sha256_and_sha1_differ_on_the_same_input() {
        // A copy-paste bug that made SHA-256 call the SHA-1 code would pass the length check
        // and fail this one.
        for input in [b"".as_slice(), b"abc".as_slice(), b"hello".as_slice()] {
            assert_ne!(sha256(&[input]).hex(), sha1(&[input]).hex());
        }
        assert_eq!(sha256(&[b""]).hex().len(), 64);
        assert_eq!(sha256(&[b""]).bytes().len(), 32);
    }

    #[test]
    fn sha256_hex_and_display_agree_and_are_lowercase() {
        let digest = sha256(&[b"hello"]);
        assert_eq!(digest.hex(), digest.to_string());
        assert_eq!(digest.hex(), digest.hex().to_lowercase());
        assert_eq!(Sha256Digest([0; 32]).hex(), "0".repeat(64));
    }

    // ------------------------------------------------------------------ FNV

    #[test]
    fn fnv1a_matches_the_reference_vectors() {
        assert_eq!(fnv1a64(b""), 0xCBF2_9CE4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xAF63_DC4C_8601_EC8C);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_F739_67E8);
        assert_eq!(fnv1a32(b""), 0x811C_9DC5);
        assert_eq!(fnv1a32(b"a"), 0xE40C_292C);
        assert_eq!(fnv1a32(b"foobar"), 0xBF9C_F968);
    }

    #[test]
    fn fnv1a_is_stable_across_runs_and_input_shapes() {
        let first = fnv1a64("module name: greeting".as_bytes());
        let second = fnv1a64("module name: greeting".as_bytes());
        assert_eq!(first, second);
        assert_ne!(first, fnv1a64("module name: chat".as_bytes()));
        assert_ne!(fnv1a32(b"a"), fnv1a32(b"b"));
    }
}
