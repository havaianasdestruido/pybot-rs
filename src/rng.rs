//! A small MT19937 stream matching CPython's integer-seeded `random.Random` methods.
//!
//! This intentionally implements the used deterministic surface (`random`, `getrandbits`,
//! `choice`, `shuffle`) rather than attempting to emulate every Python seed type. A production
//! seed can be made from operating-system/time entropy by the caller; deterministic tests pass
//! an explicit integer.

/// MT19937 state and index, initialized using the same `init_by_array` procedure as CPython.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonRandom {
    state: [u32; 624],
    index: usize,
}

impl PythonRandom {
    /// Seed from a signed integer, matching `random.Random(seed)` for CPython integer seeds.
    pub fn new(seed: i128) -> Self {
        let mut value = seed.unsigned_abs();
        let mut words = Vec::new();
        while value != 0 {
            words.push(value as u32);
            value >>= 32;
        }
        if words.is_empty() {
            words.push(0);
        }
        Self::from_key(&words)
    }

    /// Reseed this generator with the same integer semantics as [`Self::new`].
    pub fn seed(&mut self, seed: i128) {
        *self = Self::new(seed);
    }

    /// Seed from arbitrary-size little-endian 32-bit limbs, as used by CPython's integer seeding.
    /// An empty limb slice denotes zero.
    pub fn seed_words(&mut self, words: &[u32]) {
        let zero = [0_u32];
        *self = Self::from_key(if words.is_empty() { &zero } else { words });
    }

