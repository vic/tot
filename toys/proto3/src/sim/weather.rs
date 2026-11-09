//! Weather: a seeded schedule of equal-length segments, a pure function of (seed, tick). Integer only, O(1).
use crate::floorgen::{Arena, Tile, TILE_MM};
use crate::rng::{mix, Stream, Xo};
use std::num::NonZeroU32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Weather {
    Clear,
    Rain,
    AcidRain,
    /// everyone slower
    Snow,
    /// the exposed hero burns and drags
    Heat,
    /// pushes hero and foes along the segment.s wind direction
    Wind,
    /// foes frenzied: faster
    FullMoon,
    BloodMoon,
}

/// Draw table in permille, in enum order; sums to 1000.
const WEIGHTS: [(Weather, u64); 8] =
    [(Weather::Clear, 550), (Weather::Rain, 150), (Weather::AcidRain, 70), (Weather::Snow, 70), (Weather::Heat, 60), (Weather::Wind, 60), (Weather::FullMoon, 25), (Weather::BloodMoon, 15)];
/// A pillar within this many tiles shelters a point.
const SHELTER_TILES: i32 = 2;
/// Hero HP change per second in milli-HP when exposed.
const RAIN_HEAL: i32 = 500;
const ACID_HURT: i32 = 1000;
const HEAT_HURT: i32 = 400;
/// Wind push per second (mm), and the hero.s speed in snow and heat (permille).
const WIND_MMPS: i32 = 1500;
const SNOW_HERO: u32 = 800;
const HEAT_HERO: u32 = 900;

/// Multiplier in permille (1000 = unchanged).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Permille(pub u32);

