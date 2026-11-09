//! Deterministic integer sim (design sec 7, 10.3, 11). SoA arrays preallocated to caps: no heap
//! allocation per tick. World unit = milli-metre (i32), HP = milli-HP (i32).
use crate::data::{
    AttackKind, Content, FamId, FamKind, Formation, Hue, MAX_SKILLS, ShapeDef, SkillId, SkillKind, WaveEv,
};
use crate::rng::{mix, Stream, Xo};
use std::num::{NonZeroU16, NonZeroU32};

mod npc;
mod solid;
mod step;
pub mod weather;
mod world;
pub use npc::Npc;
#[cfg(test)]
pub use npc::Placed;
pub use world::hazard::TrapState;
pub use world::shop::ItemId;
#[cfg(test)]
pub use world::goblin::{Goblin, GoldPile};
pub use weather::Weather;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickChoice {
    Card(u8),
    Skip,
}

/// One tick of player intent. Same type for human keys and bots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Input {
    /// World direction signs, each -1..=1.
    pub mv: (i32, i32),
    pub dodge: bool,
    pub hack: bool,
    pub autofire: bool,
    /// manual fire: bolts aim at a target this tick even with autofire off
    pub fire: bool,
    pub pick: Option<PickChoice>,
    /// offer index to buy at the shop the hero stands at
    pub buy: Option<usize>,
    /// take off the item in this slot
    pub drop: Option<crate::data::Slot>,
    /// number key: cast the special of bar slot n (0-based)
    pub special: Option<u8>,
    /// act on a bag slot: wear it (swapping with what the slot holds) or discard it
    pub bag: Option<BagAct>,
    /// a tree node was learned: gain this skill (or rank it up)
    pub learn: Option<SkillId>,
    /// at a blacksmith: forge an item from ore
    pub forge: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Hostile,
    Hijacked,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Trash,
    Elite,
    Champion,
    Boss,
}

/// Director command (design sec 8.2): the SAME typed set any director (built-in, agent, human)
/// emits. Positions are not in the command: the sim places spawns on the ring, so a spawn in
/// view cannot be expressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Spawn { fam: FamId, n: NonZeroU16, tier: Tier, form: Formation },
    Boss { fam: FamId },
    Flee,
    Narrate { ev: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    LiveCap { live: usize, n: u16, cap: usize },
    BossActive,
    NotMob,
    NotBoss,
    BossTier,
    NoText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Src {
    /// built-in director, no agent asked
    Builtin,
    /// built-in filled a rejected / late / missing agent command
    Fallback,
    Mcp(Client),
    /// `$EDITOR` / `:spawn` / `:prompt`
    Human,
}

/// MCP client name: 1..=16 chars of [a-z0-9_-]. Copy, so log rows stay Copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Client {
    b: [u8; 16],
    n: u8,
}

impl Src {
    /// Closed set (design sec 3): builtin | fallback | human | mcp:<client>.
    pub fn parse(s: &str) -> Option<Src> {
        match s {
            "builtin" => Some(Src::Builtin),
            "fallback" => Some(Src::Fallback),
            "human" => Some(Src::Human),
            _ => {
                let name = s.strip_prefix("mcp:")?.as_bytes();
                let ok = (1..=16).contains(&name.len()) && name.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_');
                let mut b = [0u8; 16];
                b.get_mut(..name.len())?.copy_from_slice(name);
                ok.then_some(Src::Mcp(Client { b, n: name.len() as u8 }))
            }
        }
    }
    pub fn tag(&self) -> String {
        match self {
            Src::Builtin => "builtin".into(),
            Src::Fallback => "fallback".into(),
            Src::Human => "human".into(),
            Src::Mcp(c) => format!("mcp:{}", String::from_utf8_lossy(&c.b[..usize::from(c.n)])),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LogRow {
    pub tick: u32,
    pub src: Src,
    pub cmd: Cmd,
    pub ok: Result<(), Reject>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Contact { fam: FamId, tier: Tier },
    Shot { fam: FamId },
    BossAttack { fam: FamId },
    /// area blast with no family behind it (bomb chests)
    Blast,
}

/// What the hero does with a bag slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BagAct {
    Wear(u8),
    Discard(u8),
}

/// What the last number-key press did; the face answers refusals.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpecialTry {
    Cast,
    Reloading,
    Passive,
    NoTarget,
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Win,
    Dead { cause: Cause, foreseen: bool },
    Timeout,
}

/// World-space telegraph area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Band { along_x: bool, at: i32, half: i32 },
    Lattice { x: i32, y: i32, pitch: i32, half: i32 },
    Circle { x: i32, y: i32, r: i32 },
    Arena,
}

impl Shape {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        match *self {
            Shape::Band { along_x: true, at, half } => (py - at).abs() <= half,
            Shape::Band { along_x: false, at, half } => (px - at).abs() <= half,
            Shape::Lattice { x, y, pitch, half } => {
                let m = |d: i32| (d.rem_euclid(pitch)).min(pitch - d.rem_euclid(pitch)) <= half;
                m(px - x) || m(py - y)
            }
            Shape::Circle { x, y, r } => dist2(px - x, py - y) <= i64::from(r) * i64::from(r),
            Shape::Arena => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeleKind {
    HitPlayer { dmg: i32, cause: Cause },
    PurgeHijacked,
    Spawn { fam: FamId, n: u8 },
}

#[derive(Clone, Copy, Debug)]
pub struct Tele {
    pub shape: Shape,
    pub left: u16,
    pub lead: u16,
    pub kind: TeleKind,
}

#[derive(Clone, Copy, Debug)]
pub enum FxShape {
    Cone { x: i32, y: i32, dx: i32, dy: i32, r: i32, cos: i32 },
    Ring { x: i32, y: i32, r: i32 },
    Line { x0: i32, y0: i32, x1: i32, y1: i32 },
}

#[derive(Clone, Copy, Debug)]
pub struct Fx {
    pub shape: FxShape,
    pub left: u8,
    pub skill: SkillId,
}

/// What happened, for the view's floating text. Ages out after BARK_TICKS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarkKind {
    /// a thief ate loot and grew; `power` = its loot after the bite
    Stole { power: u16 },
    /// a thief crossed a loot threshold and gained this power
    Learned { ability: Ability },
}

#[derive(Clone, Copy, Debug)]
pub struct Bark {
    pub tick: u32,
    pub x: i32,
    pub y: i32,
    /// enemy id (stable), so the view can pick a line per actor
    pub id: u32,
    pub kind: BarkKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    Xp(u16),
    Chest,
    /// a real item (armour, ring...) on the floor; walking over it wears it
    Item(ItemId),
    /// XP magnet: picking it up pulls every XP orb on the floor (and a stream from beyond sight) to the hero
    Magnet,
    /// food: eating it fills the belly by FOOD_MEAL
    Food,
    /// ore: a resource the blacksmith forges with
    Ore,
}

/// How a melee foe closes on the hero, fixed by its id (O(1), no state): a crowd mixes all four.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tactic {
    Rush,
    /// heads for its own point on a ring around the hero, then closes: the crowd spreads out
    Surround,
    /// holds just outside reach and lunges in pulses, to lure the hero out
    Bait,
    /// circles to the side, then closes from behind the hero's facing
    Flank,
}

impl Tactic {
    pub fn of(id: u32) -> Tactic {
        match id % 4 {
            0 => Tactic::Rush,
            1 => Tactic::Surround,
            2 => Tactic::Bait,
            _ => Tactic::Flank,
        }
    }
}

/// Rings of one tile each that free_near searches before giving up.
const NUDGE_RINGS: i32 = 4;
/// Treasure pickup is 500 mm (morale_step); keep a chest this clear of walls.
const TREASURE_CLEAR: i32 = 500;

/// Boss footprint class by max HP (milli-HP): bigger bosses are bigger bodies. The view reads `BossClass::of_kind(tier, kind)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BossClass {
    Mini,
    Super,
    Uber,
}

/// Class edges (milli-HP): below MINI_MAX_HP is Mini, below SUPER_MAX_HP is Super, else Uber.
const MINI_MAX_HP: i32 = 10_000_000;
const SUPER_MAX_HP: i32 = 100_000_000;

impl BossClass {
    pub fn of_hp(hp: i32) -> BossClass {
        if hp < MINI_MAX_HP {
            BossClass::Mini
        } else if hp < SUPER_MAX_HP {
            BossClass::Super
        } else {
            BossClass::Uber
        }
    }

    pub fn of_kind(tier: Tier, kind: &FamKind) -> Option<BossClass> {
        match (tier, kind) {
            (Tier::Boss, FamKind::Boss { hp }) => Some(BossClass::of_hp(*hp)),
            _ => None,
        }
    }

    /// Body radius from the family's base radius, permille steps.
    pub fn radius(self, base: i32) -> i32 {
        scale(
            base,
            match self {
                BossClass::Mini => 1_000,
                BossClass::Super => 1_400,
                BossClass::Uber => 1_900,
            },
        )
    }
}

/// A power a thief learns from loot it ate (derived from `Enemies.loot`, no extra state).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ability {
    /// moves THIEF_FAST faster
    Fast,
    /// flies into a rage on the spot (once, on gaining it)
    Rage,
    /// shoots the hero like a ranged foe (THIEF_SHOT)
    Shot,
}

impl Ability {
    pub const ALL: [Ability; 3] = [Ability::Fast, Ability::Rage, Ability::Shot];

    /// Loot a thief must have eaten to own this (a chest is LOOT_CHEST, an xp gem LOOT_XP).
    pub fn loot_needed(self) -> u16 {
        match self {
            Ability::Fast => 4,
            Ability::Rage => 10,
            Ability::Shot => 16,
        }
    }

    pub fn owned_at(self, loot: u16) -> bool {
        loot >= self.loot_needed()
    }
}

/// Thief speed bonus (permille) with Ability::Fast.
const THIEF_FAST: i64 = 1_300;
/// Thief shot with Ability::Shot: (range mm, shot speed mm/tick, ticks between shots, damage permille of contact).
const THIEF_SHOT: (i32, i32, u16, u32) = (8_000, 300, 60, 1_000);

/// Timed effect on a foe or the hero. Timers count sim ticks down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    None,
    /// foe: does not move; hero: slowed
    Frozen { left: u16 },
    /// foe only: faster and hits harder
    Raging { left: u16 },
    /// loses a slice of max HP each second
    Burning { left: u16 },
    /// just thawed: cannot be frozen again until it ends (no stun lock)
    Thawed { left: u16 },
}

