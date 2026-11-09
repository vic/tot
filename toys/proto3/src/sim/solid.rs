//! Solid world for the sim: walls and pillars from floorgen::Arena block actors. Integer only, O(1) per query.
use crate::floorgen::{Arena, TILE_MM};

/// Answers "is world tile (tx, ty) solid" and "does a circle overlap a solid tile". The arena computes tiles
/// (hashes per call); `Near` reads a cache and answers `blocked` from one neighbourhood mask.
pub trait Tiles {
    fn solid(&self, tx: i32, ty: i32) -> bool;

    fn blocked(&self, x_mm: i32, y_mm: i32, radius_mm: i32) -> bool
    where
        Self: Sized,
    {
        blocked_tiles(self, x_mm, y_mm, radius_mm)
    }
}

impl Tiles for Arena {
    fn solid(&self, tx: i32, ty: i32) -> bool {
        self.tile(tx, ty).solid()
    }
}

/// Window edge in tiles (256 m): foes near the hero hit the cache. Power of two, so wrap = mask.
const WIN: i32 = 128;

/// Neighbourhood mask bits by column dx = -1, 0, 1 and by row dy = -1, 0, 1.
const COL: [u16; 3] = [0b001_001_001, 0b010_010_010, 0b100_100_100];
const ROW: [u16; 3] = [0b000_000_111, 0b000_111_000, 0b111_000_000];

/// Bit (dy + 1) * 3 + (dx + 1) of a neighbourhood mask: tile (tx + dx, ty + dy) is solid, dx, dy in -1..=1.
#[cfg(test)]
fn nb_bit(dx: i32, dy: i32) -> u16 {
    1 << ((dy + 1) * 3 + dx + 1)
}

/// Solid flags of the arena for the WIN x WIN tiles at [x0, x0 + WIN) x [y0, y0 + WIN), stored toroidally (tile
/// (tx, ty) = bit tx mod WIN of row ty mod WIN), so re-centring refills only the tiles that came into view.
/// 2 KiB: stays in L1 while the sim walks 20k actors.
/// Invariant: every window bit equals `arena.tile(..).solid()`; `fill` is the one writer.
pub struct Walls {
    x0: i32,
    y0: i32,
    rows: Box<[u128; WIN as usize]>,
}

type Span = std::ops::Range<i32>;

impl Walls {
    /// Window centred on the tile under (x_mm, y_mm).
    pub fn new(arena: &Arena, x_mm: i32, y_mm: i32) -> Walls {
        let (x0, y0) = origin(x_mm, y_mm);
        let mut w = Walls { x0, y0, rows: Box::new([0; WIN as usize]) };
        w.fill(arena, x0..x0 + WIN, y0..y0 + WIN);
        w
    }

    /// Keep the window centred on (x_mm, y_mm): refills the strips that entered (WIN tiles per tile moved). No allocation.
    pub fn follow(&mut self, arena: &Arena, x_mm: i32, y_mm: i32) {
        let (nx, ny) = origin(x_mm, y_mm);
        let (ox, oy) = (self.x0, self.y0);
        if (nx, ny) == (ox, oy) {
            return;
        }
        (self.x0, self.y0) = (nx, ny);
        if (nx - ox).abs() >= WIN || (ny - oy).abs() >= WIN {
            self.fill(arena, nx..nx + WIN, ny..ny + WIN);
            return;
        }
        // columns new in x (all rows), then rows new in y over the columns kept
        let cols = if nx > ox { ox + WIN..nx + WIN } else { nx..ox };
        let kept = nx.max(ox)..nx.min(ox) + WIN;
        let rows = if ny > oy { oy + WIN..ny + WIN } else { ny..oy };
        self.fill(arena, cols, ny..ny + WIN);
        self.fill(arena, kept, rows);
    }

