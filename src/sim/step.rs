//! One tick of the sim, phase by phase. Order is fixed; every phase reads/writes SoA columns.
use super::*;
use super::weather::Permille;
use crate::data::{Hue, Mob, MobRole, Skill, Stat, Target};

/// Effective numbers of one owned skill after ranks, rarity and traits (design sec 11.2).
#[derive(Clone, Copy)]
pub(super) struct Eff {
    pub(super) dmg: i32,
    cd: u16,
    area: i32,
    count: u8,
    pierce: u8,
    glitch: u8,
}

impl<'c> Sim<'c> {
    pub fn step(&mut self, inp: &Input) {
        if self.outcome.is_some() {
            return;
        }
        if let (Some(ch), Some(_)) = (inp.pick, self.offer) {
            self.choose(ch);
        }
        // a bought item is worn at once in its slot (replacing what was there)
        if let (Some(o), Some(n)) = (inp.buy, self.world.open_shop)
            && let Ok((id, left)) = self.world.buy(n, o, self.p.gold)
        {
            self.p.gold = left;
            self.wear(id);
        }
        match inp.bag {
            Some(BagAct::Wear(k)) if usize::from(k) < self.bag.len() => {
                let id = self.bag.remove(usize::from(k));
                self.wear(id);
            }
            Some(BagAct::Discard(k)) if usize::from(k) < self.bag.len() => {
                self.bag.remove(usize::from(k));
            }
            _ => {}
        }
        if inp.forge {
            self.forge();
        }
        let deed = inp.upgrade.and_then(|s| self.upgrade(s)).or_else(|| inp.enchant.and_then(|s| self.enchant(s))).or_else(|| inp.sell.then(|| self.sell()).flatten());
        if let Some(d) = deed {
            self.deed = Some((d, self.tick));
        }
        if let Some(id) = inp.learn {
            self.learn(id);
        }
        // taking an item off puts it in the bag; with a full bag it stays worn (never lost)
        if let Some(slot) = inp.drop
            && self.bag.len() < BAG_SLOTS
            && let Some(id) = self.gear.drop_slot(slot)
        {
            self.bag.push(id);
        }
        if (self.offer.is_some() && self.c.opt.pick_pauses) || (inp.shopping && self.world.open_shop.is_some()) {
            return;
        }
        self.tick += 1;
        self.player_step(inp);
        self.walls.follow(&self.arena, self.p.x, self.p.y);
        // the belly empties over FOOD_LAST_S seconds
        self.p.food = (self.p.food - FOOD_FULL / (FOOD_LAST_S * self.c.hz().get() as i32)).max(0);
        self.p.unhurt = self.p.unhurt.saturating_add(1);
        self.morale_step();
        self.world_step();
        self.grid.build(self.p.x, self.p.y, &self.e.x, &self.e.y, &self.e.rad);
        self.lists();
        self.taunt_spots();
        self.enemies_move();
        self.weapons(inp);
        self.specials(inp);
        self.bolts_step();
        self.mines_step();
        self.shots_step();
        self.boss_step();
        self.tele_step();
        self.contacts();
        self.crew_step();
        self.hijack_step();
        self.status_step();
        self.sweep();
        self.drops_step();
        self.levels();
        self.meter();
        let mut i = 0;
        while i < self.fx.len() {
            self.fx[i].left = self.fx[i].left.saturating_sub(1);
            if self.fx[i].left == 0 {
                self.fx.swap_remove(i);
            } else {
                i += 1;
            }
        }
        // hp only falls in hurt_player, which sets last_hit
        if let (true, Some((cause, foreseen))) = (self.p.hp <= 0, self.last_hit) {
            self.outcome = Some(Outcome::Dead { cause, foreseen });
        } else if self.tick >= self.c.spawn.timeout {
            self.outcome = Some(Outcome::Timeout);
        }
    }

    // ---------- derived stats ----------

    fn trait_val(&self, k: SkillKind) -> i64 {
        self.owned
            .iter()
            .filter(|o| self.c.skill(o.id).kind == k)
            .map(|o| {
                let s = self.c.skill(o.id);
                i64::from(s.base.value) + i64::from(s.per.value) * i64::from(o.power.saturating_sub(1000)) / 1000
            })
            .sum()
    }

    pub(super) fn eff(&self, o: &Owned) -> Eff {
        let s: &Skill = self.c.skill(o.id);
        let extra = i64::from(o.power.saturating_sub(1000));
        let inc_dmg = 1000 + i64::from(s.per.dmg) * extra / 1000 + self.trait_val(SkillKind::Might) + self.stat(Stat::Damage);
        let mut dmg = scale(s.base.dmg, inc_dmg);
        if self.mutant == Some(Mutant::Plus500(o.id)) {
            dmg *= 6;
        }
        let haste = 1000 + i64::from(s.per.haste) * extra / 1000 + self.trait_val(SkillKind::Haste) + self.stat(Stat::Haste);
        let cd = ((i64::from(s.base.cd) * 1000 / haste) as u16).max(3);
        let area_inc = 1000 + i64::from(s.per.area) * extra / 1000 + self.stat(Stat::Area);
        let area = (i64::from(s.base.area) * (area_inc * 1000).isqrt() / 1000) as i32;
        let r5 = o.rank >= self.c.picks.max_rank;
        let add = |b: u8, r: u8| b.saturating_add(if r5 { r } else { 0 });
        Eff { dmg, cd, area, count: add(s.base.count, s.r5.count), pierce: add(s.base.pierce, s.r5.pierce), glitch: add(s.base.glitch, s.r5.glitch) }
    }

    pub fn max_hp(&self) -> i32 {
        (scale(self.c.player.hp, 1000 + self.trait_val(SkillKind::Plating)) + self.stat(Stat::MaxHp) as i32).max(1000)
    }

    pub fn hijack_cap(&self) -> u16 {
        let h = &self.c.hijack;
        let rank = self.owned.iter().find(|o| self.c.skill(o.id).kind == SkillKind::Hack).map_or(0, |o| u16::from(o.rank));
        h.cap_base + h.cap_per_rank * rank
    }

    pub(super) fn contact_dmg(&self, i: usize) -> i32 {
        let sp = &self.c.spawn;
        let tier = match self.e.tier[i] {
            Tier::Elite => sp.elite_dmg,
            Tier::Champion => sp.champion_dmg,
            Tier::Trash | Tier::Boss => 1000,
        };
        let d = i64::from(sp.dmg0) * i64::from(sp.dmg_mult[self.sec_ix()]) / 1000 * i64::from(self.c.fam(self.e.fam[i]).dmg) / 1000;
        // thieves that ate loot hit harder: +10% per loot point, at most x3
        let fed = (1000 + LOOT_DMG_PER * i64::from(self.e.loot[i])).min(LOOT_DMG_CAP);
        let rage = if matches!(self.e.status[i], Status::Raging { .. }) { RAGE_DMG } else { 1000 };
        let d = d * rage / 1000;
        self.world.foe_dmg(self.tick).apply((d * i64::from(tier) / 1000 * fed / 1000) as i32)
    }

    /// Analytic single-target build DPS, HP/s (the D of rho = H/D, design sec 11.4).
    pub fn dps_capacity(&self) -> i64 {
        let hz = i64::from(self.c.hz().get());
        let cc = i64::from(self.c.player.crit_chance) + self.stat(Stat::Crit);
        let crit = 1000 + cc * i64::from(self.c.player.crit_bonus) / 1000;
        let mut d = 0i64;
        for o in &self.owned {
            let s = self.c.skill(o.id);
            let f = self.eff(o);
            let hits = match s.kind {
                SkillKind::Bolt | SkillKind::Pulse | SkillKind::Mine => i64::from(f.count),
                SkillKind::Chain | SkillKind::Orbit => 1,
                _ => 0,
            };
            d += i64::from(f.dmg) * hits * hz / i64::from(f.cd.max(1));
        }
        let hij = i64::from(self.hijacked) * i64::from(self.c.spawn.dmg0) * i64::from(self.c.spawn.dmg_mult[self.sec_ix()]) / 1000 * hz
            / i64::from(self.c.player.contact_cd.max(1));
        (d * crit / 1000 + hij) / 1000
    }

    // ---------- player ----------

    fn player_step(&mut self, inp: &Input) {
        let pd = &self.c.player;
        let (mx, my) = (inp.mv.0.signum(), inp.mv.1.signum());
        if (mx, my) != (0, 0) {
            self.p.face = (mx, my);
        }
        let sky = self.world.forecast(self.tick).now;
        let speed = self.hunger_pace().apply(sky.hero_speed().apply(scale(pd.speed, 1000 + self.stat(Stat::Speed)).max(0)));
        let walk = if matches!(self.p.status, Status::Frozen { .. }) { scale(speed, i64::from(HERO_FROZEN_SPEED)) } else { speed };
        let step = if mx != 0 && my != 0 { scale(walk, 707) } else { walk };
        // the wind pushes the hero too (walls stop it like any move)
        let (wx, wy) = self.world.wind(self.tick, self.c.hz().get());
        let (ax, ay) = solid::slide(&self.walls.near(&self.arena), self.p.x, self.p.y, pd.radius, mx * step + wx, my * step + wy);
        self.p.x += ax;
        self.p.y += ay;
        if inp.dodge && self.p.dodge_cd == 0 {
            self.p.dash_left = pd.dash_ticks;
            self.p.dash_dir = self.p.face;
            self.p.iframes = pd.iframes;
            self.p.dodge_cd = pd.dodge_cd;
        }
        if self.p.dash_left > 0 {
            self.p.dash_left -= 1;
            let (dx, dy) = toward(self.p.dash_dir.0, self.p.dash_dir.1, pd.dash);
            let (ax, ay) = solid::slide(&self.walls.near(&self.arena), self.p.x, self.p.y, pd.radius, dx, dy);
            self.p.x += ax;
            self.p.y += ay;
        }
        self.p.iframes = self.p.iframes.saturating_sub(1);
        self.p.dodge_cd = self.p.dodge_cd.saturating_sub(1);
        self.p.max_hp = self.max_hp();
        let regen = i64::from(pd.regen) + mhp_per_tick(self.trait_val(SkillKind::Regen) + self.stat(Stat::Regen), self.c.hz().get());
        self.p.hp = (self.p.hp + regen as i32).min(self.p.max_hp);
        self.p.flux = (self.p.flux + pd.flux_regen).min(pd.flux);
    }

