//! Arena generation, FP1a: pure fn of (run seed, floor index, accepted content). Integer only.
//! FP1b replaces the body (graph floors, design sec 19); the signature stays.
use crate::data::{ArenaDef, Content, Glyph};
use crate::rng::{Stream, Xo};
use std::num::NonZeroU32;

/// Decor template, substituted per run (design sec 17.1b names Arena-open/pillars/rubble).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Template {
    Open,
    Pillars,
    Rubble,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arena {
    pub salt: u64,
    pub template: Template,
    pub glyphs: Vec<Glyph>,
    /// floor glyph density, permille
    pub density: u32,
    /// spawn weight multiplier per family, permille (ecology varies per run)
    pub fam_mult: Vec<u32>,
    pub decor_chance: u32,
    pub decor_cell: i32,
    pub decor_r: i32,
}

fn range(rng: &mut Xo, (lo, hi): (u32, u32)) -> u32 {
    lo + rng.below(NonZeroU32::MIN.saturating_add(hi - lo))
}

pub fn arena(seed: u64, floor: u8, c: &Content) -> Arena {
    let a: &ArenaDef = &c.arena;
    let mut rng = Xo::stream(seed ^ u64::from(floor).rotate_left(56), Stream::Gen);
    let template = [Template::Open, Template::Pillars, Template::Rubble][rng.below(NonZeroU32::MIN.saturating_add(2)) as usize];
    let pool = &c.pal.floor_glyphs;
    let n = (range(&mut rng, a.glyphs) as usize).clamp(1, pool.len());
    let mut idx: Vec<usize> = (0..pool.len()).collect();
    // partial Fisher-Yates with own below(): first n entries = the run's glyph subset
    for i in 0..n {
        let j = i + rng.below(NonZeroU32::MIN.saturating_add((pool.len() - i - 1) as u32)) as usize;
        idx.swap(i, j);
    }
    Arena {
        salt: rng.next(),
        template,
        glyphs: idx[..n].iter().map(|&i| pool[i]).collect(),
        density: range(&mut rng, a.density),
        fam_mult: c.fams.iter().map(|_| range(&mut rng, a.fam_mult)).collect(),
        decor_chance: if template == Template::Open { 0 } else { range(&mut rng, a.decor) },
        decor_cell: a.decor_cell,
        decor_r: range(&mut rng, (a.decor_r.0 as u32, a.decor_r.1 as u32)) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;

    #[test]
    fn pure_and_varies_by_seed() {
        let c = content();
        assert_eq!(arena(5, 0, &c), arena(5, 0, &c));
        assert_ne!(arena(5, 0, &c), arena(5, 1, &c), "floor index changes the arena");
        let all: Vec<Arena> = (0..30).map(|s| arena(s, 0, &c)).collect();
        let distinct = |f: &dyn Fn(&Arena) -> String| {
            let mut v: Vec<String> = all.iter().map(f).collect();
            v.sort();
            v.dedup();
            v.len()
        };
        assert_eq!(distinct(&|a| format!("{:?}", a.template)), 3);
        assert!(distinct(&|a| format!("{:?}", a.glyphs)) > 20);
        assert!(distinct(&|a| format!("{:?}", a.fam_mult)) == 30);
    }
}
