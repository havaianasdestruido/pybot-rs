//! A small MT19937 stream matching CPython's integer-seeded `random.Random` methods.
//!
//! This intentionally implements the used deterministic surface (`random`, `getrandbits`,
//! `choice`, `shuffle`) rather than attempting to emulate every Python seed type. A production
//! seed can be made from operating-system/time entropy by the caller; deterministic tests pass
//! an explicit integer.

/// MT19937 state and index, initialized using the same `init_by_array` procedure as CPython.
#[derive(Clone, Debug)]
pub struct PythonRandom { state: [u32; 624], index: usize }

impl PythonRandom {
    /// Seed from a signed integer, matching `random.Random(seed)` for CPython integer seeds.
    pub fn new(seed: i128) -> Self {
        let mut value = seed.unsigned_abs();
        let mut words = Vec::new();
        while value != 0 { words.push(value as u32); value >>= 32; }
        if words.is_empty() { words.push(0); }
        Self::from_key(&words)
    }

    fn from_key(key: &[u32]) -> Self {
        let mut rng = Self { state: [0; 624], index: 624 };
        rng.init_genrand(19_650_218);
        let mut i = 1usize; let mut j = 0usize;
        for _ in 0..624.max(key.len()) {
            let previous = rng.state[i - 1];
            rng.state[i] = (rng.state[i] ^ ((previous ^ (previous >> 30)).wrapping_mul(1_664_525)))
                .wrapping_add(key[j]).wrapping_add(j as u32);
            i += 1; j += 1;
            if i >= 624 { rng.state[0] = rng.state[623]; i = 1; }
            if j >= key.len() { j = 0; }
        }
        for _ in 0..623 {
            let previous = rng.state[i - 1];
            rng.state[i] = (rng.state[i] ^ ((previous ^ (previous >> 30)).wrapping_mul(1_566_083_941)))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 { rng.state[0] = rng.state[623]; i = 1; }
        }
        rng.state[0] = 0x8000_0000;
        rng
    }

    fn init_genrand(&mut self, seed: u32) {
        self.state[0] = seed;
        for i in 1..624 { self.state[i] = 1_812_433_253u32.wrapping_mul(self.state[i - 1] ^ (self.state[i - 1] >> 30)).wrapping_add(i as u32); }
        self.index = 624;
    }

    fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let y = (self.state[i] & 0x8000_0000) | (self.state[(i + 1) % 624] & 0x7fff_ffff);
                self.state[i] = self.state[(i + 397) % 624] ^ (y >> 1) ^ if y & 1 == 0 { 0 } else { 0x9908_b0df };
            }
            self.index = 0;
        }
        let mut y = self.state[self.index]; self.index += 1;
        y ^= y >> 11; y ^= (y << 7) & 0x9d2c_5680; y ^= (y << 15) & 0xefc6_0000; y ^= y >> 18; y
    }

    /// CPython's `random()` 53-bit fraction in `[0, 1)`.
    pub fn random(&mut self) -> f64 {
        let a = u64::from(self.next_u32() >> 5); let b = u64::from(self.next_u32() >> 6);
        ((a << 26) + b) as f64 / 9_007_199_254_740_992.0
    }

    /// Return the high `bits` of a randomly generated integer as a little-endian `u128`.
    ///
    /// CPython's `getrandbits(k)` returns a Python arbitrary-precision integer; this bounded
    /// port supports `0..=128`, enough for indices in this application. Values beyond the
    /// range return `None` rather than silently truncate.
    pub fn getrandbits(&mut self, bits: u32) -> Option<u128> {
        if bits > 128 { return None; }
        if bits == 0 { return Some(0); }
        let words = bits.div_ceil(32); let mut output = 0u128;
        for word_index in 0..words {
            let remaining = bits - word_index * 32;
            let take = remaining.min(32);
            let word = self.next_u32() >> (32 - take);
            output |= u128::from(word) << (word_index * 32);
        }
        Some(output)
    }

    /// CPython-style `_randbelow(n)`; `None` for an empty or unsupported range.
    pub fn randbelow(&mut self, upper: usize) -> Option<usize> {
        if upper == 0 { return None; }
        let bits = usize::BITS - upper.leading_zeros();
        loop {
            let candidate = self.getrandbits(bits)?;
            if candidate < upper as u128 { return usize::try_from(candidate).ok(); }
        }
    }

    /// Select one element exactly as CPython `choice` does (raises for an empty sequence;
    /// here that case is represented by `None`).
    pub fn choice<'a, T>(&mut self, values: &'a [T]) -> Option<&'a T> {
        let index = self.randbelow(values.len())?; values.get(index)
    }

    /// In-place Fisher–Yates shuffle matching CPython's `_randbelow`-based `shuffle`.
    pub fn shuffle<T>(&mut self, values: &mut [T]) {
        for i in (1..values.len()).rev() {
            if let Some(j) = self.randbelow(i + 1) { values.swap(i, j); }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;
    #[test] fn random_stream_matches_cpython_integer_seed_vectors() {
        for (seed, expected) in [(0, [0.8444218515250481, 0.7579544029403025, 0.420571580830845]),
            (1, [0.13436424411240122, 0.8474337369372327, 0.763774618976614]) ] {
            let mut rng = PythonRandom::new(seed);
            for value in expected { assert_eq!(rng.random(), value); }
        }
    }
    #[test] fn getrandbits_zero_and_seeded_words() {
        let mut rng = PythonRandom::new(42);
        assert_eq!(rng.getrandbits(0), Some(0));
        assert_eq!(rng.getrandbits(5), Some(20));
        assert_eq!(rng.getrandbits(32), Some(478_163_327));
        assert_eq!(rng.getrandbits(129), None);
    }
    #[test] fn choice_and_shuffle_match_cpython() {
        let mut rng = PythonRandom::new(42); let values = ["a", "b", "c", "d", "e"];
        assert_eq!(rng.choice(&values), Some(&"a"));
        assert_eq!(rng.choice::<i32>(&[]), None);
        let mut rng = PythonRandom::new(42); let mut values = [0,1,2,3,4,5,6,7,8,9];
        rng.shuffle(&mut values); assert_eq!(values, [7,3,2,8,5,6,9,4,0,1]);
    }
    #[test] fn negative_and_zero_integer_seeds_are_defined() {
        let mut a = PythonRandom::new(17); let mut b = PythonRandom::new(-17);
        assert_eq!(a.random(), b.random());
        let mut z = PythonRandom::new(0); assert_eq!(z.random(), 0.8444218515250481);
    }
}