    pub(super) fn hurt_player(&mut self, d: i32, cause: Cause, foreseen: bool) {
        if self.p.iframes > 0 || d <= 0 {
            return;
        }
        // armour: taken = D (1 - A / (A + 5 D)), D in HP (design sec 11.2, [E] UNVERIFIED formula)
        let a = (i64::from(self.c.player.armour) + self.stat(Stat::Armour)).max(0) * 1000;
        let dd = i64::from(d);
        let taken = dd - dd * a / (a + 5 * dd);
        self.p.unhurt = 0;
        self.p.hp -= taken as i32;
        self.stats.taken += taken;
        self.last_hit = Some((cause, foreseen));
    }

    // ---------- enemies ----------

    fn lists(&mut self) {
        self.hij.clear();
        self.auras.clear();
        for i in 0..self.e.len() {
            if self.e.side[i] == Side::Hijacked {
                if self.hij.len() < self.hij.capacity() {
                    self.hij.push(i as u32);
                }
            } else if let FamKind::Mob(m) = &self.c.fam(self.e.fam[i]).kind
                && let MobRole::Aura { radius, ice } = m.role
                && self.auras.len() < self.auras.capacity()
            {
                self.auras.push((self.e.x[i], self.e.y[i], radius, ice));
            }
        }
    }

    fn enemies_move(&mut self) {
        let c = self.c;
        let (px, py) = (self.p.x, self.p.y);
        let n = self.e.len();
        let boss = self.boss_ix();
        let weight = i64::from(c.hijack.enemy_weight);
        let sep = i64::from(c.spawn.separation);
        let view_r2 = i64::from(c.view.view_r).pow(2);
        let sep_r2 = i64::from(c.view.view_r + SEP_MARGIN).pow(2);
        let sec = self.sec_ix();
        let pace = self.foe_pace();
        let (wx, wy) = self.world.wind(self.tick, c.hz().get());
        let mut near = std::mem::take(&mut self.near);
        let e = &mut self.e;
        e.nx.clear();
        e.ny.clear();
        for i in 0..n {
            let (x, y) = (e.x[i], e.y[i]);
            let fam = c.fam(e.fam[i]);
            let dp2 = dist2(px - x, py - y);
            if e.seen[i] == u32::MAX && dp2 <= view_r2 {
                e.seen[i] = self.tick;
            }
            // LOD: off-screen hostiles re-aim every LOD_EVERY ticks (staggered by id), else coast
            if dp2 > sep_r2 && e.side[i] == Side::Hostile && e.tier[i] != Tier::Boss && !(self.tick + e.id[i]).is_multiple_of(LOD_EVERY) {
                // one query, no sliding, and none at all deep inside a tile: an off-screen coaster just stops at a wall
                let (vx, vy) = (e.vx[i] + wx, e.vy[i] + wy);
                let (sx, sy) = if interior(x, y, e.rad[i] + vx.abs().max(vy.abs())) || !solid::blocked(&self.walls.near(&self.arena), x + vx, y + vy, e.rad[i]) { (vx, vy) } else { (0, 0) };
                e.nx.push(x + sx);
                e.ny.push(y + sy);
                continue;
            }
            let (mut tx, mut ty, mut td2) = (px, py, dp2);
            let mut chase = true;
            if e.side[i] == Side::Hostile {
                for &j in &self.hij {
                    let j = j as usize;
                    let d2 = dist2(e.x[j] - x, e.y[j] - y);
                    // weight w < 1 makes a hijacked target less attractive: pick if d < w * d_player
                    if d2 * 1_000_000 < td2 * weight * weight {
                        (tx, ty, td2) = (e.x[j], e.y[j], d2);
                    }
                }
                // a crew taunter near it draws it away from the hero
                for &(ax, ay, ar) in &self.taunts {
                    let d2 = dist2(ax - x, ay - y);
                    if d2 <= i64::from(ar).pow(2) && d2 < td2 {
                        (tx, ty, td2) = (ax, ay, d2);
                    }
                }
                if dp2 <= sep_r2 && (tx, ty) == (px, py) && !e.panic[i] && e.tier[i] != Tier::Boss && matches!(&fam.kind, FamKind::Mob(Mob { role: MobRole::Melee, .. })) {
                    let ((ax, ay), go) = tactic_aim(Tactic::of(e.id[i]), e.id[i], self.tick, (x, y), (px, py), self.p.face);
                    (tx, ty, td2, chase) = (ax, ay, dist2(ax - x, ay - y), go);
                }
                if let FamKind::Mob(Mob { role: MobRole::Ranged { range, .. }, .. }) = &fam.kind {
                    chase = td2 > i64::from(*range).pow(2) * 3 / 4;
                }
                // thief: runs from the player inside its flee radius, else dives for the nearest loot
                let flee = match &fam.kind {
                    FamKind::Mob(Mob { role: MobRole::Thief { flee }, .. }) => Some(*flee),
                    _ if e.panic[i] => Some(PANIC_FLEE),
                    _ => None,
                };
                if let Some(flee) = flee {
                    if dp2 <= i64::from(flee).pow(2) {
                        (tx, ty, td2) = (x + (x - px), y + (y - py), dp2);
                    } else if let Some(k) = (0..self.drops.x.len()).max_by_key(|&k| {
                        // best loot first: value squared over distance squared (a chest is worth a detour)
                        let v = i64::from(match self.drops.kind[k] { Drop::Xp(_) | Drop::Magnet | Drop::Food | Drop::Ore => LOOT_XP, Drop::Chest | Drop::Item(_) => LOOT_CHEST });
                        v * v * 1_000_000_000 / (dist2(self.drops.x[k] - x, self.drops.y[k] - y) + 1)
                    }) {
                        (tx, ty) = (self.drops.x[k], self.drops.y[k]);
                        td2 = dist2(tx - x, ty - y);
                    }
                }
            } else {
                self.grid.query(x, y, c.hijack.seek, &mut near);
                let mut best = i64::MAX;
                for &j in &near {
                    let j = j as usize;
                    if e.side[j] == Side::Hostile {
                        let d2 = dist2(e.x[j] - x, e.y[j] - y);
                        if d2 < best {
                            (best, tx, ty) = (d2, e.x[j], e.y[j]);
                        }
                    }
                }
                chase = best != i64::MAX || dp2 > 9_000_000;
            }
            let mut speed = i64::from(fam.speed);
            if e.swift[i] {
                speed = speed * i64::from(c.spawn.swift) / 1000;
            }
            if e.tier[i] != Tier::Boss && e.side[i] == Side::Hostile {
                // trash speeds up over the run, never to player speed (cap checked at load)
                speed = (speed * i64::from(c.spawn.speed_mult[sec]) / 1000).min(i64::from(c.spawn.speed_cap));
            }
            let cap = if e.tier[i] == Tier::Boss { 300 } else { 500 };
            speed = speed * (1000 - (i64::from(e.glitch[i]) * 30).min(cap)) / 1000;
            if Ability::Fast.owned_at(e.loot[i]) {
                speed = speed * THIEF_FAST / 1000;
            }
            speed = i64::from(pace.apply(speed as i32));
            speed = match e.status[i] {
                Status::Frozen { .. } => 0,
                Status::Raging { .. } => (speed * RAGE_SPEED / 1000).min(i64::from(c.player.speed) * RAGE_SPEED_CAP / 1000),
                _ => speed,
            };
            let (mut sx, mut sy) = if chase && td2 > i64::from(e.rad[i]).pow(2) { toward(tx - x, ty - y, speed as i32) } else { (0, 0) };
            (e.vx[i], e.vy[i]) = (sx, sy);
            // LOD: off-screen crowds skip separation (probe: separation = 70% of move at 20k)
            if e.tier[i] != Tier::Boss && dp2 <= sep_r2 {
                let g = &self.grid;
                for range in g.ranges(x, y, e.rad[i] + MOB_R_MAX) {
                    for k in range {
                        let j = g.items[k] as usize;
                        if j == i || Some(j) == boss {
                            continue;
                        }
                        let p = g.at[k];
                        let rr = e.rad[i] + p.r;
                        let (dx, dy) = (x - p.x, y - p.y);
                        let d2 = dist2(dx, dy);
                        let rr2 = i64::from(rr).pow(2);
                        if d2 < rr2 {
                            // soft push, sqrt-free: (dx,dy) * (1 - d^2/rr^2) * sep/2 (probe: isqrt+div per pair = locus)
                            let f = (rr2 - d2) * sep / SEP_RR2;
                            if d2 == 0 {
                                sx += if i < j { rr / 4 } else { -rr / 4 };
                            } else {
                                sx += (i64::from(dx) * f / 2000) as i32;
                                sy += (i64::from(dy) * f / 2000) as i32;
                            }
                        }
                    }
                }
                if let Some(b) = boss {
                    let rr = e.rad[i] + e.rad[b];
                    let (dx, dy) = (x - e.x[b], y - e.y[b]);
                    let d = dist(dx, dy);
                    if d < rr {
                        let (ux, uy) = toward(dx, dy, rr - d);
                        sx += ux;
                        sy += uy;
                    }
                }
            }
            let (sx, sy) = (sx + wx, sy + wy);
            let (sx, sy) = if interior(x, y, e.rad[i] + sx.abs().max(sy.abs())) {
                (sx, sy)
            } else {
                let near = self.walls.near(&self.arena);
                let slid = solid::slide(&near, x, y, e.rad[i], sx, sy);
                // a wall in the way: walk the flow field around it (toward the hero through the doorways)
                match (chase && slid != (sx, sy)).then(|| self.walls.flow_next(x, y)).flatten() {
                    Some((fx, fy)) => {
                        let (ax, ay) = toward(fx - x, fy - y, dist(sx, sy).max(1));
                        // off-screen coasters reuse this heading until the next re-aim: make it the way round
                        (e.vx[i], e.vy[i]) = (ax, ay);
                        solid::slide(&near, x, y, e.rad[i], ax, ay)
                    }
                    None => slid,
                }
            };
            e.nx.push(x + sx);
            e.ny.push(y + sy);
        }
        std::mem::swap(&mut e.x, &mut e.nx);
        std::mem::swap(&mut e.y, &mut e.ny);
        self.near = near;
        let far2 = i64::from(c.spawn.despawn).pow(2);
        for i in 0..n {
            if self.e.side[i] == Side::Hostile && self.e.tier[i] != Tier::Boss && dist2(self.e.x[i] - px, self.e.y[i] - py) > far2 {
                let (x, y) = self.place(Formation::Scatter, 0, 0, 1);
                (self.e.x[i], self.e.y[i], self.e.seen[i]) = (x, y, u32::MAX);
            }
        }
    }