    /// Construct a generator seeded from system time and the process identifier.
    /// This is intended for varied production sessions, not cryptographic security.
    #[must_use]
    pub fn from_entropy() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = u128::from(std::process::id());
        let seed = i128::try_from(nanos ^ pid.rotate_left(47)).unwrap_or(i128::MAX);
        Self::new(seed)
    }

    fn from_key(key: &[u32]) -> Self {
        let mut rng = Self {
            state: [0; 624],
            index: 624,
        };
        rng.init_genrand(19_650_218);
        let mut i = 1usize;
        let mut j = 0usize;
        for _ in 0..624.max(key.len()) {
            let previous = rng.state.get(i.saturating_sub(1)).copied().unwrap_or(0);
            let current = rng.state.get(i).copied().unwrap_or(0);
            let key_word = key.get(j).copied().unwrap_or(0);
            let mixed = (current ^ (previous ^ (previous >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key_word)
                .wrapping_add(j as u32);
            if let Some(slot) = rng.state.get_mut(i) {
                *slot = mixed;
            }
            i += 1;
            j += 1;
            if i >= 624 {
                let last = rng.state.last().copied().unwrap_or(0);
                if let Some(first) = rng.state.first_mut() {
                    *first = last;
                }
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..623 {
            let previous = rng.state.get(i.saturating_sub(1)).copied().unwrap_or(0);
            let current = rng.state.get(i).copied().unwrap_or(0);
            let mixed = (current ^ (previous ^ (previous >> 30)).wrapping_mul(1_566_083_941))
                .wrapping_sub(i as u32);
            if let Some(slot) = rng.state.get_mut(i) {
                *slot = mixed;
            }
            i += 1;
            if i >= 624 {
                let last = rng.state.last().copied().unwrap_or(0);
                if let Some(first) = rng.state.first_mut() {
                    *first = last;
                }
                i = 1;
            }
        }
        if let Some(first) = rng.state.first_mut() {
            *first = 0x8000_0000;
        }
        rng
    }

    fn init_genrand(&mut self, seed: u32) {
        if let Some(first) = self.state.first_mut() {
            *first = seed;
        }
        for i in 1..624 {
            let previous = self.state.get(i - 1).copied().unwrap_or(0);
            let value = (1_812_433_253u32)
                .wrapping_mul(previous ^ (previous >> 30))
                .wrapping_add(i as u32);
            if let Some(slot) = self.state.get_mut(i) {
                *slot = value;
            }
        }
        self.index = 624;
    }

    fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let current = self.state.get(i).copied().unwrap_or(0);
                let next = self.state.get((i + 1) % 624).copied().unwrap_or(0);
                let offset = self.state.get((i + 397) % 624).copied().unwrap_or(0);
                let y = (current & 0x8000_0000) | (next & 0x7fff_ffff);
                let value = offset ^ (y >> 1) ^ (if (y & 1) == 0 { 0 } else { 0x9908_b0df });
                if let Some(slot) = self.state.get_mut(i) {
                    *slot = value;
                }
            }
            self.index = 0;
        }
        let mut y = self.state.get(self.index).copied().unwrap_or(0);
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// Snapshot the complete MT state, equivalent to Python `random.getstate()` for this generator.
    #[must_use]
    pub fn get_state(&self) -> Self {
        self.clone()
    }

    /// Restore a state snapshot, equivalent to Python `random.setstate()` for this generator.
    pub fn set_state(&mut self, state: &Self) {
        *self = state.clone();
    }

    /// CPython's `random()` 53-bit fraction in `[0, 1)`.
    pub fn random(&mut self) -> f64 {
        let a = u64::from(self.next_u32() >> 5);
        let b = u64::from(self.next_u32() >> 6);
        (((a << 26) + b) as f64) / 9_007_199_254_740_992.0
    }

    /// Return the high `bits` of a randomly generated integer as a little-endian `u128`.
    ///
    /// CPython's `getrandbits(k)` returns a Python arbitrary-precision integer; this bounded
    /// port supports `0..=128`, enough for indices in this application. Values beyond the
    /// range return `None` rather than silently truncate.
    pub fn getrandbits(&mut self, bits: u32) -> Option<u128> {
        if bits > 128 {
            return None;
        }
        if bits == 0 {
            return Some(0);
        }
        let words = bits.div_ceil(32);
        let mut output = 0u128;
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
        if upper == 0 {
            return None;
        }
        let bits = usize::BITS - upper.leading_zeros();
        loop {
            let candidate = self.getrandbits(bits)?;
            if candidate < u128::try_from(upper).ok()? {
                return usize::try_from(candidate).ok();
            }
        }
    }

    /// Select one element exactly as CPython `choice` does (raises for an empty sequence;
    /// here that case is represented by `None`).
    pub fn choice<'a, T>(&mut self, values: &'a [T]) -> Option<&'a T> {
        let index = self.randbelow(values.len())?;
        values.get(index)
    }

    /// Python `randrange(stop)`, returning an integer in `0..stop`.
    pub fn randrange(&mut self, stop: i64) -> Option<i64> {
        self.randrange_step(0, stop, 1)
    }

    /// CPython's integer-only `randrange(start, stop, step)`; invalid/empty ranges return `None`.
    pub fn randrange_step(&mut self, start: i64, stop: i64, step: i64) -> Option<i64> {
        if step == 0 {
            return None;
        }
        let width = if step > 0 {
            if start >= stop {
                return None;
            }
            stop.checked_sub(start)?
                .checked_sub(1)?
                .checked_div(step)?
                .checked_add(1)?
        } else {
            if start <= stop {
                return None;
            }
            start
                .checked_sub(stop)?
                .checked_sub(1)?
                .checked_div(step.checked_neg()?)?
                .checked_add(1)?
        };
        let draw = i64::try_from(self.randbelow(usize::try_from(width).ok()?)?).ok()?;
        start.checked_add(draw.checked_mul(step)?)
    }

    /// Choose `count` unique items without replacement, matching CPython's `random.sample`
    /// selection order and pool/set algorithm for an ordered slice.
    pub fn sample<T: Clone>(&mut self, population: &[T], count: usize) -> Option<Vec<T>> {
        let length = population.len();
        if count > length {
            return None;
        }
        let mut set_size = 21usize;
        if count > 5 {
            let target = count.checked_mul(3)?;
            let mut power = 1usize;
            while power < target {
                power = power.checked_mul(4)?;
            }
            set_size = set_size.checked_add(power)?;
        }
        let mut result = Vec::with_capacity(count);
        if length <= set_size {
            let mut pool = population.to_vec();
            for index in 0..count {
                let remaining = length.checked_sub(index)?;
                let selected = self.randbelow(remaining)?;
                result.push(pool.get(selected)?.clone());
                let last = pool.get(remaining.checked_sub(1)?)?.clone();
                if let Some(slot) = pool.get_mut(selected) {
                    *slot = last;
                }
            }
        } else {
            let mut selected_indices = std::collections::BTreeSet::new();
            for _ in 0..count {
                let mut index = self.randbelow(length)?;
                while selected_indices.contains(&index) {
                    index = self.randbelow(length)?;
                }
                selected_indices.insert(index);
                result.push(population.get(index)?.clone());
            }
        }
        Some(result)
    }

    /// In-place Fisher–Yates shuffle matching CPython's `_randbelow`-based `shuffle`.
    pub fn shuffle<T>(&mut self, values: &mut [T]) {
        for i in (1..values.len()).rev() {
            if let Some(j) = self.randbelow(i + 1) {
                values.swap(i, j);
            }
        }
    }
}

static GLOBAL_RNG: std::sync::LazyLock<std::sync::Mutex<PythonRandom>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(PythonRandom::from_entropy()));

fn with_global<T>(operation: impl FnOnce(&mut PythonRandom) -> T) -> T {
    let mut generator = match GLOBAL_RNG.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    operation(&mut generator)
}

/// Seed the process-global generator with an integer (Python module-level `random.seed`).
pub fn global_seed(seed: i128) {
    with_global(|generator| generator.seed(seed));
}

/// Save the process-global generator state (Python `random.getstate` shape within this crate).
#[must_use]
pub fn global_get_state() -> PythonRandom {
    with_global(|generator| generator.get_state())
}

/// Restore a state previously returned by [`global_get_state`].
pub fn global_set_state(state: &PythonRandom) {
    with_global(|generator| generator.set_state(state));
}

