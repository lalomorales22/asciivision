//! Tiny deterministic PRNG for procedural art.
//!
//! Art must be byte-identical for the same seed on every machine, so we do
//! not use `rand::thread_rng` here. This is splitmix64: fast, tiny, and good
//! enough for visual noise.

#[derive(Clone, Debug)]
pub struct Rng64 {
    state: u64,
}

impl Rng64 {
    pub fn new(seed: u64) -> Self {
        // Mix the seed once so 0/1/2 do not produce correlated streams.
        let mut rng = Self { state: seed ^ 0x9E37_79B9_7F4A_7C15 };
        rng.next_u64();
        rng
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform float in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / ((1u64 << 24) as f32)
    }

    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }

    /// Uniform integer in `[lo, hi)`. Returns `lo` when the range is empty.
    pub fn range_usize(&mut self, lo: usize, hi: usize) -> usize {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo) as u64) as usize
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.range_usize(0, items.len())]
    }
}

/// Hash a 2D integer lattice point plus a seed into `[0, 1)`. Used by the
/// value-noise generators so noise is a pure function of `(seed, x, y)`.
pub fn hash2(seed: u64, x: i64, y: i64) -> f32 {
    let mut h = seed ^ (x as u64).wrapping_mul(0x8ED0_2BB1_7A5B_9F63) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Bilinear value noise in `[0, 1)`.
pub fn value_noise(seed: u64, x: f32, y: f32) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let tx = smoothstep(x - xi);
    let ty = smoothstep(y - yi);
    let (xi, yi) = (xi as i64, yi as i64);
    let a = hash2(seed, xi, yi);
    let b = hash2(seed, xi + 1, yi);
    let c = hash2(seed, xi, yi + 1);
    let d = hash2(seed, xi + 1, yi + 1);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * ty
}

/// Fractional Brownian motion over `value_noise`, normalized to roughly `[0, 1]`.
pub fn fbm(seed: u64, x: f32, y: f32, octaves: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut norm = 0.0;
    for octave in 0..octaves {
        total += value_noise(seed.wrapping_add(octave as u64 * 7919), x * frequency, y * frequency) * amplitude;
        norm += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    total / norm.max(1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng64::new(42);
        let mut b = Rng64::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn floats_stay_in_unit_interval() {
        let mut rng = Rng64::new(7);
        for _ in 0..10_000 {
            let f = rng.next_f32();
            assert!((0.0..1.0).contains(&f));
        }
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        for i in 0..200 {
            let x = i as f32 * 0.37;
            let y = i as f32 * 0.11;
            let a = fbm(3, x, y, 4);
            let b = fbm(3, x, y, 4);
            assert_eq!(a, b);
            assert!((0.0..=1.0).contains(&a));
        }
    }
}