    fn ice(&self, i: usize) -> Option<u32> {
        let FamKind::Mob(m) = &self.c.fam(self.e.fam[i]).kind else { return None };
        let base = match self.e.tier[i] {
            Tier::Trash => m.ice,
            Tier::Elite => self.c.hijack.elite_ice,
            Tier::Champion => self.c.hijack.champion_ice,
            Tier::Boss => return None,
        };
        let (x, y) = (self.e.x[i], self.e.y[i]);
        let aura: u32 = self.auras.iter().filter(|a| dist2(a.0 - x, a.1 - y) <= i64::from(a.2).pow(2)).map(|a| u32::from(a.3)).sum();
        Some(u32::from(base) + aura)
    }

    fn add_glitch(&mut self, i: usize, n: u8) {
        if n == 0 || self.e.side[i] != Side::Hostile || self.e.hp[i] <= 0 {
            return;
        }
        self.e.glitch[i] = self.e.glitch[i].saturating_add(n).min(50);
        let Some(ice) = self.ice(i) else { return };
        if u32::from(self.e.glitch[i]) < ice {
            return;
        }
        if self.hijacked >= self.hijack_cap() {
            // over cap: the oldest hijacked bursts early (design sec 10.3)
            let oldest = (0..self.e.len()).filter(|&j| self.e.side[j] == Side::Hijacked && self.e.timer[j] > 0).min_by_key(|&j| self.e.timer[j]);
            match oldest {
                Some(j) => self.e.timer[j] = 0,
                None => return,
            }
        }
        self.take_over(i);
    }

    /// Foe `i` fights for the hero for its family's hijack time (a crew necro raises foes the same way).
    pub(super) fn take_over(&mut self, i: usize) {
        let h = &self.c.hijack;
        let dur = match self.e.tier[i] {
            Tier::Elite => h.elite_s,
            Tier::Champion => h.champion_s,
            Tier::Trash | Tier::Boss => match &self.c.fam(self.e.fam[i]).kind {
                FamKind::Mob(m) => m.hijack,
                FamKind::Boss { .. } => 0,
            },
        };
        self.e.side[i] = Side::Hijacked;
        self.e.timer[i] = dur.max(1);
        self.e.glitch[i] = 0;
        self.hijacked += 1;
        self.stats.hijacked_peak = self.stats.hijacked_peak.max(self.hijacked);
    }

    /// Damage a hostile; returns HP removed. `src` None = hijacked army.
    pub(super) fn hurt(&mut self, i: usize, dmg: i32, hue: Hue, src: Option<SkillId>) -> i32 {
        if self.e.side[i] != Side::Hostile || self.e.hp[i] <= 0 || dmg <= 0 {
            return 0;
        }
        let fam = self.c.fam(self.e.fam[i]);
        let res = match (fam.hue == hue, self.e.tier[i]) {
            (false, _) => 0,
            (true, Tier::Boss) => 250,
            (true, _) => 500,
        };
        let cc = (i64::from(self.c.player.crit_chance) + self.stat(Stat::Crit)).max(0) as u32;
        let crits = cc / 1000 + u32::from(self.rng_combat.chance(cc % 1000));
        let d = i64::from(dmg) * (1000 + i64::from(crits) * i64::from(self.c.player.crit_bonus)) / 1000 * (1000 - res) / 1000;
        let actual = d.min(i64::from(self.e.hp[i])).max(0);
        let hz = self.c.hz().get();
        self.e.hp[i] -= d.min(i64::from(i32::MAX)) as i32;
        match src {
            Some(s) => {
                self.stats.dealt[s.ix()] += actual;
                self.stats.output[s.ix()] += d;
            }
            None => self.stats.dealt_hijack += actual,
        }
        if self.e.tier[i] != Tier::Boss && self.e.hp[i] > 0 {
            let st = self.e.status[i].inflict(hue, hz);
            let low = self.e.hp[i] < scale(self.e.max[i], i64::from(RAGE_BELOW));
            self.e.status[i] = if low { st.enrage(hz) } else { st };
        }
        actual as i32
    }

    pub(super) fn fx(&mut self, shape: FxShape, skill: SkillId) {
        if self.fx.len() < self.fx.capacity() {
            self.fx.push(Fx { shape, left: FX_TICKS, skill });
        }
    }

    pub(super) fn nearest_hostile(&self, x: i32, y: i32, range: i32, skip: &[u32]) -> Option<usize> {
        let r2 = i64::from(range).pow(2);
        (0..self.e.len())
            .filter(|&j| self.e.side[j] == Side::Hostile && self.e.hp[j] > 0 && !skip.contains(&self.e.id[j]))
            .map(|j| (dist2(self.e.x[j] - x, self.e.y[j] - y), j))
            .filter(|&(d2, _)| d2 <= r2)
            .min()
            .map(|(_, j)| j)
    }

    /// Centre of the most crowded grid cell within range (Densest targeting, design sec 6.5).
    fn densest(&self, range: i32) -> Option<(i32, i32)> {
        let (a0, b0) = self.grid.cxy(self.p.x - range, self.p.y - range);
        let (a1, b1) = self.grid.cxy(self.p.x + range, self.p.y + range);
        let mut best = (0, None);
        for b in b0..=b1 {
            for a in a0..=a1 {
                let n = self.grid.count(a, b);
                if n > best.0 {
                    let cx = self.grid.ox + a as i32 * GRID_CELL + GRID_CELL / 2;
                    let cy = self.grid.oy + b as i32 * GRID_CELL + GRID_CELL / 2;
                    if dist2(cx - self.p.x, cy - self.p.y) <= i64::from(range).pow(2) {
                        best = (n, Some((cx, cy)));
                    }
                }
            }
        }
        best.1
    }

    // ---------- weapons ----------

    /// Auto-aim point for a skill's targeting mode (design sec 6.5).
    fn aim(&self, t: Target, range: i32) -> Option<(i32, i32)> {
        match t {
            Target::Nearest => self.nearest_hostile(self.p.x, self.p.y, range, &[]).map(|j| (self.e.x[j], self.e.y[j])),
            Target::Densest => self.densest(range),
            Target::SelfCentered => Some((self.p.x, self.p.y)),
        }
    }

    fn weapons(&mut self, inp: &Input) {
        self.blades.clear();
        for k in 0..self.owned.len() {
            let o = self.owned[k];
            let s = self.c.skill(o.id);
            if s.kind.is_trait() {
                continue;
            }
            let f = self.eff(&o);
            if s.kind == SkillKind::Orbit {
                self.orbit(k, &f);
            }
            if o.cd > 0 {
                self.owned[k].cd -= 1;
                continue;
            }
            // auto-fire off: attacks wait for the fire key (orbit blades and traits are passive)
            if !(inp.autofire || inp.fire) && matches!(s.kind, SkillKind::Bolt | SkillKind::Pulse | SkillKind::Chain | SkillKind::Mine) {
                continue;
            }
            let fired = match s.kind {
                SkillKind::Bolt => self.fire_bolts(&o, &f, inp.fire),
                SkillKind::Hack => self.hack(&o, &f, inp),
                SkillKind::Pulse => self.pulse(&o, &f),
                SkillKind::Chain => self.chain(&o, &f),
                SkillKind::Mine => self.drop_mines(&o, &f),
                _ => true,
            };
            if fired {
                self.owned[k].cd = f.cd;
            }
        }
    }

    /// Hunger slows the hero: below FOOD_LOW it walks at HUNGRY_PACE, on an empty belly at STARVED_PACE.
    pub(super) fn hunger_pace(&self) -> Permille {
        Permille(match self.p.food {
            0 => STARVED_PACE,
            f if f < FOOD_LOW => HUNGRY_PACE,
            _ => 1000,
        })
    }

    /// Wear an item; what the slot held goes into the bag, or onto the floor when the bag is full.
    pub(super) fn wear(&mut self, id: ItemId) {
        if let Some(Some(old)) = self.gear.wear(self.c, id) {
            if self.bag.len() < BAG_SLOTS {
                self.bag.push(old);
            } else {
                self.drop_at(self.p.x + 1500, self.p.y, Drop::Item(old));
            }
        }
    }