/// Draw from the process-global generator.
pub fn global_random() -> f64 {
    with_global(PythonRandom::random)
}

/// Select a cloned item from the process-global generator.
pub fn global_choice<T: Clone>(values: &[T]) -> Option<T> {
    with_global(|generator| generator.choice(values).cloned())
}

/// Draw a process-global integer range.
pub fn global_randrange(stop: i64) -> Option<i64> {
    with_global(|generator| generator.randrange(stop))
}

/// Draw a process-global integer range with a custom start and step.
pub fn global_randrange_step(start: i64, stop: i64, step: i64) -> Option<i64> {
    with_global(|generator| generator.randrange_step(start, stop, step))
}

/// Draw a process-global sample without replacement.
pub fn global_sample<T: Clone>(population: &[T], count: usize) -> Option<Vec<T>> {
    with_global(|generator| generator.sample(population, count))
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
    #[test]
    fn random_stream_matches_cpython_integer_seed_vectors() {
        for (seed, expected) in [
            (
                0,
                [0.8444218515250481, 0.7579544029403025, 0.420571580830845],
            ),
            (
                1,
                [0.13436424411240122, 0.8474337369372327, 0.763774618976614],
            ),
        ] {
            let mut rng = PythonRandom::new(seed);
            for value in expected {
                assert_eq!(rng.random(), value);
            }
        }
    }
    #[test]
    fn getrandbits_zero_and_seeded_words() {
        let mut rng = PythonRandom::new(42);
        assert_eq!(rng.getrandbits(0), Some(0));
        assert_eq!(rng.getrandbits(5), Some(20));
        assert_eq!(rng.getrandbits(32), Some(478_163_327));
        assert_eq!(rng.getrandbits(129), None);
    }
    #[test]
    fn choice_and_shuffle_match_cpython() {
        let mut rng = PythonRandom::new(42);
        let values = ["a", "b", "c", "d", "e"];
        assert_eq!(rng.choice(&values), Some(&"a"));
        assert_eq!(rng.choice::<i32>(&[]), None);
        let mut rng = PythonRandom::new(42);
        let mut values = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        rng.shuffle(&mut values);
        assert_eq!(values, [7, 3, 2, 8, 5, 6, 9, 4, 0, 1]);
    }
    #[test]
    fn negative_and_zero_integer_seeds_are_defined() {
        let mut a = PythonRandom::new(17);
        let mut b = PythonRandom::new(-17);
        assert_eq!(a.random(), b.random());
        let mut z = PythonRandom::new(0);
        assert_eq!(z.random(), 0.8444218515250481);
    }
    #[test]
    fn reseeding_and_limb_seeding_match_integer_seed() {
        let seed = (1_i128 << 64) + 17;
        let expected = PythonRandom::new(seed).random();
        let mut reseeded = PythonRandom::new(0);
        reseeded.seed(seed);
        let mut words = PythonRandom::new(0);
        words.seed_words(&[17, 0, 1]);
        assert_eq!(reseeded.random(), expected);
        assert_eq!(words.random(), expected);
        let mut empty = PythonRandom::new(1);
        empty.seed_words(&[]);
        assert_eq!(empty.random(), PythonRandom::new(0).random());
    }
    #[test]
    fn entropy_constructor_returns_a_valid_fraction() {
        let value = PythonRandom::from_entropy().random();
        assert!((0.0..1.0).contains(&value));
    }
    #[test]
    fn ranges_and_sampling_handle_edges() {
        let mut rng = PythonRandom::new(8);
        assert!((0..10).contains(&rng.randrange(10).expect("non-empty range")));
        assert_eq!(rng.randrange_step(0, 0, 1), None);
        assert_eq!(rng.randrange_step(0, 5, 0), None);
        for _ in 0..100 {
            let value = rng
                .randrange_step(10, -10, -3)
                .expect("non-empty descending range");
            assert!([10, 7, 4, 1, -2, -5, -8].contains(&value));
        }
        assert_eq!(rng.sample::<u8>(&[], 1), None);
        assert_eq!(rng.sample::<u8>(&[], 0), Some(Vec::new()));
        let sample = rng
            .sample(&(0..100_u8).collect::<Vec<_>>(), 20)
            .expect("valid sample");
        let unique = sample
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), sample.len());
    }
    #[test]
    fn snapshots_and_process_global_state_restore_exactly() {
        let mut local = PythonRandom::new(91);
        let snapshot = local.get_state();
        let draws = [local.random(), local.random()];
        local.set_state(&snapshot);
        assert_eq!([local.random(), local.random()], draws);

        global_seed(712);
        let snapshot = global_get_state();
        let expected = [global_random(), global_random()];
        global_set_state(&snapshot);
        assert_eq!([global_random(), global_random()], expected);
        assert_eq!(global_randrange(1), Some(0));
        assert!(global_choice(&["one", "two"]).is_some());
        assert_eq!(global_sample(&[1, 2, 3], 4), None);
    }
}
