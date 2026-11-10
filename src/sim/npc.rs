//! NPCs: pals who call for help, shopkeepers who may cheat, neutrals who pick a side. Pure model, integer only.
use crate::floorgen::{Arena, Tile, CENTRE as HALF, PITCH, TILE_MM};
use crate::rng::{Stream, Xo};
use std::num::NonZeroU32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Npc {
    Pal { traitor: bool },
    Merchant { honest: bool },
    Blacksmith { honest: bool },
    Jeweler { honest: bool },
    Neutral,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Toucher {
    Hero,
    Foe,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Service {
    Trade,
    Forge,
    Enchant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// pal rescued: drops loot, joins the hero
    Rescued,
    /// shopkeeper offers a service
    Serves(Service),
    /// the NPC turns hostile (betrayal, scam, or neutral touched by a foe first)
    TurnsFoe,
    /// neutral touched by the hero first
    TurnsPal,
    Nothing,
}

/// What happens when `by` touches `npc`. Only the hero rescues or trades; a foe converts neutrals only.
pub fn touch(npc: Npc, by: Toucher) -> Outcome {
    match (npc, by) {
        (Npc::Neutral, Toucher::Hero) => Outcome::TurnsPal,
        (Npc::Neutral, Toucher::Foe) => Outcome::TurnsFoe,
        (_, Toucher::Foe) => Outcome::Nothing,
        (Npc::Pal { traitor: false }, Toucher::Hero) => Outcome::Rescued,
        (Npc::Pal { traitor: true }, Toucher::Hero) => Outcome::TurnsFoe,
        (Npc::Merchant { .. }, Toucher::Hero) => serve(Service::Trade),
        (Npc::Blacksmith { .. }, Toucher::Hero) => serve(Service::Forge),
        (Npc::Jeweler { .. }, Toucher::Hero) => serve(Service::Enchant),
    }
}

/// Every keeper opens shop; a deceiver cheats through its stock (world/shop.rs), not by turning on the hero.
fn serve(s: Service) -> Outcome {
    Outcome::Serves(s)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Placed {
    pub npc: Npc,
    pub x: i32,
    pub y: i32,
}

impl Npc {
    /// Shopkeepers: Some(honest). Everyone else keeps no shop.
    pub fn keeper_honest(self) -> Option<bool> {
        match self {
            Npc::Merchant { honest } | Npc::Blacksmith { honest } | Npc::Jeweler { honest } => Some(honest),
            Npc::Pal { .. } | Npc::Neutral => None,
        }
    }
}

/// Rooms around the origin room that hold NPCs, per axis.
const ROOM_SPAN: u32 = 5;

/// Rejection tries per NPC before it is skipped.
const TRIES: u32 = 32;
/// Stream salt so placement never mirrors the treasure draws on Stream::Map.
const SALT: u64 = 0x4E50_4300_0000_0001;

fn pick(rng: &mut Xo, n: u32) -> u32 {
    rng.below(NonZeroU32::MIN.saturating_add(n - 1))
}

/// Seeded NPC placement on free floor tiles in rooms within ROOM_SPAN/2 of the origin room, never in it.
/// May return fewer than `count` when tries run out (never panics).
pub fn place(seed: u64, arena: &Arena, count: u32, bad_permille: u32) -> Vec<Placed> {
    let mut rng = Xo::stream(seed ^ SALT, Stream::Map);
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let bad = pick(&mut rng, 1000) < bad_permille;
        let npc = match pick(&mut rng, 5) {
            0 | 1 => Npc::Pal { traitor: bad },
            2 => Npc::Merchant { honest: !bad },
            3 => Npc::Blacksmith { honest: !bad },
            _ => Npc::Jeweler { honest: !bad },
        };
        let npc = if pick(&mut rng, 6) == 0 { Npc::Neutral } else { npc };
        for _ in 0..TRIES {
            let half = (ROOM_SPAN / 2) as i32;
            let (rx, ry) = (pick(&mut rng, ROOM_SPAN) as i32 - half, pick(&mut rng, ROOM_SPAN) as i32 - half);
            let (lx, ly) = (pick(&mut rng, PITCH as u32 - 1) as i32 + 1 - HALF, pick(&mut rng, PITCH as u32 - 1) as i32 + 1 - HALF);
            let (tx, ty) = (rx * PITCH + lx, ry * PITCH + ly);
            if (rx, ry) != (0, 0) && arena.tile(tx, ty) == Tile::Floor {
                out.push(Placed { npc, x: tx * TILE_MM + TILE_MM / 2, y: ty * TILE_MM + TILE_MM / 2 });
                break;
            }
        }
    }
    out
}

/// A town's people: honest keepers only, the three trades in turn, placed like `place` does.
pub fn place_town(seed: u64, arena: &Arena, count: u32) -> Vec<Placed> {
    let trades = [Npc::Merchant { honest: true }, Npc::Blacksmith { honest: true }, Npc::Jeweler { honest: true }];
    place(seed, arena, count, 0).into_iter().enumerate().map(|(k, p)| Placed { npc: trades[k % trades.len()], ..p }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;
    use crate::floorgen::arena;

    const ALL: [Npc; 8] = [
        Npc::Pal { traitor: false },
        Npc::Pal { traitor: true },
        Npc::Merchant { honest: true },
        Npc::Blacksmith { honest: false },
        Npc::Jeweler { honest: true },
        Npc::Jeweler { honest: false },
        Npc::Merchant { honest: false },
        Npc::Neutral,
    ];

    #[test]
    fn touch_table() {
        assert_eq!(touch(Npc::Pal { traitor: false }, Toucher::Hero), Outcome::Rescued);
        assert_eq!(touch(Npc::Pal { traitor: true }, Toucher::Hero), Outcome::TurnsFoe);
        assert_eq!(touch(Npc::Neutral, Toucher::Hero), Outcome::TurnsPal);
        assert_eq!(touch(Npc::Neutral, Toucher::Foe), Outcome::TurnsFoe);
        assert_eq!(touch(Npc::Blacksmith { honest: true }, Toucher::Hero), Outcome::Serves(Service::Forge));
        assert!(Npc::Neutral.keeper_honest().is_none());
        assert_eq!(touch(Npc::Merchant { honest: false }, Toucher::Hero), Outcome::Serves(Service::Trade), "deceivers cheat via stock, not by attacking");
    }

    #[test]
    fn neg_foe_never_rescues_or_trades() {
        for n in ALL {
            if n != Npc::Neutral {
                assert_eq!(touch(n, Toucher::Foe), Outcome::Nothing, "{n:?}");
            }
            // NEG: only neutrals can ever become pals
            if n != Npc::Neutral {
                assert_ne!(touch(n, Toucher::Hero), Outcome::TurnsPal, "{n:?}");
            }
        }
    }

    #[test]
    fn placement_pure_free_and_outside_start_room() {
        let c = content();
        let a = arena(3, 0, &c);
        let p = place(3, &a, 12, 250);
        assert_eq!(p, place(3, &a, 12, 250));
        assert_ne!(p, place(4, &a, 12, 250));
        assert!(p.len() >= 8);
        for q in &p {
            assert_eq!(a.tile_mm(q.x, q.y), Tile::Floor);
            let (rx, ry) = ((q.x.div_euclid(TILE_MM) + HALF).div_euclid(PITCH), (q.y.div_euclid(TILE_MM) + HALF).div_euclid(PITCH));
            assert_ne!((rx, ry), (0, 0));
        }
        // mix of kinds appears across seeds
        let kinds: std::collections::HashSet<_> =
            (0..10).flat_map(|s| place(s, &a, 12, 250)).map(|q| std::mem::discriminant(&q.npc)).collect();
        assert_eq!(kinds.len(), 5);
    }
}