/// Effect lengths and strengths. Milliseconds so the data tick rate does not matter.
const FROZEN_MS: u32 = 1_500;
const THAWED_MS: u32 = 3_000;
const RAGING_MS: u32 = 6_000;
const BURNING_MS: u32 = 3_000;
/// Burn: permille of max HP lost per second (foe and hero).
const BURN_PERMILLE: i32 = 30;
/// A foe below this permille of its max HP flies into a rage (placeholder source until Berserker data lands).
const RAGE_BELOW: i32 = 250;
/// Raging foe: speed and contact damage, permille; speed never passes RAGE_SPEED_CAP of the hero's.
const RAGE_SPEED: i64 = 1_500;
const RAGE_SPEED_CAP: i64 = 950;
const RAGE_DMG: i64 = 1_500;
/// Frozen hero walks at this permille of normal speed.
const HERO_FROZEN_SPEED: i32 = 500;
/// Number-key specials: reload seconds at rank 1 and damage multiplier over the normal shot.
const SPECIAL_RELOAD_S: u64 = 8;
const SPECIAL_DMG: i32 = 3;
/// Item drops: elites drop one in ELITE_ITEM_1_IN (by id), champions and bosses always; salt for the id hash.
const ELITE_ITEM_1_IN: u64 = 4;
const ITEM_SALT: u64 = 0x17E5_D80B;
/// XP magnet: elites drop one in MAGNET_1_IN (by id); on pickup MAGNET_FAR orbs stream in from a ring
/// MAGNET_RING_MIN..+MAGNET_RING_SPAN mm beyond sight.
const MAGNET_1_IN: u64 = 10;
/// Bag slots.
pub const BAG_SLOTS: usize = 12;
/// Ore: trash drops it one in ORE_1_IN (by id); the blacksmith forges an item for FORGE_ORE.
const ORE_1_IN: u64 = 8;
pub const FORGE_ORE: u32 = 5;
/// Foes run this much faster (permille) toward a hero sheltering from rain or acid.
const SHELTER_DRAW: u32 = 1200;
/// Day and night: one cycle every DAY_S seconds (night is its last third); foes run NIGHT_PACE (permille).
pub const DAY_S: u32 = 360;
pub(crate) const NIGHT_PACE: u32 = 1100;
/// Hunger: a full belly (ppm) empties in FOOD_LAST_S seconds; a meal adds FOOD_MEAL; trash drops food one in
/// FOOD_1_IN (by id). Below FOOD_LOW the hero walks at HUNGRY_PACE, empty at STARVED_PACE (permille).
pub const FOOD_FULL: i32 = 1_000_000;
const FOOD_LAST_S: i32 = 300;
const FOOD_MEAL: i32 = 400_000;
const FOOD_1_IN: u64 = 25;
pub const FOOD_LOW: i32 = 300_000;
const HUNGRY_PACE: u32 = 800;
const STARVED_PACE: u32 = 600;
const MAGNET_FAR: u32 = 24;
const MAGNET_RING_MIN: i32 = 4_000;
const MAGNET_RING_SPAN: i32 = 20_000;

impl Status {
    fn ticks(ms: u32, hz: u32) -> u16 {
        (ms * hz / 1000).clamp(1, u32::from(u16::MAX)) as u16
    }

    /// A hit of this hue lands: Therm sets (or renews) burning, Emp freezes unless busy or just thawed.
    pub fn inflict(self, hue: Hue, hz: u32) -> Status {
        match (self, hue) {
            (Status::None | Status::Burning { .. }, Hue::Therm) => Status::Burning { left: Self::ticks(BURNING_MS, hz) },
            (Status::None, Hue::Emp) => Status::Frozen { left: Self::ticks(FROZEN_MS, hz) },
            _ => self,
        }
    }

    pub fn enrage(self, hz: u32) -> Status {
        match self {
            Status::None => Status::Raging { left: Self::ticks(RAGING_MS, hz) },
            _ => self,
        }
    }

    /// One tick later. Frozen thaws into Thawed; the rest end.
    pub fn tick(self, hz: u32) -> Status {
        let down = |left: u16| left.checked_sub(1).filter(|&l| l > 0);
        match self {
            Status::None => Status::None,
            Status::Frozen { left } => down(left).map_or(Status::Thawed { left: Self::ticks(THAWED_MS, hz) }, |left| Status::Frozen { left }),
            Status::Raging { left } => down(left).map_or(Status::None, |left| Status::Raging { left }),
            Status::Burning { left } => down(left).map_or(Status::None, |left| Status::Burning { left }),
            Status::Thawed { left } => down(left).map_or(Status::None, |left| Status::Thawed { left }),
        }
    }
}

/// Spawn point tries around the hero when the first lands in a wall.
const SPAWN_TRIES: u32 = 8;
/// Surround ring radius, mm; inside it the foe just rushes.
const RING_R: i32 = 5_000;
/// id -> ring angle step (1/1024 turn), odd so neighbouring ids spread around the ring
const RING_STEP: u32 = 389;
/// Bait holds at this distance (mm): outside contact reach, inside view.
const BAIT_HOLD: i32 = 2_500;
/// Bait lunges for the first BAIT_LUNGE of every BAIT_PERIOD ticks (staggered by id).
const BAIT_PERIOD: u32 = 60;
const BAIT_LUNGE: u32 = 15;
/// Flank waypoint: this far to the hero's side (mm).
const FLANK_R: i32 = 4_000;

/// Where a melee foe at (x, y) steers this tick, and whether it moves at all.
fn tactic_aim(t: Tactic, id: u32, tick: u32, (x, y): (i32, i32), (px, py): (i32, i32), face: (i32, i32)) -> ((i32, i32), bool) {
    let hero = (px, py);
    let dp2 = dist2(px - x, py - y);
    match t {
        Tactic::Surround if dp2 > i64::from(RING_R).pow(2) => {
            let a = id.wrapping_mul(RING_STEP);
            ((px + crate::trig::cos(a) * RING_R / 1000, py + crate::trig::sin(a) * RING_R / 1000), true)
        }
        Tactic::Bait if dp2 <= i64::from(BAIT_HOLD).pow(2) => (hero, (tick + id) % BAIT_PERIOD < BAIT_LUNGE),
        Tactic::Flank if face != (0, 0) && i64::from(x - px) * i64::from(face.0) + i64::from(y - py) * i64::from(face.1) > 0 => {
            let (fx, fy) = toward(face.0, face.1, FLANK_R);
            // side by id parity: (fx,fy) rotated +-90 degrees
            let (sx, sy) = if id % 8 < 4 { (-fy, fx) } else { (fy, -fx) };
            ((px + sx, py + sy), true)
        }
        _ => (hero, true),
    }
}

pub fn dist2(dx: i32, dy: i32) -> i64 {
    i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)
}
fn dist(dx: i32, dy: i32) -> i32 {
    isqrt(dist2(dx, dy)) as i32
}

/// Worn items, one per slot (indexed by `Slot` order, so two items in one slot cannot be represented).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gear([Option<ItemId>; 5]);

impl Gear {
    /// Wear `id` in its slot; returns the item it replaced. None if `id` is not in the catalog.
    pub fn wear(&mut self, c: &Content, id: ItemId) -> Option<Option<ItemId>> {
        let it = c.items.get(usize::from(id.0))?;
        Some(self.0[it.slot as usize].replace(id))
    }

    /// Take off whatever is in `slot`.
    pub fn drop_slot(&mut self, slot: crate::data::Slot) -> Option<ItemId> {
        self.0[slot as usize].take()
    }

    pub fn worn(&self, slot: crate::data::Slot) -> Option<ItemId> {
        self.0[slot as usize]
    }

    /// Sum of one stat's mods over worn items, in the stat's data units (milli-HP, mm, permille, armour points).
    pub fn sum(&self, c: &Content, stat: crate::data::Stat) -> i64 {
        self.0.iter().flatten().filter_map(|id| c.items.get(usize::from(id.0))).flat_map(|it| &it.mods).filter(|m| m.stat == stat).map(|m| i64::from(m.value)).sum()
    }
}
/// floor(sqrt(n)), n >= 0: same value as `i64::isqrt`, faster. The f64 root lands within one of the answer
/// (n rounds to 53 bits); the integer steps make it exact.
pub fn isqrt(n: i64) -> i64 {
    let n128 = i128::from(n);
    let mut r = (n as f64).sqrt() as i64;
    while i128::from(r) * i128::from(r) > n128 {
        r -= 1;
    }
    while i128::from(r + 1) * i128::from(r + 1) <= n128 {
        r += 1;
    }
    r
}
fn scale(v: i32, permille: i64) -> i32 {
    (i64::from(v) * permille / 1000) as i32
}
/// Vector (dx,dy) resized to length `len` (0 if dx=dy=0).
fn toward(dx: i32, dy: i32, len: i32) -> (i32, i32) {
    let d = dist(dx, dy);
    if d == 0 {
        return (0, 0);
    }
    // one division: k = len/d in 16.16 fixed point
    let k = (i64::from(len) << 16) / i64::from(d);
    (((i64::from(dx) * k) >> 16) as i32, ((i64::from(dy) * k) >> 16) as i32)
}

pub struct Player {
    pub x: i32,
    pub y: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub flux: i32,
    pub xp: u32,
    pub level: u16,
    pub face: (i32, i32),
    pub iframes: u16,
    pub dodge_cd: u16,
    /// ticks since the player last took damage (confidence grows with it)
    pub unhurt: u32,
    /// coins picked up (goblin trails and catch bursts)
    pub gold: u32,
    /// how fed the hero is, parts per million of a full belly (FOOD_FULL); hunger slows it down
    pub food: i32,
    /// ore carried in the bags: the blacksmith forges with it
    pub ore: u32,
    /// burning / frozen (slowed) by foe hues; Raging never lands on the hero
    pub status: Status,
    dash_left: u16,
    dash_dir: (i32, i32),
}

/// Where a scheduled boss waits (world mm). Pointers aim here until the boss spawns.
#[derive(Clone, Copy, Debug)]
pub struct BossHome {
    pub fam: FamId,
    pub x: i32,
    pub y: i32,
    pub spawned: bool,
}

/// Enemies, SoA. Swap-remove keeps order deterministic.
#[derive(Default)]
pub struct Enemies {
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub hp: Vec<i32>,
    pub max: Vec<i32>,
    pub fam: Vec<FamId>,
    pub tier: Vec<Tier>,
    pub side: Vec<Side>,
    pub timer: Vec<u16>,
    pub glitch: Vec<u8>,
    /// loot value a thief ate: raises its max HP and contact damage, drawn as a gold aura
    pub loot: Vec<u16>,
    /// panicked by a confident hero: flees and steals like a thief
    pub panic: Vec<bool>,
    pub seen: Vec<u32>,
    pub id: Vec<u32>,
    pub rad: Vec<i32>,
    vx: Vec<i32>,
    vy: Vec<i32>,
    pub purger: Vec<bool>,
    pub swift: Vec<bool>,
    /// timed effect on this foe (frozen, raging, burning); view draws it
    pub status: Vec<Status>,
    hit_cd: Vec<u8>,
    orb_cd: Vec<u8>,
    cd: Vec<u16>,
    nx: Vec<i32>,
    ny: Vec<i32>,
}

macro_rules! each_col {
    ($e:expr, $f:ident $(, $a:expr)*) => {{
        let e = $e;
        e.x.$f($($a),*); e.y.$f($($a),*); e.hp.$f($($a),*); e.max.$f($($a),*); e.fam.$f($($a),*);
        e.tier.$f($($a),*); e.side.$f($($a),*); e.timer.$f($($a),*); e.glitch.$f($($a),*); e.loot.$f($($a),*); e.panic.$f($($a),*); e.seen.$f($($a),*);
        e.rad.$f($($a),*); e.vx.$f($($a),*); e.vy.$f($($a),*); e.id.$f($($a),*); e.purger.$f($($a),*); e.swift.$f($($a),*); e.status.$f($($a),*); e.hit_cd.$f($($a),*); e.orb_cd.$f($($a),*);
        e.cd.$f($($a),*);
    }};
}

