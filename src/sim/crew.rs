//! Crew: agentic NPCs. The engine has no classes: a local agent the player prompted (crate::crew) sends each
//! member's kit as data (its class word, glyph, colour, HP, pace, and up to ABILITIES abilities, each one engine
//! verb with its numbers), picks its side and orders it about. The sim moves members and runs their abilities on
//! their own clocks, so they fight between orders too. A pal walks on its own body beside the hero; a foe IS an
//! enemy body (a champion): the hero's weapons hit it like any foe, and its abilities work on top. Integer only,
//! no RNG stream consumed.
use super::*;

/// Crew members at once (pals and foes together), abilities per member.
pub const CREW_MAX: usize = 6;
pub const ABILITIES: usize = 4;

/// What an ability does. The numbers of `Feat` say how strong, how far, how often.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verb {
    /// pal: heals the hero or the most hurt pal; foe: heals the most hurt foe. power = share of the target's max HP.
    Heal,
    /// pal: hits the nearest foe in reach; foe: shoots at the hero in reach. power = multiple of the hero's first
    /// skill hit (pal) or of the body's contact hit (foe).
    Strike,
    /// pal: hits every foe within radius of the nearest foe in reach; foe: a telegraphed blast of radius on the
    /// hero. power as Strike.
    Blast,
    /// pal: turns a foe in reach below power share of its HP to fight for the hero; foe: raises power (whole, 1..5)
    /// foes around itself, telegraphed.
    Raise,
    /// pal: foes within radius go for this member instead of the hero; foe: flies into a rage when the hero is in reach.
    Taunt,
}

impl Verb {
    pub const ALL: [Verb; 5] = [Verb::Heal, Verb::Strike, Verb::Blast, Verb::Raise, Verb::Taunt];

    pub fn key(self) -> &'static str {
        match self {
            Verb::Heal => "heal",
            Verb::Strike => "strike",
            Verb::Blast => "blast",
            Verb::Raise => "raise",
            Verb::Taunt => "taunt",
        }
    }

    pub fn parse(s: &str) -> Option<Verb> {
        Verb::ALL.into_iter().find(|v| v.key() == s)
    }
}

/// Allowed ranges of the kit numbers (inclusive): an ability or a kit outside them cannot be built.
pub const EVERY_MS: (u32, u32) = (200, 30_000);
pub const REACH_MM: (i32, i32) = (500, 15_000);
pub const POWER_PM: (i32, i32) = (0, 5_000);
pub const RADIUS_MM: (i32, i32) = (0, 6_000);
/// HP as permille of the hero's base HP; pace as permille of the hero's pace.
pub const HP_PM: (i32, i32) = (100, 4_000);
pub const PACE_PM: (i32, i32) = (200, 1_500);

fn within<T: PartialOrd>(v: T, (lo, hi): (T, T)) -> bool {
    lo <= v && v <= hi
}

/// One ability: a verb and its numbers, every one inside its range (`new` refuses the rest).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Feat {
    verb: Verb,
    every_ms: u32,
    reach: i32,
    power: i32,
    radius: i32,
}

impl Feat {
    pub fn new(verb: Verb, every_ms: u32, reach: i32, power: i32, radius: i32) -> Option<Feat> {
        (within(every_ms, EVERY_MS) && within(reach, REACH_MM) && within(power, POWER_PM) && within(radius, RADIUS_MM)).then_some(Feat { verb, every_ms, reach, power, radius })
    }
    pub fn verb(&self) -> Verb {
        self.verb
    }
}

/// A shown colour an agent picked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb8(pub u8, pub u8, pub u8);

/// A member as the agent made it: shown class word, glyph and colour, HP, pace and abilities.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Kit {
    pub class: Word,
    pub glyph: char,
    pub rgb: Option<Rgb8>,
    hp: i32,
    pace: i32,
    abilities: [Option<Feat>; ABILITIES],
}

impl Kit {
    /// None when a number is out of range, the glyph is not one printable ASCII char, or no ability is given.
    pub fn new(class: Word, glyph: char, rgb: Option<Rgb8>, hp: i32, pace: i32, abilities: &[Feat]) -> Option<Kit> {
        let mut a = [None; ABILITIES];
        for (slot, ab) in a.iter_mut().zip(abilities) {
            *slot = Some(*ab);
        }
        let ok = glyph.is_ascii_graphic() && within(hp, HP_PM) && within(pace, PACE_PM) && (1..=ABILITIES).contains(&abilities.len());
        ok.then_some(Kit { class, glyph, rgb, hp, pace, abilities: a })
    }