impl Permille {
    pub const ONE: Permille = Permille(1000);
    pub fn apply(self, v: i32) -> i32 {
        (i64::from(v) * i64::from(self.0) / 1000) as i32
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Forecast {
    pub now: Weather,
    pub next: Weather,
    pub ticks_to_next: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    key: u64,
    seg_ticks: NonZeroU32,
}

impl Schedule {
    pub fn new(seed: u64, seg_ticks: NonZeroU32) -> Schedule {
        Schedule { key: Xo::stream(seed, Stream::Weather).next(), seg_ticks }
    }

    fn segment(&self, k: u32) -> Weather {
        let mut roll = mix(self.key ^ u64::from(k)) % 1000;
        for (w, p) in WEIGHTS {
            if roll < p {
                return w;
            }
            roll -= p;
        }
        Weather::Clear
    }

    pub fn forecast(&self, tick: u32) -> Forecast {
        let (k, into) = (tick / self.seg_ticks, tick % self.seg_ticks);
        Forecast { now: self.segment(k), next: self.segment(k.wrapping_add(1)), ticks_to_next: self.seg_ticks.get() - into }
    }

    /// Wind push this tick (mm per tick, x and y): zero unless the sky is Wind; the direction is fixed per segment.
    pub fn wind(&self, tick: u32, hz: u32) -> (i32, i32) {
        let k = tick / self.seg_ticks;
        if self.segment(k) != Weather::Wind {
            return (0, 0);
        }
        let h = mix(self.key ^ u64::from(k).rotate_left(21));
        let a = (h % 8) as usize;
        let (dx, dy) = [(1000, 0), (707, 707), (0, 1000), (-707, 707), (-1000, 0), (-707, -707), (0, -1000), (707, -707)][a];
        let per = WIND_MMPS / hz.max(1) as i32;
        (dx * per / 1000, dy * per / 1000)
    }
}

impl Weather {
    /// Hero HP change per second (milli-HP): rain heals, acid hurts; shelter blocks both.
    pub fn hero_hp_per_s(self, sheltered: bool) -> i32 {
        match (self, sheltered) {
            (_, true) => 0,
            (Weather::Rain, false) => RAIN_HEAL,
            (Weather::AcidRain, false) => -ACID_HURT,
            (Weather::Heat, false) => -HEAT_HURT,
            _ => 0,
        }
    }

    pub fn foe_hp(self) -> Permille {
        match self {
            Weather::BloodMoon => Permille(1500),
            _ => Permille::ONE,
        }
    }

    pub fn foe_dmg(self) -> Permille {
        match self {
            Weather::BloodMoon => Permille(1300),
            _ => Permille::ONE,
        }
    }

    /// Foe walking speed: the full moon frenzies them, the blood moon a little.
    pub fn foe_speed(self) -> Permille {
        match self {
            Weather::FullMoon => Permille(1250),
            Weather::BloodMoon => Permille(1100),
            Weather::Snow => Permille(850),
            _ => Permille::ONE,
        }
    }

    /// Hero walking speed: snow and heat drag.
    pub fn hero_speed(self) -> Permille {
        match self {
            Weather::Snow => Permille(SNOW_HERO),
            Weather::Heat => Permille(HEAT_HERO),
            _ => Permille::ONE,
        }
    }
}

/// A pillar stands within SHELTER_TILES of the point (world mm).
pub fn sheltered(arena: &Arena, x_mm: i32, y_mm: i32) -> bool {
    let (tx, ty) = (x_mm.div_euclid(TILE_MM), y_mm.div_euclid(TILE_MM));
    (ty - SHELTER_TILES..=ty + SHELTER_TILES)
        .any(|y| (tx - SHELTER_TILES..=tx + SHELTER_TILES).any(|x| arena.tile(x, y) == Tile::Pillar))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;
    use crate::floorgen::arena;

    fn seg() -> NonZeroU32 {
        NonZeroU32::MIN.saturating_add(599)
    }

    #[test]
    fn deterministic_and_varies_by_seed() {
        let (a, b) = (Schedule::new(1, seg()), Schedule::new(1, seg()));
        let run = |s: &Schedule| (0..200).map(|k| s.forecast(k * 600).now).collect::<Vec<_>>();
        assert_eq!(run(&a), run(&b));
        assert_ne!(run(&a), run(&Schedule::new(2, seg())), "other seed, other schedule");
    }

    #[test]
    fn forecast_next_is_what_comes_and_countdown_hits_it() {
        let s = Schedule::new(3, seg());
        for t in [0, 1, 599, 600, 12_345, 99_999] {
            let f = s.forecast(t);
            assert!((1..=600).contains(&f.ticks_to_next));
            let later = s.forecast(t + f.ticks_to_next);
            assert_eq!(later.now, f.next);
            // NEG: one tick earlier is still the current weather
            if f.ticks_to_next > 1 {
                assert_eq!(s.forecast(t + f.ticks_to_next - 1).now, f.now);
            }
        }
    }

    #[test]
    fn all_weathers_occur_with_clear_most_common() {
        let s = Schedule::new(7, seg());
        let n = |w| (0..2000).filter(|&k| s.forecast(k * 600).now == w).count();
        let [c, r, a, b] = [Weather::Clear, Weather::Rain, Weather::AcidRain, Weather::BloodMoon].map(n);
        assert!(r > 0 && a > 0 && b > 0);
        assert!(c > r && r > b);
    }

    #[test]
    fn effects_typed() {
        assert!(Weather::Rain.hero_hp_per_s(false) > 0);
        assert!(Weather::AcidRain.hero_hp_per_s(false) < 0);
        // NEG: shelter and clear skies never change hp
        assert_eq!(Weather::AcidRain.hero_hp_per_s(true), 0);
        assert_eq!(Weather::Rain.hero_hp_per_s(true), 0);
        assert_eq!(Weather::Clear.hero_hp_per_s(false), 0);
        assert_eq!(Weather::BloodMoon.foe_hp().apply(1000), 1500);
        assert_eq!(Weather::BloodMoon.foe_dmg().apply(1000), 1300);
        assert_eq!(Weather::Rain.foe_hp(), Permille::ONE);
    }

    #[test]
    fn shelter_follows_pillars() {
        let c = content();
        let a = (0..40).map(|s| arena(s, 0, &c)).find(|a| (-12..12).any(|x| (-12..12).any(|y| a.tile(x, y) == Tile::Pillar)));
        let Some(a) = a else { panic!("some seed has pillars") };
        let (px, py) = (-12..12).flat_map(|x| (-12..12).map(move |y| (x, y))).find(|&(x, y)| a.tile(x, y) == Tile::Pillar).unwrap_or((0, 0));
        assert!(sheltered(&a, px * TILE_MM + 1, py * TILE_MM + 1));
        // NEG: the open centre of the origin room has no pillar within reach
        assert!(!sheltered(&a, 0, 0));
    }
}
