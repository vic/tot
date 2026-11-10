//! Built-in fallback director (design sec 8.2, 11.5, 11.6): threat wallet + pacing cycle +
//! wave schedule. Emits only typed `Cmd`s through `Sim::command`, the path an agent will use.
use crate::data::{Content, FamId, FamKind, Formation, WaveEv};
use crate::rng::{Stream, Xo};
use crate::sim::{Cmd, Sim, Src, Tier};
use std::num::{NonZeroU16, NonZeroU32};

pub struct Director {
    rng: Xo,
    /// milli-TP
    wallet: i64,
    phase: usize,
    phase_t: u32,
    wave_i: usize,
    thin: u32,
    final_boss: bool,
    last: Option<u32>,
    fam_mult: Vec<u32>,
    /// pacing-clock remainder (permille of a tick)
    clock_acc: u32,
    /// miniboss spawned so far, and the tick the next is due (None = the cadence start)
    mini_n: u32,
    next_mini: Option<u32>,
    /// a scripted boss that came due while the hero rested in a safe level: it comes on the return
    held_boss: Option<usize>,
    /// tick of the next wild boss (None = not drawn yet)
    next_wild: Option<u32>,
}

impl Director {
    pub fn new(seed: u64) -> Director {
        Director { rng: Xo::stream(seed, Stream::Director), wallet: 0, phase: 0, phase_t: 0, wave_i: 0, thin: 1000, final_boss: false, last: None, fam_mult: Vec::new(), clock_acc: 0, mini_n: 0, next_mini: None, next_wild: None, held_boss: None }
    }

