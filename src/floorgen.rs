//! Arena generation, FP1a: pure fn of (run seed, floor index, accepted content). Integer only.
//! FP1b replaces the body (graph floors, design sec 19); the signature stays.
use crate::data::{ArenaDef, Content, Glyph};
use crate::rng::{mix, Stream, Xo};
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

/// Tile edge (mm). Equals view.floor_cell_m so render cells and sim tiles coincide.
pub const TILE_MM: i32 = 2000;
/// Room pitch in tiles (72 m rooms); a cave band of rock lies on every boundary line.
pub const PITCH: i32 = 36;
/// Wide doorway width in tiles (12 m).
pub const DOOR_W: i32 = 6;
/// Local tile index of the room centre; world tile (0,0) is the middle of room (0,0).
pub const CENTRE: i32 = PITCH / 2;
/// Cave band half-thickness 1..=BAND_MAX tiles, noise knots every BAND_KNOT tiles along a line, and one tile in
/// ROUGH_1_IN pushes the edge out by one more (rough rock face).
const BAND_MAX: i32 = 4;
const BAND_KNOT: i32 = 5;
const ROUGH_1_IN: u64 = 3;
/// Deepest rock reach into a room: doors and pillars keep out of it, so corners never close a doorway.
const ROCK: i32 = BAND_MAX + 1;
/// Pillars stay in local [PILLAR_LO, PILLAR_HI] and out of the centre square, so doors and spawn stay clear.
const PILLAR_LO: i32 = ROCK + 2;
const PILLAR_HI: i32 = PITCH - ROCK - 2;
const CENTRE_CLEAR: i32 = 3;
/// North doors beyond the guaranteed spine column open with this chance, permille.
const NORTH_OPEN: u64 = 450;
/// Spine column of each room row lies in -SPINE..=SPINE.
const SPINE: i32 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tile {
    Floor,
    Wall,
    Pillar,
}

impl Tile {
    pub fn solid(self) -> bool {
        self != Tile::Floor
    }
}

impl Arena {
    fn edge_hash(&self, tag: u64, rx: i32, ry: i32) -> u64 {
        mix(self.salt ^ tag.rotate_left(48) ^ (u64::from(rx as u32) << 24) ^ u64::from(ry as u32))
    }

    /// Row ry joins row ry+1 through this room column (always open). Rooms in a row chain west-east, so all connect.
    fn spine(&self, ry: i32) -> i32 {
        (self.edge_hash(3, 0, ry) % (2 * SPINE as u64 + 1)) as i32 - SPINE
    }

    /// Doorway along a wall of room (rx, ry): spans DOOR_W tiles inside the interior; l = local tile along the wall.
    fn door_at(&self, tag: u64, rx: i32, ry: i32, l: i32) -> bool {
        // the span starts past the deepest rock of the crossing line, so the doorway is never closed by a corner
        let span = (PITCH - DOOR_W - 2 * ROCK + 1) as u64;
        let lo = ROCK + (self.edge_hash(tag, rx, ry) % span) as i32;
        (lo..lo + DOOR_W).contains(&l)
    }

    /// Half-thickness (tiles, 1..=BAND_MAX) of the rock band along one room boundary line at `along` (world tile
    /// along the line). Value noise with knots every BAND_KNOT tiles, linear between: caves bulge and thin smoothly.
    fn band(&self, tag: u64, line: i32, along: i32) -> i32 {
        let (k, f) = (along.div_euclid(BAND_KNOT), along.rem_euclid(BAND_KNOT));
        let v = |k: i32| (self.edge_hash(tag ^ 0x5a, line, k) % 1000) as i32;
        let v = (v(k) * (BAND_KNOT - f) + v(k + 1) * f) / BAND_KNOT;
        1 + v * BAND_MAX / 1000
    }

    /// North wall of room (rx, ry) is open at local tile l: on the spine column always, elsewhere by chance.
    fn north_door(&self, rx: i32, ry: i32, l: i32) -> bool {
        (rx == self.spine(ry - 1) || self.edge_hash(2, rx, ry) % 1000 < NORTH_OPEN) && self.door_at(2, rx, ry, l)
    }

    /// Tile at world tile coordinates. O(1), pure in (salt, tx, ty).
    /// Room boundary lines are cave bands of varying thickness with a rough edge; doorways cut through them.
    pub fn tile(&self, tx: i32, ty: i32) -> Tile {
        let (rx, lx) = ((tx + CENTRE).div_euclid(PITCH), (tx + CENTRE).rem_euclid(PITCH));
        let (ry, ly) = ((ty + CENTRE).div_euclid(PITCH), (ty + CENTRE).rem_euclid(PITCH));
        // nearest vertical line = west wall of room vx, nearest horizontal line = north wall of room hy
        let (vx, dv) = if lx <= PITCH - lx { (rx, lx) } else { (rx + 1, PITCH - lx) };
        let (hy, dh) = if ly <= PITCH - ly { (ry, ly) } else { (ry + 1, PITCH - ly) };
        let rough = i32::from(mix(self.salt ^ (u64::from(tx as u32) << 32) ^ u64::from(ty as u32)).is_multiple_of(ROUGH_1_IN));
        let rock_v = dv < self.band(1, vx, ty) + rough && !self.door_at(1, vx, ry, ly);
        let rock_h = dh < self.band(2, hy, tx) + rough && !self.north_door(rx, hy, lx);
        if rock_v || rock_h { Tile::Wall } else { self.interior(tx, ty, lx, ly) }
    }