    /// The 3x3 tiles around (tx, ty) are all in the window.
    fn inside3(&self, tx: i32, ty: i32) -> bool {
        (tx.wrapping_sub(self.x0 + 1) as u32) < (WIN - 2) as u32 && (ty.wrapping_sub(self.y0 + 1) as u32) < (WIN - 2) as u32
    }

    fn inside(&self, tx: i32, ty: i32) -> bool {
        (tx.wrapping_sub(self.x0) as u32) < WIN as u32 && (ty.wrapping_sub(self.y0) as u32) < WIN as u32
    }

    /// Bits (dx + 1) of row ty for dx in -1..=1. The rotate wraps the torus.
    fn row3(&self, tx: i32, ty: i32) -> u16 {
        (self.rows[(ty & (WIN - 1)) as usize].rotate_right(((tx - 1) & (WIN - 1)) as u32) & 0b111) as u16
    }

    /// Bit (dy + 1) * 3 + (dx + 1): tile (tx + dx, ty + dy) is solid. Only for `inside3` tiles.
    fn mask(&self, tx: i32, ty: i32) -> u16 {
        self.row3(tx, ty - 1) | self.row3(tx, ty) << 3 | self.row3(tx, ty + 1) << 6
    }

    /// Solid bits for the tiles xs x ys (inside the current window).
    fn fill(&mut self, arena: &Arena, xs: Span, ys: Span) {
        for ty in ys {
            let row = &mut self.rows[(ty & (WIN - 1)) as usize];
            for tx in xs.clone() {
                let m = 1u128 << (tx & (WIN - 1));
                *row = if arena.tile(tx, ty).solid() { *row | m } else { *row & !m };
            }
        }
    }

    /// Cached view: window tiles from the bits, the rest from the arena (same answer, slower).
    pub fn near<'a>(&'a self, arena: &'a Arena) -> Near<'a> {
        Near { walls: self, arena }
    }
}

/// Window origin (tiles) that centres the tile under (x_mm, y_mm).
fn origin(x_mm: i32, y_mm: i32) -> (i32, i32) {
    (x_mm.div_euclid(TILE_MM) - WIN / 2, y_mm.div_euclid(TILE_MM) - WIN / 2)
}

#[derive(Clone, Copy)]
pub struct Near<'a> {
    walls: &'a Walls,
    arena: &'a Arena,
}

impl Near<'_> {
    /// 3x3 solid mask around a tile whose neighbourhood is in the window.
    #[cfg(test)]
    fn mask(&self, tx: i32, ty: i32) -> Option<u16> {
        self.walls.inside3(tx, ty).then(|| self.walls.mask(tx, ty))
    }
}

impl Tiles for Near<'_> {
    fn solid(&self, tx: i32, ty: i32) -> bool {
        let w = self.walls;
        if w.inside(tx, ty) { w.rows[(ty & (WIN - 1)) as usize] >> (tx & (WIN - 1)) & 1 == 1 } else { self.arena.tile(tx, ty).solid() }
    }

    /// Same answer as `blocked_tiles`: r <= one tile keeps the circle inside the 3x3 around its centre tile, so one
    /// mask holds every tile it can touch; only the solid ones its bounding box reaches get the distance test.
    fn blocked(&self, x_mm: i32, y_mm: i32, radius_mm: i32) -> bool {
        let r = radius_mm.max(0);
        let (tx, ty) = (x_mm.div_euclid(TILE_MM), y_mm.div_euclid(TILE_MM));
        if r > TILE_MM || !self.walls.inside3(tx, ty) {
            return blocked_tiles(self, x_mm, y_mm, radius_mm);
        }
        let m = self.walls.mask(tx, ty);
        if m == 0 {
            return false;
        }
        let (lx, ly) = (x_mm - tx * TILE_MM, y_mm - ty * TILE_MM);
        // bounding box reaches the column / row on each side: lx - r < 0, lx + r >= TILE_MM
        let cols = COL[1] | if lx < r { COL[0] } else { 0 } | if lx + r >= TILE_MM { COL[2] } else { 0 };
        let rows = ROW[1] | if ly < r { ROW[0] } else { 0 } | if ly + r >= TILE_MM { ROW[2] } else { 0 };
        let mut hit = m & cols & rows;
        let r2 = i64::from(r) * i64::from(r);
        while hit != 0 {
            let b = hit.trailing_zeros() as i32;
            if touches(x_mm, y_mm, tx + b % 3 - 1, ty + b / 3 - 1, r2) {
                return true;
            }
            hit &= hit - 1;
        }
        false
    }
}