    /// A skill-tree node grants its skill: a new one takes the next hotkey slot (with its special), one the
    /// hero already has ranks up (up to the pick rules' max rank).
    fn learn(&mut self, id: SkillId) {
        let max = self.c.picks.max_rank;
        if let Some(o) = self.owned.iter_mut().find(|o| o.id == id) {
            o.rank = (o.rank + 1).min(max);
        } else if self.owned.len() < MAX_SKILLS {
            self.owned.push(Owned { id, rank: 1, power: 1000, cd: 0, sp: 0, phase: 0 });
        }
    }

    /// At a blacksmith the hero trades FORGE_ORE ore for a rare-or-better item, worn at once (picked by the tick,
    /// no RNG stream consumed). Elsewhere, or short of ore, nothing happens.
    fn forge(&mut self) {
        let at_smith = self.world.open_shop.and_then(|i| self.world.npcs.get(i)).is_some_and(|p| matches!(p.npc, npc::Npc::Blacksmith { .. }));
        let good: Vec<usize> = (0..self.c.items.len()).filter(|&k| self.c.items[k].rarity >= crate::data::Rarity::Rare).collect();
        if !at_smith || self.p.ore < FORGE_ORE || good.is_empty() {
            return;
        }
        self.p.ore -= FORGE_ORE;
        let k = good[(mix(u64::from(self.tick) ^ ITEM_SALT) % good.len() as u64) as usize];
        self.wear(ItemId(k as u16));
    }

    /// One stat over worn pieces plus the legacy's boon, in the stat's data units.
    pub fn stat(&self, s: Stat) -> i64 {
        self.gear.sum(self.c, s) + self.boons.stat(s, self.c.player.hp)
    }

    pub(super) fn gain_xp(&mut self, v: u16) {
        let milli = u32::from(v) * (1000 + self.boons.xp) + self.p.xp_frac;
        self.p.xp += milli / 1000;
        self.p.xp_frac = milli % 1000;
    }

    /// Start the run with the legacy's boons: more HP (full), starting gold.
    pub fn bless(&mut self, b: crate::legacy::Boons) {
        self.boons = b;
        self.p.gold = self.p.gold.saturating_add(b.gold);
        self.p.max_hp = self.max_hp();
        self.p.hp = self.p.max_hp;
    }

    /// The keeper the hero stands at, if any.
    fn keeper(&self) -> Option<npc::Npc> {
        self.world.open_shop.and_then(|i| self.world.npcs.get(i)).map(|p| p.npc)
    }

    /// Blacksmith: raise the piece in `slot` one plus for gold and ore. A deceiving smith botches one job in
    /// BOTCH_1_IN (by tick) and keeps the pay.
    fn upgrade(&mut self, slot: crate::data::Slot) -> Option<Deed> {
        let Some(npc::Npc::Blacksmith { honest }) = self.keeper() else { return None };
        let Some(p) = self.gear.piece(slot) else { return Some(Deed::NoPiece) };
        if p.plus.get() >= MAX_PLUS {
            return Some(Deed::Maxed);
        }
        let step = u32::from(p.plus.get()) + 1;
        let (gold, ore) = (UPGRADE_GOLD * step, step);
        if self.p.gold < gold || self.p.ore < ore {
            return Some(Deed::Poor { gold, ore });
        }
        self.p.gold -= gold;
        self.p.ore -= ore;
        if !honest && mix(u64::from(self.tick) ^ ITEM_SALT).is_multiple_of(BOTCH_1_IN) {
            return Some(Deed::Botched { slot });
        }
        Some(match self.gear.upgrade(slot) {
            Ok(plus) => Deed::Upgraded { slot, plus },
            Err(_) => Deed::Maxed,
        })
    }

    /// Jeweler: put one enchantment (picked by the tick from ENCHANTS) on the piece in `slot` for gold. A
    /// deceiving jeweler's enchantment is a curse: the same stat, negative.
    fn enchant(&mut self, slot: crate::data::Slot) -> Option<Deed> {
        let Some(npc::Npc::Jeweler { honest }) = self.keeper() else { return None };
        if self.gear.piece(slot).is_none() {
            return Some(Deed::NoPiece);
        }
        if self.p.gold < ENCHANT_GOLD {
            return Some(Deed::Poor { gold: ENCHANT_GOLD, ore: 0 });
        }
        self.p.gold -= ENCHANT_GOLD;
        let (stat, value) = ENCHANTS[(mix(u64::from(self.tick) ^ ITEM_SALT) % ENCHANTS.len() as u64) as usize];
        let m = crate::data::Mod { stat, value: if honest { value } else { -value } };
        self.gear.enchant(slot, m);
        self.p.max_hp = self.max_hp();
        Some(Deed::Enchanted { slot, m })
    }

    /// Merchant: sell the first bag item for SELL_PERMILLE of its fair price.
    fn sell(&mut self) -> Option<Deed> {
        let Some(npc::Npc::Merchant { .. }) = self.keeper() else { return None };
        if self.bag.is_empty() {
            return Some(Deed::NoPiece);
        }
        let item = self.bag.remove(0);
        let gold = world::shop::resale(self.c, item);
        self.p.gold = self.p.gold.saturating_add(gold);
        Some(Deed::Sold { item, gold })
    }

    fn eat(&mut self) {
        self.p.food = (self.p.food + FOOD_MEAL).min(FOOD_FULL);
    }

    /// How fast the horde runs this tick (permille): the sky sets it, a hero sheltering from rain or acid draws
    /// them faster, and night speeds them up.
    pub(super) fn foe_pace(&self) -> Permille {
        let sky = self.world.forecast(self.tick).now;
        let drawn = matches!(sky, Weather::Rain | Weather::AcidRain) && super::weather::sheltered(&self.arena, self.p.x, self.p.y);
        let pace = if drawn { sky.foe_speed().0 * SHELTER_DRAW / 1000 } else { sky.foe_speed().0 };
        Permille(if self.night() { pace * NIGHT_PACE / 1000 } else { pace })
    }

    /// Ticks a special of rank `rank` takes to reload: SPECIAL_RELOAD_S, shorter by a quarter per rank above 1.
    pub fn special_reload(&self, rank: u8) -> u16 {
        let base = u64::from(self.c.hz().get()) * SPECIAL_RELOAD_S;
        (base * 1000 / (1000 + 250 * u64::from(rank.saturating_sub(1)))).min(u64::from(u16::MAX)) as u16
    }

    /// Number keys: slot n casts its power's special (a much stronger shot of the same weapon) when reloaded.
    /// Passive powers (traits, orbit) have none. A special with no target to hit does not spend the reload.
    fn specials(&mut self, inp: &Input) {
        for o in &mut self.owned {
            o.sp = o.sp.saturating_sub(1);
        }
        let Some(k) = inp.special.map(usize::from) else { return };
        let tried = self.try_special(k);
        self.special_try = Some((tried, self.tick));
    }

    /// Cast slot k's special if it can: what happened, for the face to answer.
    fn try_special(&mut self, k: usize) -> SpecialTry {
        let Some(o) = self.owned.get(k).copied() else { return SpecialTry::Empty };
        if !self.c.skill(o.id).kind.has_special() {
            return SpecialTry::Passive;
        }
        if o.sp > 0 {
            return SpecialTry::Reloading;
        }
        let f = self.eff(&o);
        let big = Eff { dmg: f.dmg.saturating_mul(SPECIAL_DMG), area: f.area.saturating_mul(2), count: f.count.saturating_mul(3), pierce: f.pierce.saturating_add(2), ..f };
        let cast = match self.c.skill(o.id).kind {
            // a full ring: 36 bolts, one every 10 degrees
            SkillKind::Bolt => self.fire_bolts(&o, &Eff { count: 36, ..big }, true),
            SkillKind::Hack => self.hack(&o, &big, &Input { hack: true, ..Input::default() }),
            SkillKind::Pulse => self.pulse(&o, &big),
            SkillKind::Chain => self.chain(&o, &big),
            SkillKind::Mine => self.drop_mines(&o, &big),
            _ => false,
        };
        if !cast {
            return SpecialTry::NoTarget;
        }
        self.owned[k].sp = self.special_reload(o.rank);
        SpecialTry::Cast
    }

    /// Bolts at the target; with nothing in range only the held fire key shoots, along the facing.
    fn fire_bolts(&mut self, o: &Owned, f: &Eff, held: bool) -> bool {
        let s = self.c.skill(o.id);
        let (px, py) = (self.p.x, self.p.y);
        let dir = match self.aim(s.target, s.base.range) {
            Some((x, y)) => (x - px, y - py),
            None if held => (self.p.face.0 * 1000, self.p.face.1 * 1000),
            None => return false,
        };
        let (vx, vy) = toward(dir.0, dir.1, s.base.speed);
        let ttl = (s.base.range / s.base.speed.max(1)) as u16;
        for k in 0..f.count {
            if self.bolts.x.len() >= self.bolts.x.capacity() {
                break;
            }
            // spread: 0, +10deg, -10deg, +20deg ...
            let step = (i32::from(k) + 1) / 2 * if k % 2 == 0 { 1 } else { -1 };
            let (c, sn) = self.trig[(step.rem_euclid(36) as u32 * TRIG_N / 36) as usize];
            let rx = (i64::from(vx) * i64::from(c) - i64::from(vy) * i64::from(sn)) / 1000;
            let ry = (i64::from(vx) * i64::from(sn) + i64::from(vy) * i64::from(c)) / 1000;
            let b = &mut self.bolts;
            b.x.push(px);
            b.y.push(py);
            b.vx.push(rx as i32);
            b.vy.push(ry as i32);
            b.ttl.push(ttl);
            b.pierce.push(f.pierce);
            b.dmg.push(f.dmg);
            b.last.push(u32::MAX);
            b.skill.push(o.id);
            b.gl.push(f.glitch);
        }
        true
    }