    fn interior(&self, tx: i32, ty: i32, lx: i32, ly: i32) -> Tile {
        let core = (PILLAR_LO..=PILLAR_HI).contains(&lx) && (PILLAR_LO..=PILLAR_HI).contains(&ly);
        let clear = (lx - CENTRE).abs() <= CENTRE_CLEAR && (ly - CENTRE).abs() <= CENTRE_CLEAR;
        if core && !clear && self.decor_hit(tx * TILE_MM + TILE_MM / 2, ty * TILE_MM + TILE_MM / 2) {
            Tile::Pillar
        } else {
            Tile::Floor
        }
    }

    /// Point (mm) lies in a decor disc (one disc per decor super-cell).
    fn decor_hit(&self, ax: i32, ay: i32) -> bool {
        let dc = self.decor_cell;
        let (sx, sy) = (ax.div_euclid(dc), ay.div_euclid(dc));
        let hd = mix(self.salt.rotate_left(17) ^ ((sx as u64) << 32) ^ (sy as u32 as u64));
        let (cx, cy) = (sx * dc + ((hd >> 10) % dc as u64) as i32, sy * dc + ((hd >> 30) % dc as u64) as i32);
        let (dx, dy) = (i64::from(ax - cx), i64::from(ay - cy));
        (hd % 1000) < u64::from(self.decor_chance) && dx * dx + dy * dy <= i64::from(self.decor_r).pow(2)
    }

    /// Tile under a world point (mm).
    pub fn tile_mm(&self, x: i32, y: i32) -> Tile {
        self.tile(x.div_euclid(TILE_MM), y.div_euclid(TILE_MM))
    }
}

fn range(rng: &mut Xo, (lo, hi): (u32, u32)) -> u32 {
    lo + rng.below(NonZeroU32::MIN.saturating_add(hi - lo))
}

pub fn arena(seed: u64, floor: u8, c: &Content) -> Arena {
    let a: &ArenaDef = &c.arena;
    let mut rng = Xo::stream(seed ^ u64::from(floor).rotate_left(56), Stream::Gen);
    let template = [Template::Open, Template::Pillars, Template::Rubble][rng.below(NonZeroU32::MIN.saturating_add(2)) as usize];
    let pool = &c.pal().floor_glyphs;
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

    /// Rooms -R..=R square, tile window, flood fill from the origin tile; true when every room centre is reached.
    const R: i32 = 3;
    fn all_rooms_reachable(blocked: &dyn Fn(i32, i32) -> bool) -> bool {
        let (lo, hi) = (-R * PITCH - CENTRE, R * PITCH + CENTRE);
        let w = (hi - lo + 1) as usize;
        let at = |x: i32, y: i32| (y - lo) as usize * w + (x - lo) as usize;
        let mut seen = vec![false; w * w];
        let mut q = vec![(0, 0)];
        seen[at(0, 0)] = true;
        while let Some((x, y)) = q.pop() {
            for (nx, ny) in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
                if (lo..=hi).contains(&nx) && (lo..=hi).contains(&ny) && !seen[at(nx, ny)] && !blocked(nx, ny) {
                    seen[at(nx, ny)] = true;
                    q.push((nx, ny));
                }
            }
        }
        (-R..=R).all(|rx| (-R..=R).all(|ry| seen[at(rx * PITCH, ry * PITCH)]))
    }

    #[test]
    fn rooms_connected_and_cut_wall_breaks_it() {
        let c = content();
        for seed in 0..8 {
            let a = arena(seed, 0, &c);
            assert!(!a.tile(0, 0).solid(), "spawn tile free");
            assert!(all_rooms_reachable(&|x, y| a.tile(x, y).solid()), "seed {seed}");
            // NEG: a full wall line between room columns 1 and 2 must disconnect
            let cut = |x: i32, y: i32| a.tile(x, y).solid() || x == PITCH + PITCH / 2;
            assert!(!all_rooms_reachable(&cut), "seed {seed}: cut must break connectivity");
        }
    }

    #[test]
    fn doorways_wide_and_walls_solid() {
        let c = content();
        let a = arena(9, 0, &c);
        let (mut west, mut north, mut thick) = (0, 0, std::collections::BTreeSet::new());
        for rx in -R..=R {
            for ry in -R..=R {
                let (x0, y0) = (rx * PITCH - CENTRE, ry * PITCH - CENTRE);
                // a doorway = a tile along the line where the whole band, both sides, is open
                let w = (1..PITCH).filter(|&l| (-ROCK..=ROCK).all(|d| !a.tile(x0 + d, y0 + l).solid())).count() as i32;
                assert_eq!(w, DOOR_W, "west boundary: one doorway through the band");
                let n = (1..PITCH).filter(|&l| (-ROCK..=ROCK).all(|d| !a.tile(x0 + l, y0 + d).solid())).count() as i32;
                assert!(n == 0 || n == DOOR_W, "north boundary closed or one doorway, got {n}");
                west += 1;
                north += i32::from(n > 0);
                assert!(a.tile(x0, y0).solid(), "corner rock");
                // band thickness along the line varies (cave, not a straight wall)
                for l in 1..PITCH {
                    thick.insert((0..=ROCK).take_while(|&d| a.tile(x0 + d, y0 + l).solid()).count());
                }
            }
        }
        assert!(north > 0 && north < west, "some north doors open, some closed");
        assert!(thick.len() >= 4, "band depth varies: {thick:?}");
    }

    #[test]
    fn tiles_deterministic_per_seed() {
        let c = content();
        let grid = |s: u64| -> Vec<Tile> {
            let a = arena(s, 0, &c);
            (-60..60).flat_map(|y| (-60..60).map(move |x| (x, y))).map(|(x, y)| a.tile(x, y)).collect()
        };
        assert_eq!(grid(4), grid(4));
        assert_ne!(grid(4), grid(5), "other seed, other layout");
    }
}