    pub fn phase_name(&self) -> &'static str {
        ["BUILD", "PEAK", "FADE", "RELAX"][self.phase]
    }

    /// Advance the pacing cycle one tick; a hero ahead of par runs the clock faster (shorter waves).
    fn pace(&mut self, c: &Content, t: u32, level: u16) -> u32 {
        let p = c.pace[self.phase];
        self.clock_acc += c.scale.phase_rate(c.hz(), t, level) - 1000;
        self.phase_t += 1 + self.clock_acc / 1000;
        self.clock_acc %= 1000;
        if self.phase_t >= p.len {
            self.phase = (self.phase + 1) % 4;
            self.phase_t = 0;
        }
        let p = c.pace[self.phase];
        let t = i64::from(self.phase_t);
        (i64::from(p.from) + (i64::from(p.to) - i64::from(p.from)) * t / i64::from(p.len.max(1))) as u32
    }

    fn pick_fam(&mut self, c: &Content, tick: u32) -> Option<FamId> {
        let mut w = [0u32; 256];
        for f in c.fam_ids() {
            if let FamKind::Mob(m) = &c.fam(f).kind
                && tick >= m.unlock
            {
                w[f.ix()] = m.weight * self.fam_mult.get(f.ix()).copied().unwrap_or(1000);
            }
        }
        let n = NonZeroU32::new(w.iter().sum())?;
        let mut r = self.rng.below(n);
        for f in c.fam_ids() {
            if r < w[f.ix()] {
                return Some(f);
            }
            r -= w[f.ix()];
        }
        None
    }

    /// Plan this tick's commands and send them through the sim's validator.
    pub fn tick(&mut self, sim: &mut Sim) {
        let c = sim.c;
        let t = sim.tick;
        // a paused sim (PICK) does not advance the tick: plan each tick once
        if self.last == Some(t) {
            return;
        }
        self.last = Some(t);
        if self.fam_mult.is_empty() {
            self.fam_mult.clone_from(&sim.arena().fam_mult);
        }
        let one = NonZeroU16::MIN;
        let level = sim.p.level;
        let safe = sim.safe(sim.level);
        // back from a safe level: the boss that came due there comes now (its script slot already passed)
        if !safe
            && sim.boss_ix().is_none()
            && let Some(k) = self.held_boss.take()
            && let Some(WaveEv::Boss { fam, last, .. }) = c.waves.get(k)
        {
            self.final_boss |= *last;
            if *last {
                let _ = sim.command(Src::Builtin, Cmd::Flee);
            }
            let _ = sim.command(Src::Builtin, Cmd::Boss { fam: *fam });
        }
        while let Some(ev) = c.waves.get(self.wave_i).filter(|e| e.at() <= t) {
            // in a safe level time passes for the script: hordes due there are gone, a boss due there is held back
            if safe {
                if matches!(ev, WaveEv::Boss { .. }) && self.held_boss.is_none() {
                    self.held_boss = Some(self.wave_i);
                }
                self.wave_i += 1;
                continue;
            }
            let cmd = match *ev {
                WaveEv::Horde { pattern, n, fam, .. } => Some(Cmd::Spawn { fam, n: c.scale.horde_n(n, level, c.spawn.live_cap), tier: Tier::Trash, form: pattern }),
                WaveEv::Boss { fam, last, .. } => {
                    if sim.boss_ix().is_some() {
                        break; // a wild boss holds the stage: the scripted one waits for it
                    }
                    self.final_boss |= last;
                    if last {
                        let _ = sim.command(Src::Builtin, Cmd::Flee);
                    }
                    Some(Cmd::Boss { fam })
                }
                WaveEv::Thin { mult, .. } => {
                    self.thin = mult;
                    None
                }
                WaveEv::Narrate { .. } => Some(Cmd::Narrate { ev: self.wave_i }),
            };
            if let Some(cmd) = cmd {
                let _ = sim.command(Src::Builtin, cmd);
            }
            self.wave_i += 1;
        }
        // a safe level pauses the horde: nothing is banked or due there, so the return brings no backlog
        if safe {
            return;
        }
        let near_boss = |at: u32| c.waves.iter().any(|w| matches!(w, WaveEv::Boss { .. }) && w.at().abs_diff(at) < c.hz().ticks(30.0));
        for (cad, tier) in [(c.elite, Tier::Elite), (c.champion, Tier::Champion)] {
            if t >= cad.from
                && (t - cad.from).is_multiple_of(cad.every.get())
                && !near_boss(t)
                && let Some(fam) = self.pick_fam(c, t)
            {
                let _ = sim.command(Src::Builtin, Cmd::Spawn { fam, n: one, tier, form: Formation::Scatter });
            }
        }
        // named miniboss on its own cadence: spawned as a champion, pre-paid from the trash wallet
        // (cost per attack kind: a wider attack mix is more threat)
        // the gap to the next one shrinks as the hero levels (ScaleDef::mini_every)
        if let Some(plan) = c.mini.as_ref()
            && t >= self.next_mini.unwrap_or(plan.cadence.from)
            && !near_boss(t)
            && let mini = plan.nth(self.mini_n)
            && let FamKind::Mob(m) = &c.fam(mini.fam).kind
            && sim.command(Src::Builtin, Cmd::Spawn { fam: mini.fam, n: one, tier: Tier::Champion, form: Formation::Scatter }).is_ok()
        {
            self.wallet -= i64::from(m.cost) * mini.attacks.len() as i64;
            self.mini_n += 1;
            self.next_mini = Some(t + c.scale.mini_every(plan.cadence.every, level).get());
        }
        // wild boss: a low/mid-class boss off the script, at a seeded random time and gap (every/2 .. 3*every/2).
        // It never starts so close to a scripted boss that its fight would run into it (the family's enrage time).
        if let Some(wild) = c.wild
            && sim.boss_ix().is_none()
            && t >= *self.next_wild.get_or_insert_with(|| wild.from + self.rng.below(wild.every))
        {
            let n = NonZeroU32::new(sim.wild_pool().count() as u32);
            let fam = n.and_then(|n| sim.wild_pool().nth(self.rng.below(n) as usize));
            let blocks = |fam: FamId| c.boss_of(fam).is_some_and(|b| c.waves.iter().any(|w| matches!(w, WaveEv::Boss { at, .. } if *at > t && *at - t <= b.enrage)));
            let sent = match fam {
                None => true,
                Some(fam) => !blocks(fam) && sim.command(Src::Builtin, Cmd::Wild { fam }).is_ok(),
            };
            if sent {
                self.next_wild = Some(t + wild.every.get() / 2 + self.rng.below(wild.every));
            }
        }
        let mult = self.pace(c, t, level);
        if self.final_boss {
            return; // trash fled; the boss is the content
        }
        let rate = i64::from(c.spawn.rate[(t / c.hz().get()) as usize % c.spawn.rate.len()]);
        self.wallet += rate * i64::from(mult) / 1000 * i64::from(self.thin) / 1000 / i64::from(c.hz().get());
        let elite_cost = 8_000i64;
        let bank_cap = elite_cost * i64::from(c.spawn.pack_bank) / 1000;
        if sim.live() >= c.spawn.live_cap {
            self.wallet = self.wallet.min(bank_cap * 2);
            return;
        }
        let Some(fam) = self.pick_fam(c, t) else { return };
        let FamKind::Mob(m) = &c.fam(fam).kind else { return };
        let cost = i64::from(m.cost);
        // too-cheap rule (RoR2): a fat bank is spent as a pack
        let n = if self.wallet > bank_cap { c.spawn.pack_min } else { 1 };
        let Some(n16) = NonZeroU16::new(n) else { return };
        if self.wallet >= cost * i64::from(n) && sim.command(Src::Builtin, Cmd::Spawn { fam, n: n16, tier: Tier::Trash, form: Formation::Scatter }).is_ok() {
            self.wallet -= cost * i64::from(n);
        }
    }
}

