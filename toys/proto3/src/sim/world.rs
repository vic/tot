//! World events on top of the solid arena: weather, NPCs. Everything here is deterministic and called once
//! per tick from step() via `world_step`.
use super::npc::{place, touch, Outcome, Placed, Service, Toucher};
use super::weather::{sheltered, Forecast, Permille, Schedule};
use super::{dist2, BossState, Cause, FamKind, Sim, Tier};
use chest::{place as place_chests, BossClass, ChestKind, WorldChest};
use goblin::{Fate, Goblin, GoldPile};
use hazard::{Trap, TrapState};
use shop::{BuyErr, Catalog, Deal, ItemId};
use crate::floorgen::Arena;
use crate::data::FamId;
use crate::rng::mix;
use std::num::NonZeroU32;

/// A false chest shows its tell when the hero is this close (mm).
const TELL_R: i32 = 6000;
/// Bomb: fuse (s), blast radius (mm), damage as a fraction of max HP (permille), mimic pack size.
const FUSE_S: u32 = 2;
const BLAST_R: i32 = 6000;
const BLAST_DMG: i32 = 300;
const PACK: i32 = 6;
/// Mimic pack spawns on a ring of this radius around the chest (mm).
const PACK_R: i32 = 1500;
/// Weather segment length (s).
const SEGMENT_S: NonZeroU32 = NonZeroU32::MIN.saturating_add(29);
/// Touch radii (mm): hero meets an NPC, a foe meets an NPC.
const HERO_TOUCH: i32 = 1000;
const FOE_TOUCH: i32 = 1500;
/// A goblin appears every this many seconds (while fewer than GOBLIN_MAX run), at this distance (mm).
const GOBLIN_EVERY_S: u32 = 25;
const GOBLIN_MAX: usize = 2;
const GOBLIN_SPAWN_R: i32 = 14_000;
/// Gold piles are picked up within this radius (mm).
const GOLD_R: i32 = 1000;
/// Ticks the goblin test lets a goblin run (a few trail intervals).
#[cfg(test)]
const TRAIL_TICKS: u32 = goblin::TRAIL_EVERY * 4;
const MAX_PILES: usize = 64;
/// A living boss drops a telegraphed bomb on the hero every BOMB_EVERY_S and a trap at its own feet every TRAP_EVERY_S.
const BOMB_EVERY_S: u32 = 6;
const TRAP_EVERY_S: u32 = 4;
const MAX_TRAPS: usize = 16;
const GOBLIN_SALT: u64 = 0x474F_4200_0000_0001;
/// Salt so the foe-family pick is independent of the weather key.
const FOE_SALT: u64 = 0x464F_4500_0000_0001;

pub mod chest;
pub mod goblin;
pub mod hazard;
pub mod shop;

/// A lit fuse; the view draws the blast area while it burns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Blast {
    pub x: i32,
    pub y: i32,
    pub ticks_left: u32,
}

/// Position folded to one hash input (distinct NPCs/chests sit on distinct points).
fn pos_key(x: i32, y: i32) -> u64 {
    (u64::from(x as u32) << 32) | u64::from(y as u32)
}

pub struct World {
    /// world chests not yet opened (view draws them; `tell` marks a revealed false chest)
    pub chests: Vec<WorldChest>,
    pub blasts: Vec<Blast>,
    /// treasure goblins on the run (view draws them)
    pub goblins: Vec<Goblin>,
    /// traps a living boss dropped (view draws them; dim while arming)
    pub traps: Vec<Trap>,
    /// gold lying around: goblin trails and catch bursts
    pub gold_piles: Vec<GoldPile>,
    goblins_spawned: u32,
    schedule: Schedule,
    /// seeds which mob family an NPC turns into (mixed with its position)
    foe_key: u64,
    /// NPCs still standing; view draws these (kind readable from `Placed::npc`)
    pub npcs: Vec<Placed>,
    /// services the hero has earned and not yet used (the shop UI consumes these)
    pub services: Vec<Service>,
    /// stock per NPC, parallel to `npcs` (empty for non-shops); filled by `stock_shops`
    pub shops: Vec<Vec<Deal>>,
    /// index into `npcs` of the shop the hero stands at, if any
    pub open_shop: Option<usize>,
    /// pals won (rescued, or neutrals the hero touched first)
    pub pals: u16,
}

impl World {
    pub fn new(seed: u64, arena: &Arena, hz: NonZeroU32, def: &crate::data::WorldDef) -> World {
        // total: a tick count that would overflow u32 saturates, the schedule just gets one long segment
        let seg = hz.saturating_mul(SEGMENT_S);
        let npcs = place(seed, arena, def.npcs, def.bad);
        let shops = vec![Vec::new(); npcs.len()];
        World { schedule: Schedule::new(seed, seg), foe_key: mix(seed ^ FOE_SALT), chests: place_chests(seed, arena, def.chests), blasts: Vec::new(), goblins: Vec::new(), traps: Vec::new(), gold_piles: Vec::new(), goblins_spawned: 0, npcs, services: Vec::new(), shops, open_shop: None, pals: 0 }
    }

    pub fn forecast(&self, tick: u32) -> Forecast {
        self.schedule.forecast(tick)
    }

    /// Blood-moon buffs the foe-side code multiplies in (driver wires these into spawn hp / contact damage).
    pub fn foe_hp(&self, tick: u32) -> Permille {
        self.forecast(tick).now.foe_hp()
    }