    pub fn abilities(&self) -> impl Iterator<Item = &Feat> {
        self.abilities.iter().flatten()
    }

    /// Steps in to fight foes near the hero (it can hit up close or draws foes).
    fn fighter(&self) -> bool {
        self.abilities().any(|a| a.verb == Verb::Taunt || (a.verb == Verb::Strike && a.reach <= 2_500))
    }
}

/// Which side a crew member fights on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Allegiance {
    Pal,
    Foe,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stance {
    /// pal: stays by the hero (fighters step out to foes near it); foe: hunts the hero
    Follow,
    /// stands where it is
    Hold,
    /// pal: runs at the nearest foe; foe: hunts the hero
    Charge,
    /// runs from the nearest threat
    Flee,
}

impl Stance {
    pub const ALL: [Stance; 4] = [Stance::Follow, Stance::Hold, Stance::Charge, Stance::Flee];

    pub fn key(self) -> &'static str {
        match self {
            Stance::Follow => "follow",
            Stance::Hold => "hold",
            Stance::Charge => "charge",
            Stance::Flee => "flee",
        }
    }

    pub fn parse(s: &str) -> Option<Stance> {
        Stance::ALL.into_iter().find(|r| r.key() == s)
    }
}

/// A short word an agent gives (an id, a name, a class): 1..=16 chars of [A-Za-z0-9_-] and space, not blank. Copy,
/// so the sim stays allocation-free.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Word {
    b: [u8; 16],
    n: u8,
}

impl Word {
    pub fn parse(s: &str) -> Option<Word> {
        let s = s.trim();
        let ok = (1..=16).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_- ".contains(&c));
        let mut b = [0u8; 16];
        b.get_mut(..s.len())?.copy_from_slice(s.as_bytes());
        ok.then_some(Word { b, n: s.len() as u8 })
    }

    pub fn as_str(&self) -> &str {
        // only ASCII bytes are ever stored (parse)
        std::str::from_utf8(&self.b[..usize::from(self.n)]).unwrap_or_default()
    }
}

