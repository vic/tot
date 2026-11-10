//! Crew: agentic NPCs. A local agent the player prompted (crate::crew) names them, picks their side and role and
//! orders them about; the sim moves them and runs their role every tick, so they fight between orders too.
//! A pal walks on its own body beside the hero. A foe IS an enemy body (a champion): the hero's weapons hit it
//! like any foe, and its role works on top. Integer only, no RNG stream consumed.
use super::*;

/// Crew members at once (pals and foes together).
pub const CREW_MAX: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Healer,
    Tank,
    Necro,
    Mage,
    Warrior,
    Archer,
}

impl Role {
    pub const ALL: [Role; 6] = [Role::Healer, Role::Tank, Role::Necro, Role::Mage, Role::Warrior, Role::Archer];

    /// Name in the agent protocol.
    pub fn key(self) -> &'static str {
        match self {
            Role::Healer => "healer",
            Role::Tank => "tank",
            Role::Necro => "necro",
            Role::Mage => "mage",
            Role::Warrior => "warrior",
            Role::Archer => "archer",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.key() == s)
    }

    /// (pal HP permille of the hero's base HP, foe HP as trash HP multiples, act every ms, reach mm, power permille
    /// of the hero's first skill hit)
    fn stats(self) -> (i64, i64, u32, i32, i64) {
        match self {
            Role::Healer => (700, 10, 2_000, 9_000, 0),
            Role::Tank => (2_000, 30, 900, 1_800, 600),
            Role::Necro => (800, 12, 3_000, 7_000, 0),
            Role::Mage => (700, 10, 2_500, 9_000, 2_500),
            Role::Warrior => (1_200, 14, 500, 1_800, 2_000),
            Role::Archer => (800, 10, 700, 11_000, 1_200),
        }
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
    /// pal: stays by the hero (fighters step in front of it); foe: hunts the hero
    Follow,
    /// stands where it is
    Hold,
    /// runs at the nearest enemy of its side
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

/// A short word an agent gives (an id or a shown name): 1..=16 chars of [A-Za-z0-9_-] and space, not blank. Copy,
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
    pub role: Role,
    pub body: Body,
    pub stance: Stance,
    /// where an agent told it to go (world mm); cleared on arrival
    pub goal: Option<(i32, i32)>,
    /// ticks until its role acts again
    cd: u16,
}

impl Mate {
    pub fn side(&self) -> Allegiance {
        match self.body {
            Body::Pal { .. } => Allegiance::Pal,
            Body::Foe { .. } => Allegiance::Foe,
        }
    }
}