    fn hack(&mut self, o: &Owned, f: &Eff, inp: &Input) -> bool {
        let s = self.c.skill(o.id);
        if self.p.flux < s.base.flux {
            return false;
        }
        let (px, py) = (self.p.x, self.p.y);
        let (dx, dy) = if inp.autofire && !inp.hack {
            match self.aim(s.target, f.area) {
                Some((x, y)) => (x - px, y - py),
                None => return false,
            }
        } else if inp.hack {
            (self.p.face.0 * 1000, self.p.face.1 * 1000)
        } else {
            return false;
        };
        let (ux, uy) = toward(dx, dy, 1000);
        let mut near = std::mem::take(&mut self.near);
        self.grid.query(px, py, f.area + MOB_R_MAX, &mut near);
        let r2 = i64::from(f.area).pow(2);
        for &j in &near {
            let j = j as usize;
            let (ex, ey) = (self.e.x[j] - px, self.e.y[j] - py);
            let d2 = dist2(ex, ey);
            let d = isqrt(d2);
            if d2 <= r2 && (d == 0 || (i64::from(ex) * i64::from(ux) + i64::from(ey) * i64::from(uy)) >= i64::from(s.base.arc_cos) * d) {
                self.add_glitch(j, f.glitch);
            }
        }
        self.near = near;
        self.p.flux -= s.base.flux;
        self.fx(FxShape::Cone { x: px, y: py, dx: ux, dy: uy, r: f.area, cos: s.base.arc_cos }, o.id);
        true
    }

    pub(super) fn area_hit(&mut self, x: i32, y: i32, r: i32, dmg: i32, hue: Hue, src: Option<SkillId>) {
        let mut near = std::mem::take(&mut self.near);
        self.grid.query(x, y, r + MOB_R_MAX, &mut near);
        for &j in &near {
            let j = j as usize;
            if dist2(self.e.x[j] - x, self.e.y[j] - y) <= i64::from(r + self.e.rad[j]).pow(2) {
                self.hurt(j, dmg, hue, src);
            }
        }
        self.near = near;
        if let Some(b) = self.boss_ix()
            && dist(self.e.x[b] - x, self.e.y[b] - y) <= r + self.e.rad[b]
        {
            self.hurt(b, dmg, hue, src);
        }
    }

    fn pulse(&mut self, o: &Owned, f: &Eff) -> bool {
        let hue = self.c.skill(o.id).hue;
        let (px, py) = (self.p.x, self.p.y);
        for _ in 0..f.count {
            self.area_hit(px, py, f.area, f.dmg, hue, Some(o.id));
        }
        self.fx(FxShape::Ring { x: px, y: py, r: f.area }, o.id);
        true
    }

    fn chain(&mut self, o: &Owned, f: &Eff) -> bool {
        let s = self.c.skill(o.id);
        let (mut x, mut y) = (self.p.x, self.p.y);
        let mut hit = [u32::MAX; 16];
        let mut range = s.base.range;
        let mut dmg = f.dmg;
        for k in 0..usize::from(f.count).min(16) {
            let Some(j) = self.nearest_hostile(x, y, range, &hit[..k]) else { return k > 0 };
            let (jx, jy) = (self.e.x[j], self.e.y[j]);
            self.fx(FxShape::Line { x0: x, y0: y, x1: jx, y1: jy }, o.id);
            self.hurt(j, dmg, s.hue, Some(o.id));
            hit[k] = self.e.id[j];
            (x, y, range) = (jx, jy, JUMP_R);
            dmg = scale(dmg, CHAIN_FALLOFF);
        }
        true
    }

    fn orbit(&mut self, k: usize, f: &Eff) {
        let o = self.owned[k];
        let s = self.c.skill(o.id);
        // blade angular speed from linear speed at radius: turns/tick = v / (2 pi r)
        let dphase = (i64::from(s.base.speed) * i64::from(TRIG_N) * 1000 / (6283 * i64::from(f.area.max(1)))).max(1) as u32;
        self.owned[k].phase = (o.phase + dphase) % TRIG_N;
        let n = u32::from(f.count.max(1));
        for b in 0..n {
            let (c, sn) = self.trig[((o.phase + b * TRIG_N / n) % TRIG_N) as usize];
            let (bx, by) = (self.p.x + scale(c, i64::from(f.area)), self.p.y + scale(sn, i64::from(f.area)));
            if self.blades.len() < self.blades.capacity() {
                self.blades.push((bx, by));
            }
            let mut near = std::mem::take(&mut self.near);
            self.grid.query(bx, by, 500 + MOB_R_MAX, &mut near);
            for &j in &near {
                let j = j as usize;
                if self.e.orb_cd[j] == 0 && dist2(self.e.x[j] - bx, self.e.y[j] - by) <= i64::from(500 + self.e.rad[j]).pow(2) && self.hurt(j, f.dmg, s.hue, Some(o.id)) > 0 {
                    self.e.orb_cd[j] = f.cd.min(255) as u8;
                }
            }
            self.near = near;
        }
    }

    fn drop_mines(&mut self, o: &Owned, f: &Eff) -> bool {
        let s = self.c.skill(o.id);
        for k in 0..f.count {
            if self.mines.len() >= self.mines.capacity() {
                return k > 0;
            }
            self.mines.push(Mine { x: self.p.x + i32::from(k) * 800, y: self.p.y, ttl: s.base.duration, dmg: f.dmg, r: f.area, skill: o.id });
        }
        true
    }

    fn mines_step(&mut self) {
        let mut i = 0;
        while i < self.mines.len() {
            let m = self.mines[i];
            let trip = self.nearest_hostile(m.x, m.y, MINE_TRIGGER, &[]).is_some();
            if trip || m.ttl == 0 {
                let hue = self.c.skill(m.skill).hue;
                self.area_hit(m.x, m.y, m.r, m.dmg, hue, Some(m.skill));
                self.fx(FxShape::Ring { x: m.x, y: m.y, r: m.r }, m.skill);
                self.mines.swap_remove(i);
            } else {
                self.mines[i].ttl -= 1;
                i += 1;
            }
        }
    }

    pub fn mines(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.mines.iter().map(|m| (m.x, m.y))
    }

    pub fn bolts(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.bolts.x.iter().zip(&self.bolts.y).map(|(&x, &y)| (x, y))
    }

    fn bolts_step(&mut self) {
        let boss = self.boss_ix();
        let mut near = std::mem::take(&mut self.near);
        let mut i = 0;
        while i < self.bolts.x.len() {
            self.bolts.x[i] += self.bolts.vx[i];
            self.bolts.y[i] += self.bolts.vy[i];
            let (bx, by) = (self.bolts.x[i], self.bolts.y[i]);
            let mut dead = self.bolts.ttl[i] == 0;
            self.bolts.ttl[i] = self.bolts.ttl[i].saturating_sub(1);
            self.grid.query(bx, by, BOLT_R + MOB_R_MAX, &mut near);
            if let Some(b) = boss {
                near.push(b as u32);
            }
            for &j in &near {
                let j = j as usize;
                if dead || self.e.side[j] != Side::Hostile || self.e.hp[j] <= 0 || self.e.id[j] == self.bolts.last[i] {
                    continue;
                }
                let r = BOLT_R + self.e.rad[j];
                if dist2(self.e.x[j] - bx, self.e.y[j] - by) <= i64::from(r).pow(2) {
                    let s = self.bolts.skill[i];
                    self.hurt(j, self.bolts.dmg[i], self.c.skill(s).hue, Some(s));
                    self.add_glitch(j, self.bolts.gl[i]);
                    self.bolts.last[i] = self.e.id[j];
                    if self.bolts.pierce[i] == 0 {
                        dead = true;
                    } else {
                        self.bolts.pierce[i] -= 1;
                    }
                }
            }
            if dead {
                let b = &mut self.bolts;
                for v in [&mut b.x, &mut b.y, &mut b.vx, &mut b.vy, &mut b.dmg] {
                    v.swap_remove(i);
                }
                b.ttl.swap_remove(i);
                b.pierce.swap_remove(i);
                b.last.swap_remove(i);
                b.skill.swap_remove(i);
                b.gl.swap_remove(i);
            } else {
                i += 1;
            }
        }
        self.near = near;
    }

    /// Shot parameters of foe i: its Ranged role, or a thief's learned Shot.
    fn shooter(&self, i: usize) -> Option<(i32, i32, u16, u32)> {
        match self.c.fam(self.e.fam[i]).kind {
            FamKind::Mob(Mob { role: MobRole::Ranged { range, shot_v, every, dmg }, .. }) => Some((range, shot_v, every, dmg)),
            _ if Ability::Shot.owned_at(self.e.loot[i]) => Some(THIEF_SHOT),
            _ => None,
        }
    }

