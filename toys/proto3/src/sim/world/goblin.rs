//! Treasure goblins: appear now and then near the hero, flee from them, drop a gold trail, vanish when far.
//! Touching one catches it for a gold burst. Pure state machine over integer mm; Sim glue lives in world.rs.
use crate::floorgen::Arena;
use crate::sim::solid::{blocked, slide};
use crate::sim::{dist, dist2, toward};

/// Goblin body radius, flee speed (mm per tick), catch radius, despawn distance (mm).
pub const RADIUS: i32 = 300;
pub const SPEED: i32 = 14;
pub const CATCH_R: i32 = 900;
pub const GONE_R: i32 = 60_000;
/// Gold dropped per trail pile and per catch burst.
pub const TRAIL_GOLD: u16 = 5;
pub const CATCH_GOLD: u16 = 50;
/// Ticks between trail piles.
pub const TRAIL_EVERY: u32 = 30;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Goblin {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GoldPile {
    pub x: i32,
    pub y: i32,
    pub gold: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fate {
    Running,
    /// hero touched it: burst of gold at its position
    Caught,
    /// ran out of the hero's range
    Escaped,
}

/// One tick: flee along the away-vector, sliding along walls. Caught beats Escaped beats Running.
pub fn step(g: &mut Goblin, arena: &Arena, hero: (i32, i32)) -> Fate {
    let (dx, dy) = (g.x - hero.0, g.y - hero.1);
    let d2 = dist2(dx, dy);
    if d2 <= i64::from(CATCH_R).pow(2) {
        return Fate::Caught;
    }
    if d2 > i64::from(GONE_R).pow(2) {
        return Fate::Escaped;
    }
    let (mx, my) = toward(dx, dy, SPEED);
    // cornered against a wall: slide; if both axes are blocked it stays put (and gets caught)
    let (ax, ay) = slide(arena, g.x, g.y, RADIUS, mx, my);
    g.x += ax;
    g.y += ay;
    Fate::Running
}

/// Spawn point on the ring `r` mm from the hero at angle given by a unit-ish (cx, cy), first free floor.
pub fn spawn_at(arena: &Arena, hero: (i32, i32), (cx, cy): (i32, i32), r: i32) -> Option<Goblin> {
    let (vx, vy) = toward(cx, cy, r);
    let (x, y) = (hero.0 + vx, hero.1 + vy);
    (!blocked(arena, x, y, RADIUS) && dist(x - hero.0, y - hero.1) > CATCH_R).then_some(Goblin { x, y })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;
    use crate::floorgen::arena;

    #[test]
    fn flees_away_from_hero_and_never_enters_a_wall() {
        let c = content();
        let a = arena(1, 0, &c);
        let mut g = Goblin { x: 5000, y: 0 };
        let mut last = dist2(g.x, g.y);
        for _ in 0..200 {
            let f = step(&mut g, &a, (0, 0));
            assert!(!blocked(&a, g.x, g.y, RADIUS));
            if f != Fate::Running {
                break;
            }
            let d = dist2(g.x, g.y);
            assert!(d >= last, "never closes in");
            last = d;
        }
        assert!(g.x > 5000, "moved away");
    }

    #[test]
    fn catch_and_escape_fates_with_neg() {
        let c = content();
        let a = arena(1, 0, &c);
        let mut near = Goblin { x: CATCH_R, y: 0 };
        assert_eq!(step(&mut near, &a, (0, 0)), Fate::Caught);
        let mut far = Goblin { x: GONE_R + 1, y: 0 };
        assert_eq!(step(&mut far, &a, (0, 0)), Fate::Escaped);
        // NEG: just outside catch range and inside gone range keeps running
        let mut mid = Goblin { x: CATCH_R + 100, y: 0 };
        assert_eq!(step(&mut mid, &a, (0, 0)), Fate::Running);
    }

    #[test]
    fn spawn_only_on_free_floor_outside_catch_range() {
        let c = content();
        let a = arena(1, 0, &c);
        let ok = spawn_at(&a, (0, 0), (1, 0), 6000);
        assert!(ok.is_some());
        // NEG: a spawn point inside a wall is refused
        let wall_x = (0..).map(|t| t * crate::floorgen::TILE_MM).find(|&x| blocked(&a, x, 0, RADIUS)).expect("a wall east of origin");
        assert!(spawn_at(&a, (0, 0), (1, 0), wall_x).is_none());
    }
}