    /// Wind push this tick (mm per tick), zero unless the sky is Wind.
    pub fn wind(&self, tick: u32, hz: u32) -> (i32, i32) {
        self.schedule.wind(tick, hz)
    }

    pub fn foe_dmg(&self, tick: u32) -> Permille {
        self.forecast(tick).now.foe_dmg()
    }

    /// Fill every service NPC's stock from the item catalog (Sim::new passes Content).
    pub fn stock_shops(&mut self, cat: &impl Catalog) {
        for (p, deals) in self.npcs.iter().zip(self.shops.iter_mut()) {
            if let Some(honest) = p.npc.keeper_honest() {
                *deals = shop::stock(cat, honest, mix(self.foe_key ^ pos_key(p.x, p.y)));
            }
        }
    }

    /// Buy offer `offer_ix` from NPC `npc_ix` with `gold`; only the shop the hero stands at is open.
    /// Returns the item and the gold left (the caller stores it back into Player.gold).
    pub fn buy(&mut self, npc_ix: usize, offer_ix: usize, gold: u32) -> Result<(ItemId, u32), BuyErr> {
        if self.open_shop != Some(npc_ix) {
            return Err(BuyErr::Closed);
        }
        let deals = self.shops.get_mut(npc_ix).ok_or(BuyErr::Closed)?;
        shop::buy(deals, offer_ix, gold)
    }

    /// Removing an NPC keeps `shops` parallel and shifts `open_shop` with it.
    fn remove_npc(&mut self, i: usize) {
        self.npcs.remove(i);
        self.shops.remove(i);
        self.open_shop = match self.open_shop {
            Some(o) if o == i => None,
            Some(o) if o > i => Some(o - 1),
            other => other,
        };
    }
}