/// Built-in refill when an outside command is rejected (src `fallback`, design sec 3):
/// a rejected spawn becomes as many trash of the first pool family as the live cap allows.
pub fn fallback(sim: &mut Sim, rejected: &Cmd) {
    let Cmd::Spawn { n, .. } = *rejected else { return };
    let room = sim.c.spawn.live_cap.saturating_sub(sim.live()).min(usize::from(n.get()));
    let fam = sim.c.fam_ids().find(|&f| matches!(&sim.c.fam(f).kind, FamKind::Mob(m) if m.weight > 0));
    if let (Some(fam), Some(n)) = (fam, NonZeroU16::new(room as u16)) {
        let _ = sim.command(Src::Fallback, Cmd::Spawn { fam, n, tier: Tier::Trash, form: Formation::Scatter });
    }
}

const TIERS: [(&str, Tier); 3] = [("trash", Tier::Trash), ("elite", Tier::Elite), ("champion", Tier::Champion)];
const FORMS: [(&str, Formation); 4] = [("scatter", Formation::Scatter), ("ring", Formation::Ring), ("wall", Formation::Wall), ("spiral", Formation::Spiral)];

pub fn fam_by_key(c: &Content, k: &str) -> Option<FamId> {
    c.fam_ids().find(|&f| c.fam(f).key == k)
}

/// Command file (JSON Lines, head file "cmds"): `{"t":"cmd","at_s":S,"cmd":<cmd>}` with <cmd> =
/// "flee" | {"spawn":{"family","n","tier","form"}} | {"boss":{"family"}}. The input format an
/// agent or tool sends; parsed into the same typed `Cmd` the built-in director emits.
pub fn parse_cmds(c: &Content, text: &str) -> crate::Result<Vec<(u32, Cmd)>> {
    let mut vals = Vec::new();
    let mut out = Vec::new();
    for (t, r) in crate::json::records("cmds.jsonl", text, &mut vals)? {
        if t != "cmd" {
            return Err(r.err("t", "expected \"cmd\""));
        }
        r.allow(&["t", "at_s", "cmd"])?;
        let at = c.hz().ticks(r.num("at_s", 0.0, 7200.0)?);
        let (name, body) = r.variant("cmd")?;
        let fam = |b: &crate::json::Rec| -> crate::Result<FamId> {
            let k = b.text("family")?;
            fam_by_key(c, k).ok_or_else(|| b.err("family", format!("no family '{k}'")))
        };
        let cmd = match (name, body) {
            ("flee", None) => Cmd::Flee,
            ("spawn", Some(b)) => {
                b.allow(&["family", "n", "tier", "form"])?;
                let n = NonZeroU16::new(b.int("n", 1, 2000)? as u16).ok_or_else(|| b.err("n", "> 0"))?;
                Cmd::Spawn { fam: fam(&b)?, n, tier: b.pick("tier", &TIERS)?, form: b.pick("form", &FORMS)? }
            }
            ("boss", Some(b)) => {
                b.allow(&["family"])?;
                Cmd::Boss { fam: fam(&b)? }
            }
            _ => return Err(r.err("cmd", "one of \"flee\" {\"spawn\":{..}} {\"boss\":{..}}")),
        };
        out.push((at, cmd));
    }
    Ok(out)
}