impl Enemies {
    fn with_capacity(n: usize) -> Enemies {
        let mut e = Enemies::default();
        each_col!(&mut e, reserve_exact, n);
        e.nx.reserve_exact(n);
        e.ny.reserve_exact(n);
        e
    }
    pub fn len(&self) -> usize {
        self.x.len()
    }
    fn remove(&mut self, i: usize) {
        each_col!(self, swap_remove, i);
    }
}

#[derive(Default)]
struct Bolts {
    x: Vec<i32>,
    y: Vec<i32>,
    vx: Vec<i32>,
    vy: Vec<i32>,
    ttl: Vec<u16>,
    pierce: Vec<u8>,
    dmg: Vec<i32>,
    last: Vec<u32>,
    skill: Vec<SkillId>,
    gl: Vec<u8>,
}

#[derive(Default)]
pub struct Shots {
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub vx: Vec<i32>,
    pub vy: Vec<i32>,
    ttl: Vec<u16>,
    age: Vec<u16>,
    dmg: Vec<i32>,
    pub fam: Vec<FamId>,
}

#[derive(Default)]
pub struct Drops {
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub kind: Vec<Drop>,
    pub(crate) pulled: Vec<bool>,
}

#[derive(Clone, Copy)]
struct Mine {
    x: i32,
    y: i32,
    ttl: u16,
    dmg: i32,
    r: i32,
    skill: SkillId,
}

#[derive(Clone, Copy, Debug)]
pub struct Owned {
    pub id: SkillId,
    pub rank: u8,
    /// sum of rarity gains, permille (first pick = 1000 at common)
    pub power: u32,
    cd: u16,
    /// special (number key) reload ticks left; 0 = ready
    pub sp: u16,
    phase: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card {
    pub skill: SkillId,
    pub rarity: u8,
    pub new_rank: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub cards: [Option<Card>; 3],
    pub chest: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PickRec {
    pub tick: u32,
    pub skill: Option<SkillId>,
    pub rarity: u8,
}

struct BossState {
    id: u32,
    fam: FamId,
    phase: usize,
    timers: [u16; 4],
    hard_t: u16,
    age: u32,
    last: bool,
}

/// Run counters for the harness and telemetry.
pub struct Stats {
    pub kills: u32,
    pub dealt: [i64; MAX_SKILLS],
    /// damage output before the overkill clamp (degenerate detector reads this)
    pub output: [i64; MAX_SKILLS],
    pub dealt_hijack: i64,
    pub taken: i64,
    win_spawned: i64,
    pub rho: Vec<(u32, u32)>,
    pub picks: Vec<PickRec>,
    pub first_pick: Option<u32>,
    pub boss_kill_ticks: Vec<(FamId, u32)>,
    pub hijacked_peak: u16,
}

/// Negative-control mutants (design sec 14.2). Harness-only; data cannot express them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutant {
    ZeroTelegraph,
    Plus500(SkillId),
    DoubleTrashHp,
}

const BOLT_R: i32 = 200;
const GRID_N: usize = 128;
const GRID_CELL: i32 = 1000;
/// Largest mob radius the grid queries add to a search radius (enemies.jsonl max is 0.5 m).
const MOB_R_MAX: i32 = 600;
const TRIG_N: u32 = crate::trig::TURN;
/// Treasures scattered per run. shortcut: fixed count, upgrade to data when treasure density gets tuning.
const TREASURES: u32 = 10;
/// Thief growth: loot per XP drop / chest, damage +permille per loot point and cap, bark lifetime (ticks).
pub const LOOT_XP: u16 = 1;
pub const LOOT_CHEST: u16 = 8;
const LOOT_DMG_PER: i64 = 100;
const LOOT_DMG_CAP: i64 = 3000;
pub const BARK_TICKS: u32 = 90;
/// Morale: seconds unhurt to full confidence, confidence (permille) where trash starts to panic,
/// 1-in-N trash in view panics per second, flee radius of a panicked foe (mm).
const CONF_FULL_S: u32 = 20;
pub const CONF_PANIC: u32 = 600;
const PANIC_ODDS: u32 = 20;
const PANIC_FLEE: i32 = 6_000;
/// Picker time limits (ms): start, per-level cut, cap with a boss near or live, floor. Boss "near" = within this many s.
const PICK_MAX_MS: u64 = 15_000;
const PICK_STEP_MS: u64 = 500;
const PICK_BOSS_MS: u64 = 7_000;
const PICK_MIN_MS: u64 = 5_000;
const BOSS_NEAR_S: u32 = 60;
const JUMP_R: i32 = 4000;
const CHAIN_FALLOFF: i64 = 850;
const FX_TICKS: u8 = 6;
const MINE_TRIGGER: i32 = 1000;
/// Reference contact distance^2 (2 x 0.35 m) for the soft push: constant divisor, no per-pair div.
const SEP_RR2: i64 = 700 * 700;
/// Separation runs only within view radius + this margin (mm).
const SEP_MARGIN: i32 = 4000;
/// Off-screen hostiles recompute their heading every this many ticks.
const LOD_EVERY: u32 = 4;

struct Grid {
    ox: i32,
    oy: i32,
    start: Vec<u32>,
    items: Vec<u32>,
    cell: Vec<u32>,
    // position + radius copied in cell order, one record per item: neighbour scans read one cache line each
    at: Vec<GridPos>,
}

#[derive(Clone, Copy, Default)]
struct GridPos {
    x: i32,
    y: i32,
    r: i32,
}

impl Grid {
    fn new(cap: usize) -> Grid {
        Grid { ox: 0, oy: 0, start: vec![0; GRID_N * GRID_N + 1], items: vec![0; cap], cell: vec![0; cap], at: vec![GridPos::default(); cap] }
    }
    fn cxy(&self, x: i32, y: i32) -> (usize, usize) {
        let c = |v: i32| ((v.div_euclid(GRID_CELL)).clamp(0, GRID_N as i32 - 1)) as usize;
        (c(x - self.ox), c(y - self.oy))
    }
    fn build(&mut self, cx: i32, cy: i32, xs: &[i32], ys: &[i32], rs: &[i32]) {
        self.ox = cx - GRID_CELL * GRID_N as i32 / 2;
        self.oy = cy - GRID_CELL * GRID_N as i32 / 2;
        self.start.fill(0);
        for (i, (&x, &y)) in xs.iter().zip(ys).enumerate() {
            let (a, b) = self.cxy(x, y);
            let c = b * GRID_N + a;
            self.cell[i] = c as u32;
            self.start[c] += 1;
        }
        // inclusive prefix: start[c] = end of cell c
        for c in 1..GRID_N * GRID_N {
            self.start[c] += self.start[c - 1];
        }
        self.start[GRID_N * GRID_N] = xs.len() as u32;
        // reverse scatter with falling cursors keeps ascending i inside a cell and leaves start[c] = begin of c
        for i in (0..xs.len()).rev() {
            let c = self.cell[i] as usize;
            self.start[c] -= 1;
            let k = self.start[c] as usize;
            self.items[k] = i as u32;
            self.at[k] = GridPos { x: xs[i], y: ys[i], r: rs[i] };
        }
    }
    /// Enemy indices in cells touching the square of half-size r around (x, y).
    fn query(&self, x: i32, y: i32, r: i32, out: &mut Vec<u32>) {
        out.clear();
        let (a0, b0) = self.cxy(x - r, y - r);
        let (a1, b1) = self.cxy(x + r, y + r);
        for b in b0..=b1 {
            let s = self.start[b * GRID_N + a0] as usize;
            let e = self.start[b * GRID_N + a1 + 1] as usize;
            out.extend_from_slice(&self.items[s..e]);
        }
    }
    /// Contiguous ranges (into items/gx/gy/gr), one per grid row touching the square.
    fn ranges(&self, x: i32, y: i32, r: i32) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
        let (a0, b0) = self.cxy(x - r, y - r);
        let (a1, b1) = self.cxy(x + r, y + r);
        (b0..=b1).map(move |b| self.start[b * GRID_N + a0] as usize..self.start[b * GRID_N + a1 + 1] as usize)
    }
    fn count(&self, a: usize, b: usize) -> u32 {
        let c = b * GRID_N + a;
        self.start[c + 1] - self.start[c]
    }
}

pub struct Sim<'c> {
    pub c: &'c Content,
    pub tick: u32,
    pub p: Player,
    pub e: Enemies,
    bolts: Bolts,
    pub shots: Shots,
    pub drops: Drops,
    pub boss_home: Vec<BossHome>,
    /// uncollected treasure (world mm); touching one = one chest pick
    pub treasures: Vec<(i32, i32)>,
    pub world: world::World,
    /// items the hero wears (bought at shops)
    pub gear: Gear,
    /// items carried, not worn (at most BAG_SLOTS): what wearing replaced
    pub bag: Vec<ItemId>,
    mines: Vec<Mine>,
    pub fx: Vec<Fx>,
    pub blades: Vec<(i32, i32)>,
    /// private: `walls` caches it, so every write goes through `with_arena` (keeps cache == arena)
    arena: crate::floorgen::Arena,
    /// solid-tile cache around the hero (follows it each tick); sim wall queries read it
    walls: solid::Walls,
    pub tele: Vec<Tele>,
    pub owned: Vec<Owned>,
    pub offer: Option<Offer>,
    pending: u16,
    pending_chest: u16,
    since_rare: u16,
    boss: Option<BossState>,
    pub log: Vec<LogRow>,
    pub stats: Stats,
    pub outcome: Option<Outcome>,
    /// last number-key press: what it did and the tick it was pressed
    pub special_try: Option<(SpecialTry, u32)>,
    pub narration: Option<(usize, u32)>,
    /// short-lived world events the view shows as floating text (sim stays text-free)
    pub barks: Vec<Bark>,
    pub hijacked: u16,
    mutant: Option<Mutant>,
    grid: Grid,
    near: Vec<u32>,
    hij: Vec<u32>,
    auras: Vec<(i32, i32, i32, u8)>,
    trig: Vec<(i32, i32)>,
    rng_spawn: Xo,
    rng_combat: Xo,
    rng_picks: Xo,
    next_id: u32,
    last_hit: Option<(Cause, bool)>,
}