/// Where a crew member is: a pal has its own body, a foe is the enemy with this id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Body {
    Pal { x: i32, y: i32, hp: i32, max: i32 },
    Foe { id: u32 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mate {
    pub key: Word,
    pub name: Word,
    pub kit: Kit,
    pub body: Body,
    pub stance: Stance,
    /// where an agent told it to go (world mm); cleared on arrival
    pub goal: Option<(i32, i32)>,
    /// ticks until each ability acts again
    cd: [u16; ABILITIES],
}

impl Mate {
    pub fn side(&self) -> Allegiance {
        match self.body {
            Body::Pal { .. } => Allegiance::Pal,
            Body::Foe { .. } => Allegiance::Foe,
        }
    }
}

/// What the crew did, for the view and for the agents' report (drained by the play loop).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrewEv {
    Joined(Word),
    /// fell (a pal at 0 HP, a foe's body killed)
    Down(Word),
    Healed { by: Word, hp: i32 },
    /// raised foes to fight for its side
    Raised { by: Word },
    Struck { by: Word, dmg: i32 },
    /// a foe's blast, volley or rage is on its way
    Cast { by: Word, verb: Verb },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrewErr {
    Full,
    /// a member with this key already runs
    Taken,
    NoSuchMate,
    /// the adventure has no foe family to give a foe a body
    NoBody,
}

/// Pals follow on a ring this far from the hero (mm), fighters step out to foes this near the hero.
const FOLLOW_R: i32 = 1_800;
const GUARD_R: i32 = 6_000;
const CHARGE_R: i32 = 12_000;
const FLEE_R: i32 = 6_000;
/// A pal farther than this from the hero is brought back beside it.
const LOST_R: i32 = 25_000;
/// Pal body radius, arrival distance for a goal.
const MATE_R: i32 = 350;
const ARRIVE: i32 = 500;
/// Patients above this permille of max HP wait.
const HEAL_BELOW: i64 = 900;
/// Foe projectile speed (mm/tick); foe telegraph lead (ms); foe raise count range.
const SHOT_V: i32 = 350;
const TELE_MS: u32 = 1_000;
const RAISE_MAX: i32 = 5;
/// Events kept until the play loop drains them.
pub const CREW_EV_CAP: usize = 256;

impl Sim<'_> {
    fn ev(&mut self, e: CrewEv) {
        if self.crew_ev.len() < CREW_EV_CAP {
            self.crew_ev.push(e);
        }
    }

    fn mate_ix(&self, key: Word) -> Result<usize, CrewErr> {
        self.crew.iter().position(|m| m.key == key).ok_or(CrewErr::NoSuchMate)
    }

    fn ticks(&self, ms: u32) -> u16 {
        (ms * self.c.hz().get() / 1000).clamp(1, u32::from(u16::MAX)) as u16
    }

    /// A new crew member: a pal beside the hero, or a foe body (a champion of the first foe family, HP scaled with
    /// the run like any foe) on the spawn ring, coming in from out of view.
    pub fn crew_join(&mut self, key: Word, name: Word, kit: Kit, side: Allegiance) -> Result<(), CrewErr> {
        if self.crew.len() >= CREW_MAX {
            return Err(CrewErr::Full);
        }
        if self.crew.iter().any(|m| m.key == key) {
            return Err(CrewErr::Taken);
        }
        let base = i64::from(self.c.player.hp) * i64::from(kit.hp) / 1000;
        let body = match side {
            Allegiance::Pal => {
                let max = base as i32;
                let (dx, dy) = self.trig[(self.crew.len() as u32 * TRIG_N / CREW_MAX as u32) as usize];
                let (x, y) = self.free_near(self.p.x + scale(dx, i64::from(FOLLOW_R)), self.p.y + scale(dy, i64::from(FOLLOW_R)), MATE_R);
                Body::Pal { x, y, hp: max, max }
            }
            Allegiance::Foe => {
                let fam = self.c.fam_ids().find(|&f| matches!(self.c.fam(f).kind, FamKind::Mob(_))).ok_or(CrewErr::NoBody)?;
                let hp = (base * i64::from(self.c.spawn.hp_mult[self.sec_ix()]) / 1000).clamp(1, i64::from(i32::MAX)) as i32;
                let at = self.place(Formation::Scatter, 0, 0, 1);
                let id = self.next_id;
                self.push_enemy(fam, Tier::Champion, at, hp, (false, false));
                Body::Foe { id }
            }
        };
        self.crew.push(Mate { key, name, kit, body, stance: Stance::Follow, goal: None, cd: [0; ABILITIES] });
        self.ev(CrewEv::Joined(key));
        Ok(())
    }

    pub fn crew_order(&mut self, key: Word, stance: Stance) -> Result<(), CrewErr> {
        let i = self.mate_ix(key)?;
        let at = self.mate_at(&self.crew[i]).map(|(x, y, ..)| (x, y));
        let m = &mut self.crew[i];
        m.stance = stance;
        // holding keeps the spot it stood on when told
        m.goal = if stance == Stance::Hold { at } else { None };
        Ok(())
    }

    /// Send a member to a spot `(dx, dy)` tiles from the hero.
    pub fn crew_goal(&mut self, key: Word, dx: i32, dy: i32) -> Result<(), CrewErr> {
        let i = self.mate_ix(key)?;
        let t = crate::floorgen::TILE_MM;
        self.crew[i].goal = Some((self.p.x + dx.clamp(-40, 40) * t, self.p.y + dy.clamp(-40, 40) * t));
        Ok(())
    }

    /// The member leaves the agent's hands: a pal walks off, a foe's body stays as a plain champion.
    pub fn crew_leave(&mut self, key: Word) -> Result<(), CrewErr> {
        let i = self.mate_ix(key)?;
        self.crew.remove(i);
        Ok(())
    }

    fn body_ix(&self, id: u32) -> Option<usize> {
        self.e.id.iter().position(|&j| j == id)
    }

    /// (x, y, hp, max) of a member's body; None for a foe whose body is gone.
    pub fn mate_at(&self, m: &Mate) -> Option<(i32, i32, i32, i32)> {
        match m.body {
            Body::Pal { x, y, hp, max } => Some((x, y, hp, max)),
            Body::Foe { id } => self.body_ix(id).map(|b| (self.e.x[b], self.e.y[b], self.e.hp[b], self.e.max[b])),
        }
    }

    /// Spots pal taunters hold, with their radius: hostiles inside go for them (enemies_move reads this).
    pub(super) fn taunt_spots(&mut self) {
        self.taunts.clear();
        for m in &self.crew {
            if let Body::Pal { x, y, .. } = m.body {
                for a in m.kit.abilities().filter(|a| a.verb == Verb::Taunt) {
                    self.taunts.push((x, y, a.radius));
                }
            }
        }
    }

    /// A pal's hit: the hero's first skill hit, times the ability's power and the Kinship boon.
    fn pal_hit(&self, power: i32) -> i32 {
        let base = self.owned.first().map_or(0, |o| self.eff(o).dmg);
        (i64::from(base) * i64::from(power) / 1000 * (1000 + self.boons.kin) / 1000) as i32
    }

    /// One tick of the crew: bodies gone fall, then every member moves and its abilities act.
    pub(super) fn crew_step(&mut self) {
        let mut k = 0;
        while k < self.crew.len() {
            let m = self.crew[k];
            let fell = match m.body {
                Body::Pal { hp, .. } => hp <= 0,
                Body::Foe { id } => self.body_ix(id).is_none_or(|b| self.e.hp[b] <= 0),
            };
            if fell {
                self.crew.remove(k);
                self.ev(CrewEv::Down(m.key));
            } else {
                k += 1;
            }
        }
        for k in 0..self.crew.len() {
            match self.crew[k].body {
                Body::Pal { .. } => {
                    self.pal_move(k);
                    self.pal_hurt(k);
                }
                Body::Foe { id } => {
                    if let Some(b) = self.body_ix(id) {
                        self.foe_move(k, b);
                    }
                }
            }
            for a in 0..ABILITIES {
                let Some(ab) = self.crew[k].kit.abilities[a] else { continue };
                if self.crew[k].cd[a] > 0 {
                    self.crew[k].cd[a] -= 1;
                    continue;
                }
                let acted = match self.crew[k].body {
                    Body::Pal { x, y, .. } => self.pal_act(k, ab, (x, y)),
                    Body::Foe { id } => self.body_ix(id).is_some_and(|b| self.foe_act(k, ab, b)),
                };
                if acted {
                    self.crew[k].cd[a] = self.ticks(ab.every_ms);
                }
            }
        }
    }

    fn pal_move(&mut self, k: usize) {
        let m = self.crew[k];
        let Body::Pal { x, y, hp, max } = m.body else { return };
        let (px, py) = (self.p.x, self.p.y);
        if dist2(px - x, py - y) > i64::from(LOST_R).pow(2) {
            let (nx, ny) = self.free_near(px, py, MATE_R);
            self.crew[k].body = Body::Pal { x: nx, y: ny, hp, max };
            return;
        }
        let near_foe = |r: i32, cx: i32, cy: i32| self.nearest_hostile(cx, cy, r, &[]).map(|j| (self.e.x[j], self.e.y[j]));
        let (ox, oy) = self.trig[(k as u32 * TRIG_N / CREW_MAX as u32) as usize];
        let beside = (px + scale(ox, i64::from(FOLLOW_R)), py + scale(oy, i64::from(FOLLOW_R)));
        let to = match (m.goal, m.stance) {
            (Some(g), _) => Some(g),
            (None, Stance::Hold) => None,
            (None, Stance::Charge) => near_foe(CHARGE_R, x, y).or(Some(beside)),
            (None, Stance::Flee) => near_foe(FLEE_R, x, y).map(|(fx, fy)| (x + (x - fx), y + (y - fy))).or(Some(beside)),
            (None, Stance::Follow) => (if m.kit.fighter() { near_foe(GUARD_R, px, py) } else { None }).or(Some(beside)),
        };
        let Some((tx, ty)) = to else { return };
        if dist2(tx - x, ty - y) <= i64::from(ARRIVE).pow(2) {
            if m.stance != Stance::Hold {
                self.crew[k].goal = None;
            }
            return;
        }
        let pace = scale(self.c.player.speed, i64::from(m.kit.pace));
        let (dx, dy) = toward(tx - x, ty - y, pace);
        let (ax, ay) = solid::slide(&self.walls.near(&self.arena), x, y, MATE_R, dx, dy);
        self.crew[k].body = Body::Pal { x: x + ax, y: y + ay, hp, max };
    }

    /// Hostiles touching a pal hit it, each on its own contact cooldown.
    fn pal_hurt(&mut self, k: usize) {
        let Body::Pal { x, y, mut hp, max } = self.crew[k].body else { return };
        let mut near = std::mem::take(&mut self.near);
        self.grid.query(x, y, MATE_R + MOB_R_MAX, &mut near);
        for &j in &near {
            let j = j as usize;
            let r = MATE_R + self.e.rad[j];
            if self.e.side[j] == Side::Hostile && self.e.hit_cd[j] == 0 && self.e.hp[j] > 0 && dist2(self.e.x[j] - x, self.e.y[j] - y) <= i64::from(r).pow(2) {
                self.e.hit_cd[j] = self.c.player.contact_cd;
                hp -= self.contact_dmg(j);
            }
        }
        self.near = near;
        self.crew[k].body = Body::Pal { x, y, hp, max };
    }

    /// A pal ability; true when it acted (its clock restarts).
    fn pal_act(&mut self, k: usize, a: Feat, (x, y): (i32, i32)) -> bool {
        let by = self.crew[k].key;
        let r2 = i64::from(a.reach).pow(2);
        let fx = self.owned.first().map(|o| o.id);
        let kin = |v: i64| v * (1000 + self.boons.kin) / 1000;
        match a.verb {
            Verb::Heal => {
                // the hero first, then the most hurt pal
                if i64::from(self.p.hp) * 1000 < i64::from(self.p.max_hp) * HEAL_BELOW && dist2(self.p.x - x, self.p.y - y) <= r2 {
                    let h = kin(i64::from(self.p.max_hp) * i64::from(a.power) / 1000) as i32;
                    self.p.hp = (self.p.hp + h).min(self.p.max_hp);
                    self.mate_fx(FxShape::Line { x0: x, y0: y, x1: self.p.x, y1: self.p.y }, fx);
                    self.ev(CrewEv::Healed { by, hp: h });
                    return true;
                }
                let pal = (0..self.crew.len())
                    .filter_map(|j| match self.crew[j].body {
                        Body::Pal { x: jx, y: jy, hp, max } if i64::from(hp) * 1000 < i64::from(max) * HEAL_BELOW && dist2(jx - x, jy - y) <= r2 => Some((i64::from(hp) * 1000 / i64::from(max.max(1)), j)),
                        _ => None,
                    })
                    .min();
                let Some((_, j)) = pal else { return false };
                let Body::Pal { x: jx, y: jy, hp, max } = self.crew[j].body else { return false };
                let h = kin(i64::from(max) * i64::from(a.power) / 1000) as i32;
                self.crew[j].body = Body::Pal { x: jx, y: jy, hp: (hp + h).min(max), max };
                self.mate_fx(FxShape::Line { x0: x, y0: y, x1: jx, y1: jy }, fx);
                self.ev(CrewEv::Healed { by, hp: h });
                true
            }
            Verb::Raise => {
                let below = i64::from(a.power.min(1000));
                let weak = (0..self.e.len()).find(|&j| {
                    self.e.side[j] == Side::Hostile && matches!(self.e.tier[j], Tier::Trash | Tier::Elite) && self.e.hp[j] > 0 && i64::from(self.e.hp[j]) * 1000 < i64::from(self.e.max[j]) * below && dist2(self.e.x[j] - x, self.e.y[j] - y) <= r2
                });
                match weak {
                    Some(j) if self.hijacked < self.hijack_cap().max(CREW_MAX as u16) => {
                        self.take_over(j);
                        self.mate_fx(FxShape::Ring { x: self.e.x[j], y: self.e.y[j], r: 700 }, fx);
                        self.ev(CrewEv::Raised { by });
                        true
                    }
                    _ => false,
                }
            }
            // passive: taunt_spots draws the foes
            Verb::Taunt => false,
            Verb::Strike | Verb::Blast => {
                let Some(j) = self.nearest_hostile(x, y, a.reach, &[]) else { return false };
                let (tx, ty) = (self.e.x[j], self.e.y[j]);
                let dmg = self.pal_hit(a.power);
                if a.verb == Verb::Blast {
                    self.area_hit(tx, ty, a.radius, dmg, Hue::Code, None);
                    self.mate_fx(FxShape::Ring { x: tx, y: ty, r: a.radius }, fx);
                } else {
                    self.hurt(j, dmg, Hue::Code, None);
                    self.mate_fx(FxShape::Line { x0: x, y0: y, x1: tx, y1: ty }, fx);
                }
                self.ev(CrewEv::Struck { by, dmg });
                true
            }
        }
    }

    fn mate_fx(&mut self, shape: FxShape, skill: Option<SkillId>) {
        if let Some(s) = skill {
            self.fx(shape, s);
        }
    }

    /// A foe body moves by the chase like any foe; fleeing or a goal (holding is one) overrides it.
    fn foe_move(&mut self, k: usize, b: usize) {
        let m = self.crew[k];
        let (x, y) = (self.e.x[b], self.e.y[b]);
        let to = match (m.goal, m.stance) {
            (Some(g), _) => g,
            (None, Stance::Flee) => (x + (x - self.p.x), y + (y - self.p.y)),
            (None, Stance::Follow | Stance::Charge | Stance::Hold) => return,
        };
        let pace = scale(self.c.player.speed, i64::from(m.kit.pace));
        let (dx, dy) = toward(to.0 - x, to.1 - y, pace.min(dist(to.0 - x, to.1 - y)));
        let (ax, ay) = solid::slide(&self.walls.near(&self.arena), x, y, self.e.rad[b], dx, dy);
        (self.e.x[b], self.e.y[b]) = (x + ax, y + ay);
        if m.stance != Stance::Hold && dist2(to.0 - x, to.1 - y) <= i64::from(ARRIVE).pow(2) {
            self.crew[k].goal = None;
        }
    }

    /// A foe ability; true when it acted.
    fn foe_act(&mut self, k: usize, a: Feat, b: usize) -> bool {
        let by = self.crew[k].key;
        let (x, y) = (self.e.x[b], self.e.y[b]);
        let (px, py) = (self.p.x, self.p.y);
        let hero_near = dist2(px - x, py - y) <= i64::from(a.reach).pow(2);
        let fam = self.e.fam[b];
        let hit = (i64::from(self.contact_dmg(b)) * i64::from(a.power) / 1000) as i32;
        let cast = |s: &mut Self| s.ev(CrewEv::Cast { by, verb: a.verb });
        match a.verb {
            Verb::Heal => {
                let r2 = i64::from(a.reach).pow(2);
                let hurt = (0..self.e.len())
                    .filter(|&j| self.e.side[j] == Side::Hostile && self.e.hp[j] > 0 && self.e.hp[j] < self.e.max[j] && dist2(self.e.x[j] - x, self.e.y[j] - y) <= r2)
                    .min_by_key(|&j| i64::from(self.e.hp[j]) * 1000 / i64::from(self.e.max[j].max(1)));
                let Some(j) = hurt else { return false };
                let h = (i64::from(self.e.max[j]) * i64::from(a.power) / 1000) as i32;
                self.e.hp[j] = (self.e.hp[j] + h).min(self.e.max[j]);
                self.ev(CrewEv::Healed { by, hp: h });
                true
            }
            _ if !hero_near => false,
            Verb::Raise => {
                let n = (a.power / 1000).clamp(1, RAISE_MAX) as u8;
                let lead = self.ticks(TELE_MS * 2);
                self.tele(Shape::Circle { x, y, r: 1_500 }, lead, TeleKind::Spawn { fam, n });
                cast(self);
                true
            }
            Verb::Blast => {
                let lead = self.ticks(TELE_MS);
                self.tele(Shape::Circle { x: px, y: py, r: a.radius.max(MATE_R) }, lead, TeleKind::HitPlayer { dmg: hit, cause: Cause::Contact { fam, tier: Tier::Champion } });
                cast(self);
                true
            }
            Verb::Taunt => {
                self.e.status[b] = self.e.status[b].enrage(self.c.hz().get());
                cast(self);
                true
            }
            Verb::Strike if self.shots.x.len() < self.shots.x.capacity() => {
                let (vx, vy) = toward(px - x, py - y, SHOT_V);
                let s = &mut self.shots;
                s.x.push(x);
                s.y.push(y);
                s.vx.push(vx);
                s.vy.push(vy);
                s.ttl.push((a.reach * 3 / 2 / SHOT_V) as u16);
                s.age.push(0);
                s.dmg.push(hit);
                s.fam.push(fam);
                cast(self);
                true
            }
            Verb::Strike => false,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::data::tests::content;

    pub(crate) fn w(s: &str) -> Word {
        Word::parse(s).unwrap()
    }

    /// A one-ability kit with numbers fit for the verb.
    pub(crate) fn kit(v: Verb) -> Kit {
        let a = match v {
            Verb::Heal => Feat::new(v, 2_000, 9_000, 150, 0),
            Verb::Strike => Feat::new(v, 500, 11_000, 1_500, 0),
            Verb::Blast => Feat::new(v, 2_500, 9_000, 2_500, 2_200),
            Verb::Raise => Feat::new(v, 3_000, 7_000, 600, 0),
            Verb::Taunt => Feat::new(v, 1_000, 2_000, 0, 5_000),
        };
        Kit::new(w(v.key()), 'K', None, 1_000, 900, &[a.unwrap()]).unwrap()
    }

    fn quiet(c: &Content) -> Sim<'_> {
        let mut s = Sim::new(c, 7, None);
        s.world.npcs.clear();
        s.world.chests.clear();
        s
    }

    fn run(s: &mut Sim, ticks: u32) {
        for _ in 0..ticks {
            s.step(&Input::default());
        }
    }

    #[test]
    fn kits_and_words_refuse_numbers_out_of_range() {
        assert_eq!(w(" Mira ").as_str(), "Mira");
        assert!(Word::parse("").is_none() && Word::parse("x".repeat(17).as_str()).is_none() && Word::parse("a<b").is_none(), "NEG: blank, long, markup");
        assert!(Feat::new(Verb::Heal, 100, 9_000, 150, 0).is_none(), "NEG: faster than EVERY_MS");
        assert!(Feat::new(Verb::Strike, 500, 99_000, 1_000, 0).is_none(), "NEG: reach");
        assert!(Feat::new(Verb::Blast, 500, 9_000, 9_000, 0).is_none(), "NEG: power");
        let a = Feat::new(Verb::Heal, 2_000, 9_000, 150, 0).unwrap();
        assert!(Kit::new(w("x"), 'K', None, 1_000, 900, &[]).is_none(), "NEG: no ability");
        assert!(Kit::new(w("x"), 'é', None, 1_000, 900, &[a]).is_none(), "NEG: glyph");
        assert!(Kit::new(w("x"), 'K', None, 99_000, 900, &[a]).is_none(), "NEG: hp");
        assert!(Kit::new(w("x"), 'K', None, 1_000, 900, &[a; 5]).is_none(), "NEG: too many abilities");
        assert!(Verb::ALL.iter().all(|v| Verb::parse(v.key()) == Some(*v)) && Stance::parse("hold") == Some(Stance::Hold));
    }

    #[test]
    fn join_caps_keys_and_sides() {
        let c = content();
        let mut s = quiet(&c);
        assert_eq!(s.crew_join(w("a"), w("A"), kit(Verb::Heal), Allegiance::Pal), Ok(()));
        assert_eq!(s.crew_join(w("a"), w("A"), kit(Verb::Taunt), Allegiance::Pal), Err(CrewErr::Taken));
        let n = s.e.len();
        assert_eq!(s.crew_join(w("rival"), w("RIVAL"), kit(Verb::Strike), Allegiance::Foe), Ok(()));
        assert_eq!(s.e.len(), n + 1, "a foe is an enemy body");
        assert_eq!(s.crew[1].side(), Allegiance::Foe);
        for k in 2..CREW_MAX {
            s.crew_join(w(&format!("m{k}")), w("M"), kit(Verb::Blast), Allegiance::Pal).unwrap();
        }
        assert_eq!(s.crew_join(w("late"), w("L"), kit(Verb::Blast), Allegiance::Pal), Err(CrewErr::Full));
        assert_eq!(s.crew_order(w("ghost"), Stance::Hold), Err(CrewErr::NoSuchMate));
        assert_eq!(s.crew_ev.first(), Some(&CrewEv::Joined(w("a"))));
    }

    #[test]
    fn a_healing_pal_heals_the_hero_and_follows() {
        let c = content();
        let mut s = quiet(&c);
        s.crew_join(w("doc"), w("DOC"), kit(Verb::Heal), Allegiance::Pal).unwrap();
        s.p.hp = s.p.max_hp / 2;
        let hp0 = s.p.hp;
        run(&mut s, 3);
        assert!(s.p.hp > hp0, "healed: {} -> {}", hp0, s.p.hp);
        assert!(s.crew_ev.iter().any(|e| matches!(e, CrewEv::Healed { .. })));
        // the hero walks off: the pal follows
        s.p.x += 8_000;
        run(&mut s, 120);
        let (x, y, ..) = s.mate_at(&s.crew[0]).unwrap();
        assert!(dist(s.p.x - x, s.p.y - y) < 4_000, "followed to {x},{y}");
        // NEG: told to hold, it stays
        s.crew_order(w("doc"), Stance::Hold).unwrap();
        let at = s.mate_at(&s.crew[0]).map(|m| (m.0, m.1));
        s.p.x += 8_000;
        run(&mut s, 60);
        assert_eq!(s.mate_at(&s.crew[0]).map(|m| (m.0, m.1)), at, "held");
    }

    #[test]
    fn pal_verbs_strike_raise_and_taunt() {
        let c = content();
        let fam = c.fam_ids().find(|&f| matches!(c.fam(f).kind, FamKind::Mob(_))).unwrap();
        for v in [Verb::Strike, Verb::Blast] {
            let mut s = quiet(&c);
            s.crew_join(w("x"), w("X"), kit(v), Allegiance::Pal).unwrap();
            let (x, y, ..) = s.mate_at(&s.crew[0]).unwrap();
            s.push_enemy(fam, Tier::Trash, (x + 1_000, y), 1_000_000, (false, false));
            run(&mut s, 5);
            assert!(s.crew_ev.iter().any(|e| matches!(e, CrewEv::Struck { .. })), "{v:?} struck");
        }
        let mut s = quiet(&c);
        s.crew_join(w("n"), w("N"), kit(Verb::Raise), Allegiance::Pal).unwrap();
        let (x, y, ..) = s.mate_at(&s.crew[0]).unwrap();
        s.push_enemy(fam, Tier::Trash, (x + 3_000, y), 1_000_000, (false, false));
        run(&mut s, 2);
        assert_eq!(s.hijacked, 0, "control: a healthy foe is not raised");
        let j = s.e.len() - 1;
        s.e.hp[j] = s.e.max[j] / 3;
        run(&mut s, 2);
        assert!(s.hijacked == 1 && s.crew_ev.iter().any(|e| matches!(e, CrewEv::Raised { .. })), "weak foe raised");
        // a taunter draws the foes near it; a healer on the same spot does not (differential control)
        let left = |v: Verb| {
            let mut s = quiet(&c);
            s.crew_join(w("t"), w("T"), kit(v), Allegiance::Pal).unwrap();
            let (tx, ty) = s.free_near(s.p.x + 8_000, s.p.y, MATE_R);
            s.crew[0].body = Body::Pal { x: tx, y: ty, hp: 1_000_000, max: 1_000_000 };
            s.crew_order(w("t"), Stance::Hold).unwrap();
            s.push_enemy(fam, Tier::Trash, (tx, ty + 4_000), 1_000_000, (false, false));
            let j = s.e.len() - 1;
            run(&mut s, 30);
            dist(s.e.x[j] - tx, s.e.y[j] - ty)
        };
        assert!(left(Verb::Taunt) + 1_000 < left(Verb::Heal), "drawn to the taunter: {} vs {}", left(Verb::Taunt), left(Verb::Heal));
    }

    #[test]
    fn foe_crew_attacks_the_hero_and_falls_with_its_body() {
        let c = content();
        let foe_near = |v: Verb| {
            let mut s = quiet(&c);
            s.crew_join(w("f"), w("F"), kit(v), Allegiance::Foe).unwrap();
            let Body::Foe { id } = s.crew[0].body else { panic!("foe body") };
            let b = s.body_ix(id).unwrap();
            (s.e.x[b], s.e.y[b]) = (s.p.x + 6_000, s.p.y);
            s
        };
        let mut s = foe_near(Verb::Strike);
        let shots = s.shots.x.len();
        run(&mut s, 2);
        assert!(s.shots.x.len() > shots, "strike from range: a shot at the hero");
        let mut s = foe_near(Verb::Blast);
        run(&mut s, 2);
        assert!(s.tele.iter().any(|t| matches!(t.kind, TeleKind::HitPlayer { .. })), "a telegraphed blast at the hero");
        let mut s = foe_near(Verb::Raise);
        run(&mut s, 2);
        assert!(s.tele.iter().any(|t| matches!(t.kind, TeleKind::Spawn { .. })), "a telegraphed raise");
        let Body::Foe { id } = s.crew[0].body else { panic!("foe body") };
        let b = s.body_ix(id).unwrap();
        s.e.hp[b] = 0;
        run(&mut s, 2);
        assert!(s.crew.is_empty() && s.crew_ev.contains(&CrewEv::Down(w("f"))), "killed body: the foe falls");
    }
}
