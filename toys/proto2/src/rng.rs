//! Own RNG (design sec 7.3): xoshiro256++ seeded by SplitMix64, one stream per system.
use std::num::NonZeroU32;

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Xo([u64; 4]);

/// Fixed stream index list. New systems go at the END (design sec 7.3).
#[derive(Clone, Copy)]
pub enum Stream {
    Spawn = 0,
    Combat = 1,
    Picks = 2,
    Director = 3,
    Bot = 4,
    Gen = 5,
}

impl Xo {
    pub fn seeded(seed: u64) -> Self {
        let mut sm = SplitMix64(seed);
        Xo([sm.next(), sm.next(), sm.next(), sm.next()])
    }

    /// Stream k = base state after k jumps (2^128 calls each) => no overlap by construction.
    pub fn stream(seed: u64, s: Stream) -> Self {
        let mut x = Self::seeded(seed);
        for _ in 0..s as u8 {
            x.jump();
        }
        x
    }

    pub fn next(&mut self) -> u64 {
        let s = &mut self.0;
        let r = s[0].wrapping_add(s[3]).rotate_left(23).wrapping_add(s[0]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        r
    }

    fn jump(&mut self) {
        const J: [u64; 4] = [0x180e_c6d3_3cfd_0aba, 0xd5a6_1266_f0c9_392c, 0xa958_2618_e03f_c9aa, 0x39ab_dc45_29b1_661c];
        let mut acc = [0u64; 4];
        for j in J {
            for b in 0..64 {
                if j & (1u64 << b) != 0 {
                    for (a, s) in acc.iter_mut().zip(self.0) {
                        *a ^= s;
                    }
                }
                self.next();
            }
        }
        self.0 = acc;
    }

    /// Uniform in 0..n by rejection (no modulo bias).
    pub fn below(&mut self, n: NonZeroU32) -> u32 {
        let n = u64::from(n.get());
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let r = self.next();
            if r < zone {
                return (r % n) as u32;
            }
        }
    }

    /// True with probability permille/1000.
    pub fn chance(&mut self, permille: u32) -> bool {
        self.below(NonZeroU32::MIN.saturating_add(999)) < permille
    }

    pub fn words(&self) -> [u64; 4] {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xoshiro_reference_vectors() {
        // Vigna reference: state {1,2,3,4}
        let mut x = Xo([1, 2, 3, 4]);
        assert_eq!([x.next(), x.next(), x.next()], [41_943_041, 58_720_359, 3_588_806_011_781_223]);
    }

    #[test]
    fn splitmix_reference_vector() {
        assert_eq!(SplitMix64(0).next(), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn streams_differ_and_repeat() {
        let a = Xo::stream(7, Stream::Spawn).next();
        assert_ne!(a, Xo::stream(7, Stream::Combat).next());
        assert_eq!(a, Xo::stream(7, Stream::Spawn).next());
        assert_ne!(a, Xo::stream(8, Stream::Spawn).next());
    }

    #[test]
    fn below_stays_in_range_and_covers() {
        let mut x = Xo::seeded(1);
        let n = NonZeroU32::new(7).unwrap();
        let mut seen = [0u32; 7];
        for _ in 0..7000 {
            seen[x.below(n) as usize] += 1;
        }
        assert!(seen.iter().all(|&c| (800..1200).contains(&c)), "{seen:?}");
    }
}