/// What the crew did, for the view and for the agent's report (drained by the play loop).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrewEv {
    Joined(Word),
    /// fell (a pal at 0 HP, a foe's body killed)
    Down(Word),
    Healed { by: Word, hp: i32 },
    /// a necro raised a foe to fight for its side
    Raised { by: Word },
    Struck { by: Word, dmg: i32 },
    /// a foe's spell or volley is on its way
    Cast { by: Word },
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
/// Pal body radius, walking pace (permille of the hero's), arrival distance for a goal.
const MATE_R: i32 = 350;
const MATE_PACE: i64 = 900;
const ARRIVE: i32 = 500;
/// Foes this near a pal tank go for the tank instead of the hero.
pub(super) const TAUNT_R: i32 = 5_000;
/// Healing: heal permille of the target's max; patients above this permille of max wait.
const HEAL_PERMILLE: i64 = 150;
const HEAL_BELOW: i64 = 900;
/// Necro raises foes below this permille of their max.
const RAISE_BELOW: i64 = 600;
/// Mage blast radius; foe archer shot speed (mm/tick); foe mage telegraph lead (ms) and circle radius.
const BLAST_R: i32 = 2_200;
const SHOT_V: i32 = 350;
const TELE_MS: u32 = 1_000;
const TELE_R: i32 = 1_800;
/// Foe necro raises this many trash at once.
const RAISE_N: u8 = 3;
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

    /// A new crew member: a pal beside the hero, or a foe body (a champion of the first foe family) on the spawn
    /// ring, coming in from out of view.
    pub fn crew_join(&mut self, key: Word, name: Word, role: Role, side: Allegiance) -> Result<(), CrewErr> {
        if self.crew.len() >= CREW_MAX {
            return Err(CrewErr::Full);
        }
        if self.crew.iter().any(|m| m.key == key) {
            return Err(CrewErr::Taken);
        }
        let (pal_hp, foe_hp, ..) = role.stats();
        let body = match side {
            Allegiance::Pal => {
                let max = (i64::from(self.c.player.hp) * pal_hp / 1000) as i32;
                let (dx, dy) = self.trig[(self.crew.len() as u32 * TRIG_N / CREW_MAX as u32) as usize];
                let (x, y) = self.free_near(self.p.x + scale(dx, i64::from(FOLLOW_R)), self.p.y + scale(dy, i64::from(FOLLOW_R)), MATE_R);
                Body::Pal { x, y, hp: max, max }
            }
            Allegiance::Foe => {
                let fam = self.c.fam_ids().find(|&f| matches!(self.c.fam(f).kind, FamKind::Mob(_))).ok_or(CrewErr::NoBody)?;
                let sp = &self.c.spawn;
                let hp = (i64::from(sp.trash_hp) * i64::from(sp.hp_mult[self.sec_ix()]) / 1000 * foe_hp).clamp(1, i64::from(i32::MAX)) as i32;
                let at = self.place(Formation::Scatter, 0, 0, 1);
                let id = self.next_id;
                self.push_enemy(fam, Tier::Champion, at, hp, (false, false));
                Body::Foe { id }
            }
        };
        self.crew.push(Mate { key, name, role, body, stance: Stance::Follow, goal: None, cd: 0 });
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

    /// Spots pal tanks hold: hostiles near one go for it (enemies_move reads this).
    pub(super) fn taunt_spots(&mut self) {
        self.taunts.clear();
        for m in &self.crew {
            if let (Role::Tank, Body::Pal { x, y, .. }) = (m.role, m.body) {
                self.taunts.push((x, y));
            }
        }
    }

    /// The hit a pal's role deals: the hero's first skill hit, times the role's power and the Kinship boon.
    fn pal_hit(&self, power: i64) -> i32 {
        let base = self.owned.first().map_or(0, |o| self.eff(o).dmg);
        (i64::from(base) * power / 1000 * (1000 + self.boons.kin) / 1000) as i32
    }

    /// One tick of the crew: bodies gone fall, then every member moves and acts.
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
                    self.pal_act(k);
                }
                Body::Foe { id } => {
                    if let Some(b) = self.body_ix(id) {
                        self.foe_move(k, b);
                        self.foe_act(k, b);
                    }
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
        let fighter = matches!(m.role, Role::Tank | Role::Warrior);
        let to = match (m.goal, m.stance) {
            (Some(g), _) => Some(g),
            (None, Stance::Hold) => None,
            (None, Stance::Charge) => near_foe(CHARGE_R, x, y).or(Some(beside)),
            (None, Stance::Flee) => near_foe(FLEE_R, x, y).map(|(fx, fy)| (x + (x - fx), y + (y - fy))).or(Some(beside)),
            (None, Stance::Follow) => (if fighter { near_foe(GUARD_R, px, py) } else { None }).or(Some(beside)),
        };
        let Some((tx, ty)) = to else { return };
        if dist2(tx - x, ty - y) <= i64::from(ARRIVE).pow(2) {
            if m.stance != Stance::Hold {
                self.crew[k].goal = None;
            }
            return;
        }
        let pace = scale(self.c.player.speed, MATE_PACE);
        let (dx, dy) = toward(tx - x, ty - y, pace);
        let (ax, ay) = solid::slide(&self.walls.near(&self.arena), x, y, MATE_R, dx, dy);
        self.crew[k].body = Body::Pal { x: x + ax, y: y + ay, hp, max };
    }

    /// Hostiles touching a pal hit it (each on its own contact cooldown); a tank takes half.
    fn pal_hurt(&mut self, k: usize) {
        let Body::Pal { x, y, mut hp, max } = self.crew[k].body else { return };
        let mut near = std::mem::take(&mut self.near);
        self.grid.query(x, y, MATE_R + MOB_R_MAX, &mut near);
        for &j in &near {
            let j = j as usize;
            let r = MATE_R + self.e.rad[j];
            if self.e.side[j] == Side::Hostile && self.e.hit_cd[j] == 0 && self.e.hp[j] > 0 && dist2(self.e.x[j] - x, self.e.y[j] - y) <= i64::from(r).pow(2) {
                self.e.hit_cd[j] = self.c.player.contact_cd;
                let d = self.contact_dmg(j);
                hp -= if self.crew[k].role == Role::Tank { d / 2 } else { d };
            }
        }
        self.near = near;
        self.crew[k].body = Body::Pal { x, y, hp, max };
    }

    fn pal_act(&mut self, k: usize) {
        let m = self.crew[k];
        let Body::Pal { x, y, .. } = m.body else { return };
        if m.cd > 0 {
            self.crew[k].cd -= 1;
            return;
        }
        let (_, _, every, reach, power) = m.role.stats();
        let r2 = i64::from(reach).pow(2);
        let fx = self.owned.first().map(|o| o.id);
        let acted = match m.role {
            Role::Healer => {
                // the hero first, then the most hurt pal
                let hero = (self.p.hp < self.p.max_hp && i64::from(self.p.hp) * 1000 < i64::from(self.p.max_hp) * HEAL_BELOW && dist2(self.p.x - x, self.p.y - y) <= r2).then_some(None);
                let pal = || {
                    (0..self.crew.len())
                        .filter_map(|j| match self.crew[j].body {
                            Body::Pal { x: jx, y: jy, hp, max } if i64::from(hp) * 1000 < i64::from(max) * HEAL_BELOW && dist2(jx - x, jy - y) <= r2 => Some((i64::from(hp) * 1000 / i64::from(max.max(1)), j)),
                            _ => None,
                        })
                        .min()
                        .map(|(_, j)| Some(j))
                };
                match hero.or_else(pal) {
                    Some(None) => {
                        let h = (i64::from(self.p.max_hp) * HEAL_PERMILLE / 1000 * (1000 + self.boons.kin) / 1000) as i32;
                        self.p.hp = (self.p.hp + h).min(self.p.max_hp);
                        self.mate_fx(FxShape::Line { x0: x, y0: y, x1: self.p.x, y1: self.p.y }, fx);
                        self.ev(CrewEv::Healed { by: m.key, hp: h });
                        true
                    }
                    Some(Some(j)) => {
                        let Body::Pal { x: jx, y: jy, hp, max } = self.crew[j].body else { return };
                        let h = (i64::from(max) * HEAL_PERMILLE / 1000 * (1000 + self.boons.kin) / 1000) as i32;
                        self.crew[j].body = Body::Pal { x: jx, y: jy, hp: (hp + h).min(max), max };
                        self.mate_fx(FxShape::Line { x0: x, y0: y, x1: jx, y1: jy }, fx);
                        self.ev(CrewEv::Healed { by: m.key, hp: h });
                        true
                    }
                    None => false,
                }
            }
            Role::Necro => {
                let weak = (0..self.e.len()).find(|&j| {
                    self.e.side[j] == Side::Hostile && matches!(self.e.tier[j], Tier::Trash | Tier::Elite) && self.e.hp[j] > 0 && i64::from(self.e.hp[j]) * 1000 < i64::from(self.e.max[j]) * RAISE_BELOW && dist2(self.e.x[j] - x, self.e.y[j] - y) <= r2
                });
                match weak {
                    Some(j) if self.hijacked < self.hijack_cap().max(CREW_MAX as u16) => {
                        self.take_over(j);
                        self.mate_fx(FxShape::Ring { x: self.e.x[j], y: self.e.y[j], r: 700 }, fx);
                        self.ev(CrewEv::Raised { by: m.key });
                        true
                    }
                    _ => false,
                }
            }
            Role::Tank | Role::Mage | Role::Warrior | Role::Archer => match self.nearest_hostile(x, y, reach, &[]) {
                Some(j) => {
                    let (tx, ty) = (self.e.x[j], self.e.y[j]);
                    let dmg = self.pal_hit(power);
                    if m.role == Role::Mage {
                        self.area_hit(tx, ty, BLAST_R, dmg, Hue::Code, None);
                        self.mate_fx(FxShape::Ring { x: tx, y: ty, r: BLAST_R }, fx);
                    } else {
                        self.hurt(j, dmg, Hue::Code, None);
                        self.mate_fx(FxShape::Line { x0: x, y0: y, x1: tx, y1: ty }, fx);
                    }
                    self.ev(CrewEv::Struck { by: m.key, dmg });
                    true
                }
                None => false,
            },
        };
        if acted {
            self.crew[k].cd = self.ticks(every);
        }
    }

    fn mate_fx(&mut self, shape: FxShape, skill: Option<SkillId>) {
        if let Some(s) = skill {
            self.fx(shape, s);
        }
    }

    /// A foe body moves by the chase like any foe; holding, fleeing or a goal overrides it.
    fn foe_move(&mut self, k: usize, b: usize) {
        let m = self.crew[k];
        let (x, y) = (self.e.x[b], self.e.y[b]);
        let to = match (m.goal, m.stance) {
            (Some(g), _) => g,
            (None, Stance::Flee) => (x + (x - self.p.x), y + (y - self.p.y)),
            (None, Stance::Follow | Stance::Charge | Stance::Hold) => return,
        };
        // undo this tick's chase, then walk toward the spot at the family's pace
        let pace = self.c.fam(self.e.fam[b]).speed;
        let (dx, dy) = toward(to.0 - x, to.1 - y, pace.min(dist(to.0 - x, to.1 - y)));
        let (ax, ay) = solid::slide(&self.walls.near(&self.arena), x, y, self.e.rad[b], dx, dy);
        (self.e.x[b], self.e.y[b]) = (x + ax, y + ay);
        if m.stance != Stance::Hold && dist2(to.0 - x, to.1 - y) <= i64::from(ARRIVE).pow(2) {
            self.crew[k].goal = None;
        }
    }

    fn foe_act(&mut self, k: usize, b: usize) {
        let m = self.crew[k];
        if m.cd > 0 {
            self.crew[k].cd -= 1;
            return;
        }
        let (_, _, every, reach, _) = m.role.stats();
        let (x, y) = (self.e.x[b], self.e.y[b]);
        let (px, py) = (self.p.x, self.p.y);
        let hero_near = dist2(px - x, py - y) <= i64::from(reach).pow(2);
        let fam = self.e.fam[b];
        let cause = Cause::Contact { fam, tier: Tier::Champion };
        let acted = match m.role {
            Role::Healer => {
                let r2 = i64::from(reach).pow(2);
                let hurt = (0..self.e.len())
                    .filter(|&j| j != b && self.e.side[j] == Side::Hostile && self.e.hp[j] > 0 && self.e.hp[j] < self.e.max[j] && dist2(self.e.x[j] - x, self.e.y[j] - y) <= r2)
                    .min_by_key(|&j| i64::from(self.e.hp[j]) * 1000 / i64::from(self.e.max[j].max(1)));
                match hurt {
                    Some(j) => {
                        let h = (i64::from(self.e.max[j]) * HEAL_PERMILLE / 1000) as i32;
                        self.e.hp[j] = (self.e.hp[j] + h).min(self.e.max[j]);
                        self.ev(CrewEv::Healed { by: m.key, hp: h });
                        true
                    }
                    None => false,
                }
            }
            Role::Tank => false,
            Role::Necro if hero_near => {
                let lead = self.ticks(TELE_MS * 2);
                self.tele(Shape::Circle { x, y, r: 1_500 }, lead, TeleKind::Spawn { fam, n: RAISE_N });
                self.ev(CrewEv::Cast { by: m.key });
                true
            }
            Role::Mage if hero_near => {
                let lead = self.ticks(TELE_MS);
                let dmg = self.contact_dmg(b) * 2;
                self.tele(Shape::Circle { x: px, y: py, r: TELE_R }, lead, TeleKind::HitPlayer { dmg, cause });
                self.ev(CrewEv::Cast { by: m.key });
                true
            }
            Role::Warrior if hero_near => {
                self.e.status[b] = self.e.status[b].enrage(self.c.hz().get());
                self.ev(CrewEv::Cast { by: m.key });
                true
            }
            Role::Archer if hero_near && self.shots.x.len() < self.shots.x.capacity() => {
                let (vx, vy) = toward(px - x, py - y, SHOT_V);
                let dmg = self.contact_dmg(b);
                let s = &mut self.shots;
                s.x.push(x);
                s.y.push(y);
                s.vx.push(vx);
                s.vy.push(vy);
                s.ttl.push((reach * 3 / 2 / SHOT_V) as u16);
                s.age.push(0);
                s.dmg.push(dmg);
                s.fam.push(fam);
                true
            }
            Role::Necro | Role::Mage | Role::Warrior | Role::Archer => false,
        };
        if acted {
            self.crew[k].cd = self.ticks(every);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;

    fn w(s: &str) -> Word {
        Word::parse(s).unwrap()
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
    fn words_roles_and_stances_parse_closed_sets() {
        assert_eq!(w(" Mira ").as_str(), "Mira");
        assert!(Word::parse("").is_none() && Word::parse("x".repeat(17).as_str()).is_none() && Word::parse("a<b").is_none(), "NEG: blank, long, markup");
        assert_eq!(Role::parse("necro"), Some(Role::Necro));
        assert_eq!(Role::parse("bard"), None);
        assert_eq!(Stance::parse("hold"), Some(Stance::Hold));
        assert!(Role::ALL.iter().all(|r| Role::parse(r.key()) == Some(*r)));
    }

    #[test]
    fn join_caps_keys_and_sides() {
        let c = content();
        let mut s = quiet(&c);
        assert_eq!(s.crew_join(w("a"), w("A"), Role::Healer, Allegiance::Pal), Ok(()));
        assert_eq!(s.crew_join(w("a"), w("A"), Role::Tank, Allegiance::Pal), Err(CrewErr::Taken));
        let n = s.e.len();
        assert_eq!(s.crew_join(w("rival"), w("RIVAL"), Role::Archer, Allegiance::Foe), Ok(()));
        assert_eq!(s.e.len(), n + 1, "a foe is an enemy body");
        assert_eq!(s.crew[1].side(), Allegiance::Foe);
        for k in 2..CREW_MAX {
            s.crew_join(w(&format!("m{k}")), w("M"), Role::Mage, Allegiance::Pal).unwrap();
        }
        assert_eq!(s.crew_join(w("late"), w("L"), Role::Mage, Allegiance::Pal), Err(CrewErr::Full));
        assert_eq!(s.crew_order(w("ghost"), Stance::Hold), Err(CrewErr::NoSuchMate));
        assert_eq!(s.crew_ev.first(), Some(&CrewEv::Joined(w("a"))));
    }

    #[test]
    fn a_healer_pal_heals_the_hero_and_follows() {
        let c = content();
        let mut s = quiet(&c);
        s.crew_join(w("doc"), w("DOC"), Role::Healer, Allegiance::Pal).unwrap();
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
    fn fighting_pals_strike_and_a_necro_raises_and_a_tank_draws_foes() {
        let c = content();
        let fam = c.fam_ids().find(|&f| matches!(c.fam(f).kind, FamKind::Mob(_))).unwrap();
        for role in [Role::Warrior, Role::Archer, Role::Mage, Role::Tank] {
            let mut s = quiet(&c);
            s.crew_join(w("x"), w("X"), role, Allegiance::Pal).unwrap();
            let (x, y, ..) = s.mate_at(&s.crew[0]).unwrap();
            s.push_enemy(fam, Tier::Trash, (x + 1_000, y), 1_000_000, (false, false));
            run(&mut s, 5);
            assert!(s.crew_ev.iter().any(|e| matches!(e, CrewEv::Struck { .. })), "{role:?} struck");
        }
        let mut s = quiet(&c);
        s.crew_join(w("n"), w("N"), Role::Necro, Allegiance::Pal).unwrap();
        let (x, y, ..) = s.mate_at(&s.crew[0]).unwrap();
        s.push_enemy(fam, Tier::Trash, (x + 3_000, y), 1_000_000, (false, false));
        run(&mut s, 2);
        assert_eq!(s.hijacked, 0, "control: a healthy foe is not raised");
        let j = s.e.len() - 1;
        s.e.hp[j] = s.e.max[j] / 3;
        run(&mut s, 2);
        assert!(s.hijacked == 1 && s.crew_ev.iter().any(|e| matches!(e, CrewEv::Raised { .. })), "weak foe raised");
        // a tank pal draws the foes near it; a healer on the same spot does not (differential control)
        let left = |role: Role| {
            let mut s = quiet(&c);
            s.crew_join(w("t"), w("T"), role, Allegiance::Pal).unwrap();
            let (tx, ty) = s.free_near(s.p.x + 8_000, s.p.y, MATE_R);
            s.crew[0].body = Body::Pal { x: tx, y: ty, hp: 1_000_000, max: 1_000_000 };
            s.crew_order(w("t"), Stance::Hold).unwrap();
            s.push_enemy(fam, Tier::Trash, (tx, ty + 4_000), 1_000_000, (false, false));
            let j = s.e.len() - 1;
            run(&mut s, 30);
            dist(s.e.x[j] - tx, s.e.y[j] - ty)
        };
        assert!(left(Role::Tank) + 1_000 < left(Role::Healer), "drawn to the tank: {} vs {}", left(Role::Tank), left(Role::Healer));
    }

    #[test]
    fn foe_crew_attacks_the_hero_and_falls_with_its_body() {
        let c = content();
        let mut s = quiet(&c);
        s.crew_join(w("bow"), w("BOW"), Role::Archer, Allegiance::Foe).unwrap();
        let Body::Foe { id } = s.crew[0].body else { panic!("foe body") };
        let b = s.body_ix(id).unwrap();
        (s.e.x[b], s.e.y[b]) = (s.p.x + 6_000, s.p.y);
        let shots = s.shots.x.len();
        run(&mut s, 2);
        assert!(s.shots.x.len() > shots, "the archer shoots at the hero");
        let mut s = quiet(&c);
        s.crew_join(w("hex"), w("HEX"), Role::Mage, Allegiance::Foe).unwrap();
        let Body::Foe { id } = s.crew[0].body else { panic!("foe body") };
        let b = s.body_ix(id).unwrap();
        (s.e.x[b], s.e.y[b]) = (s.p.x + 6_000, s.p.y);
        run(&mut s, 2);
        assert!(s.tele.iter().any(|t| matches!(t.kind, TeleKind::HitPlayer { .. })), "the mage telegraphs a blast at the hero");
        let b = s.body_ix(id).unwrap();
        s.e.hp[b] = 0;
        run(&mut s, 2);
        assert!(s.crew.is_empty() && s.crew_ev.contains(&CrewEv::Down(w("hex"))), "killed body: the foe falls");
    }
}