    fn shots_step(&mut self) {
        // fire
        let lead_min = self.c.hz().ticks(0.4) as i32;
        for i in 0..self.e.len() {
            if self.e.side[i] != Side::Hostile {
                continue;
            }
            let Some((range, shot_v, every, dmg)) = self.shooter(i) else { continue };
            self.e.timer[i] = self.e.timer[i].saturating_sub(1);
            let (dx, dy) = (self.p.x - self.e.x[i], self.p.y - self.e.y[i]);
            let d = dist(dx, dy);
            // trash shots: min travel 0.4 s (design sec 8.4)
            if self.e.timer[i] == 0 && d <= range && d >= shot_v * lead_min && self.shots.x.len() < self.shots.x.capacity() {
                self.e.timer[i] = every;
                let (vx, vy) = toward(dx, dy, shot_v);
                let sd = scale(self.contact_dmg(i), i64::from(dmg));
                let s = &mut self.shots;
                s.x.push(self.e.x[i]);
                s.y.push(self.e.y[i]);
                s.vx.push(vx);
                s.vy.push(vy);
                s.ttl.push((range * 3 / 2 / shot_v.max(1)) as u16);
                s.age.push(0);
                s.dmg.push(sd);
                s.fam.push(self.e.fam[i]);
            }
        }
        // move + hit
        let r = 200 + self.c.player.radius;
        let mut i = 0;
        while i < self.shots.x.len() {
            self.shots.x[i] += self.shots.vx[i];
            self.shots.y[i] += self.shots.vy[i];
            self.shots.age[i] += 1;
            let hit = dist2(self.shots.x[i] - self.p.x, self.shots.y[i] - self.p.y) <= i64::from(r).pow(2) && self.p.iframes == 0;
            if hit {
                let foreseen = i32::from(self.shots.age[i]) >= lead_min;
                self.hurt_player(self.shots.dmg[i], Cause::Shot { fam: self.shots.fam[i] }, foreseen);
            }
            if hit || self.shots.ttl[i] == 0 {
                let s = &mut self.shots;
                for v in [&mut s.x, &mut s.y, &mut s.vx, &mut s.vy, &mut s.dmg] {
                    v.swap_remove(i);
                }
                s.ttl.swap_remove(i);
                s.age.swap_remove(i);
                s.fam.swap_remove(i);
            } else {
                self.shots.ttl[i] -= 1;
                i += 1;
            }
        }
    }

    // ---------- boss + telegraphs ----------

    fn boss_step(&mut self) {
        let Some(b) = self.boss_ix() else { return };
        let Some(st) = self.boss.as_mut() else { return };
        let Some(def) = self.c.boss_of(st.fam) else { return };
        st.age += 1;
        let hp_pm = (i64::from(self.e.hp[b].max(0)) * 1000 / i64::from(self.e.max[b].max(1))) as u32;
        let mut changed = false;
        while st.phase + 1 < def.phases.len() && hp_pm < def.phases[st.phase].until {
            st.phase += 1;
            st.timers = [0; 4];
            changed = true;
        }
        let enraged = st.age >= def.enrage;
        let mut fire = [None; 5];
        for (k, a) in def.phases[st.phase].attacks.iter().take(4).enumerate() {
            st.timers[k] += 1;
            let every = if enraged { scale(i32::from(a.every), i64::from(def.enrage_cadence)) } else { i32::from(a.every) };
            if i32::from(st.timers[k]) >= every.max(1) {
                st.timers[k] = 0;
                fire[k] = Some(*a);
            }
        }
        if st.age >= def.hardcap {
            st.hard_t += 1;
            if st.hard_t >= def.hard.every {
                st.hard_t = 0;
                fire[4] = Some(def.hard);
            }
        }
        let fam = st.fam;
        if changed {
            // boss phase change clears hijacked (bursts still fire, design sec 10.3)
            for j in 0..self.e.len() {
                if self.e.side[j] == Side::Hijacked {
                    self.e.timer[j] = 0;
                }
            }
        }
        for a in fire.into_iter().flatten() {
            self.launch(a, fam, def.ref_hp, (self.e.x[b], self.e.y[b]));
        }
    }

    pub(super) fn tele(&mut self, shape: Shape, lead: u16, kind: TeleKind) {
        if self.tele.len() < self.tele.capacity() {
            self.tele.push(Tele { shape, left: lead, lead, kind });
        }
    }

    fn launch(&mut self, a: crate::data::Attack, fam: FamId, ref_hp: i32, (bx, by): (i32, i32)) {
        let lead = if self.mutant == Some(Mutant::ZeroTelegraph) { 0 } else { a.lead.ticks() };
        let (px, py) = (self.p.x, self.p.y);
        match a.what {
            AttackKind::Spawn { fam: f, n } => {
                let (dx, dy) = toward(px - bx, py - by, 2500);
                self.tele(Shape::Circle { x: bx + dx, y: by + dy, r: 1500 }, lead, TeleKind::Spawn { fam: f, n });
            }
            AttackKind::Area { shape, dmg } => {
                let kind = TeleKind::HitPlayer { dmg: scale(ref_hp, i64::from(dmg)), cause: Cause::BossAttack { fam } };
                let flip = self.rng_combat.chance(500);
                match shape {
                    ShapeDef::Band { half } => self.tele(Shape::Band { along_x: flip, at: if flip { py } else { px }, half }, lead, kind),
                    ShapeDef::Cross { half } => {
                        self.tele(Shape::Band { along_x: true, at: py, half }, lead, kind);
                        self.tele(Shape::Band { along_x: false, at: px, half }, lead, kind);
                    }
                    ShapeDef::Lattice { pitch, half } => self.tele(Shape::Lattice { x: px, y: py, pitch, half }, lead, kind),
                    ShapeDef::Circle { r } => self.tele(Shape::Circle { x: px, y: py, r }, lead, kind),
                    ShapeDef::Fan { n, r, spread } => {
                        let (ux, uy) = toward(px - bx, py - by, 1000);
                        for k in 0..i32::from(n) {
                            let off = (k - i32::from(n) / 2) * spread / 1000;
                            self.tele(Shape::Circle { x: px - uy * off, y: py + ux * off, r }, lead, kind);
                        }
                    }
                    ShapeDef::Arena => self.tele(Shape::Arena, lead, kind),
                }
            }
        }
    }

    fn tele_step(&mut self) {
        // elite PURGER pulses (design sec 10.3)
        let (every, lead, r) = (self.c.spawn.purge_every, self.c.spawn.purge_lead, self.c.spawn.purge_r);
        for i in 0..self.e.len() {
            if self.e.purger[i] && self.e.side[i] == Side::Hostile {
                self.e.cd[i] = self.e.cd[i].saturating_sub(1);
                if self.e.cd[i] == 0 {
                    self.e.cd[i] = every;
                    let lead = if self.mutant == Some(Mutant::ZeroTelegraph) { 0 } else { lead };
                    self.tele(Shape::Circle { x: self.e.x[i], y: self.e.y[i], r }, lead, TeleKind::PurgeHijacked);
                }
            }
        }
        let half_s = (self.c.hz().get() / 2) as u16;
        let mut i = 0;
        while i < self.tele.len() {
            let t = self.tele[i];
            if t.left > 0 {
                self.tele[i].left -= 1;
                i += 1;
                continue;
            }
            match t.kind {
                TeleKind::HitPlayer { dmg, cause } => {
                    if t.shape.contains(self.p.x, self.p.y) {
                        self.hurt_player(dmg, cause, t.lead >= half_s);
                    }
                }
                TeleKind::PurgeHijacked => {
                    for j in 0..self.e.len() {
                        if self.e.side[j] == Side::Hijacked && t.shape.contains(self.e.x[j], self.e.y[j]) {
                            self.e.hp[j] = 0;
                        }
                    }
                }
                TeleKind::Spawn { fam, n } => {
                    if let (Shape::Circle { x, y, .. }, Some(n)) = (t.shape, NonZeroU16::new(u16::from(n))) {
                        let cmd = Cmd::Spawn { fam, n, tier: Tier::Trash, form: Formation::Scatter };
                        if self.validate(&cmd).is_ok() {
                            for k in 0..i32::from(n.get()) {
                                self.spawn_mob(fam, Tier::Trash, x + (k - 1) * 900, y);
                            }
                        }
                    }
                }
            }
            self.tele.swap_remove(i);
        }
    }

    // ---------- contact, hijack, deaths, pickups ----------

    fn contacts(&mut self) {
        let cd = self.c.player.contact_cd;
        let (px, py, pr) = (self.p.x, self.p.y, self.c.player.radius);
        let mut near = std::mem::take(&mut self.near);
        self.grid.query(px, py, pr + MOB_R_MAX, &mut near);
        if let Some(b) = self.boss_ix() {
            near.push(b as u32);
        }
        for &j in &near {
            let j = j as usize;
            let r = pr + self.e.rad[j];
            if self.e.side[j] == Side::Hostile && self.e.hit_cd[j] == 0 && dist2(self.e.x[j] - px, self.e.y[j] - py) <= i64::from(r).pow(2) {
                self.e.hit_cd[j] = cd;
                let foreseen = self.e.seen[j] != u32::MAX && self.tick.saturating_sub(self.e.seen[j]) >= self.c.hz().get() / 2;
                let cause = Cause::Contact { fam: self.e.fam[j], tier: self.e.tier[j] };
                let hp0 = self.p.hp;
                self.hurt_player(self.contact_dmg(j), cause, foreseen);
                if self.p.hp < hp0 {
                    self.p.status = self.p.status.inflict(self.c.fam(self.e.fam[j]).hue, self.c.hz().get());
                }
            }
        }
        // hijacked army vs hostiles, both ways
        let power = 1000 + i64::from(self.c.hijack.dmg_mult);
        let code = Hue::Code;
        for h in 0..self.hij.len() {
            let i = self.hij[h] as usize;
            let ri = self.e.rad[i];
            self.grid.query(self.e.x[i], self.e.y[i], ri + MOB_R_MAX, &mut near);
            for &j in &near {
                let j = j as usize;
                if self.e.side[j] != Side::Hostile {
                    continue;
                }
                let r = ri + self.e.rad[j];
                if dist2(self.e.x[j] - self.e.x[i], self.e.y[j] - self.e.y[i]) > i64::from(r).pow(2) {
                    continue;
                }
                if self.e.hit_cd[i] == 0 {
                    self.e.hit_cd[i] = cd;
                    let d = scale(self.contact_dmg(i), power);
                    self.hurt(j, d, code, None);
                }
                if self.e.hit_cd[j] == 0 {
                    self.e.hit_cd[j] = cd;
                    self.e.hp[i] -= self.contact_dmg(j);
                }
            }
        }
        self.near = near;
        for v in self.e.hit_cd.iter_mut().chain(self.e.orb_cd.iter_mut()) {
            *v = v.saturating_sub(1);
        }
    }

