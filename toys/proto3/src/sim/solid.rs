//! Solid world for the sim: walls and pillars from floorgen::Arena block actors. Integer only, O(1) per query.
use crate::floorgen::{Arena, TILE_MM};

/// Circle (centre mm, radius mm) overlaps a solid tile. Tests the tiles under its bounding box:
/// closest-point distance per tile, so round corners behave.
pub fn blocked(arena: &Arena, x_mm: i32, y_mm: i32, radius_mm: i32) -> bool {
    let r = radius_mm.max(0);
    let (tx0, tx1) = ((x_mm - r).div_euclid(TILE_MM), (x_mm + r).div_euclid(TILE_MM));
    let (ty0, ty1) = ((y_mm - r).div_euclid(TILE_MM), (y_mm + r).div_euclid(TILE_MM));
    let r2 = i64::from(r) * i64::from(r);
    (ty0..=ty1).any(|ty| {
        (tx0..=tx1).any(|tx| {
            arena.tile(tx, ty).solid() && {
                let nx = x_mm.clamp(tx * TILE_MM, (tx + 1) * TILE_MM - 1);
                let ny = y_mm.clamp(ty * TILE_MM, (ty + 1) * TILE_MM - 1);
                let (dx, dy) = (i64::from(x_mm - nx), i64::from(y_mm - ny));
                dx * dx + dy * dy <= r2
            }
        })
    })
}

/// Allowed move for an attempted (dx, dy): axes separated, so a blocked axis slides along the wall.
/// Returns the (dx, dy) actually applied (each either the attempt or 0).
pub fn slide(arena: &Arena, x_mm: i32, y_mm: i32, radius_mm: i32, dx: i32, dy: i32) -> (i32, i32) {
    let ax = if blocked(arena, x_mm + dx, y_mm, radius_mm) { 0 } else { dx };
    let ay = if blocked(arena, x_mm + ax, y_mm + dy, radius_mm) { 0 } else { dy };
    (ax, ay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;
    use crate::floorgen::{arena, Tile};

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