impl Sim<'_> {
    pub(super) fn world_step(&mut self) {
        let hz = self.c.hz().get();
        let per_second = self.tick.is_multiple_of(hz);
        if per_second {
            self.weather_hero();
        }
        self.npc_touches(per_second);
        self.chest_touches();
        self.blasts_burn();
        self.goblins_step();
        self.boss_hazards();
    }

    /// Rain heals, acid hurts, shelter blocks both. Weather alone never kills: hp floors at 1.
    fn weather_hero(&mut self) {
        let w = self.world.forecast(self.tick).now;
        let d = w.hero_hp_per_s(sheltered(&self.arena, self.p.x, self.p.y));
        let before = self.p.hp;
        let ceiling = self.p.max_hp.max(before);
        let floor = before.min(1);
        self.p.hp = (before + d).min(ceiling).max(floor);
        // only the HP actually lost counts as damage taken
        self.stats.taken += i64::from((before - self.p.hp).max(0));
    }

    /// Hero touches every tick; foes are sampled once a second (npcs x enemies stays cheap).
    fn npc_touches(&mut self, foes_too: bool) {
        let mut i = 0;
        let mut at_shop = None;
        while i < self.world.npcs.len() {
            let Placed { npc, x, y } = self.world.npcs[i];
            let by = if dist2(x - self.p.x, y - self.p.y) <= i64::from(HERO_TOUCH).pow(2) {
                Some(Toucher::Hero)
            } else if foes_too && self.foe_near(x, y) {
                Some(Toucher::Foe)
            } else {
                None
            };
            match by.map_or(Outcome::Nothing, |b| touch(npc, b)) {
                Outcome::Nothing => i += 1,
                // a keeper stays at the counter: the shop is open while the hero stands here
                Outcome::Serves(s) => {
                    at_shop = Some(i);
                    if self.world.open_shop != at_shop {
                        self.world.services.push(s);
                    }
                    i += 1;
                }
                out => {
                    self.world.remove_npc(i);
                    self.npc_outcome(out, x, y);
                }
            }
        }
        self.world.open_shop = at_shop;
    }

    fn foe_near(&self, x: i32, y: i32) -> bool {
        let r2 = i64::from(FOE_TOUCH).pow(2);
        (0..self.e.x.len()).any(|k| self.e.side[k] == super::Side::Hostile && dist2(self.e.x[k] - x, self.e.y[k] - y) <= r2)
    }

    fn npc_outcome(&mut self, out: Outcome, x: i32, y: i32) {
        match out {
            Outcome::Rescued => {
                self.world.pals += 1;
                self.pending_chest += self.c.world.rescue_chests;
            }
            Outcome::TurnsPal => self.world.pals += 1,
            Outcome::TurnsFoe => {
                if let Some(fam) = self.foe_fam(mix(self.world.foe_key ^ pos_key(x, y))) {
                    self.spawn_mob(fam, Tier::Trash, x, y);
                }
            }
            // keepers are handled in npc_touches (they stay at the counter)
            Outcome::Serves(_) | Outcome::Nothing => {}
        }
    }

    /// Hero touch opens a chest; a false chest shows its tell first, when the hero is near.
    fn chest_touches(&mut self) {
        let (px, py) = (self.p.x, self.p.y);
        let mut i = 0;
        while i < self.world.chests.len() {
            let w = self.world.chests[i];
            let d2 = dist2(w.x - px, w.y - py);
            if w.kind.is_false() && d2 <= i64::from(TELL_R).pow(2) {
                self.world.chests[i].tell = true;
            }
            if d2 > i64::from(HERO_TOUCH).pow(2) || !self.open_chest(w) {
                i += 1;
            } else {
                self.world.chests.swap_remove(i);
            }
        }
    }

    /// Returns false when the chest stays shut (a boss chest while another boss lives).
    fn open_chest(&mut self, w: WorldChest) -> bool {
        match w.kind {
            ChestKind::Plain => self.pending_chest += 1,
            ChestKind::Boss(class) => return self.boss_from_chest(class, w.x, w.y),
            ChestKind::Bomb => {
                let ticks_left = self.c.hz().get() * FUSE_S;
                self.world.blasts.push(Blast { x: w.x, y: w.y, ticks_left });
            }
            ChestKind::Mimic => {
                for k in 0..PACK {
                    let Some(fam) = self.foe_fam(mix(self.world.foe_key ^ pos_key(w.x, w.y) ^ u64::from(k as u32))) else { return true };
                    let (cx, cy) = self.trig[(k as usize * self.trig.len() / PACK as usize) % self.trig.len().max(1)];
                    self.spawn_mob(fam, Tier::Trash, w.x + super::scale(cx, i64::from(PACK_R)), w.y + super::scale(cy, i64::from(PACK_R)));
                }
            }
        }
        true
    }

    /// Mob family for a turned NPC / pack member: weighted draw over mob families with `weight > 0`
    /// (the data's spawn weight), keyed by `draw` (hash of run seed and position), so it is deterministic.
    fn foe_fam(&self, draw: u64) -> Option<FamId> {
        let c = self.c;
        let weight = |f: FamId| match c.fam(f).kind {
            FamKind::Mob(ref m) => u64::from(m.weight),
            _ => 0,
        };
        let total: u64 = c.fam_ids().map(weight).sum();
        let mut roll = draw.checked_rem(total)?;
        c.fam_ids().find(|&f| {
            let w = weight(f);
            let hit = roll < w;
            roll = roll.saturating_sub(w);
            hit
        })
    }

    /// Boss fams of `class`: bosses ranked by HP (see BossClass::of_rank); an empty class falls back to any boss. Chest bosses never end the run (last = false).
    fn boss_of_class(&self, class: BossClass) -> Option<FamId> {
        let c = self.c;
        let mut bosses: Vec<(i32, FamId)> = c.fam_ids().filter_map(|f| if let FamKind::Boss { hp } = c.fam(f).kind { Some((hp, f)) } else { None }).collect();
        bosses.sort_by_key(|&(hp, _)| hp);
        let n = bosses.len();
        let mut ofs: Vec<FamId> = bosses.iter().enumerate().filter(|&(r, _)| BossClass::of_rank(r, n) == class).map(|(_, &(_, f))| f).collect();
        if ofs.is_empty() {
            ofs = bosses.iter().map(|&(_, f)| f).collect();
        }
        // picked by tick so repeated runs agree
        ofs.get(self.tick as usize % ofs.len().max(1)).copied()
    }

    fn boss_from_chest(&mut self, class: BossClass, x: i32, y: i32) -> bool {
        let Some(fam) = self.boss_of_class(class) else { return true };
        let FamKind::Boss { hp } = self.c.fam(fam).kind else { return true };
        if self.boss.is_some() {
            return false;
        }
        self.boss = Some(BossState { id: self.next_id, fam, phase: 0, timers: [0; 4], hard_t: 0, age: 0, last: false });
        self.stats.win_spawned += i64::from(hp);
        self.push_enemy(fam, Tier::Boss, (x, y), hp, (false, false));
        true
    }

    /// Goblins spawn on a seeded schedule, flee, leave a trail; the hero collects piles by walking over them.
    fn goblins_step(&mut self) {
        let hero = (self.p.x, self.p.y);
        let hz = self.c.hz().get();
        if self.tick > 0 && self.tick.is_multiple_of(hz * GOBLIN_EVERY_S) && self.world.goblins.len() < GOBLIN_MAX {
            let n = self.world.goblins_spawned;
            self.world.goblins_spawned += 1;
            let ang = mix(self.world.foe_key ^ GOBLIN_SALT ^ u64::from(n)) % self.trig.len().max(1) as u64;
            if let Some(g) = goblin::spawn_at(&self.arena, hero, self.trig[ang as usize], GOBLIN_SPAWN_R) {
                self.world.goblins.push(g);
            }
        }
        let trail = self.tick.is_multiple_of(goblin::TRAIL_EVERY);
        let mut i = 0;
        while i < self.world.goblins.len() {
            let mut g = self.world.goblins[i];
            match goblin::step(&mut g, &self.arena, hero) {
                Fate::Running => {
                    if trail {
                        self.world.gold_piles.push(GoldPile { x: g.x, y: g.y, gold: goblin::TRAIL_GOLD });
                    }
                    self.world.goblins[i] = g;
                    i += 1;
                }
                Fate::Caught => {
                    self.world.gold_piles.push(GoldPile { x: g.x, y: g.y, gold: goblin::CATCH_GOLD });
                    self.world.goblins.swap_remove(i);
                }
                Fate::Escaped => {
                    self.world.goblins.swap_remove(i);
                }
            }
        }
        // oldest piles fall off first so an ignored trail cannot grow without bound
        let excess = self.world.gold_piles.len().saturating_sub(MAX_PILES);
        self.world.gold_piles.drain(..excess);
        let r2 = i64::from(GOLD_R).pow(2);
        let mut got = 0u32;
        self.world.gold_piles.retain(|p| {
            let hit = dist2(p.x - hero.0, p.y - hero.1) <= r2;
            if hit {
                got += u32::from(p.gold);
            }
            !hit
        });
        self.p.gold += got;
    }

    /// Lit fuses burn down; at zero the blast hurts a hero inside the radius (iframes protect).
    fn blasts_burn(&mut self) {
        let (px, py, max) = (self.p.x, self.p.y, self.p.max_hp);
        let mut hit = false;
        self.world.blasts.retain_mut(|b| {
            b.ticks_left = b.ticks_left.saturating_sub(1);
            if b.ticks_left > 0 {
                return true;
            }
            hit |= dist2(b.x - px, b.y - py) <= i64::from(BLAST_R).pow(2);
            false
        });
        // the fuse is telegraphed, so a blast hit is foreseen
        if hit {
            self.hurt_player((i64::from(max) * i64::from(BLAST_DMG) / 1000) as i32, Cause::Blast, true);
        }
    }

    /// Direct HP loss of `permille` of `max` unless iframes protect (traps: armour not applied, last_hit unset).
    fn hero_hit(&mut self, max: i32, permille: i32) {
        if self.p.iframes == 0 {
            let d = (i64::from(max) * i64::from(permille) / 1000) as i32;
            self.p.unhurt = 0;
            self.p.hp -= d;
            self.stats.taken += i64::from(d);
        }
    }

    /// While a boss lives it bombs the hero's spot (telegraphed fuse) and lays traps at its feet.
    /// No boss, no new hazards; existing traps still age out and still bite.
    fn boss_hazards(&mut self) {
        let hz = self.c.hz().get();
        let boss = (0..self.e.x.len()).find(|&k| self.e.tier[k] == Tier::Boss).map(|k| (self.e.x[k], self.e.y[k]));
        if let Some((bx, by)) = boss {
            if self.tick > 0 && self.tick.is_multiple_of(hz * BOMB_EVERY_S) {
                self.world.blasts.push(Blast { x: self.p.x, y: self.p.y, ticks_left: hz * FUSE_S });
            }
            if self.tick > 0 && self.tick.is_multiple_of(hz * TRAP_EVERY_S) && self.world.traps.len() < MAX_TRAPS {
                self.world.traps.push(Trap { x: bx, y: by, age: 0 });
            }
        }
        let (px, py, max) = (self.p.x, self.p.y, self.p.max_hp);
        let mut sprung = 0;
        self.world.traps.retain_mut(|t| {
            t.age += 1;
            match t.state(hz) {
                TrapState::Expired => false,
                TrapState::Armed if dist2(t.x - px, t.y - py) <= i64::from(hazard::TRIGGER_R).pow(2) => {
                    sprung += 1;
                    false
                }
                _ => true,
            }
        });
        for _ in 0..sprung {
            self.hero_hit(max, hazard::DMG);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::npc::Npc;
    use super::super::weather::Weather;
    use super::*;
    use crate::data::tests::content;

    fn sim_with(c: &crate::data::Content, npc: Npc) -> Sim<'_> {
        let mut s = Sim::new(c, 7, None);
        s.world.npcs = vec![Placed { npc, x: s.p.x + 5000, y: s.p.y }];
        s
    }

    #[test]
    fn rescued_pal_drops_chest_and_traitor_spawns_foe() {
        let c = content();
        let mut s = sim_with(&c, Npc::Pal { traitor: false });
        s.world_step();
        assert_eq!(s.pending_chest, 0, "out of reach");
        s.p.x += 4500;
        s.world_step();
        assert_eq!((s.pending_chest, s.world.pals, s.world.npcs.len()), (1, 1, 0));

        let mut s = sim_with(&c, Npc::Pal { traitor: true });
        let before = s.e.x.len();
        s.p.x += 4500;
        s.world_step();
        assert_eq!((s.e.x.len(), s.pending_chest), (before + 1, 0), "traitor becomes a foe, no loot");
    }

    #[test]
    fn world_record_sets_pal_reward_and_population() {
        let at = crate::data::tests::texts().iter().position(|t| t.contains("\"t\":\"world\"")).unwrap();
        let mut ts = crate::data::tests::texts();
        ts[at] = ts[at].replace("\"npcs\":12,\"bad_chance\":0.25,\"chests\":10,\"rescue_chests\":1", "\"npcs\":3,\"bad_chance\":0.25,\"chests\":10,\"rescue_chests\":2");
        let (base, c) = (content(), crate::data::Content::parse(&ts).unwrap());
        assert_ne!(base.world.rescue_chests, c.world.rescue_chests, "control edit applied");
        let mut s = sim_with(&c, Npc::Pal { traitor: false });
        s.p.x += 4500;
        s.world_step();
        assert_eq!(s.pending_chest, 2, "a rescued pal gives the adventure's chest count");
        assert!(Sim::new(&c, 7, None).world.npcs.len() <= 3 && Sim::new(&base, 7, None).world.npcs.len() > 3, "the adventure's NPC count");
    }

    #[test]
    fn neg_foe_touching_pal_does_nothing() {
        let c = content();
        let mut s = sim_with(&c, Npc::Pal { traitor: false });
        let (px, py) = (s.world.npcs[0].x, s.world.npcs[0].y);
        s.spawn_mob(c.fam_ids().find(|&f| matches!(c.fam(f).kind, FamKind::Mob(_))).expect("content has a mob family"), Tier::Trash, px, py);
        let foes = s.e.x.len();
        s.tick = 0; // per-second sample tick
        s.world_step();
        assert_eq!((s.world.npcs.len(), s.e.x.len(), s.pending_chest, s.world.pals), (1, foes, 0, 0));
        // control: a neutral next to the same foe does turn
        s.world.npcs[0].npc = Npc::Neutral;
        s.world_step();
        assert_eq!((s.world.npcs.len(), s.e.x.len()), (0, foes + 1));
    }

    /// First per-second tick inside a segment of weather `w`.
    fn tick_of(s: &Sim, w: Weather) -> u32 {
        let seg = s.c.hz().get() * 30;
        (0..4000).map(|k| k * seg).find(|&t| s.world.forecast(t).now == w).expect("weather occurs in 4000 segments")
    }

    fn pillar_spot(s: &Sim) -> (i32, i32) {
        (-12..12)
            .flat_map(|x| (-12..12).map(move |y| (x, y)))
            .find(|&(x, y)| s.arena.tile(x, y) == crate::floorgen::Tile::Pillar)
            .expect("the seed-7 arena has a pillar near the origin")
    }

    #[test]
    fn acid_lowers_hp_unless_sheltered_rain_heals() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        s.world.npcs.clear();
        s.world.chests.clear();
        s.tick = tick_of(&s, Weather::AcidRain);
        let full = s.p.hp;
        s.world_step();
        assert!(s.p.hp < full, "exposed: acid hurts");
        assert_eq!(s.stats.taken, i64::from(full - s.p.hp), "taken = the real loss");
        let (tx, ty) = pillar_spot(&s);
        (s.p.x, s.p.y) = (tx * crate::floorgen::TILE_MM, ty * crate::floorgen::TILE_MM);
        let (hp, taken) = (s.p.hp, s.stats.taken);
        s.world_step();
        assert_eq!((s.p.hp, s.stats.taken), (hp, taken), "sheltered: no acid");
        s.tick = tick_of(&s, Weather::Rain);
        (s.p.x, s.p.y, s.p.hp) = (0, 0, 1000);
        s.world_step();
        assert!(s.p.hp > 1000, "rain heals");
    }

    /// Under the full moon foes run faster than under a clear sky (same foe, same spot, one step).
    /// A hero sheltering from acid rain draws the horde: foes come faster than under a clear sky, same spot.
    /// A mid-boss (mini) leaves a false chest where it falls; a super boss does not.
    #[test]
    fn shelter_draws_foes_and_mini_boss_leaves_a_false_chest() {
        use crate::floorgen::TILE_MM;
        let c = content();
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        let stride = |w: Weather| {
            let mut s = Sim::new(&c, 7, None);
            s.world.npcs.clear();
            s.tick = tick_of(&s, w);
            let (px, py) = pillar_spot(&s);
            let spot = (-3..=3).flat_map(|dx| (-3..=3).map(move |dy| (px + dx, py + dy))).find(|&(x, y)| s.arena.tile(x, y) == crate::floorgen::Tile::Floor && super::super::weather::sheltered(&s.arena, x * TILE_MM + TILE_MM / 2, y * TILE_MM + TILE_MM / 2)).unwrap();
            (s.p.x, s.p.y) = (spot.0 * TILE_MM + TILE_MM / 2, spot.1 * TILE_MM + TILE_MM / 2);
            s.walls = crate::sim::solid::Walls::new(&s.arena, s.p.x, s.p.y);
            s.push_enemy(fam, crate::sim::Tier::Trash, (s.p.x + 8_000, s.p.y + 3_000), 1_000_000, (false, false));
            let (x0, y0) = (s.e.x[0], s.e.y[0]);
            s.step(&crate::sim::Input::default());
            crate::sim::isqrt(dist2(s.e.x[0] - x0, s.e.y[0] - y0))
        };
        let (clear, acid) = (stride(Weather::Clear), stride(Weather::AcidRain));
        assert!(clear > 0 && acid * 1000 >= clear * 1150, "sheltered under acid {acid} vs clear {clear}");
        let falls = |key: &str| {
            let mut s = Sim::new(&c, 7, None);
            s.world.npcs.clear();
            s.world.chests.clear();
            let f = c.fam_ids().find(|&f| c.fam(f).key == key).unwrap();
            s.push_enemy(f, crate::sim::Tier::Boss, (s.p.x + 20_000, s.p.y), 1, (false, false));
            s.e.hp[0] = 0;
            s.step(&crate::sim::Input::default());
            s.world.chests.iter().filter(|w| matches!(w.kind, chest::ChestKind::Bomb | chest::ChestKind::Mimic) && !w.tell).count()
        };
        assert_eq!((falls("kiosk"), falls("sweeper")), (1, 0), "mini leaves one false chest, super none");
    }

    /// Snow slows the hero, heat burns the exposed hero, wind pushes a standing hero downwind.
    #[test]
    fn snow_heat_and_wind_act_on_the_hero() {
        let c = content();
        let walk = |w: Weather, mv: (i32, i32)| {
            let mut s = Sim::new(&c, 7, None);
            s.world.npcs.clear();
            s.tick = tick_of(&s, w);
            let (x0, y0, hp0) = (s.p.x, s.p.y, s.p.hp);
            s.step(&crate::sim::Input { mv, ..crate::sim::Input::default() });
            ((s.p.x - x0, s.p.y - y0), hp0 - s.p.hp, s.world.wind(s.tick, c.hz().get()))
        };
        let (clear, _, calm) = walk(Weather::Clear, (1, 0));
        let (snow, _, _) = walk(Weather::Snow, (1, 0));
        assert!(calm == (0, 0) && snow.0 < clear.0 && snow.0 > 0, "snow slows: {snow:?} vs {clear:?}");
        let (still, _, wind) = walk(Weather::Wind, (0, 0));
        assert!(wind != (0, 0) && still == wind, "wind pushes a standing hero: {still:?} by {wind:?}");
        let mut s = Sim::new(&c, 7, None);
        s.tick = tick_of(&s, Weather::Heat);
        (s.p.x, s.p.y, s.p.hp) = (0, 0, 100_000);
        s.world_step();
        assert!(s.p.hp < 100_000, "heat burns the exposed hero");
    }

    /// The horde runs faster at night (same clear sky, same spot).
    #[test]
    fn foes_run_faster_at_night() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let hz = c.hz().get();
        let clear = |s: &Sim, t: u32| s.world.forecast(t).now == Weather::Clear;
        let day = (0..2000u32).map(|k| k * hz).find(|&t| clear(&s, t) && t / hz % crate::sim::DAY_S < crate::sim::DAY_S * 2 / 3).unwrap();
        let night = (0..2000u32).map(|k| k * hz).find(|&t| clear(&s, t) && t / hz % crate::sim::DAY_S >= crate::sim::DAY_S * 2 / 3).unwrap();
        s.tick = day;
        assert_eq!(s.foe_pace().0, 1000, "control: a clear day is the base pace");
        s.tick = night;
        assert_eq!(s.foe_pace().0, crate::sim::NIGHT_PACE, "a clear night runs at night pace");
    }

    #[test]
    fn full_moon_frenzies_foes() {
        let c = content();
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        let stride = |w: Weather| {
            let mut s = Sim::new(&c, 7, None);
            s.tick = tick_of(&s, w);
            s.world.npcs.clear();
            s.push_enemy(fam, crate::sim::Tier::Trash, (s.p.x + 8_000, s.p.y), 1_000_000, (false, false));
            let x0 = s.e.x[0];
            s.step(&crate::sim::Input::default());
            (x0 - s.e.x[0]).abs()
        };
        let (clear, full) = (stride(Weather::Clear), stride(Weather::FullMoon));
        assert!(clear > 0 && full * 1000 >= clear * 1200, "full moon {full} vs clear {clear}");
    }

    #[test]
    fn neg_weather_never_kills_and_taken_is_the_real_loss() {
        let c = content();
        for start in [1, 300] {
            let mut s = Sim::new(&c, 7, None);
            s.world.npcs.clear();
            s.world.chests.clear();
            s.tick = tick_of(&s, Weather::AcidRain);
            s.p.hp = start;
            s.world_step();
            assert_eq!(s.p.hp, 1, "hp {start}: floors at 1");
            assert_eq!(s.stats.taken, i64::from(start - 1), "hp {start}: only the real loss is counted");
        }
    }

    fn sim_chest(c: &crate::data::Content, kind: ChestKind) -> Sim<'_> {
        let mut s = Sim::new(c, 7, None);
        s.world.npcs.clear();
        s.world.chests = vec![WorldChest { kind, x: s.p.x + 20_000, y: s.p.y, tell: false }];
        s
    }

    #[test]
    fn boss_chest_spawns_boss_of_class_and_stays_shut_while_one_lives() {
        let c = content();
        for class in [BossClass::Mini, BossClass::Super, BossClass::Uber] {
            let mut s = sim_chest(&c, ChestKind::Boss(class));
            s.world_step();
            assert!(s.boss.is_none() && s.world.chests.len() == 1, "out of reach");
            s.p.x += 19_500;
            s.world_step();
            assert!(s.world.chests.is_empty());
            let Some(b) = s.boss.as_ref() else { panic!("{class:?}: no boss") };
            assert!(!b.last, "chest boss never ends the run");
            assert_eq!(s.e.tier.last(), Some(&Tier::Boss));
        }
        // NEG: a second boss chest stays shut while a boss lives
        let mut s = sim_chest(&c, ChestKind::Boss(BossClass::Mini));
        s.p.x += 19_500;
        s.world_step();
        s.world.chests = vec![WorldChest { kind: ChestKind::Boss(BossClass::Uber), x: s.p.x, y: s.p.y, tell: false }];
        let bosses = s.e.tier.iter().filter(|&&t| t == Tier::Boss).count();
        s.world_step();
        assert_eq!((s.world.chests.len(), s.e.tier.iter().filter(|&&t| t == Tier::Boss).count()), (1, bosses));
    }

    #[test]
    fn bomb_telegraphs_then_hurts_only_inside_radius() {
        let c = content();
        let mut s = sim_chest(&c, ChestKind::Bomb);
        s.p.x += 19_500;
        s.world_step();
        assert_eq!(s.world.blasts.len(), 1, "fuse lit");
        let full = s.p.hp;
        for _ in 0..c.hz().get() * FUSE_S - 2 {
            s.world_step();
        }
        assert_eq!((s.p.hp, s.world.blasts.len()), (full, 1), "no damage while the fuse burns");
        s.world_step();
        assert!(s.world.blasts.is_empty() && s.p.hp < full, "boom");
        // NEG: hero outside the radius is untouched
        let mut s = sim_chest(&c, ChestKind::Bomb);
        s.p.x += 19_500;
        s.world_step();
        s.p.x -= 19_000;
        s.p.y -= 2 * BLAST_R;
        let full = s.p.hp;
        for _ in 0..c.hz().get() * FUSE_S {
            s.world_step();
        }
        assert_eq!(s.p.hp, full);
    }

    #[test]
    fn mimic_spawns_pack_and_tell_only_after_near() {
        let c = content();
        let mut s = sim_chest(&c, ChestKind::Mimic);
        s.world_step();
        assert!(!s.world.chests[0].tell, "far: no tell");
        s.p.x += 15_000;
        s.world_step();
        assert!(s.world.chests[0].tell, "near: tell shown");
        let foes = s.e.x.len();
        s.p.x += 4_500;
        s.world_step();
        assert_eq!(s.e.x.len(), foes + PACK as usize);
        // NEG: plain and boss chests never show a tell
        let mut s = sim_chest(&c, ChestKind::Plain);
        s.p.x += 15_000;
        s.world_step();
        assert!(!s.world.chests[0].tell);
        s.p.x += 4_500;
        s.world_step();
        assert_eq!((s.pending_chest, s.world.chests.len()), (1, 0));
    }

    #[test]
    fn foe_family_is_a_weighted_mob_and_deterministic() {
        let c = content();
        let s = Sim::new(&c, 7, None);
        for d in 0..200u64 {
            let f = s.foe_fam(mix(d)).expect("a weighted mob family exists");
            assert_eq!(Some(f), s.foe_fam(mix(d)));
            // NEG: never a boss and never a zero-weight family
            assert!(matches!(c.fam(f).kind, FamKind::Mob(ref m) if m.weight > 0), "draw {d}");
        }
    }

    #[test]
    fn goblin_appears_flees_trails_gold_and_catch_pays() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        s.world.npcs.clear();
        s.world.chests.clear();
        let hz = c.hz().get();
        s.tick = hz * GOBLIN_EVERY_S;
        s.world.goblins_spawned = 0;
        // some ring angle may land in a wall: try the schedule a few times, it is deterministic per index
        for _ in 0..8 {
            s.goblins_step();
            if !s.world.goblins.is_empty() {
                break;
            }
            s.tick += hz * GOBLIN_EVERY_S;
        }
        assert_eq!(s.world.goblins.len(), 1, "a goblin spawned");
        let start = s.world.goblins[0];
        for _ in 0..TRAIL_TICKS {
            s.tick += 1;
            s.p.x = 0;
            s.p.y = 0;
            s.goblins_step();
        }
        assert!(dist2(s.world.goblins[0].x, s.world.goblins[0].y) > dist2(start.x, start.y), "fled");
        assert!(!s.world.gold_piles.is_empty(), "left a trail");
        // hero walks onto a pile: gold goes up, pile gone
        let p = s.world.gold_piles[0];
        (s.p.x, s.p.y) = (p.x, p.y);
        s.goblins_step();
        assert!(s.p.gold >= u32::from(p.gold));
        // catch: hero on the goblin pays the burst
        let g = s.world.goblins[0];
        (s.p.x, s.p.y) = (g.x, g.y);
        s.world.gold_piles.clear();
        let before = s.p.gold;
        s.goblins_step();
        assert!(s.world.goblins.is_empty());
        assert_eq!(s.p.gold, before + u32::from(goblin::CATCH_GOLD), "caught: burst collected on the spot");
        // NEG: nothing happens off-schedule
        let mut s = Sim::new(&c, 7, None);
        s.tick = 1;
        s.goblins_step();
        assert!(s.world.goblins.is_empty() && s.p.gold == 0);
    }

    fn boss_sim(c: &crate::data::Content) -> Sim<'_> {
        let mut s = Sim::new(c, 7, None);
        s.world.npcs.clear();
        s.world.chests.clear();
        let fam = c.fam_ids().find(|&f| matches!(c.fam(f).kind, FamKind::Boss { .. })).expect("content has a boss");
        s.world.chests = vec![WorldChest { kind: ChestKind::Boss(BossClass::Mini), x: s.p.x, y: s.p.y, tell: false }];
        s.world_step();
        assert!(s.e.tier.contains(&Tier::Boss), "boss {fam:?} spawned via chest");
        s
    }

    #[test]
    fn living_boss_bombs_and_lays_traps_that_arm_then_bite() {
        let c = content();
        let mut s = boss_sim(&c);
        let hz = c.hz().get();
        s.p.iframes = 0;
        s.tick = hz * TRAP_EVERY_S;
        s.boss_hazards();
        assert_eq!(s.world.traps.len(), 1, "trap dropped at the boss");
        let t = s.world.traps[0];
        // still arming: standing on it is harmless
        (s.p.x, s.p.y) = (t.x, t.y);
        let hp = s.p.hp;
        s.tick += 1;
        s.boss_hazards();
        assert_eq!((s.p.hp, s.world.traps.len()), (hp, 1), "arming trap is a tell, not a hit");
        for _ in 0..hz {
            s.tick += 1;
            s.boss_hazards();
        }
        assert!(s.p.hp < hp && s.world.traps.is_empty(), "armed trap bites once and is spent");
        // bombs: the 6 s mark lights a fuse on the hero
        s.tick = hz * BOMB_EVERY_S;
        let fuses = s.world.blasts.len();
        s.boss_hazards();
        assert_eq!(s.world.blasts.len(), fuses + 1);
    }

    #[test]
    fn neg_no_boss_no_hazards_and_far_hero_untouched() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let hz = c.hz().get();
        s.tick = hz * TRAP_EVERY_S * BOMB_EVERY_S;
        s.boss_hazards();
        assert!(s.world.traps.is_empty() && s.world.blasts.is_empty(), "no boss, no hazards");
        // NEG: an armed trap far from the hero never bites, and expires
        s.world.traps.push(Trap { x: 50_000, y: 0, age: hz });
        let hp = s.p.hp;
        s.boss_hazards();
        assert_eq!((s.p.hp, s.world.traps.len()), (hp, 1));
        s.world.traps[0].age = hz * hazard::LIFE_S;
        s.boss_hazards();
        assert!(s.world.traps.is_empty());
    }

    struct Cat;
    impl Catalog for Cat {
        fn len(&self) -> usize {
            10
        }
        fn info(&self, id: ItemId) -> Option<shop::ItemInfo> {
            use shop::Rarity::*;
            let rarity = [Common, Uncommon, Rare, Epic, Legendary][usize::from(id.0) % 5];
            Some(shop::ItemInfo { rarity, cursed: id.0 % 5 == 4 })
        }
    }

    #[test]
    fn keeper_opens_only_while_hero_is_at_the_counter_and_buy_pays() {
        let c = content();
        let mut s = sim_with(&c, Npc::Merchant { honest: true });
        s.world.stock_shops(&Cat);
        assert!(!s.world.shops[0].is_empty());
        assert_eq!(s.world.buy(0, 0, 1000), Err(BuyErr::Closed), "hero is away");
        s.p.x += 4500;
        s.world_step();
        assert_eq!((s.world.open_shop, s.world.npcs.len()), (Some(0), 1), "keeper stays");
        assert_eq!(s.world.services.len(), 1, "service recorded once on opening");
        s.world_step();
        assert_eq!(s.world.services.len(), 1);
        let price = s.world.shops[0][0].price;
        let n = s.world.shops[0].len();
        let (_, left) = s.world.buy(0, 0, price + 7).expect("affordable");
        assert_eq!((left, s.world.shops[0].len()), (7, n - 1));
        assert_eq!(s.world.buy(0, 99, 1000), Err(BuyErr::NoSuchOffer));
        // NEG: too poor
        let p2 = s.world.shops[0][0].price;
        assert_eq!(s.world.buy(0, 0, p2 - 1), Err(BuyErr::NotEnough { price: p2, have: p2 - 1 }));
        // walking off closes it
        s.p.x -= 20_000;
        s.world_step();
        assert_eq!((s.world.open_shop, s.world.buy(0, 0, 1000)), (None, Err(BuyErr::Closed)));
    }

    #[test]
    fn content_items_stock_the_shop_and_input_buy_spends_the_purse() {
        let c = content();
        let mut s = sim_with(&c, Npc::Merchant { honest: true });
        s.world.stock_shops(&c);
        assert!(!s.world.shops[0].is_empty() && s.world.shops[0].iter().all(|d| usize::from(d.item.0) < c.items.len()));
        let buy = crate::sim::Input { buy: Some(0), ..crate::sim::Input::default() };
        let price = s.world.shops[0][0].price;
        s.p.gold = price;
        s.step(&buy);
        assert_eq!(s.p.gold, price, "NEG: away from the counter nothing is sold");
        s.p.x += 4500;
        s.world_step();
        let (n, item) = (s.world.shops[0].len(), s.world.shops[0][0].item);
        s.step(&buy);
        assert_eq!((s.p.gold, s.world.shops[0].len()), (0, n - 1), "bought: purse paid, offer gone");
        assert_eq!(s.gear.worn(c.items[usize::from(item.0)].slot), Some(item), "bought item is worn");
        s.step(&buy);
        assert_eq!(s.world.shops[0].len(), n - 1, "NEG: empty purse buys nothing");
    }

    #[test]
    fn deceiver_keeper_stocks_trash_and_non_keepers_have_no_shop() {
        let c = content();
        let mut s = sim_with(&c, Npc::Witch { honest: false });
        s.world.stock_shops(&Cat);
        assert!(s.world.shops[0].iter().all(|d| !d.honest));
        let mut s = sim_with(&c, Npc::Pal { traitor: false });
        s.world.stock_shops(&Cat);
        assert!(s.world.shops[0].is_empty(), "NEG: a pal keeps no shop");
    }

    #[test]
    fn removing_an_npc_keeps_shops_parallel_and_open_shop_pointing_at_the_same_keeper() {
        let c = content();
        let mut s = sim_with(&c, Npc::Pal { traitor: false });
        let (x, y) = (s.world.npcs[0].x, s.world.npcs[0].y);
        s.world.npcs.push(Placed { npc: Npc::Merchant { honest: true }, x, y });
        s.world.shops = vec![Vec::new(); 2];
        s.world.open_shop = Some(1);
        s.world.remove_npc(0);
        assert_eq!((s.world.npcs.len(), s.world.shops.len(), s.world.open_shop), (1, 1, Some(0)));
        s.world.remove_npc(0);
        assert_eq!((s.world.shops.len(), s.world.open_shop), (0, None));
    }
}