    fn hijack_step(&mut self) {
        let decay = self.tick.is_multiple_of(u32::from(self.c.hijack.glitch_decay));
        for i in 0..self.e.len() {
            if decay {
                self.e.glitch[i] = self.e.glitch[i].saturating_sub(1);
            }
            if self.e.side[i] != Side::Hijacked || self.e.hp[i] <= 0 {
                continue;
            }
            if self.e.timer[i] > 0 {
                self.e.timer[i] -= 1;
                continue;
            }
            // DATA BURST on expiry: 50% max HP as CODE (design sec 10.3)
            let d = scale(self.e.max[i], i64::from(self.c.hijack.burst_hp));
            let (x, y) = (self.e.x[i], self.e.y[i]);
            self.e.hp[i] = 0;
            self.area_hit(x, y, self.c.hijack.burst_r, d, Hue::Code, None);
            self.fx(FxShape::Ring { x, y, r: self.c.hijack.burst_r }, self.owned[0].id);
        }
    }

    /// An item from the catalog picked by the foe's id (deterministic, no RNG stream consumed).
    fn drop_item(&mut self, x: i32, y: i32, id: u32) {
        let n = self.c.items.len() as u64;
        if n > 0 {
            self.drop_at(x, y, Drop::Item(ItemId((mix(u64::from(id) ^ ITEM_SALT.rotate_left(17)) % n) as u16)));
        }
    }

    fn drop_at(&mut self, x: i32, y: i32, kind: Drop) {
        if self.drops.x.len() >= self.drops.x.capacity() {
            // no room on the floor: the loot goes straight to the hero
            match kind {
                Drop::Xp(v) => self.gain_xp(v),
                Drop::Chest => self.pending_chest += 1,
                Drop::Item(id) => {
                    self.wear(id);
                }
                Drop::Magnet => self.magnet_pull(),
                Drop::Food => self.eat(),
                Drop::Ore => self.p.ore += 1,
            }
            return;
        }
        self.drops.x.push(x);
        self.drops.y.push(y);
        self.drops.kind.push(kind);
        self.drops.pulled.push(false);
    }

    /// Timers run down; burning bites once a second. Burn damage is not credited to a skill, and the
    /// hero's burn alone never kills (hp floors at 1, like weather).
    fn status_step(&mut self) {
        let hz = self.c.hz().get();
        let bite = self.tick.is_multiple_of(hz);
        for i in 0..self.e.len() {
            let st = self.e.status[i];
            if st == Status::None {
                continue;
            }
            if bite && matches!(st, Status::Burning { .. }) {
                self.e.hp[i] -= scale(self.e.max[i], i64::from(BURN_PERMILLE)).max(1);
            }
            self.e.status[i] = st.tick(hz);
        }
        if bite && matches!(self.p.status, Status::Burning { .. }) {
            let d = scale(self.p.max_hp, i64::from(BURN_PERMILLE)).max(1).min(self.p.hp - 1).max(0);
            self.p.hp -= d;
            self.stats.taken += i64::from(d);
        }
        self.p.status = self.p.status.tick(hz);
    }


    pub(super) fn sweep(&mut self) {
        let sp = &self.c.spawn;
        let mut i = 0;
        while i < self.e.len() {
            if self.e.hp[i] > 0 {
                i += 1;
                continue;
            }
            let (x, y, tier) = (self.e.x[i], self.e.y[i], self.e.tier[i]);
            if self.e.side[i] == Side::Hijacked {
                self.hijacked = self.hijacked.saturating_sub(1);
            }
            self.stats.kills += 1;
            self.p.flux = (self.p.flux + self.c.player.flux_per_kill).min(self.c.player.flux);
            match tier {
                Tier::Trash => {
                    self.drop_at(x, y, Drop::Xp(sp.xp_trash));
                    if mix(u64::from(self.e.id[i]) ^ ITEM_SALT.rotate_left(41)).is_multiple_of(FOOD_1_IN) {
                        self.drop_at(x + 500, y, Drop::Food);
                    }
                    if mix(u64::from(self.e.id[i]) ^ ITEM_SALT.rotate_left(53)).is_multiple_of(ORE_1_IN) {
                        self.drop_at(x, y + 500, Drop::Ore);
                    }
                }
                Tier::Elite => {
                    self.drop_at(x, y, Drop::Xp(sp.xp_elite));
                    if mix(u64::from(self.e.id[i]) ^ ITEM_SALT.rotate_left(7)).is_multiple_of(MAGNET_1_IN) {
                        self.drop_at(x + 600, y, Drop::Magnet);
                    }
                    if mix(u64::from(self.e.id[i]) ^ ITEM_SALT).is_multiple_of(ELITE_ITEM_1_IN) {
                        self.drop_item(x - 600, y, self.e.id[i]);
                    }
                }
                Tier::Champion => {
                    self.drop_at(x, y, Drop::Xp(sp.xp_champion));
                    self.drop_at(x + 600, y, Drop::Chest);
                    self.drop_item(x - 600, y, self.e.id[i]);
                }
                Tier::Boss => {
                    if let Some(b) = self.boss.take() {
                        self.stats.boss_kill_ticks.push((b.fam, b.age));
                        if b.last {
                            self.outcome = Some(Outcome::Win);
                        }
                    }
                    self.drop_at(x, y, Drop::Chest);
                    self.drop_item(x + 600, y, self.e.id[i]);
                    // a mid-boss leaves a trap behind: a false chest (bomb or mimic) where it fell
                    if BossClass::of_kind(Tier::Boss, &self.c.fam(self.e.fam[i]).kind) == Some(BossClass::Mini) {
                        use world::chest::{ChestKind, WorldChest};
                        let kind = if self.e.id[i].is_multiple_of(2) { ChestKind::Bomb } else { ChestKind::Mimic };
                        self.world.chests.push(WorldChest { kind, x: x - 1200, y, tell: false });
                    }
                }
            }
            self.e.remove(i);
        }
    }

    /// Thieves eat loot they reach before the player does: loot gone, thief healed by a fifth of its max
    /// (chest: half). shortcut: fixed heal shares, upgrade to per-family data when thieves get tuning.
    pub(super) fn thieves_steal(&mut self) {
        let c = self.c;
        let now = self.tick;
        self.barks.retain(|b| now.saturating_sub(b.tick) < BARK_TICKS);
        let mut near = std::mem::take(&mut self.near);
        let mut k = self.drops.x.len();
        while k > 0 {
            k -= 1;
            let (dx, dy) = (self.drops.x[k], self.drops.y[k]);
            self.grid.query(dx, dy, 500, &mut near);
            // the grid can hold indices from earlier this tick (kills swap-remove): bounds-check them
            let thief = near.iter().map(|&j| j as usize).find(|&j| {
                j < self.e.len()
                    && self.e.side[j] == Side::Hostile
                    && (self.e.panic[j] || matches!(c.fam(self.e.fam[j]).kind, FamKind::Mob(Mob { role: MobRole::Thief { .. }, .. })))
            });
            let Some(j) = thief else { continue };
            // eat it: loot grows max HP by a tenth per point (and heals that much), contact damage grows in contact_dmg
            let bite = match self.drops.kind[k] {
                Drop::Xp(_) => LOOT_XP,
                Drop::Chest | Drop::Item(_) => LOOT_CHEST,
                Drop::Magnet | Drop::Food | Drop::Ore => LOOT_XP,
            };
            let before = self.e.loot[j];
            self.e.loot[j] = before.saturating_add(bite);
            let grow = self.e.max[j] / 10 * i32::from(bite);
            self.e.max[j] = self.e.max[j].saturating_add(grow);
            self.e.hp[j] = (self.e.hp[j].saturating_add(grow)).min(self.e.max[j]);
            if self.barks.len() >= self.barks.capacity() {
                self.barks.remove(0);
            }
            self.barks.push(Bark { tick: self.tick, x: self.e.x[j], y: self.e.y[j], id: self.e.id[j], kind: BarkKind::Stole { power: self.e.loot[j] } });
            for a in Ability::ALL {
                if !a.owned_at(before) && a.owned_at(self.e.loot[j]) {
                    if a == Ability::Rage {
                        self.e.status[j] = Status::None.enrage(self.c.hz().get());
                    }
                    if self.barks.len() >= self.barks.capacity() {
                        self.barks.remove(0);
                    }
                    self.barks.push(Bark { tick: self.tick, x: self.e.x[j], y: self.e.y[j], id: self.e.id[j], kind: BarkKind::Learned { ability: a } });
                }
            }
            let d = &mut self.drops;
            d.x.swap_remove(k);
            d.y.swap_remove(k);
            d.kind.swap_remove(k);
            d.pulled.swap_remove(k);
        }
        self.near = near;
    }

    /// Hero confidence, permille: 0 just after a hit, full after CONF_FULL_S seconds unhurt.
    pub fn confidence(&self) -> u32 {
        let full = CONF_FULL_S * self.c.hz().get();
        (u64::from(self.p.unhurt) * 1000 / u64::from(full.max(1))).min(1000) as u32
    }

    /// A confident hero scares weak foes: once a second, some trash in view panics (flees, then steals).
    pub(super) fn morale_step(&mut self) {
        let hz = self.c.hz().get();
        if self.confidence() < CONF_PANIC || !self.tick.is_multiple_of(hz) {
            return;
        }
        let r2 = i64::from(self.c.view.view_r).pow(2);
        let beat = self.tick / hz;
        for i in 0..self.e.len() {
            let e = &mut self.e;
            if e.side[i] == Side::Hostile && e.tier[i] == Tier::Trash && !e.panic[i] && dist2(e.x[i] - self.p.x, e.y[i] - self.p.y) <= r2 && (e.id[i] + beat).is_multiple_of(PANIC_ODDS) {
                e.panic[i] = true;
            }
        }
    }