/// Closest point of tile (tx, ty) to (x, y) lies within sqrt(r2): round corners behave.
fn touches(x_mm: i32, y_mm: i32, tx: i32, ty: i32, r2: i64) -> bool {
    let nx = x_mm.clamp(tx * TILE_MM, (tx + 1) * TILE_MM - 1);
    let ny = y_mm.clamp(ty * TILE_MM, (ty + 1) * TILE_MM - 1);
    let (dx, dy) = (i64::from(x_mm - nx), i64::from(y_mm - ny));
    dx * dx + dy * dy <= r2
}

/// Circle (centre mm, radius mm) overlaps a solid tile. Tests the tiles under its bounding box.
fn blocked_tiles(t: &impl Tiles, x_mm: i32, y_mm: i32, radius_mm: i32) -> bool {
    let r = radius_mm.max(0);
    let (tx0, tx1) = ((x_mm - r).div_euclid(TILE_MM), (x_mm + r).div_euclid(TILE_MM));
    let (ty0, ty1) = ((y_mm - r).div_euclid(TILE_MM), (y_mm + r).div_euclid(TILE_MM));
    let r2 = i64::from(r) * i64::from(r);
    (ty0..=ty1).any(|ty| (tx0..=tx1).any(|tx| t.solid(tx, ty) && touches(x_mm, y_mm, tx, ty, r2)))
}

/// Circle (centre mm, radius mm) overlaps a solid tile.
pub fn blocked(t: &impl Tiles, x_mm: i32, y_mm: i32, radius_mm: i32) -> bool {
    t.blocked(x_mm, y_mm, radius_mm)
}

