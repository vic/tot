//! Levels of the world: a safe town and the deeps under it, linked by stairs. Each level has its own map (the same
//! seed, another floor), its own people and loot; foes grow tougher with depth. The engine names no place: the view
//! shows the adventure's words, and agents may name levels in their own.
use super::*;
use std::num::NonZeroU8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    /// no waves: keepers and people, the place to rest, trade and talk
    Town,
    /// depth 1, 2, ...: the horde, scaled by depth
    Deep(NonZeroU8),
}

impl Level {
    pub const FIRST: Level = Level::Deep(NonZeroU8::MIN);

    /// The arena floor of this level (town = 0).
    fn floor(self) -> u8 {
        match self {
            Level::Town => 0,
            Level::Deep(d) => d.get(),
        }
    }

    /// Foe toughness here, permille: depth 1 = 1000, each level deeper DEPTH_STEP more.
    pub fn foe_pm(self) -> i64 {
        match self {
            Level::Town => 1000,
            Level::Deep(d) => 1000 + DEPTH_STEP * i64::from(d.get() - 1),
        }
    }

    /// The level the down stairs lead to (the deepest level stays the deepest).
    fn below(self) -> Level {
        match self {
            Level::Town => Level::FIRST,
            Level::Deep(d) => Level::Deep(d.saturating_add(1)),
        }
    }
}

/// A stair on the map: touching it takes the hero to `to`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stair {
    pub x: i32,
    pub y: i32,
    pub to: Level,
}

/// Each level deeper adds this permille to foe HP and contact damage.
const DEPTH_STEP: i64 = 300;
/// An agent's danger 0..=DANGER_MAX: foe toughness DANGER_BASE + DANGER_STEP per step, permille.
const DANGER_BASE: i64 = 500;
const DANGER_STEP: i64 = 250;
pub const DANGER_MAX: u8 = 10;

/// Danger an agent set: 0..=DANGER_MAX only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Danger(u8);

impl Danger {
    pub fn new(d: u8) -> Option<Danger> {
        (d <= DANGER_MAX).then_some(Danger(d))
    }
    pub fn get(self) -> u8 {
        self.0
    }
}

/// What an agent set for a level; None keeps the engine's default.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LevelDef {
    pub name: Option<crew::Word>,
    pub safe: Option<bool>,
    pub danger: Option<Danger>,
    /// (floor, wall) colours
    pub look: Option<(crew::Rgb8, crew::Rgb8)>,
}
/// Stairs stand this far from the level's start (mm), in seeded directions; the hero takes one within STAIR_R.
const STAIR_FAR: i32 = 30_000;
const STAIR_R: i32 = 900;
const STAIR_SALT: u64 = 0x57A1_0000_0000_0005;