impl<'c> Sim<'c> {
    pub fn new(c: &'c Content, seed: u64, mutant: Option<Mutant>) -> Sim<'c> {
        let cap = c.spawn.live_cap + 64;
        let p = &c.player;
        let arena = crate::floorgen::arena(seed, 0, c);
        let walls = solid::Walls::new(&arena, 0, 0);
        let mut s = Sim {
            c,
            tick: 0,
            p: Player {
                x: 0,
                y: 0,
                hp: p.hp,
                max_hp: p.hp,
                flux: p.flux,
                xp: 0,
                level: 1,
                face: (1, 0),
                iframes: 0,
                dodge_cd: 0,
                unhurt: 0,
                gold: 0,
                food: FOOD_FULL,
                ore: 0,
                status: Status::None,
                dash_left: 0,
                dash_dir: (0, 0),
            },
            e: Enemies::with_capacity(cap),
            bolts: Bolts::default(),
            shots: Shots::default(),
            drops: Drops::default(),
            boss_home: Vec::new(),
            treasures: Vec::with_capacity(TREASURES as usize),
            world: world::World::new(seed, &arena, NonZeroU32::MIN.saturating_add(c.hz().get() - 1)),
            gear: Gear::default(),
            bag: Vec::with_capacity(BAG_SLOTS),
            mines: Vec::with_capacity(64),
            fx: Vec::with_capacity(256),
            blades: Vec::with_capacity(64),
            arena,
            walls,
            tele: Vec::with_capacity(64),
            owned: Vec::with_capacity(MAX_SKILLS),
            offer: None,
            pending: 0,
            pending_chest: 0,
            since_rare: 0,
            boss: None,
            log: Vec::with_capacity(1 << 16),
            stats: Stats {
                kills: 0,
                dealt: [0; MAX_SKILLS],
                output: [0; MAX_SKILLS],
                dealt_hijack: 0,
                taken: 0,
                win_spawned: 0,
                rho: Vec::with_capacity(256),
                picks: Vec::with_capacity(256),
                first_pick: None,
                boss_kill_ticks: Vec::with_capacity(4),
                hijacked_peak: 0,
            },
            outcome: None,
            special_try: None,
            narration: None,
            barks: Vec::with_capacity(64),
            hijacked: 0,
            mutant,
            grid: Grid::new(cap),
            near: Vec::with_capacity(cap),
            hij: Vec::with_capacity(256),
            auras: Vec::with_capacity(256),
            trig: (0..TRIG_N).map(|a| (crate::trig::cos(a), crate::trig::sin(a))).collect(),
            rng_spawn: Xo::stream(seed, Stream::Spawn),
            rng_combat: Xo::stream(seed, Stream::Combat),
            rng_picks: Xo::stream(seed, Stream::Picks),
            next_id: 0,
            last_hit: None,
        };
        let cap_side = 4096;
        macro_rules! res { ($($v:expr),*) => { $( $v.reserve_exact(cap_side); )* } }
        res!(s.bolts.x, s.bolts.y, s.bolts.vx, s.bolts.vy, s.bolts.ttl, s.bolts.pierce, s.bolts.dmg, s.bolts.last, s.bolts.skill, s.bolts.gl);
        res!(s.shots.x, s.shots.y, s.shots.vx, s.shots.vy, s.shots.ttl, s.shots.age, s.shots.dmg, s.shots.fam);
        res!(s.drops.x, s.drops.y, s.drops.kind, s.drops.pulled);
        for &id in &c.player.skills {
            s.owned.push(Owned { id, rank: 1, power: 1000, cd: 0, sp: 0, phase: 0 });
        }
        s.world.stock_shops(c);
        // boss homes: seeded direction, fixed distance from the start
        // shortcut: distance = 4 x ring_max for every boss, upgrade to per-boss data when bosses.jsonl gets a home field
        let mut rng_map = Xo::stream(seed, Stream::Map);
        let far = c.spawn.ring_max.saturating_mul(4);
        for w in &c.waves {
            if let WaveEv::Boss { fam, .. } = w {
                let ang = rng_map.below(NonZeroU32::MIN.saturating_add(TRIG_N - 1)) % TRIG_N;
                let (cx, cy) = s.trig[ang as usize];
                let (x, y) = (scale(cx, i64::from(far)), scale(cy, i64::from(far)));
                let (x, y) = s.free_near(x, y, c.fam(*fam).radius);
                s.boss_home.push(BossHome { fam: *fam, x, y, spawned: false });
            }
        }
        // treasures: scattered on seeded rings between 1x and 3x ring_max; pointers aim at them until taken
        for _ in 0..TREASURES {
            let ang = rng_map.below(NonZeroU32::MIN.saturating_add(TRIG_N - 1)) % TRIG_N;
            let span = (c.spawn.ring_max * 2).max(1) as u32;
            let r = c.spawn.ring_max + rng_map.below(NonZeroU32::MIN.saturating_add(span - 1)) as i32;
            let (cx, cy) = s.trig[ang as usize];
            let at = s.free_near(scale(cx, i64::from(r)), scale(cy, i64::from(r)), TREASURE_CLEAR);
            s.treasures.push(at);
        }
        s
    }

    pub fn arena(&self) -> &crate::floorgen::Arena {
        &self.arena
    }

    /// The one arena writer: edits it, then rebuilds the wall cache from it.
    #[cfg(test)]
    pub fn with_arena(&mut self, f: impl FnOnce(&mut crate::floorgen::Arena)) {
        f(&mut self.arena);
        self.walls = solid::Walls::new(&self.arena, self.p.x, self.p.y);
    }

    /// The nearest wall-free spot to (x, y) for a circle of radius r: the point itself, else the first free one on
    /// square rings of growing tile steps. Actors that start inside a wall cannot move (slide blocks both axes).
    /// shortcut: gives up after NUDGE_RINGS rings and keeps the point, upgrade if rooms ever get that thick.
    fn free_near(&self, x: i32, y: i32, r: i32) -> (i32, i32) {
        let t = crate::floorgen::TILE_MM;
        if !solid::blocked(&self.walls.near(&self.arena), x, y, r) {
            return (x, y);
        }
        for k in 1..=NUDGE_RINGS {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, 1), (1, -1), (-1, -1)] {
                let (nx, ny) = (x + dx * k * t, y + dy * k * t);
                if !solid::blocked(&self.walls.near(&self.arena), nx, ny, r) {
                    return (nx, ny);
                }
            }
        }
        (x, y)
    }

    pub fn secs(&self) -> u32 {
        self.tick / self.c.hz().get()
    }

    /// Upgrade picker time limit: 15 s at level 1, 0.5 s less per level, capped near a boss, never under 5 s.
    /// Night: the last third of every DAY_S cycle. Darker, and the horde runs faster.
    pub fn night(&self) -> bool {
        let s = self.tick / self.c.hz().get() % DAY_S;
        s >= DAY_S * 2 / 3
    }

    /// Power-up offers waiting for the player: the open one and those queued behind it.
    pub fn offers_waiting(&self) -> u16 {
        u16::from(self.offer.is_some()) + self.pending
    }

    pub fn pick_timeout_ms(&self) -> u64 {
        let by_level = PICK_MAX_MS.saturating_sub(u64::from(self.p.level.saturating_sub(1)) * PICK_STEP_MS);
        let hz = self.c.hz().get();
        let boss_soon = self.c.waves.iter().any(|w| matches!(w, WaveEv::Boss { at, .. } if *at > self.tick && *at - self.tick <= BOSS_NEAR_S * hz));
        let t = if boss_soon || self.boss_ix().is_some() { by_level.min(PICK_BOSS_MS) } else { by_level };
        t.max(PICK_MIN_MS)
    }

    fn sec_ix(&self) -> usize {
        (self.secs() as usize).min(self.c.spawn.hp_mult.len() - 1)
    }

    pub fn live(&self) -> usize {
        self.e.len()
    }


    pub fn boss_ix(&self) -> Option<usize> {
        let id = self.boss.as_ref()?.id;
        self.e.id.iter().position(|&x| x == id)
    }

    pub fn rank_of(&self, s: SkillId) -> u8 {
        self.owned.iter().find(|o| o.id == s).map_or(0, |o| o.rank)
    }

    // ---------- commands ----------

    pub fn validate(&self, cmd: &Cmd) -> Result<(), Reject> {
        let cap = self.c.spawn.live_cap;
        match *cmd {
            Cmd::Spawn { fam, n, tier, .. } => {
                if tier == Tier::Boss {
                    return Err(Reject::BossTier);
                }
                if !matches!(self.c.fam(fam).kind, FamKind::Mob(_)) {
                    return Err(Reject::NotMob);
                }
                if self.live() + usize::from(n.get()) > cap {
                    return Err(Reject::LiveCap { live: self.live(), n: n.get(), cap });
                }
                Ok(())
            }
            Cmd::Boss { fam } => {
                if self.boss.is_some() {
                    return Err(Reject::BossActive);
                }
                if self.c.boss_of(fam).is_none() {
                    return Err(Reject::NotBoss);
                }
                if self.live() + 1 > cap + 64 {
                    return Err(Reject::LiveCap { live: self.live(), n: 1, cap });
                }
                Ok(())
            }
            Cmd::Flee => Ok(()),
            Cmd::Narrate { ev } => match self.c.waves.get(ev) {
                Some(WaveEv::Narrate { .. }) => Ok(()),
                _ => Err(Reject::NoText),
            },
        }
    }

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Spawn { fam, n, tier, form } => {
                let base = self.rng_spawn.below(NonZeroU32::MIN.saturating_add(TRIG_N - 1));
                for k in 0..n.get() {
                    let (x, y) = self.place(form, base, k, n.get());
                    self.spawn_mob(fam, tier, x, y);
                }
            }
            Cmd::Boss { fam } => self.spawn_boss(fam),
            Cmd::Flee => {
                let mut i = 0;
                while i < self.e.len() {
                    if self.e.tier[i] == Tier::Trash && self.e.side[i] == Side::Hostile {
                        self.e.remove(i);
                    } else {
                        i += 1;
                    }
                }
            }
            Cmd::Narrate { ev } => self.narration = Some((ev, self.tick)),
        }
    }

    pub fn command(&mut self, src: Src, cmd: Cmd) -> Result<(), Reject> {
        let ok = self.validate(&cmd);
        if ok.is_ok() {
            self.apply(cmd);
        }
        self.log.push(LogRow { tick: self.tick, src, cmd, ok });
        ok
    }

    /// Ring position outside view, never inside the player's motion cone (design sec 8.4).
    fn place(&mut self, form: Formation, base: u32, k: u16, n: u16) -> (i32, i32) {
        let sp = &self.c.spawn;
        let one = NonZeroU32::MIN;
        let span = (sp.ring_max - sp.ring_min).max(0) as u32;
        let (ang, r, off) = match form {
            Formation::Scatter => (self.rng_spawn.below(one.saturating_add(TRIG_N - 1)), sp.ring_min + self.rng_spawn.below(one.saturating_add(span)) as i32, 0),
            Formation::Ring => (base + u32::from(k) * TRIG_N / u32::from(n), sp.ring_min, 0),
            Formation::Spiral => (base + u32::from(k) * TRIG_N * 2 / u32::from(n), sp.ring_min + (span as i32) * i32::from(k) / i32::from(n), 0),
            Formation::Wall => (base, sp.ring_min, (i32::from(k) - i32::from(n) / 2) * 1000),
        };
        let mut a = ang % TRIG_N;
        let (fx, fy) = toward(self.p.face.0, self.p.face.1, 1000);
        let (cx, cy) = self.trig[a as usize];
        if form != Formation::Ring && i64::from(cx) * i64::from(fx) + i64::from(cy) * i64::from(fy) > i64::from(sp.cone_cos) * 1000 {
            a = (a + TRIG_N / 2) % TRIG_N;
        }
        // a point inside a wall is turned around the hero in equal steps until it is free
        for j in 0..SPAWN_TRIES {
            let (cx, cy) = self.trig[((a + j * (TRIG_N / SPAWN_TRIES)) % TRIG_N) as usize];
            let (x, y) = (self.p.x + scale(cx, i64::from(r)) - scale(cy, i64::from(off)), self.p.y + scale(cy, i64::from(r)) + scale(cx, i64::from(off)));
            if !solid::blocked(&self.walls.near(&self.arena), x, y, MOB_R_MAX) {
                return (x, y);
            }
        }
        // every direction is walled: the hero's own spot is free by construction
        (self.p.x, self.p.y)
    }

    fn spawn_mob(&mut self, fam: FamId, tier: Tier, x: i32, y: i32) {
        let c = self.c;
        let FamKind::Mob(m) = &c.fam(fam).kind else { return };
        let sp = &c.spawn;
        let mut hp = i64::from(sp.trash_hp) * i64::from(sp.hp_mult[self.sec_ix()]) / 1000 * i64::from(m.hp) / 1000;
        if self.mutant == Some(Mutant::DoubleTrashHp) {
            hp *= 2;
        }
        let (mult, purger, swift) = match tier {
            Tier::Trash | Tier::Boss => (1000, false, false),
            Tier::Elite => {
                let p = self.rng_spawn.chance(500);
                (sp.elite_hp, p, !p)
            }
            Tier::Champion => (sp.champion_hp, true, true),
        };
        let hp = (hp * i64::from(mult) / 1000).clamp(1, i64::from(i32::MAX)) as i32;
        let hp = self.world.foe_hp(self.tick).apply(hp).max(1);
        self.stats.win_spawned += i64::from(hp);
        self.push_enemy(fam, tier, (x, y), hp, (purger, swift));
    }


    pub(crate) fn push_enemy(&mut self, fam: FamId, tier: Tier, (x, y): (i32, i32), hp: i32, (purger, swift): (bool, bool)) {
        let base = self.c.fam(fam).radius;
        let rad = BossClass::of_kind(tier, &self.c.fam(fam).kind).map_or(base, |k| k.radius(base));
        let (x, y) = self.free_near(x, y, rad);
        let e = &mut self.e;
        e.x.push(x);
        e.y.push(y);
        e.hp.push(hp);
        e.max.push(hp);
        e.fam.push(fam);
        e.tier.push(tier);
        e.side.push(Side::Hostile);
        e.timer.push(0);
        e.glitch.push(0);
        e.loot.push(0);
        e.panic.push(false);
        e.seen.push(u32::MAX);
        e.id.push(self.next_id);
        e.rad.push(rad);
        e.vx.push(0);
        e.vy.push(0);
        e.purger.push(purger);
        e.swift.push(swift);
        e.status.push(Status::None);
        e.hit_cd.push(0);
        e.orb_cd.push(0);
        e.cd.push(self.c.spawn.purge_every);
        self.next_id += 1;
    }

    fn spawn_boss(&mut self, fam: FamId) {
        let FamKind::Boss { hp } = self.c.fam(fam).kind else { return };
        let (x, y) = match self.boss_home.iter_mut().find(|h| h.fam == fam && !h.spawned) {
            Some(h) => {
                h.spawned = true;
                (h.x, h.y)
            }
            None => self.place(Formation::Scatter, 0, 0, 1),
        };
        let last = self.c.waves.iter().any(|w| matches!(w, WaveEv::Boss { fam: f, last: true, .. } if *f == fam));
        self.boss = Some(BossState { id: self.next_id, fam, phase: 0, timers: [0; 4], hard_t: 0, age: 0, last });
        self.stats.win_spawned += i64::from(hp);
        self.push_enemy(fam, Tier::Boss, (x, y), hp, (false, false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::{content, texts};
    use crate::director::Director;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::time::Instant;

    // Counting allocator (tests only): per-thread count, so parallel tests do not interfere.
    struct Counting;
    thread_local! { static ALLOCS: Cell<u64> = const { Cell::new(0) }; }
    // SAFETY: forwards to System unchanged; only adds a thread-local counter.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            ALLOCS.with(|a| a.set(a.get() + 1));
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            ALLOCS.with(|a| a.set(a.get() + 1));
            unsafe { System.realloc(p, l, n) }
        }
    }
    #[global_allocator]
    static A: Counting = Counting;
    fn allocs() -> u64 {
        ALLOCS.with(Cell::get)
    }

    /// Scripted input: circle walk + periodic dodge; picks the first card.
    fn script(t: u32) -> Input {
        let dirs = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
        Input { mv: dirs[(t / 20 % 8) as usize], dodge: t.is_multiple_of(97), hack: false, autofire: true, fire: false, pick: Some(PickChoice::Card(0)), buy: None, drop: None, special: None, bag: None, learn: None, forge: false }
    }

    fn run_hash(c: &Content, seed: u64, ticks: u32) -> u64 {
        let mut sim = Sim::new(c, seed, None);
        let mut d = Director::new(seed);
        for t in 0..ticks {
            d.tick(&mut sim);
            sim.step(&script(t));
        }
        sim.hash()
    }

    #[test]
    fn confident_hero_panics_trash_and_hurt_hero_does_not() {
        let c = content();
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        let panics = |unhurt: u32| {
            let mut s = Sim::new(&c, 7, None);
            for k in 0..60 {
                s.push_enemy(fam, Tier::Trash, (s.p.x + 3_000 + k * 100, s.p.y), 1000, (false, false));
            }
            s.p.unhurt = unhurt;
            let hz = c.hz().get();
            for sec in 1..=PANIC_ODDS {
                s.tick = sec * hz;
                s.morale_step();
            }
            s.e.panic.iter().filter(|&&p| p).count()
        };
        assert!(panics(u32::MAX) > 0, "full confidence: some trash panics");
        assert_eq!(panics(0), 0, "NEG: just hurt, nobody panics");
    }

    #[test]
    fn thief_eats_chest_grows_hp_and_damage_and_barks() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "thief").unwrap();
        let (x, y) = (s.p.x + 20_000, s.p.y);
        s.push_enemy(fam, Tier::Trash, (x, y), 1000, (false, false));
        let j = s.e.len() - 1;
        let (max0, dmg0) = (s.e.max[j], s.contact_dmg(j));
        s.drops.x.push(x);
        s.drops.y.push(y);
        s.drops.kind.push(Drop::Chest);
        s.drops.pulled.push(false);
        s.grid.build(s.p.x, s.p.y, &s.e.x, &s.e.y, &s.e.rad);
        s.thieves_steal();
        assert!(s.drops.x.is_empty(), "chest eaten");
        assert_eq!(s.e.loot[j], LOOT_CHEST);
        assert!(s.e.max[j] > max0, "max HP grew {} -> {}", max0, s.e.max[j]);
        assert!(s.contact_dmg(j) > dmg0, "damage grew {} -> {}", dmg0, s.contact_dmg(j));
        assert!(s.barks.iter().any(|b| matches!(b.kind, BarkKind::Stole { power } if power == LOOT_CHEST)));
    }

    #[test]
    fn loot_thresholds_grant_abilities_with_barks() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "thief").unwrap();
        let (x, y) = (s.p.x + 20_000, s.p.y);
        s.push_enemy(fam, Tier::Trash, (x, y), 1000, (false, false));
        let j = s.e.len() - 1;
        let eat = |s: &mut Sim| {
            s.drops.x.push(x);
            s.drops.y.push(y);
            s.drops.kind.push(Drop::Chest);
            s.drops.pulled.push(false);
            s.grid.build(s.p.x, s.p.y, &s.e.x, &s.e.y, &s.e.rad);
            s.thieves_steal();
        };
        let learned = |s: &Sim| s.barks.iter().filter_map(|b| if let BarkKind::Learned { ability } = b.kind { Some(ability) } else { None }).collect::<Vec<_>>();
        eat(&mut s);
        assert_eq!(learned(&s), [Ability::Fast], "one chest: faster only");
        assert_eq!(s.e.status[j], Status::None, "control: no rage yet");
        eat(&mut s);
        assert_eq!(learned(&s), [Ability::Fast, Ability::Rage, Ability::Shot]);
        assert!(matches!(s.e.status[j], Status::Raging { .. }));
        eat(&mut s);
        assert_eq!(learned(&s).len(), 3, "a threshold is crossed once");
    }

    #[test]
    fn thief_with_shot_ability_fires_and_one_without_does_not() {
        let c = content();
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "thief").unwrap();
        let shots = |loot: u16| {
            let mut s = Sim::new(&c, 7, None);
            s.push_enemy(fam, Tier::Trash, (s.p.x + 6_000, s.p.y), 1_000_000, (false, false));
            s.e.loot[0] = loot;
            s.step(&Input::default());
            s.shots.x.len()
        };
        assert_eq!(shots(0), 0, "control: no loot, no shot");
        assert_eq!(shots(Ability::Shot.loot_needed()), 1);
    }

    #[test]
    fn pick_timeout_15s_at_start_shrinks_with_level_floor_5s() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        assert_eq!(s.pick_timeout_ms(), 15_000);
        s.p.level = 5;
        assert_eq!(s.pick_timeout_ms(), 13_000);
        s.p.level = 200;
        assert_eq!(s.pick_timeout_ms(), 5_000);
    }

    #[test]
    fn treasure_taken_on_touch_becomes_one_chest() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        s.treasures = vec![(s.p.x, s.p.y), (s.p.x + 10_000_000, s.p.y)];
        s.step(&Input::default());
        assert_eq!(s.treasures.len(), 1, "only the touched treasure goes");
        // the chest either waits in pending_chest or already opened as an offer this tick
        assert!(s.pending_chest == 1 || s.offer.is_some_and(|o| o.chest));
    }

    #[test]
    fn status_rules_and_negative_controls() {
        let hz = 20;
        let none = Status::None;
        assert!(matches!(none.inflict(Hue::Emp, hz), Status::Frozen { left: 30 }));
        assert!(matches!(none.inflict(Hue::Therm, hz), Status::Burning { .. }));
        // NEG: other hues do nothing; a frozen/raging/thawed target is not overwritten
        assert_eq!(none.inflict(Hue::Kin, hz), none);
        let thawed = Status::Thawed { left: 5 };
        assert_eq!(thawed.inflict(Hue::Emp, hz), thawed, "no stun lock");
        let rage = Status::Raging { left: 5 };
        assert_eq!(rage.inflict(Hue::Therm, hz), rage);
        assert_eq!(rage.enrage(hz), rage);
        // timers: frozen -> thawed -> none, exactly `left` ticks each
        let mut s = Status::Frozen { left: 3 };
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(s);
            s = s.tick(hz);
        }
        assert_eq!(seen[2], Status::Frozen { left: 1 });
        assert!(matches!(s, Status::Thawed { .. }));
        assert_eq!(Status::Burning { left: 1 }.tick(hz), Status::None);
    }

    /// Foe 8 m east of the hero, standing in the open; returns its x after `n` ticks with the given status.
    fn foe_x_after(c: &Content, st: Status, n: u32) -> i32 {
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        let mut s = Sim::new(c, 7, None);
        s.push_enemy(fam, Tier::Trash, (s.p.x + 8_000, s.p.y), 1_000_000, (false, false));
        s.e.status[0] = st;
        for _ in 0..n {
            s.step(&Input::default());
        }
        s.e.x[0]
    }

    #[test]
    fn frozen_foes_stand_and_raging_foes_run() {
        let c = content();
        let plain = foe_x_after(&c, Status::None, 10);
        assert!(plain < 8_000, "control: a plain foe closes in ({plain})");
        assert_eq!(foe_x_after(&c, Status::Frozen { left: 100 }, 10), 8_000, "frozen foe stays");
        assert!(foe_x_after(&c, Status::Raging { left: 100 }, 10) < plain, "raging foe is faster");
    }

    #[test]
    fn burning_bites_once_a_second_and_never_kills_the_hero() {
        let c = content();
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        let hz = c.hz().get();
        let mut s = Sim::new(&c, 7, None);
        s.push_enemy(fam, Tier::Trash, (s.p.x + 30_000, s.p.y), 10_000, (false, false));
        s.push_enemy(fam, Tier::Trash, (s.p.x + 30_000, s.p.y + 2_000), 10_000, (false, false));
        s.e.status[0] = Status::Burning { left: 100 };
        s.p.status = Status::Burning { left: 100 };
        s.p.hp = 2;
        for _ in 0..hz {
            s.step(&Input::default());
        }
        assert!(s.e.hp[0] < s.e.hp[1], "burner lost hp ({} vs {})", s.e.hp[0], s.e.hp[1]);
        assert_eq!(s.p.hp, 1, "burn floors at 1");
    }

    #[test]
    fn blast_goes_through_armour() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let hp = s.p.hp;
        s.hurt_player(1000, Cause::Blast, true);
        assert!(s.p.hp < hp && s.last_hit == Some((Cause::Blast, true)), "blast hurts and is the last hit");
        let hp = s.p.hp;
        s.p.iframes = 10;
        s.hurt_player(1000, Cause::Blast, true);
        assert_eq!(s.p.hp, hp, "control: iframes still protect");
    }

    #[test]
    fn boss_footprint_grows_with_hp_class() {
        assert_eq!([0, MINI_MAX_HP - 1, MINI_MAX_HP, SUPER_MAX_HP - 1, SUPER_MAX_HP].map(BossClass::of_hp), [
            BossClass::Mini,
            BossClass::Mini,
            BossClass::Super,
            BossClass::Super,
            BossClass::Uber
        ]);
        assert_eq!(BossClass::Mini.radius(1000), 1000, "control: mini keeps the family radius");
        assert!(BossClass::Mini.radius(1000) < BossClass::Super.radius(1000) && BossClass::Super.radius(1000) < BossClass::Uber.radius(1000));
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let boss = c.fam_ids().find(|&f| c.fam(f).key == "sweeper").unwrap();
        let hp = match c.fam(boss).kind {
            FamKind::Boss { hp } => hp,
            FamKind::Mob(_) => 0,
        };
        assert!(hp >= MINI_MAX_HP, "sweeper is a super boss");
        s.spawn_boss(boss);
        let b = s.e.len() - 1;
        assert_eq!(BossClass::of_kind(s.e.tier[b], &c.fam(s.e.fam[b]).kind), Some(BossClass::of_hp(hp)));
        assert_eq!(s.e.rad[b], BossClass::of_hp(hp).radius(c.fam(boss).radius));
        assert!(s.e.rad[b] > c.fam(boss).radius, "sweeper is bigger than its base radius");
        let thief = c.fam_ids().find(|&f| c.fam(f).key == "thief").unwrap();
        s.push_enemy(thief, Tier::Trash, (s.p.x + 9_000, s.p.y), 10, (false, false));
        let t = s.e.len() - 1;
        assert_eq!((BossClass::of_kind(s.e.tier[t], &c.fam(s.e.fam[t]).kind), s.e.rad[t]), (None, c.fam(thief).radius));
    }

    #[test]
    fn autofire_and_manual_fire_aim_at_the_foe_not_the_facing() {
        let c = content();
        let first_bolt_vx = |inp: Input| {
            let mut s = Sim::new(&c, 7, None);
            let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
            s.p.face = (1, 0);
            s.push_enemy(fam, Tier::Trash, (s.p.x - 3_000, s.p.y), 1_000_000, (false, false));
            s.step(&inp);
            s.bolts.vx.first().copied()
        };
        assert!(first_bolt_vx(Input { autofire: true, ..Input::default() }).is_some_and(|v| v < 0), "autofire aims west at the foe");
        assert!(first_bolt_vx(Input { fire: true, ..Input::default() }).is_some_and(|v| v < 0), "held fire aims west too");
        assert_eq!(first_bolt_vx(Input::default()), None, "auto-fire off and no fire key: the hero holds fire");
    }

    #[test]
    fn emp_hits_freeze_and_low_hp_enrages() {
        let c = content();
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        let mut s = Sim::new(&c, 7, None);
        s.push_enemy(fam, Tier::Trash, (s.p.x + 30_000, s.p.y), 1_000, (false, false));
        s.hurt(0, 10, Hue::Emp, None);
        assert!(matches!(s.e.status[0], Status::Frozen { .. }));
        s.e.status[0] = Status::None;
        s.hurt(0, 10, Hue::Code, None);
        assert_eq!(s.e.status[0], Status::None, "control: a high-hp code hit changes nothing");
        s.hurt(0, 800, Hue::Code, None);
        assert!(matches!(s.e.status[0], Status::Raging { .. }), "low hp rages");
    }

    #[test]
    fn boss_homes_one_per_boss_wave_far_and_seeded() {
        let c = content();
        let s = Sim::new(&c, 7, None);
        let again = Sim::new(&c, 7, None);
        let bosses = c.waves.iter().filter(|w| matches!(w, WaveEv::Boss { .. })).count();
        assert_eq!(s.boss_home.len(), bosses);
        let far = f64::from(c.spawn.ring_max * 4);
        for (h, g) in s.boss_home.iter().zip(&again.boss_home) {
            assert_eq!((h.x, h.y), (g.x, g.y));
            // free_near moves a home out of rock by at most NUDGE_RINGS tiles on a diagonal
            let reach = f64::from(NUDGE_RINGS * crate::floorgen::TILE_MM) * std::f64::consts::SQRT_2;
            assert!((f64::from(h.x).hypot(f64::from(h.y)) - far).abs() <= reach, "home {},{} far {far}", h.x, h.y);
        }
    }

    #[test]
    fn same_seed_same_hash_other_seed_differs() {
        let c = content();
        let a = run_hash(&c, 42, 900);
        assert_eq!(a, run_hash(&c, 42, 900), "O-hash: replay must match");
        assert_ne!(a, run_hash(&c, 43, 900), "negative control: other seed must differ");
    }

    #[test]
    fn nothing_starts_inside_a_wall() {
        let c = content();
        let fam = c.fam_ids().next().unwrap();
        for seed in 1..=4 {
            let mut s = Sim::new(&c, seed, None);
            let r = s.c.fam(fam).radius;
            assert!(!solid::blocked(&s.arena, s.p.x, s.p.y, c.player.radius), "player start");
            for &(x, y) in &s.treasures {
                assert!(!solid::blocked(&s.arena, x, y, TREASURE_CLEAR), "treasure {x},{y} seed {seed}");
            }
            for h in &s.boss_home {
                assert!(!solid::blocked(&s.arena, h.x, h.y, c.fam(h.fam).radius), "boss home seed {seed}");
            }
            // NEG: a point put straight into a wall IS blocked, so the push below proves the nudge
            let wall = (0..).find(|&x| s.arena.tile(x, 0).solid()).unwrap_or(0) * crate::floorgen::TILE_MM + 1000;
            assert!(solid::blocked(&s.arena, wall, 0, r), "control point is in a wall");
            s.push_enemy(fam, Tier::Trash, (wall, 0), 10, (false, false));
            let last = s.e.len() - 1;
            assert!(!solid::blocked(&s.arena, s.e.x[last], s.e.y[last], r), "pushed into a wall, seed {seed}");
        }
    }

    /// A foe behind the room wall walks round through the doorway (flow field) instead of pressing the wall.
    #[test]
    fn walled_off_foe_finds_the_doorway() {
        use crate::floorgen::{CENTRE, PITCH, TILE_MM};
        let c = content();
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        let reach = |seed: u64| -> (i64, bool) {
            let mut s = Sim::new(&c, seed, None);
            // hero in the room centre, foe in the next room east, level with the hero: the wall is in between
            let (fx, fy) = ((PITCH - CENTRE + 6) * TILE_MM + TILE_MM / 2, TILE_MM / 2);
            s.push_enemy(fam, Tier::Trash, (fx, fy), 1_000_000, (false, false));
            let start = dist2(s.e.x[0] - s.p.x, s.e.y[0] - s.p.y);
            let line_blocked = (1..20).any(|k| solid::blocked(&s.walls.near(&s.arena), s.p.x + (s.e.x[0] - s.p.x) * k / 20, s.p.y, 300));
            for _ in 0..1800 {
                s.p.hp = s.p.max_hp;
                s.step(&Input::default());
            }
            assert!(dist2(s.e.x[0] - s.p.x, s.e.y[0] - s.p.y) < start, "moved at all");
            (isqrt(dist2(s.e.x[0] - s.p.x, s.e.y[0] - s.p.y)), line_blocked)
        };
        let mut walled = 0;
        for seed in 0..6 {
            let (d, blocked) = reach(seed);
            if blocked {
                walled += 1;
                assert!(d < 4_000, "seed {seed}: walled-off foe got to {d} mm, not to the hero");
            }
        }
        assert!(walled >= 2, "control: the wall stood between them in some seeds ({walled})");
    }

    /// No foe pops in or out: between two ticks a foe id moves at most a few metres when either end is in sight.
    #[test]
    fn foes_never_jump_in_sight() {
        let c = content();
        let sight2 = i64::from(c.view.fog_end).pow(2);
        let mut moves = 0u64;
        for seed in [1u64, 5, 9] {
            let mut s = Sim::new(&c, seed, None);
            let mut d = Director::new(seed);
            let mut last: std::collections::HashMap<u32, (i32, i32)> = std::collections::HashMap::new();
            for k in 0..1500u32 {
                s.p.hp = s.p.max_hp;
                d.tick(&mut s);
                s.step(&script(k));
                let hero = (s.p.x, s.p.y);
                for i in 0..s.e.len() {
                    let (id, now) = (s.e.id[i], (s.e.x[i], s.e.y[i]));
                    if let Some(&was) = last.get(&id) {
                        let near = |p: (i32, i32)| dist2(p.0 - hero.0, p.1 - hero.1) < sight2;
                        let jump = dist2(now.0 - was.0, now.1 - was.1);
                        assert!(jump <= 3_000i64.pow(2) || !(near(was) || near(now)), "seed {seed} tick {} foe id {id} jumped {} mm from {was:?} to {now:?} (hero {hero:?})", s.tick, isqrt(jump));
                        moves += 1;
                    }
                    last.insert(id, now);
                }
            }
        }
        assert!(moves > 10_000, "control: moves checked ({moves})");
    }

    /// No foe ever overlaps a wall or pillar: director-driven runs with the hero walking, every tick, every foe.
    #[test]
    fn foes_never_inside_walls() {
        let c = content();
        let (mut checked, mut pressing) = (0u64, 0u64);
        for seed in [1u64, 5, 9] {
            let mut s = Sim::new(&c, seed, None);
            let mut d = Director::new(seed);
            for k in 0..1500u32 {
                s.p.hp = s.p.max_hp;
                d.tick(&mut s);
                s.step(&script(k));
                let near = s.walls.near(&s.arena);
                for i in 0..s.e.len() {
                    assert!(!solid::blocked(&near, s.e.x[i], s.e.y[i], s.e.rad[i]), "seed {seed} tick {} foe {i} tier {:?} at {},{} r {}", s.tick, s.e.tier[i], s.e.x[i], s.e.y[i], s.e.rad[i]);
                    checked += 1;
                    pressing += u64::from(solid::blocked(&near, s.e.x[i], s.e.y[i], s.e.rad[i] + 400));
                }
            }
        }
        assert!(checked > 10_000 && pressing > 400, "control: foes checked ({checked}) and pressed against walls ({pressing})");
    }

    #[test]
    fn walls_stop_the_hero_and_spawns_avoid_walls() {
        let c = content();
        let mut s = Sim::new(&c, 3, None);
        let wall = (0..).find(|&x| s.arena.tile(x, 0).solid()).unwrap_or(0) * crate::floorgen::TILE_MM;
        assert!(wall > 0);
        for _ in 0..3000 {
            s.step(&Input { mv: (1, 0), ..Input::default() });
        }
        // control: the walk was long enough to cross the wall had it not blocked
        assert!(3000 * c.player.speed > wall + 2 * c.player.radius);
        assert!(s.p.x + c.player.radius < wall, "hero {} wall {wall}", s.p.x);
        assert!(!solid::blocked(&s.arena, s.p.x, s.p.y, c.player.radius));
        for k in 0..300 {
            let (x, y) = s.place(Formation::Scatter, 0, k, 1);
            assert!(!solid::blocked(&s.arena, x, y, MOB_R_MAX), "spawn {x},{y}");
        }
    }

    #[test]
    fn tactics_steer_apart_and_rush_is_the_control() {
        let aim = |t, id, tick, at, face| tactic_aim(t, id, tick, at, (0, 0), face);
        assert_eq!((0..4).map(Tactic::of).collect::<Vec<_>>(), [Tactic::Rush, Tactic::Surround, Tactic::Bait, Tactic::Flank]);
        // control: rush aims at the hero
        assert_eq!(aim(Tactic::Rush, 0, 0, (20_000, 0), (1, 0)), ((0, 0), true));
        // surround: a ring point (about RING_R from the hero), differing per id
        let (p1, _) = aim(Tactic::Surround, 1, 0, (20_000, 0), (1, 0));
        let (p5, _) = aim(Tactic::Surround, 5, 0, (20_000, 0), (1, 0));
        assert!((dist(p1.0, p1.1) - RING_R).abs() < 50 && p1 != p5, "{p1:?} {p5:?}");
        assert_eq!(aim(Tactic::Surround, 1, 0, (RING_R, 0), (1, 0)).0, (0, 0), "inside the ring it closes");
        // bait: inside the hold radius it moves only BAIT_LUNGE of every BAIT_PERIOD ticks
        let moves = (0..BAIT_PERIOD).filter(|&t| aim(Tactic::Bait, 2, t, (BAIT_HOLD, 0), (1, 0)).1).count();
        assert_eq!(moves as u32, BAIT_LUNGE);
        assert!(aim(Tactic::Bait, 2, 50, (BAIT_HOLD + 1, 0), (1, 0)).1, "outside the hold it always comes");
        // flank: in front it steers to the side (perpendicular to facing), behind it rushes
        let (side, _) = aim(Tactic::Flank, 3, 0, (10_000, 0), (1, 0));
        assert!(side.0 == 0 && side.1.abs() == FLANK_R, "{side:?}");
        assert_eq!(aim(Tactic::Flank, 3, 0, (-10_000, 0), (1, 0)).0, (0, 0));
    }

    /// Pinned before the iso renderer landed (2026-11-03): render/data view knobs never touch sim state.
    /// Re-pinned 2026-11-08: sim changed on purpose (hero confidence makes trash panic).
    /// Re-pinned 2026-11-08: foe tactics (surround/bait/flank) solid walls (player, foes, spawns) and status effects (freeze/burn/rage) and thief abilities change the run.
    /// Re-pinned 2026-11-08: integration: all lanes (wave 1): world NPCs, goblins, blasts via hurt_player, shop stock from Content.
    /// Re-pinned 2026-11-08: cave walls (72 m rooms, noisy rock bands) and bolts hold fire with auto-fire off.
    /// Re-pinned 2026-11-08: blocked chasers walk the flow field round walls.
    /// Re-pinned 2026-11-09: a hero sheltering from rain or acid draws foes faster.
    /// Re-pinned 2026-11-09: hunger (walking pace) and food drops from trash.
    /// Re-pinned 2026-11-09: ore drops from trash.
    #[test]
    fn iso_render_leaves_sim_hash_unchanged() {
        let h = run_hash(&content(), 7, 900);
        assert_eq!(h, 0x30e7_a5b8_29c5_051c, "sim hash {h:#x}");
    }

    #[test]
    fn zero_heap_allocs_per_tick() {
        let c = content();
        let mut sim = Sim::new(&c, 5, None);
        let mut d = Director::new(5);
        for t in 0..5400 {
            d.tick(&mut sim);
            sim.step(&script(t));
        }
        assert!(sim.live() > 10 && sim.owned.len() > 2, "armed: real load ({} live)", sim.live());
        let kills = sim.stats.kills;
        let before = allocs();
        for t in 5400..7200 {
            d.tick(&mut sim);
            sim.step(&script(t));
        }
        assert_eq!(allocs() - before, 0, "allocs in 1800 ticks");
        assert!(sim.stats.kills - kills > 50, "armed: spawns + kills + drops ran ({} kills)", sim.stats.kills - kills);
        // negative control: the counter sees an allocation
        let v: Vec<u8> = Vec::with_capacity(16);
        assert!(allocs() > before, "{}", v.capacity());
    }

    #[test]
    fn tick_p99_under_4ms_at_20k_entities() {
        let mut t = texts();
        t[1] = t[1].replacen("\"live_cap\":600", "\"live_cap\":20000", 1).replacen("\"despawn_m\":40", "\"despawn_m\":400", 1);
        let c = Content::parse(&t).unwrap();
        let mut sim = Sim::new(&c, 9, None);
        let fam = c.fam_ids().next().unwrap();
        let mut rng = Xo::seeded(9);
        let n60 = NonZeroU32::new(120_000).unwrap();
        for _ in 0..20_000 {
            let (x, y) = (rng.below(n60) as i32 - 60_000, rng.below(n60) as i32 - 60_000);
            sim.push_enemy(fam, Tier::Trash, (x, y), 1_000_000, (false, false));
        }
        let mut times = Vec::with_capacity(100);
        let before = allocs();
        for k in 0..100 {
            // a dead hero stops the sim (step returns at once): keep it alive so every tick carries the load
            sim.p.hp = sim.p.max_hp;
            let s = Instant::now();
            sim.step(&script(k));
            times.push(s.elapsed().as_micros());
        }
        let allocated = allocs() - before;
        times.sort_unstable();
        let p99 = times[98];
        assert!(sim.live() >= 19_000, "load held: {}", sim.live());
        assert_eq!((sim.tick, sim.outcome), (100, None), "every tick ran");
        eprintln!("PERF 20k: tick p50 {} us p99 {p99} us max {} us", times[50], times[99]);
        assert!(p99 <= 4000, "p99 {p99} us (p50 {} us)", times[50]);
        assert_eq!(allocated, 0);
    }

    /// Number key = special: a 36-bolt ring at 3x damage, then a reload; rank shortens it; passive slots never cast.
    /// Champions always drop a real item, elites one in ELITE_ITEM_1_IN; walking over it wears it.
    /// XP magnet: far orbs stay put without it; with it every orb (and a stream from beyond sight) flies in.
    #[test]
    fn xp_magnet_pulls_every_orb_and_a_stream_from_beyond_sight() {
        let c = content();
        let put = |s: &mut Sim, x: i32, y: i32, kind: Drop| {
            s.drops.x.push(x);
            s.drops.y.push(y);
            s.drops.kind.push(kind);
            s.drops.pulled.push(false);
        };
        let mut s = Sim::new(&c, 7, None);
        s.world.npcs.clear();
        let (hx, hy) = (s.p.x, s.p.y);
        put(&mut s, hx + 15_000, hy, Drop::Xp(5));
        put(&mut s, hx, hy - 18_000, Drop::Xp(5));
        for _ in 0..30 {
            s.p.hp = s.p.max_hp;
            s.step(&Input::default());
        }
        assert_eq!(s.drops.x.len(), 2, "control: far orbs stay without a magnet");
        let xp0 = (s.p.level, s.p.xp);
        let (px, py) = (s.p.x, s.p.y);
        put(&mut s, px, py, Drop::Magnet);
        s.p.hp = s.p.max_hp;
        s.step(&Input::default());
        let far = (0..s.drops.x.len()).filter(|&k| dist2(s.drops.x[k] - s.p.x, s.drops.y[k] - s.p.y) > i64::from(c.view.fog_end).pow(2)).count();
        assert!(s.drops.pulled.iter().all(|&p| p) && far >= MAGNET_FAR as usize, "all pulled, a stream from beyond sight ({far})");
        for _ in 0..900 {
            s.p.hp = s.p.max_hp;
            s.step(&Input::default());
        }
        assert!(s.drops.x.is_empty(), "every orb arrived");
        assert!((s.p.level, s.p.xp) > xp0, "the bar grew (or levelled up)");
    }

    /// The belly empties over FOOD_LAST_S; hunger slows the hero; eating food fills it again.
    #[test]
    fn hunger_slows_and_food_restores() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        s.world.npcs.clear();
        assert_eq!((s.p.food, s.hunger_pace().0), (FOOD_FULL, 1000), "control: fed");
        let hz = c.hz().get() as i32;
        for _ in 0..(FOOD_LAST_S * hz * 72 / 100) {
            s.p.hp = s.p.max_hp;
            s.step(&Input::default());
        }
        assert!(s.p.food < FOOD_LOW && s.p.food > 0, "hungry after 72% of the belly's life ({})", s.p.food);
        assert_eq!(s.hunger_pace().0, HUNGRY_PACE);
        s.p.food = 0;
        assert_eq!(s.hunger_pace().0, STARVED_PACE, "empty belly: slower still");
        let (x, y) = (s.p.x, s.p.y);
        s.drops.x.push(x);
        s.drops.y.push(y);
        s.drops.kind.push(Drop::Food);
        s.drops.pulled.push(false);
        s.step(&Input::default());
        assert!(s.p.food > FOOD_MEAL - FOOD_FULL / 100, "a meal fills the belly");
        let hits = (0..2000u32).filter(|&id| mix(u64::from(id) ^ ITEM_SALT.rotate_left(41)).is_multiple_of(FOOD_1_IN)).count();
        assert!((50..110).contains(&hits), "food drop rate {hits}/2000");
        let stride = |food: i32| {
            let mut s = Sim::new(&c, 7, None);
            s.p.food = food;
            let x0 = s.p.x;
            s.step(&Input { mv: (1, 0), ..Input::default() });
            s.p.x - x0
        };
        assert!(stride(0) < stride(FOOD_FULL), "a starved hero walks slower ({} vs {})", stride(0), stride(FOOD_FULL));
    }

    #[test]
    fn kills_drop_items_and_pickup_wears_them() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        s.world.npcs.clear();
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        s.push_enemy(fam, Tier::Champion, (s.p.x + 20_000, s.p.y), 1, (false, false));
        s.e.hp[0] = 0;
        s.step(&Input::default());
        let k = s.drops.kind.iter().position(|d| matches!(d, Drop::Item(_))).expect("champion dropped an item");
        let Drop::Item(id) = s.drops.kind[k] else { unreachable!() };
        let slot = c.items[usize::from(id.0)].slot;
        assert_eq!(s.gear.worn(slot), None, "control: not worn before pickup");
        (s.p.x, s.p.y) = (s.drops.x[k], s.drops.y[k]);
        s.step(&Input::default());
        assert_eq!(s.gear.worn(slot), Some(id), "walked over it: worn");
        // elites: about one in ELITE_ITEM_1_IN by id
        let hits = (0..400u32).filter(|&id| mix(u64::from(id) ^ ITEM_SALT).is_multiple_of(ELITE_ITEM_1_IN)).count();
        assert!((60..140).contains(&hits), "elite drop rate {hits}/400");
    }

    /// Bags hold what wearing replaced; wear from the bag swaps; discard destroys; taking off goes to the bag;
    /// Ore drops from trash, is carried; at a blacksmith 5 ore forge a rare-or-better item, worn at once. Away
    /// from a blacksmith, or short of ore, forging does nothing.
    #[test]
    fn ore_is_collected_and_the_blacksmith_forges() {
        use crate::data::Rarity;
        let c = content();
        let hits = (0..800u32).filter(|&id| mix(u64::from(id) ^ ITEM_SALT.rotate_left(53)).is_multiple_of(ORE_1_IN)).count();
        assert!((60..150).contains(&hits), "ore drop rate {hits}/800");
        let mut s = Sim::new(&c, 7, None);
        let (x, y) = (s.p.x, s.p.y);
        s.drops.x.push(x);
        s.drops.y.push(y);
        s.drops.kind.push(Drop::Ore);
        s.drops.pulled.push(false);
        s.step(&Input::default());
        assert_eq!(s.p.ore, 1, "picked up");
        s.p.ore = FORGE_ORE;
        s.step(&Input { forge: true, ..Input::default() });
        assert_eq!(s.p.ore, FORGE_ORE, "NEG: no blacksmith here, nothing forged");
        s.world.npcs = vec![npc::Placed { npc: npc::Npc::Blacksmith { honest: true }, x: s.p.x + 500, y: s.p.y }];
        s.world.shops = vec![Vec::new()];
        s.world_step();
        assert_eq!(s.world.open_shop, Some(0), "at the counter");
        let worn = |s: &Sim| crate::data::Slot::ALL.iter().filter_map(|&sl| s.gear.worn(sl)).count();
        let before = worn(&s);
        s.step(&Input { forge: true, ..Input::default() });
        assert_eq!(s.p.ore, 0, "ore spent");
        assert!(worn(&s) > before && crate::data::Slot::ALL.iter().filter_map(|&sl| s.gear.worn(sl)).all(|id| c.items[usize::from(id.0)].rarity >= Rarity::Rare), "a rare-or-better item forged and worn");
        s.step(&Input { forge: true, ..Input::default() });
        assert_eq!(s.p.ore, 0, "NEG: short of ore, nothing happens");
    }

    /// nothing is ever lost: a full bag keeps a taken-off item worn and sends a replaced one to the floor.
    #[test]
    fn bags_keep_replaced_items_and_never_lose_one() {
        use crate::data::Slot;
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        s.world.npcs.clear();
        let id = |k: &str| ItemId(c.items.iter().position(|it| it.key == k).unwrap() as u16);
        let (plate, coat) = (id("hazmat_plate"), id("firewall_coat"));
        s.wear(plate);
        assert!(s.bag.is_empty(), "control: an empty slot replaces nothing");
        s.wear(coat);
        assert_eq!((s.gear.worn(Slot::Armor), s.bag.clone()), (Some(coat), vec![plate]), "the plate went to the bag");
        s.step(&Input { bag: Some(BagAct::Wear(0)), ..Input::default() });
        assert_eq!((s.gear.worn(Slot::Armor), s.bag.clone()), (Some(plate), vec![coat]), "wear from the bag swaps");
        s.step(&Input { bag: Some(BagAct::Discard(0)), ..Input::default() });
        assert!(s.bag.is_empty(), "discarded");
        s.step(&Input { drop: Some(Slot::Armor), ..Input::default() });
        assert_eq!((s.gear.worn(Slot::Armor), s.bag.clone()), (None, vec![plate]), "taken off into the bag");
        // full bag
        s.bag = vec![coat; BAG_SLOTS];
        s.wear(plate);
        s.step(&Input { drop: Some(Slot::Armor), ..Input::default() });
        assert_eq!(s.gear.worn(Slot::Armor), Some(plate), "full bag: stays worn, not lost");
        let floor = s.drops.kind.len();
        s.wear(coat);
        assert_eq!(s.drops.kind.len(), floor + 1, "full bag: the replaced plate drops to the floor");
    }

    #[test]
    fn special_casts_reloads_and_rank_shortens_it() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let bolt = s.owned.iter().position(|o| c.skill(o.id).kind == SkillKind::Bolt).expect("hero starts with a bolt");
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        s.push_enemy(fam, Tier::Trash, (s.p.x + 4_000, s.p.y), 1_000_000, (false, false));
        let hold = Input { autofire: false, ..Input::default() };
        s.step(&hold);
        assert!(s.bolts.x.is_empty(), "control: auto-fire off, no key: no bolt");
        let cast = Input { special: Some(bolt as u8), ..hold };
        s.step(&cast);
        let normal = s.eff(&s.owned[bolt]).dmg;
        assert_eq!(s.bolts.x.len(), 36, "a full ring");
        assert!(s.bolts.dmg.iter().all(|&d| d == normal * SPECIAL_DMG), "3x damage");
        let reload = s.special_reload(1);
        assert_eq!(s.owned[bolt].sp, reload, "reload starts");
        s.bolts = Bolts::default();
        s.step(&cast);
        assert!(s.bolts.x.is_empty(), "NEG: reloading, no second ring");
        for _ in 0..reload {
            s.step(&hold);
        }
        assert_eq!(s.owned[bolt].sp, 0, "reloaded");
        assert!(s.special_reload(3) < s.special_reload(1), "upgrades reload faster");
        // empty slot and out-of-range slot cast nothing
        s.bolts = Bolts::default();
        s.step(&Input { special: Some(7), ..hold });
        assert!(s.bolts.x.is_empty(), "NEG: empty slot");
    }

    /// One item per slot; worn mods change the hero (max HP, armour, speed); dropping takes them away.
    #[test]
    fn gear_one_per_slot_mods_apply_drop_removes() {
        use crate::data::{Slot, Stat};
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let id = |k: &str| ItemId(c.items.iter().position(|it| it.key == k).unwrap() as u16);
        let (plate, coat) = (id("hazmat_plate"), id("firewall_coat"));
        let hp0 = s.max_hp();
        assert_eq!(s.gear.wear(&c, plate), Some(None));
        let plus = s.gear.sum(&c, Stat::MaxHp);
        assert!(plus > 0 && s.gear.sum(&c, Stat::Armour) == 6, "plate: +max hp, +6 armour");
        assert_eq!(i64::from(s.max_hp()), i64::from(hp0) + plus);
        assert_eq!(s.gear.wear(&c, coat), Some(Some(plate)), "same slot: the coat replaces the plate");
        assert_eq!(s.gear.wear(&c, ItemId(u16::MAX)), None, "NEG: not in the catalog");
        // armour cuts a hit: same hit, less HP lost with the coat on
        s.p.hp = s.max_hp();
        let hp = s.p.hp;
        s.hurt_player(20_000, Cause::Blast, true);
        let lost_coat = hp - s.p.hp;
        s.step(&Input { drop: Some(Slot::Armor), ..Input::default() });
        assert_eq!(s.gear.worn(Slot::Armor), None, "dropped");
        assert_eq!(s.gear.sum(&c, Stat::Armour), 0);
        s.p.hp = s.max_hp();
        s.p.iframes = 0;
        let hp = s.p.hp;
        s.hurt_player(20_000, Cause::Blast, true);
        assert!(hp - s.p.hp > lost_coat, "without armour the same hit costs more ({} vs {lost_coat})", hp - s.p.hp);
    }

    /// Same value as i64::isqrt: small n, around perfect squares up to the top (where f64 rounds n), extremes.
    #[test]
    fn isqrt_equals_std() {
        let mut ns: Vec<i64> = (0..5000).collect();
        let mut k = 3i64;
        while k < 3_037_000_499 {
            ns.extend([k * k - 1, k * k, k * k + 1]);
            k = k * 9 / 8 + 1;
        }
        let mut rng = Xo::seeded(3);
        ns.extend((0..5000).map(|_| (rng.next() >> 1) as i64));
        ns.push(i64::MAX);
        let mut fixed = 0;
        for &n in &ns {
            assert_eq!(isqrt(n), n.isqrt(), "n {n}");
            fixed += u32::from(((n as f64).sqrt() as i64) != n.isqrt());
        }
        assert!(fixed > 10, "control: the f64 root alone is wrong for some n ({fixed}), so the integer steps are exercised");
    }

    #[test]
    fn shapes_contain() {
        assert!(Shape::Band { along_x: true, at: 0, half: 500 }.contains(9000, 400));
        assert!(!Shape::Band { along_x: true, at: 0, half: 500 }.contains(0, 600));
        let l = Shape::Lattice { x: 0, y: 0, pitch: 6000, half: 1000 };
        assert!(l.contains(6500, 3000) && l.contains(3000, -5500) && !l.contains(3000, 3000));
        assert!(Shape::Circle { x: 0, y: 0, r: 1000 }.contains(700, 700) && !Shape::Circle { x: 0, y: 0, r: 1000 }.contains(800, 800));
    }

    #[test]
    fn hack_flips_trash_and_cap_bursts_oldest() {
        let c = content();
        let mut sim = Sim::new(&c, 1, None);
        let fam = c.fam_ids().next().unwrap();
        for k in 0..12 {
            sim.push_enemy(fam, Tier::Trash, (1500 + k * 100, 0), 1_000_000, (false, false));
        }
        sim.p.face = (1, 0);
        sim.step(&Input { hack: true, ..Input::default() });
        // 12 flip (3 GLITCH >= ICE 3); cap 8 + 2 x rank 1 = 10: the 2 oldest burst at once
        assert_eq!(sim.hijack_cap(), 10);
        assert_eq!(sim.hijacked, 10);
        assert_eq!(sim.e.side.iter().filter(|s| **s == Side::Hijacked).count(), 10);
    }
}