/// `:spawn <family> [n] [tier] [form]` (src `human`).
pub fn parse_spawn(c: &Content, words: &[&str]) -> Result<Cmd, String> {
    let [fam, rest @ ..] = words else { return Err("spawn <family> [n] [trash|elite|champion] [scatter|ring|wall|spiral]".into()) };
    let fam = fam_by_key(c, fam).ok_or_else(|| format!("no family '{fam}'"))?;
    let n = rest.first().map_or(Ok(1), |s| s.parse::<u16>().map_err(|_| format!("n '{s}'")))?;
    let n = NonZeroU16::new(n).ok_or("n must be > 0")?;
    Ok(Cmd::Spawn { fam, n, tier: word(rest.get(1), &TIERS)?, form: word(rest.get(2), &FORMS)? })
}

/// Optional word from a closed table; absent = first entry.
fn word<T: Copy>(w: Option<&&str>, t: &[(&str, T)]) -> Result<T, String> {
    match w {
        None => Ok(t[0].1),
        Some(s) => t.iter().find(|(k, _)| k == s).map(|(_, v)| *v).ok_or(format!("'{s}' not one of {}", t.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(" "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;
    use crate::data::WaveEv;
    use crate::sim::{Input, Reject};

    #[test]
    fn director_spawns_on_ring_through_logged_commands() {
        let c = content();
        let mut sim = Sim::new(&c, 1, None);
        let mut d = Director::new(1);
        for _ in 0..30 * 20 {
            d.tick(&mut sim);
            sim.step(&Input { autofire: true, ..Input::default() });
            if sim.offer.is_some() {
                sim.step(&Input { pick: Some(crate::sim::PickChoice::Skip), ..Input::default() });
            }
        }
        assert!(sim.log.iter().filter(|r| r.ok.is_ok()).count() > 10);
        assert!(sim.log.iter().all(|r| r.src == Src::Builtin));
        assert!(sim.live() > 0);
    }

    #[test]
    fn director_spawns_each_miniboss_on_the_cadence() {
        // short cadence so an idle hero lives through 3 slots
        let mut t = crate::data::tests::texts();
        t[6] = t[6].replacen("\"tier\":\"miniboss\",\"from_s\":120,\"every_s\":75", "\"tier\":\"miniboss\",\"from_s\":5,\"every_s\":5", 1);
        let c = Content::parse(&t).unwrap();
        let plan = c.mini.as_ref().unwrap();
        let (from, every) = (plan.cadence.from, plan.cadence.every.get());
        let mut sim = Sim::new(&c, 1, None);
        let mut d = Director::new(1);
        for _ in 0..from + every * 2 + 1 {
            d.tick(&mut sim);
            sim.step(&Input { autofire: true, ..Input::default() });
            if sim.offer.is_some() {
                sim.step(&Input { pick: Some(crate::sim::PickChoice::Skip), ..Input::default() });
            }
        }
        let minis: Vec<_> = sim.log.iter().filter_map(|r| match r.cmd {
            Cmd::Spawn { fam, tier: Tier::Champion, .. } if plan.iter().any(|m| m.fam == fam) && r.ok.is_ok() => Some((r.tick, fam)),
            _ => None,
        }).collect();
        assert_eq!(minis.iter().map(|m| m.1).collect::<Vec<_>>(), (0..3).map(|k| plan.nth(k).fam).collect::<Vec<_>>(), "round robin, one per cadence slot");
        assert_eq!(minis[0].0, from, "first on the cadence start");
        assert!(minis.windows(2).all(|w| w[1].0 - w[0].0 <= every && w[1].0 > w[0].0), "gaps never exceed the base cadence");
    }

    #[test]
    fn validator_rejects_over_cap_and_boss_as_mob() {
        let c = content();
        let mut sim = Sim::new(&c, 1, None);
        let mob = c.fam_ids().next().unwrap_or_else(|| unreachable!());
        let boss = c.fam_ids().find(|&f| matches!(c.fam(f).kind, FamKind::Boss { .. })).unwrap();
        let n = NonZeroU16::new(c.spawn.live_cap as u16 + 1).unwrap();
        let r = sim.command(Src::Builtin, Cmd::Spawn { fam: mob, n, tier: Tier::Trash, form: Formation::Ring });
        assert!(matches!(r, Err(Reject::LiveCap { .. })));
        assert_eq!(sim.command(Src::Builtin, Cmd::Spawn { fam: boss, n: NonZeroU16::MIN, tier: Tier::Trash, form: Formation::Ring }), Err(Reject::NotMob));
        assert_eq!(sim.command(Src::Builtin, Cmd::Boss { fam: mob }), Err(Reject::NotBoss));
        assert!(sim.command(Src::Builtin, Cmd::Boss { fam: boss }).is_ok());
        assert_eq!(sim.command(Src::Builtin, Cmd::Boss { fam: boss }), Err(Reject::BossActive));
        assert_eq!(sim.live(), 1);
        assert_eq!(sim.log.len(), 5);
    }

    #[test]
    fn spawns_land_outside_view() {
        let c = content();
        let mut sim = Sim::new(&c, 3, None);
        let fam = c.fam_ids().next().unwrap();
        for form in [Formation::Scatter, Formation::Ring, Formation::Wall, Formation::Spiral] {
            let _ = sim.command(Src::Builtin, Cmd::Spawn { fam, n: NonZeroU16::new(20).unwrap(), tier: Tier::Trash, form });
        }
        for i in 0..sim.live() {
            let d = crate::sim::dist2(sim.e.x[i], sim.e.y[i]).isqrt() as i32;
            assert!(d > c.view.view_r, "spawn {i} at {d} mm inside view");
        }
    }

    const HEAD: &str = "{\"t\":\"head\",\"schema_version\":1,\"file\":\"cmds\"}\n";

    #[test]
    fn cmd_file_parses_and_blames() {
        let c = content();
        let ok = format!("{HEAD}{{\"t\":\"cmd\",\"at_s\":2,\"cmd\":{{\"spawn\":{{\"family\":\"brute\",\"n\":5,\"tier\":\"elite\",\"form\":\"ring\"}}}}}}\n{{\"t\":\"cmd\",\"at_s\":3,\"cmd\":\"flee\"}}");
        let v = parse_cmds(&c, &ok).unwrap();
        assert_eq!(v.len(), 2);
        assert!(matches!(v[0], (60, Cmd::Spawn { tier: Tier::Elite, form: Formation::Ring, .. })));
        let bad = format!("{HEAD}{{\"t\":\"cmd\",\"at_s\":2,\"cmd\":{{\"spawn\":{{\"family\":\"dragon\",\"n\":5,\"tier\":\"elite\",\"form\":\"ring\"}}}}}}");
        assert_eq!(parse_cmds(&c, &bad).unwrap_err().to_string(), "cmds.jsonl:2 .cmd.spawn.family: no family 'dragon'");
        // peer chat text is never a command (O-src): it does not even parse
        assert!(parse_cmds(&c, &format!("{HEAD}spawn brute 500")).is_err());
    }

    #[test]
    fn source_tags_closed_set_and_fallback_row() {
        assert_eq!(Src::parse("mcp:claude-1").map(|s| s.tag()), Some("mcp:claude-1".into()));
        for bad in ["mcp:", "mcp:UPPER", "agent", "mcp:waytoolongclientname1"] {
            assert_eq!(Src::parse(bad), None, "{bad}");
        }
        let c = content();
        let mut sim = Sim::new(&c, 1, None);
        let fam = c.fam_ids().next().unwrap();
        let big = Cmd::Spawn { fam, n: NonZeroU16::new(c.spawn.live_cap as u16 + 5).unwrap(), tier: Tier::Trash, form: Formation::Ring };
        let src = Src::parse("mcp:test").unwrap();
        assert!(sim.command(src, big).is_err());
        fallback(&mut sim, &big);
        let tags: Vec<String> = sim.log.iter().map(|r| r.src.tag()).collect();
        assert_eq!(tags, ["mcp:test", "fallback"]);
        assert_eq!(sim.live(), c.spawn.live_cap);
        let h = parse_spawn(&c, &["brute", "3", "champion"]).unwrap();
        assert!(sim.command(Src::Human, h).is_err(), "cap full: human edits pass the same validator");
        assert!(parse_spawn(&c, &["brute", "3", "boss"]).is_err());
    }

    fn wild_cmds(sim: &Sim) -> Vec<(u32, FamId)> {
        sim.log.iter().filter_map(|r| match r.cmd { Cmd::Wild { fam } if r.ok.is_ok() => Some((r.tick, fam)), _ => None }).collect()
    }

    #[test]
    fn wild_boss_comes_early_at_seeded_times_and_is_never_the_final_boss() {
        let c = content();
        let wild = c.wild.expect("shipped waves carry a wildboss cadence");
        let scripted = c.waves.iter().find_map(|w| if let WaveEv::Boss { at, .. } = w { Some(*at) } else { None }).unwrap();
        let mut firsts = Vec::new();
        for seed in 1..=6 {
            let (mut sim, mut d) = (Sim::new(&c, seed, None), Director::new(seed));
            for t in 0..wild.from + wild.every.get() {
                sim.tick = t;
                d.tick(&mut sim);
            }
            let w = wild_cmds(&sim);
            assert_eq!(w.len(), 1, "seed {seed}: one wild boss (the stage then stays taken)");
            assert!(w[0].0 >= wild.from && w[0].0 < wild.from + wild.every.get() && w[0].0 < scripted, "seed {seed}: first wild boss in [from, from+every) and before the script: {w:?}");
            assert!(sim.wild_pool().any(|f| f == w[0].1));
            firsts.push(w[0].0);
        }
        firsts.sort_unstable();
        firsts.dedup();
        assert!(firsts.len() > 1, "times vary with the seed: {firsts:?}");
    }

    #[test]
    fn wild_boss_keeps_out_of_the_scripted_bosss_way_and_the_script_waits_for_it() {
        let c = content();
        let (at, enrage) = c.waves.iter().find_map(|w| if let WaveEv::Boss { at, fam, .. } = w { Some((*at, c.boss_of(*fam)?.enrage)) } else { None }).unwrap();
        let due = |tick: u32| {
            let (mut sim, mut d) = (Sim::new(&c, 1, None), Director::new(1));
            d.next_wild = Some(0);
            sim.tick = tick;
            d.tick(&mut sim);
            wild_cmds(&sim).len()
        };
        assert_eq!(due(at - enrage - 1), 1, "control: outside the reserve window a due wild boss spawns");
        assert_eq!(due(at - 1), 0, "inside the scripted boss reserve window it holds back");
        let boss_at = |wild_first: bool| {
            let (mut sim, mut d) = (Sim::new(&c, 1, None), Director::new(1));
            if wild_first {
                let fam = sim.wild_pool().next().unwrap();
                assert!(sim.command(Src::Builtin, Cmd::Wild { fam }).is_ok());
            }
            sim.tick = at;
            d.wave_i = c.waves.iter().position(|w| matches!(w, WaveEv::Boss { .. })).unwrap();
            d.tick(&mut sim);
            (sim.log.iter().filter(|r| matches!(r.cmd, Cmd::Boss { .. })).count(), d.wave_i)
        };
        assert_eq!(boss_at(false).0, 1, "control: with the stage free the scripted boss spawns");
        let (n, wave_i) = boss_at(true);
        assert_eq!(n, 0, "a wild boss on stage: the scripted boss is not sent (and lost)");
        assert_eq!(wave_i, c.waves.iter().position(|w| matches!(w, WaveEv::Boss { .. })).unwrap(), "the script stays on that boss until the stage frees");
    }

    #[test]
    fn a_stay_in_town_does_not_bank_a_horde_for_the_return_and_keeps_the_boss() {
        let c = content();
        let after_town = |secs: u32| {
            let mut s = Sim::new(&c, 5, None);
            let mut d = Director::new(5);
            s.travel(crate::sim::Level::Town);
            for _ in 0..secs * c.hz().get() {
                d.tick(&mut s);
                s.step(&Input::default());
            }
            s.travel(crate::sim::Level::FIRST);
            for _ in 0..c.hz().get() {
                d.tick(&mut s);
                s.step(&Input::default());
            }
            (s.live(), d.wave_i, s.boss_ix().is_some())
        };
        let (short, _, boss_short) = after_town(5);
        let (long, wave_i, boss_long) = after_town(550);
        assert!(long <= short + 12, "a long stay sends no banked flood: {long} live after 550 s vs {short} after 5 s");
        let first_boss = c.waves.iter().position(|w| matches!(w, WaveEv::Boss { .. })).unwrap();
        assert!(wave_i > first_boss, "the script moved on past the town stay (at {wave_i}, first boss at {first_boss})");
        assert!(boss_long && !boss_short, "the boss that came due in town comes on the return (control: none due after 5 s)");
    }
}