/// Allowed move for an attempted (dx, dy): axes separated, so a blocked axis slides along the wall.
/// Returns the (dx, dy) actually applied (each either the attempt or 0).
pub fn slide(t: &impl Tiles, x_mm: i32, y_mm: i32, radius_mm: i32, dx: i32, dy: i32) -> (i32, i32) {
    let ax = if t.blocked(x_mm + dx, y_mm, radius_mm) { 0 } else { dx };
    let ay = if t.blocked(x_mm + ax, y_mm + dy, radius_mm) { 0 } else { dy };
    (ax, ay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;
    use crate::floorgen::{arena, Tile};

    /// Cache == arena on every tile in and around the window, after each step of a walk that mixes
    /// small steps (strip refills, all 4 directions, diagonals) and long jumps (full refill).
    #[test]
    fn walls_cache_equals_arena_along_a_walk() {
        let c = content();
        let a = arena(3, 0, &c);
        let (mut x, mut y) = (0, 0);
        let mut w = Walls::new(&a, x, y);
        let mut rng = crate::rng::Xo::seeded(5);
        let steps = std::num::NonZeroU32::new(9).unwrap();
        let (mut solids, mut clears, mut hits) = (0, 0, 0);
        for k in 0..60 {
            let jump = if k % 20 == 19 { 300 } else { 1 };
            x += (rng.below(steps) as i32 - 4) * TILE_MM * jump;
            y += (rng.below(steps) as i32 - 4) * TILE_MM * jump;
            w.follow(&a, x, y);
            let n = w.near(&a);
            let (cx, cy) = (x.div_euclid(TILE_MM), y.div_euclid(TILE_MM));
            for ty in cy - WIN / 2 - 2..cy + WIN / 2 + 2 {
                for tx in cx - WIN / 2 - 2..cx + WIN / 2 + 2 {
                    let s = a.tile(tx, ty).solid();
                    solids += u32::from(s);
                    assert_eq!(n.solid(tx, ty), s, "tile {tx},{ty} step {k}");
                    let want = (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| (dx, dy))).filter(|&(dx, dy)| a.tile(tx + dx, ty + dy).solid()).fold(0, |m, (dx, dy)| m | nb_bit(dx, dy));
                    if let Some(m) = n.mask(tx, ty) {
                        assert_eq!(m, want, "mask at {tx},{ty} step {k}");
                        clears += u32::from(m == 0);
                    }
                }
            }
            // blocked through the cache == blocked on the arena, near walls and in the open
            let span = std::num::NonZeroU32::new(200_000).unwrap();
            for _ in 0..1000 {
                let (px, py) = (x + rng.below(span) as i32 - 100_000, y + rng.below(span) as i32 - 100_000);
                let r = rng.below(std::num::NonZeroU32::new(2700).unwrap()) as i32;
                assert_eq!(blocked(&n, px, py, r), blocked(&a, px, py, r), "blocked {px},{py} r {r} step {k}");
                hits += u32::from(blocked(&a, px, py, r));
            }
        }
        assert!(solids > 1000 && clears > 1000 && hits > 100, "control: the walk sees walls, open floor, blocks ({solids} {clears} {hits})");
    }

    /// (tile x, tile y) of the first solid tile east of the origin room's centre, on row 0.
    fn wall_x(a: &Arena) -> i32 {
        (0..).find(|&x| a.tile(x, 0).solid()).unwrap_or(0)
    }

    #[test]
    fn solid_blocks_and_floor_does_not() {
        let c = content();
        let a = arena(1, 0, &c);
        assert!(!blocked(&a, 0, 0, 400), "origin free");
        let wx = wall_x(&a);
        assert!(wx > 0);
        let inside = wx * TILE_MM + TILE_MM / 2;
        assert!(blocked(&a, inside, 0, 100), "centre in a wall");
        // radius reaches the wall face from the floor side
        let face = wx * TILE_MM;
        assert!(!blocked(&a, face - 500, 0, 400));
        assert!(blocked(&a, face - 300, 0, 400));
        // NEG: a floor tile is never solid
        assert_eq!(a.tile(0, 0), Tile::Floor);
    }

    #[test]
    fn slide_stops_at_wall_but_keeps_parallel_motion() {
        let c = content();
        let a = arena(1, 0, &c);
        let face = wall_x(&a) * TILE_MM;
        let (x, y) = (face - 500, 0);
        // into the wall: x killed, y kept
        assert_eq!(slide(&a, x, y, 400, 300, 100), (0, 100));
        // free move passes whole
        assert_eq!(slide(&a, x, y, 400, -300, 100), (-300, 100));
        // NEG: result never ends blocked
        for dx in [-400, 0, 400] {
            for dy in [-400, 0, 400] {
                let (ax, ay) = slide(&a, x, y, 400, dx, dy);
                assert!(!blocked(&a, x + ax, y + ay, 400));
            }
        }
    }

    #[test]
    fn pillar_blocks_and_walls_hold_a_walker() {
        let c = content();
        // pillars exist for some seed, and sliding from the origin never leaves the free set
        let a = (0..40).map(|s| arena(s, 0, &c)).find(|a| (-12..12).any(|x| (-12..12).any(|y| a.tile(x, y) == Tile::Pillar)));
        assert!(a.is_some(), "some seed has pillars");
        let a = a.unwrap_or_else(|| arena(0, 0, &c));
        let (mut x, mut y) = (0, 0);
        for k in 0..2000 {
            let (dx, dy) = [(150, 90), (-70, 160), (-160, -90), (90, -150)][(k / 150) % 4];
            let (ax, ay) = slide(&a, x, y, 400, dx, dy);
            (x, y) = (x + ax, y + ay);
            assert!(!blocked(&a, x, y, 400));
        }
    }
}