impl Sim<'_> {
    /// Stairs of `level`: down to the next depth, and from the deeps up to the town.
    fn place_stairs(&mut self, level: Level) {
        self.stairs.clear();
        let ups = match level {
            Level::Town => vec![level.below()],
            Level::Deep(_) => vec![level.below(), Level::Town],
        };
        for (k, to) in ups.into_iter().enumerate() {
            let h = mix(self.seed ^ STAIR_SALT ^ u64::from(level.floor()) << 8 ^ k as u64);
            let (cx, cy) = self.trig[(h % u64::from(TRIG_N)) as usize];
            let (x, y) = self.free_near(scale(cx, i64::from(STAIR_FAR)), scale(cy, i64::from(STAIR_FAR)), STAIR_R);
            self.stairs.push(Stair { x, y, to });
        }
    }

    /// An agent shapes `level`: the fields it gives replace earlier ones (the rest keep the engine's defaults).
    pub fn shape_level(&mut self, level: Level, d: LevelDef) {
        let i = match self.level_defs.iter().position(|(l, _)| *l == level) {
            Some(i) => i,
            None => {
                self.level_defs.push((level, LevelDef::default()));
                self.level_defs.len() - 1
            }
        };
        let cur = &mut self.level_defs[i].1;
        cur.name = d.name.or(cur.name);
        cur.safe = d.safe.or(cur.safe);
        cur.danger = d.danger.or(cur.danger);
        cur.look = d.look.or(cur.look);
    }

    fn def(&self, level: Level) -> LevelDef {
        self.level_defs.iter().find(|(l, _)| *l == level).map_or(LevelDef::default(), |(_, d)| *d)
    }

    /// The name an agent gave `level`, if any.
    pub fn named(&self, level: Level) -> Option<&str> {
        self.level_defs.iter().find(|(l, _)| *l == level).and_then(|(_, d)| d.name.as_ref()).map(crew::Word::as_str)
    }

    /// A safe level has no waves and town-style people: the town by default, any level an agent makes safe.
    pub fn safe(&self, level: Level) -> bool {
        self.def(level).safe.unwrap_or(level == Level::Town)
    }

    /// Foe toughness here, permille: the agent's danger when it set one (0 = DANGER_BASE, each step DANGER_STEP
    /// more), else the depth.
    pub fn level_pm(&self) -> i64 {
        self.def(self.level).danger.map_or(self.level.foe_pm(), |d| DANGER_BASE + DANGER_STEP * i64::from(d.get()))
    }

    /// The (floor, wall) colours an agent gave the level the hero is on.
    pub fn look(&self) -> Option<(crew::Rgb8, crew::Rgb8)> {
        self.def(self.level).look
    }

    /// Set up the level the run starts on.
    pub(super) fn first_level(&mut self) {
        self.place_stairs(self.level);
    }

    /// The hero on a stair takes it.
    pub(super) fn stairs_step(&mut self) {
        let r2 = i64::from(STAIR_R).pow(2);
        if let Some(st) = self.stairs.iter().find(|s| dist2(s.x - self.p.x, s.y - self.p.y) <= r2).copied() {
            self.travel(st.to);
        }
    }

    /// Go to `to`: its map, its people and loot, no foes; pals come along, foe crew bodies stay behind (they leave
    /// the crew). The hero arrives at the level's start.
    pub fn travel(&mut self, to: Level) {
        self.level = to;
        self.arena = crate::floorgen::arena(self.seed, to.floor(), self.c);
        (self.p.x, self.p.y) = (0, 0);
        self.walls = solid::Walls::new(&self.arena, 0, 0);
        (self.p.x, self.p.y) = self.free_near(0, 0, self.c.player.radius);
        self.walls.follow(&self.arena, self.p.x, self.p.y);
        for i in (0..self.e.len()).rev() {
            self.e.remove(i);
        }
        self.hijacked = 0;
        self.boss = None;
        self.shots = Shots::default();
        self.drops = Drops::default();
        self.tele.clear();
        self.mines.clear();
        self.crew.retain(|m| matches!(m.body, crew::Body::Pal { .. }));
        for k in 0..self.crew.len() {
            if let crew::Body::Pal { hp, max, .. } = self.crew[k].body {
                let (x, y) = self.free_near(self.p.x + 1_500, self.p.y, 400);
                self.crew[k].body = crew::Body::Pal { x, y, hp, max };
            }
        }
        let town = self.safe(to);
        self.world.enter(self.seed ^ u64::from(to.floor()).rotate_left(40), &self.arena, &self.c.world, town);
        self.world.stock_shops(self.c);
        if town {
            self.treasures.clear();
        }
        self.place_stairs(to);
        self.seat_people();
        self.level_ev.push(to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;

    #[test]
    fn stairs_link_town_and_deeps_and_each_level_is_its_own_place() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        assert_eq!(s.level, Level::FIRST);
        let tos: Vec<Level> = s.stairs.iter().map(|st| st.to).collect();
        assert!(tos.contains(&Level::Town) && tos.contains(&Level::Deep(NonZeroU8::new(2).unwrap())), "{tos:?}");
        let map1 = s.arena().salt;
        // walk onto the down stair: depth 2, a new map, no foes left
        s.push_enemy(c.fam_ids().next().unwrap(), Tier::Trash, (s.p.x + 5_000, s.p.y), 1_000, (false, false));
        let down = *s.stairs.iter().find(|st| st.to != Level::Town).unwrap();
        (s.p.x, s.p.y) = (down.x, down.y);
        s.stairs_step();
        assert_eq!(s.level, Level::Deep(NonZeroU8::new(2).unwrap()));
        assert_ne!(s.arena().salt, map1, "another map");
        assert_eq!(s.e.len(), 0, "the foes stayed above");
        assert_eq!(s.level_ev, vec![s.level]);
        assert!(Level::Deep(NonZeroU8::new(2).unwrap()).foe_pm() > Level::FIRST.foe_pm(), "deeper is tougher");
        // up to the town: keepers only, and the director may not send foes there
        let up = *s.stairs.iter().find(|st| st.to == Level::Town).unwrap();
        (s.p.x, s.p.y) = (up.x, up.y);
        s.stairs_step();
        assert_eq!(s.level, Level::Town);
        assert!(!s.world.npcs.is_empty() && s.world.npcs.iter().all(|p| p.npc.keeper_honest() == Some(true)), "honest keepers in town");
        let fam = c.fam_ids().find(|&f| matches!(c.fam(f).kind, FamKind::Mob(_))).unwrap();
        let spawn = Cmd::Spawn { fam, n: NonZeroU16::MIN, tier: Tier::Trash, form: Formation::Scatter };
        assert_eq!(s.validate(&spawn), Err(Reject::Town), "no waves in town");
        assert_eq!(s.stairs.iter().map(|st| st.to).collect::<Vec<_>>(), vec![Level::FIRST], "the town's stair leads down");
        // NEG: a hero off the stairs stays where it is
        let lvl = s.level;
        s.p.x += 5_000;
        s.stairs_step();
        assert_eq!(s.level, lvl);
    }

    #[test]
    fn agents_shape_levels_safe_havens_deadly_dungeons_and_their_look() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let fam = c.fam_ids().find(|&f| matches!(c.fam(f).kind, FamKind::Mob(_))).unwrap();
        let spawn = Cmd::Spawn { fam, n: NonZeroU16::MIN, tier: Tier::Trash, form: Formation::Scatter };
        let deep3 = Level::Deep(NonZeroU8::new(3).unwrap());
        let look = (crew::Rgb8(10, 20, 30), crew::Rgb8(200, 100, 50));
        s.shape_level(deep3, LevelDef { safe: Some(true), look: Some(look), ..LevelDef::default() });
        s.shape_level(deep3, LevelDef { name: crew::Word::parse("Haven"), ..LevelDef::default() });
        assert_eq!(s.validate(&spawn), Ok(()), "control: the deeps take waves");
        s.travel(deep3);
        assert_eq!((s.validate(&spawn), s.look(), s.named(deep3)), (Err(Reject::Town), Some(look), Some("Haven")), "a safe haven an agent made deep down, the later field kept the earlier ones");
        assert!(s.world.npcs.iter().all(|p| p.npc.keeper_honest() == Some(true)), "town-style people");
        // danger: a deadly dungeon at depth 1, a gentle one at depth 5
        let pm = |s: &mut Sim, l: Level, d: Option<u8>| {
            s.shape_level(l, LevelDef { danger: d.and_then(Danger::new), ..LevelDef::default() });
            s.travel(l);
            s.level_pm()
        };
        let base1 = pm(&mut s, Level::FIRST, None);
        assert_eq!(base1, Level::FIRST.foe_pm(), "control: no danger set, the depth decides");
        assert!(pm(&mut s, Level::FIRST, Some(10)) > pm(&mut s, Level::Deep(NonZeroU8::new(5).unwrap()), Some(0)));
        assert!(Danger::new(DANGER_MAX + 1).is_none(), "NEG: danger past the cap");
    }
}
