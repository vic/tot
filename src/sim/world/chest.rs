//! World chests: plain loot, boss chests (mini/super/uber), and false chests (bomb, mimic pack).
//! Pure model + seeded placement; Sim effects live in world.rs.
use crate::floorgen::{Arena, Tile, CENTRE as HALF, PITCH, TILE_MM};
use crate::rng::{Stream, Xo};
use std::num::NonZeroU32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BossClass {
    Mini,
    Super,
    Uber,
}

impl BossClass {
    /// Class of the boss with HP rank `rank` among `n` bosses sorted by HP: spread evenly, weakest Mini, strongest
    /// Uber (with n >= 2 every class below Uber is reached by rounding, and Uber is never empty).
    pub fn of_rank(rank: usize, n: usize) -> BossClass {
        match (rank * 4 + n.saturating_sub(1)) / (2 * n.saturating_sub(1)).max(1) {
            0 => BossClass::Mini,
            1 => BossClass::Super,
            _ => BossClass::Uber,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChestKind {
    /// one upgrade pick
    Plain,
    /// opens into a boss of this class
    Boss(BossClass),
    /// explodes after a telegraphed fuse
    Bomb,
    /// opens into a pack of foes
    Mimic,
}

impl ChestKind {
    /// False chests give themselves away once the hero is near.
    pub fn is_false(self) -> bool {
        matches!(self, ChestKind::Bomb | ChestKind::Mimic)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WorldChest {
    pub kind: ChestKind,
    pub x: i32,
    pub y: i32,
    /// hint state for the view: set once the hero has come near a false chest, never before
    pub tell: bool,
}

const ROOM_SPAN: u32 = 7;
const TRIES: u32 = 32;
const SALT: u64 = 0x4348_5354_0000_0001;

fn pick(rng: &mut Xo, n: u32) -> u32 {
    rng.below(NonZeroU32::MIN.saturating_add(n - 1))
}

/// Seeded chests on free floor in rooms around the origin (also the origin room: loot near the start is fine).
/// Mix: 4 plain, 2 boss, 2 bomb, 2 mimic per 10. May return fewer than `count`.
pub fn place(seed: u64, arena: &Arena, count: u32) -> Vec<WorldChest> {
    let mut rng = Xo::stream(seed ^ SALT, Stream::Map);
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let kind = match pick(&mut rng, 10) {
            0..=3 => ChestKind::Plain,
            4 | 5 => ChestKind::Boss([BossClass::Mini, BossClass::Super, BossClass::Uber][pick(&mut rng, 3) as usize]),
            6 | 7 => ChestKind::Bomb,
            _ => ChestKind::Mimic,
        };
        for _ in 0..TRIES {
            let half = (ROOM_SPAN / 2) as i32;
            let (rx, ry) = (pick(&mut rng, ROOM_SPAN) as i32 - half, pick(&mut rng, ROOM_SPAN) as i32 - half);
            let (lx, ly) = (pick(&mut rng, PITCH as u32 - 1) as i32 + 1 - HALF, pick(&mut rng, PITCH as u32 - 1) as i32 + 1 - HALF);
            let (tx, ty) = (rx * PITCH + lx, ry * PITCH + ly);
            // not on the start tile area: the hero must walk to it
            if arena.tile(tx, ty) == Tile::Floor && (tx.abs() > 2 || ty.abs() > 2) {
                out.push(WorldChest { kind, x: tx * TILE_MM + TILE_MM / 2, y: ty * TILE_MM + TILE_MM / 2, tell: false });
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;
    use crate::floorgen::arena;

    #[test]
    fn classes_spread_weakest_to_strongest() {
        let cs: Vec<_> = (0..6).map(|r| BossClass::of_rank(r, 6)).collect();
        assert_eq!(cs, [BossClass::Mini, BossClass::Mini, BossClass::Super, BossClass::Super, BossClass::Uber, BossClass::Uber]);
        assert_eq!(BossClass::of_rank(0, 1), BossClass::Mini);
        let two: Vec<_> = (0..2).map(|r| BossClass::of_rank(r, 2)).collect();
        assert_eq!(two, [BossClass::Mini, BossClass::Uber], "two bosses still reach Uber");
    }

    #[test]
    fn placement_pure_free_untold_and_mixed() {
        let c = content();
        let a = arena(2, 0, &c);
        let p = place(2, &a, 40);
        assert_eq!(p, place(2, &a, 40));
        assert_ne!(p, place(3, &a, 40));
        assert!(p.iter().all(|w| a.tile_mm(w.x, w.y) == Tile::Floor && !w.tell));
        let has = |f: &dyn Fn(ChestKind) -> bool| p.iter().any(|w| f(w.kind));
        assert!(has(&|k| matches!(k, ChestKind::Boss(_))) && has(&|k| k == ChestKind::Bomb) && has(&|k| k == ChestKind::Mimic));
        // NEG: plain and boss chests are not false
        assert!(p.iter().all(|w| w.kind.is_false() == matches!(w.kind, ChestKind::Bomb | ChestKind::Mimic)));
        assert!(!ChestKind::Plain.is_false() && !ChestKind::Boss(BossClass::Uber).is_false());
    }
}