    /// Touching a treasure takes it: one chest pick (the upgrade offer) per treasure.
    fn treasure_step(&mut self) {
        let (px, py) = (self.p.x, self.p.y);
        let before = self.treasures.len();
        self.treasures.retain(|&(x, y)| dist2(x - px, y - py) > 500 * 500);
        self.pending_chest += (before - self.treasures.len()) as u16;
    }

    pub(super) fn drops_step(&mut self) {
        self.thieves_steal();
        self.treasure_step();
        let r = scale(self.c.player.pickup + self.stat(Stat::Pickup) as i32, 1000 + self.trait_val(SkillKind::Magnet));
        let v = self.c.player.speed * 2;
        let (mut i, mut magnet) = (0, false);
        while i < self.drops.x.len() {
            let (dx, dy) = (self.p.x - self.drops.x[i], self.p.y - self.drops.y[i]);
            let d2 = dist2(dx, dy);
            if d2 <= i64::from(r).pow(2) {
                self.drops.pulled[i] = true;
            }
            if d2 <= 500 * 500 {
                match self.drops.kind[i] {
                    Drop::Xp(x) => self.gain_xp(x),
                    Drop::Chest => self.pending_chest += 1,
                    Drop::Item(id) => {
                        self.wear(id);
                    }
                    Drop::Magnet => magnet = true,
                    Drop::Food => self.eat(),
                    Drop::Ore => self.p.ore += 1,
                }
                let d = &mut self.drops;
                d.x.swap_remove(i);
                d.y.swap_remove(i);
                d.kind.swap_remove(i);
                d.pulled.swap_remove(i);
                continue;
            }
            if self.drops.pulled[i] {
                let (mx, my) = toward(dx, dy, v);
                self.drops.x[i] += mx;
                self.drops.y[i] += my;
            }
            i += 1;
        }
        if magnet {
            self.magnet_pull();
        }
    }

    /// XP magnet: every XP orb on the floor flies to the hero, and MAGNET_FAR more stream in from a ring beyond
    /// sight (the orbs of fights the hero never saw). Positions come from the tick, no RNG stream consumed.
    fn magnet_pull(&mut self) {
        for k in 0..self.drops.x.len() {
            if matches!(self.drops.kind[k], Drop::Xp(_)) {
                self.drops.pulled[k] = true;
            }
        }
        let r0 = self.c.view.fog_end + MAGNET_RING_MIN;
        for k in 0..MAGNET_FAR {
            let h = mix(u64::from(self.tick) ^ u64::from(k).rotate_left(29) ^ ITEM_SALT);
            let (cx, cy) = self.trig[(h % u64::from(TRIG_N)) as usize];
            let r = r0 + ((h >> 20) % MAGNET_RING_SPAN as u64) as i32;
            let (x, y) = (self.p.x + scale(cx, i64::from(r)), self.p.y + scale(cy, i64::from(r)));
            let n = self.drops.x.len();
            self.drop_at(x, y, Drop::Xp(self.c.spawn.xp_trash));
            if self.drops.x.len() > n {
                self.drops.pulled[n] = true;
            }
        }
    }

    pub fn xp_need(&self) -> u32 {
        let t = &self.c.xp_need;
        t[usize::from(self.p.level).min(t.len() - 1)]
    }

    fn levels(&mut self) {
        while self.p.xp >= self.xp_need() {
            self.p.xp -= self.xp_need();
            self.p.level += 1;
            self.pending += 1;
        }
        if self.offer.is_none() && (self.pending > 0 || self.pending_chest > 0) {
            let chest = self.pending_chest > 0;
            if chest {
                self.pending_chest -= 1;
            } else {
                self.pending -= 1;
            }
            self.make_offer(chest);
        }
    }

    // ---------- picks (design sec 12.2) ----------

    fn make_offer(&mut self, chest: bool) {
        let c = self.c;
        let max_rank = c.picks.max_rank;
        let (mut skills, mut traits) = (0, 0);
        let mut owned_tags = crate::data::Tags::default();
        for o in &self.owned {
            let s = c.skill(o.id);
            owned_tags = owned_tags.or(s.tags);
            if s.kind.is_trait() { traits += 1 } else { skills += 1 }
        }
        let mut w = [0u32; MAX_SKILLS];
        let mut owned_w = [0u32; MAX_SKILLS];
        for id in c.skill_ids() {
            let s = c.skill(id);
            let rank = self.rank_of(id);
            let open = if rank > 0 { rank < max_rank } else if s.kind.is_trait() { traits < c.player.trait_slots } else { skills < c.player.skill_slots };
            if open {
                let base = 1000 + if s.tags.overlap(owned_tags) > 0 { c.picks.tag_bonus } else { 0 };
                w[id.ix()] = base;
                if rank > 0 {
                    owned_w[id.ix()] = base;
                }
            }
        }
        let mut cards = [None; 3];
        let mut any_rare = false;
        for (k, slot) in cards.iter_mut().enumerate().take(c.picks.cards) {
            let use_owned = k == 0 && owned_w.iter().any(|&x| x > 0) && self.rng_picks.chance(c.picks.owned_bias);
            let table = if use_owned { &mut owned_w } else { &mut w };
            let Some(ix) = weighted(&mut self.rng_picks, table) else { continue };
            w[ix] = 0;
            owned_w[ix] = 0;
            let mut rarity = weighted(&mut self.rng_picks, &mut c.picks.rarity.clone()).unwrap_or(0) as u8;
            if k == c.picks.cards - 1 && !any_rare && rarity == 0 && (chest || self.since_rare + 1 >= c.picks.pity_every) {
                rarity = 1; // pity / chest: rare+ guaranteed
            }
            any_rare |= rarity > 0;
            let skill = c.skill_ids().nth(ix).unwrap_or(c.player.skills[0]);
            *slot = Some(Card { skill, rarity, new_rank: self.rank_of(skill) + 1 });
        }
        self.since_rare = if any_rare { 0 } else { self.since_rare + 1 };
        if cards.iter().any(Option::is_some) {
            self.offer = Some(Offer { cards, chest });
        }
    }

    fn choose(&mut self, ch: PickChoice) {
        let Some(offer) = self.offer.take() else { return };
        let card = match ch {
            PickChoice::Card(i) => offer.cards.get(usize::from(i)).copied().flatten(),
            PickChoice::Skip => None,
        };
        self.stats.first_pick.get_or_insert(self.tick);
        if self.stats.picks.len() < self.stats.picks.capacity() {
            self.stats.picks.push(PickRec { tick: self.tick, skill: card.map(|c| c.skill), rarity: card.map_or(0, |c| c.rarity) });
        }
        let Some(card) = card else { return };
        let gain = self.c.picks.gain[usize::from(card.rarity.min(3))];
        match self.owned.iter_mut().find(|o| o.id == card.skill) {
            Some(o) => {
                o.rank += 1;
                o.power += gain;
            }
            None => {
                if self.owned.len() < self.owned.capacity() {
                    self.owned.push(Owned { id: card.skill, rank: 1, power: gain, cd: 0, sp: 0, phase: 0 });
                }
            }
        }
        let old = self.p.max_hp;
        self.p.max_hp = self.max_hp();
        self.p.hp += (self.p.max_hp - old).max(0);
    }

    // ---------- meter (rho, design sec 11.4) ----------

    fn meter(&mut self) {
        let win = self.c.hz().get() * 30;
        if !self.tick.is_multiple_of(win) {
            return;
        }
        let h = self.stats.win_spawned / 30 / 1000;
        let d = self.dps_capacity().max(1);
        if self.stats.rho.len() < self.stats.rho.capacity() {
            self.stats.rho.push((self.tick, (h * 1000 / d).min(99_999) as u32));
        }
        self.stats.win_spawned = 0;
    }

    /// Order-sensitive FNV-1a over the whole sim state (O-hash).
    pub fn hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut f = |v: i64| {
            for b in v.to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        };
        let p = &self.p;
        for v in [i64::from(self.tick), p.x.into(), p.y.into(), p.hp.into(), p.flux.into(), p.xp.into(), p.level.into(), self.e.len() as i64] {
            f(v);
        }
        for i in 0..self.e.len() {
            for v in [self.e.x[i], self.e.y[i], self.e.hp[i], i32::from(self.e.glitch[i]), self.e.side[i] as i32, self.e.id[i] as i32] {
                f(v.into());
            }
        }
        for v in self.bolts.x.iter().chain(&self.bolts.y).chain(&self.shots.x).chain(&self.drops.x) {
            f((*v).into());
        }
        for o in &self.owned {
            f(i64::from(o.rank) << 32 | i64::from(o.power));
        }
        for r in [&self.rng_spawn, &self.rng_combat, &self.rng_picks] {
            for w in r.words() {
                f(w as i64);
            }
        }
        f(self.tele.len() as i64);
        f(i64::from(self.stats.kills));
        h
    }
}

fn mhp_per_tick(permille_hp_per_s: i64, hz: u32) -> i64 {
    permille_hp_per_s / i64::from(hz)
}

/// Draw an index with probability weight[i]/sum; None if all zero.
fn weighted(rng: &mut Xo, w: &mut [u32]) -> Option<usize> {
    let total: u32 = w.iter().sum();
    let n = NonZeroU32::new(total)?;
    let mut r = rng.below(n);
    for (i, &x) in w.iter().enumerate() {
        if r < x {
            return Some(i);
        }
        r -= x;
    }
    None
}

/// Circle of radius `r` at (x, y) lies wholly inside one tile (so, if it stood on floor, still does).
fn interior(x: i32, y: i32, r: i32) -> bool {
    let t = crate::floorgen::TILE_MM;
    let (lx, ly) = (x.rem_euclid(t), y.rem_euclid(t));
    lx >= r && lx + r < t && ly >= r && ly + r < t
}
