//! Headless balance harness (design sec 14.2): move bots x pick bots over N seeds, per-run table,
//! summary vs bars, negative controls that must fire.
use crate::data::{Content, SkillId};
use crate::director::Director;
use crate::rng::{Stream, Xo};
use crate::sim::{Cause, Cmd, Input, Mutant, Outcome, PickChoice, Side, Sim, Src, TeleKind, Tier, dist2};
use crate::{Error, Result};
use std::num::NonZeroU32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MoveBot {
    Kiter,
    Oracle,
    Still,
    Random,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickBot {
    Tag,
    Dps,
    Random,
    Skip,
}

impl MoveBot {
    pub fn parse(s: &str) -> Result<MoveBot> {
        Ok(match s {
            "kiter" => MoveBot::Kiter,
            "oracle" => MoveBot::Oracle,
            "still" => MoveBot::Still,
            "random" => MoveBot::Random,
            _ => return Err(Error::Usage(format!("--move {s}: one of kiter oracle still random"))),
        })
    }
}

impl PickBot {
    pub fn parse(s: &str) -> Result<PickBot> {
        Ok(match s {
            "tag" => PickBot::Tag,
            "dps" => PickBot::Dps,
            "random" => PickBot::Random,
            "none" => PickBot::Skip,
            _ => return Err(Error::Usage(format!("--pick {s}: one of tag dps random none"))),
        })
    }
}

const DIRS: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
/// Kiter reads a telegraph only after a human-like reaction delay (design sec 10.4: ~250 ms).
const REACT_S: f64 = 0.25;

struct Bot {
    rng: Xo,
    dir: (i32, i32),
}

impl Bot {
    fn input(&mut self, sim: &Sim, mv: MoveBot) -> Input {
        let mut inp = Input { autofire: true, ..Input::default() };
        inp.mv = match mv {
            MoveBot::Still => (0, 0),
            MoveBot::Random => {
                if sim.tick.is_multiple_of(sim.c.hz().get()) {
                    let k = self.rng.below(NonZeroU32::MIN.saturating_add(8)) as usize;
                    self.dir = if k == 8 { (0, 0) } else { DIRS[k] };
                }
                self.dir
            }
            MoveBot::Kiter | MoveBot::Oracle => self.kite(sim, mv == MoveBot::Oracle, &mut inp.dodge),
        };
        inp
    }

    /// Score 8 directions: away from crowd + shots, out of telegraphs, toward loot when safe.
    fn kite(&mut self, sim: &Sim, oracle: bool, dodge: &mut bool) -> (i32, i32) {
        let (px, py) = (sim.p.x, sim.p.y);
        let speed = sim.c.player.speed;
        let react = sim.c.hz().ticks(REACT_S) as u16;
        let mut best = (i64::MIN, (0, 0));
        let mut in_danger = false;
        for &(dx, dy) in DIRS.iter().chain(&[(0, 0)]) {
            let look = 10;
            let (nx, ny) = (px + dx * speed * look * 7 / 10, py + dy * speed * look * 7 / 10);
            let mut s = 0i64;
            for i in 0..sim.e.len() {
                if sim.e.side[i] != Side::Hostile {
                    continue;
                }
                let d2 = dist2(sim.e.x[i] - nx, sim.e.y[i] - ny).max(250_000);
                let w = match sim.e.tier[i] {
                    Tier::Trash => 1,
                    Tier::Elite => 3,
                    Tier::Champion => 6,
                    Tier::Boss => 20,
                };
                if d2 < 100_000_000 {
                    s -= w * 4_000_000_000_000 / (d2 * (d2.isqrt() + 1));
                }
            }
            // engage the boss: keep it inside bolt range (bots that flee forever never win)
            if let Some(b) = sim.boss_ix() {
                let d = dist2(sim.e.x[b] - nx, sim.e.y[b] - ny).isqrt();
                if d > 8000 {
                    s -= (d - 8000) / 50;
                }
            }
            for i in 0..sim.shots.x.len() {
                let d2 = dist2(sim.shots.x[i] + sim.shots.vx[i] * 6 - nx, sim.shots.y[i] + sim.shots.vy[i] * 6 - ny).max(10_000);
                if d2 < 4_000_000 {
                    s -= 2_000_000_000 / d2;
                }
            }
            for t in &sim.tele {
                let visible = oracle || t.lead.saturating_sub(t.left) >= react;
                if let (true, TeleKind::HitPlayer { .. }) = (visible, t.kind) {
                    let steps = i32::from(t.left.min(10)).max(1);
                    let (fx, fy) = (px + dx * speed * steps, py + dy * speed * steps);
                    if t.shape.contains(fx, fy) {
                        s -= 50_000;
                    }
                    if t.shape.contains(px, py) {
                        in_danger = true;
                    }
                }
            }
            if (dx, dy) != (0, 0) {
                s += 1; // prefer moving over standing
                // loot: every shard within 12 m pulls (sum), so the bot walks into its XP
                s += sim.drops.x.iter().zip(&sim.drops.y).map(|(&x, &y)| dist2(x - nx, y - ny)).filter(|&d| d < 144_000_000).map(|d| 3000 / (d / 1_000_000 + 4)).sum::<i64>();
                // mild pull back toward the start keeps the camera area familiar
                s -= dist2(nx, ny) / 2_000_000_000;
            }
            if (dx, dy) == self.dir {
                s += 3; // hysteresis
            }
            if s > best.0 {
                best = (s, (dx, dy));
            }
        }
        let close = (0..sim.e.len()).filter(|&i| sim.e.side[i] == Side::Hostile && dist2(sim.e.x[i] - px, sim.e.y[i] - py) < 1_440_000).count();
        let tele_now = sim.tele.iter().any(|t| matches!(t.kind, TeleKind::HitPlayer { .. }) && t.left <= 1 && t.shape.contains(px, py));
        *dodge = close >= 3 || (in_danger && tele_now);
        self.dir = best.1;
        best.1
    }

    fn pick(&mut self, sim: &Sim, pk: PickBot) -> PickChoice {
        let Some(offer) = sim.offer else { return PickChoice::Skip };
        let cards: Vec<(u8, SkillId, u8)> = offer.cards.iter().enumerate().filter_map(|(i, c)| c.map(|c| (i as u8, c.skill, c.rarity))).collect();
        let c = sim.c;
        let owned_tags = sim.owned.iter().fold(crate::data::Tags::default(), |t, o| t.or(c.skill(o.id).tags));
        let score = |s: SkillId, r: u8| -> i64 {
            let sk = c.skill(s);
            let owned = i64::from(sim.rank_of(s) > 0);
            match pk {
                PickBot::Tag => i64::from(sk.tags.overlap(owned_tags)) * 100 + owned * 50 + i64::from(r) * 10,
                PickBot::Dps => {
                    let dps = i64::from(sk.base.dmg) * i64::from(sk.base.count) / i64::from(sk.base.cd.max(1));
                    let tr = if sk.kind.is_trait() { 300 } else { 0 };
                    dps / 10 + tr + owned * 200 + i64::from(r) * 100
                }
                PickBot::Random | PickBot::Skip => 0,
            }
        };
        match pk {
            PickBot::Skip => PickChoice::Skip,
            PickBot::Random => {
                let n = NonZeroU32::new(cards.len() as u32);
                n.map_or(PickChoice::Skip, |n| PickChoice::Card(cards[self.rng.below(n) as usize].0))
            }
            _ => cards.iter().max_by_key(|&&(i, s, r)| (score(s, r), -i64::from(i))).map_or(PickChoice::Skip, |&(i, _, _)| PickChoice::Card(i)),
        }
    }
}

pub struct Run {
    pub seed: u64,
    pub secs: f64,
    pub outcome: Outcome,
    pub kills: u32,
    pub level: u16,
    pub rho: Vec<(u32, u32)>,
    pub picks: Vec<Option<SkillId>>,
    pub dealt: Vec<i64>,
    pub output: Vec<i64>,
    pub dealt_hijack: i64,
    /// (boss name, fight seconds) per boss killed
    pub bosses: Vec<(String, f64)>,
    pub first_pick_s: Option<f64>,
    /// (src tag, applied, rejected)
    pub srcs: Vec<(String, usize, usize)>,
}

/// KITER+TAG play up to `secs` (for `proto2 snapshot`).
pub fn sim_at(c: &Content, seed: u64, secs: u32) -> (Sim<'_>, Director) {
    let mut sim = Sim::new(c, seed, None);
    let mut dir = Director::new(seed);
    let mut bot = Bot { rng: Xo::stream(seed, Stream::Bot), dir: (0, 0) };
    while sim.outcome.is_none() && sim.secs() < secs {
        dir.tick(&mut sim);
        let mut inp = bot.input(&sim, MoveBot::Kiter);
        if sim.offer.is_some() {
            inp.pick = Some(bot.pick(&sim, PickBot::Tag));
        }
        sim.step(&inp);
    }
    (sim, dir)
}

/// Outside commands (an agent stand-in) applied at their tick with their source tag.
pub struct Inject {
    pub cmds: Vec<(u32, Cmd)>,
    pub src: Src,
}

/// Demo driver: the headless bot's moves and picks, fed into the live play loop (`play --auto`).
pub struct Autopilot {
    bot: Bot,
    mv: MoveBot,
    pk: PickBot,
}

impl Autopilot {
    pub fn new(seed: u64, mv: MoveBot, pk: PickBot) -> Autopilot {
        Autopilot { bot: Bot { rng: Xo::stream(seed, Stream::Bot), dir: (0, 0) }, mv, pk }
    }

    /// Movement only: (world move, dodge).
    pub fn walk(&mut self, sim: &Sim) -> ((i32, i32), bool) {
        let inp = self.bot.input(sim, self.mv);
        (inp.mv, inp.dodge)
    }

    pub fn choose(&mut self, sim: &Sim) -> PickChoice {
        self.bot.pick(sim, self.pk)
    }
}

pub fn run(c: &Content, seed: u64, mv: MoveBot, pk: PickBot, mutant: Option<Mutant>, inj: Option<&Inject>) -> Run {
    let mut sim = Sim::new(c, seed, mutant);
    let mut dir = Director::new(seed);
    let mut bot = Bot { rng: Xo::stream(seed, Stream::Bot), dir: (0, 0) };
    let mut next_cmd = 0;
    while sim.outcome.is_none() {
        if let Some(inj) = inj {
            while let Some((_, cmd)) = inj.cmds.get(next_cmd).filter(|(at, _)| *at <= sim.tick) {
                if sim.command(inj.src, *cmd).is_err() {
                    crate::director::fallback(&mut sim, cmd);
                }
                next_cmd += 1;
            }
        }
        dir.tick(&mut sim);
        let mut inp = bot.input(&sim, mv);
        if sim.offer.is_some() {
            inp.pick = Some(bot.pick(&sim, pk));
        }
        sim.step(&inp);
    }
    let hz = f64::from(c.hz().get());
    Run {
        seed,
        secs: f64::from(sim.tick) / hz,
        outcome: sim.outcome.unwrap_or(Outcome::Timeout),
        kills: sim.stats.kills,
        level: sim.p.level,
        rho: sim.stats.rho.clone(),
        picks: sim.stats.picks.iter().map(|p| p.skill).collect(),
        dealt: sim.stats.dealt[..c.skills.len()].to_vec(),
        output: sim.stats.output[..c.skills.len()].to_vec(),
        dealt_hijack: sim.stats.dealt_hijack,
        bosses: sim.stats.boss_kill_ticks.iter().map(|(f, t)| (c.fam(*f).name.clone(), f64::from(*t) / hz)).collect(),
        first_pick_s: sim.stats.first_pick.map(|t| f64::from(t) / hz),
        srcs: tally(&sim),
    }
}

pub fn runs(c: &Content, seeds: u64, mv: MoveBot, pk: PickBot, mutant: Option<Mutant>, inj: Option<&Inject>) -> Vec<Run> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(seeds as usize).max(1);
    let mut out: Vec<Run> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|t| s.spawn(move || (1..=seeds).filter(|k| (*k as usize) % threads == t).map(|k| run(c, k, mv, pk, mutant, inj)).collect::<Vec<_>>()))
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    out.sort_by_key(|r| r.seed);
    out
}

pub fn cause_name(c: &Content, o: &Outcome) -> String {
    match o {
        Outcome::Win => "-".into(),
        Outcome::Timeout => "timeout".into(),
        Outcome::Dead { cause, foreseen } => {
            let f = if *foreseen { "" } else { " (unseen)" };
            match cause {
                Cause::Contact { fam, tier } => format!("{} {:?}{f}", c.fam(*fam).name, tier),
                Cause::Shot { fam } => format!("{} shot{f}", c.fam(*fam).name),
                Cause::BossAttack { fam } => format!("{} attack{f}", c.fam(*fam).name),
            }
        }
    }
}

fn mmss(s: f64) -> String {
    format!("{:02}:{:02}", (s / 60.0) as u32, (s % 60.0) as u32)
}

fn rho_at(r: &Run, hz: u32, min: f64) -> String {
    let t = (min * 60.0 * f64::from(hz)) as u32;
    r.rho.iter().min_by_key(|(k, _)| k.abs_diff(t)).filter(|(k, _)| k.abs_diff(t) <= hz * 30).map_or("-".into(), |(_, v)| format!("{:.2}", f64::from(*v) / 1000.0))
}

pub fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() { 0.0 } else { v[v.len() / 2] }
}

pub struct Summary {
    pub win: f64,
    pub median_s: f64,
    pub foreseen: f64,
    pub flagged: Vec<String>,
    pub share_max: (String, f64),
    /// optional skill with the highest mean damage share among its takers
    pub top_dmg: (String, f64),
}

pub fn table(c: &Content, rs: &[Run]) -> String {
    let hz = c.hz().get();
    let mut s = String::from("seed result  time  kills  lvl picks rho@3 rho@7.5 rho@12 first-pick death\n");
    for r in rs {
        let res = match r.outcome {
            Outcome::Win => "WIN ",
            Outcome::Dead { .. } => "DEAD",
            Outcome::Timeout => "T/O ",
        };
        s += &format!(
            "{:>4} {res}  {} {:>6} {:>4} {:>5} {:>5} {:>7} {:>6} {:>10} {}\n",
            r.seed,
            mmss(r.secs),
            r.kills,
            r.level,
            r.picks.iter().flatten().count(),
            rho_at(r, hz, 3.0),
            rho_at(r, hz, 7.5),
            rho_at(r, hz, 12.0),
            r.first_pick_s.map_or("-".into(), |v| format!("{v:.1}s")),
            cause_name(c, &r.outcome)
        );
    }
    s
}

pub fn summarize(c: &Content, rs: &[Run]) -> Summary {
    let n = rs.len().max(1) as f64;
    let wins: Vec<&Run> = rs.iter().filter(|r| r.outcome == Outcome::Win).collect();
    let deaths: Vec<&Run> = rs.iter().filter(|r| matches!(r.outcome, Outcome::Dead { .. })).collect();
    let foreseen = deaths.iter().filter(|r| matches!(r.outcome, Outcome::Dead { foreseen: true, .. })).count() as f64 / deaths.len().max(1) as f64;
    let mut flagged = Vec::new();
    let mut share_max = (String::from("-"), 0.0);
    let mut top_dmg = (String::from("-"), 0.0);
    for id in c.skill_ids() {
        let base = c.player.skills.contains(&id);
        let took = |r: &Run| base || r.picks.contains(&Some(id));
        if !wins.is_empty() {
            let share = wins.iter().filter(|r| took(r)).count() as f64 / wins.len() as f64;
            if !base && share > share_max.1 {
                share_max = (c.skill(id).key.clone(), share);
            }
        }
        // degenerate detector: damage share among the runs that took the skill
        let shares: Vec<f64> = rs
            .iter()
            .filter(|r| took(r))
            .map(|r| {
                let total: i64 = r.output.iter().sum::<i64>();
                r.output[id.ix()] as f64 / total.max(1) as f64
            })
            .collect();
        let mean = shares.iter().sum::<f64>() / shares.len().max(1) as f64;
        if !base && mean > top_dmg.1 {
            top_dmg = (c.skill(id).key.clone(), mean);
        }
        if !shares.is_empty() && mean >= c.bars.degenerate_dmg_share && !base {
            flagged.push(format!("{} dmg share {:.0}%", c.skill(id).key, mean * 100.0));
        }
    }
    Summary { win: wins.len() as f64 / n, median_s: median(rs.iter().map(|r| r.secs).collect()), foreseen, flagged, share_max, top_dmg }
}

pub fn summary_text(c: &Content, rs: &[Run], mv: MoveBot, pk: PickBot) -> String {
    let s = summarize(c, rs);
    let b = &c.bars;
    let mut out = format!(
        "SUMMARY {mv:?}+{pk:?} n={}: win {:.0}% | median survival {} | median kills {:.0} | deaths foreseen {:.0}% | boss kills/run {:.2}\n",
        rs.len(),
        s.win * 100.0,
        mmss(s.median_s),
        median(rs.iter().map(|r| f64::from(r.kills)).collect()),
        s.foreseen * 100.0,
        rs.iter().map(|r| r.bosses.len() as f64).sum::<f64>() / rs.len().max(1) as f64,
    );
    let bar = |name: &str, ok: bool, got: String| format!("  bar {name:<34} {got:<12} {}\n", if ok { "PASS" } else { "FAIL" });
    match (mv, pk) {
        (MoveBot::Kiter, PickBot::Tag) => out += &bar("KITER+TAG win 55-75%", (b.kiter_tag_win.0..=b.kiter_tag_win.1).contains(&s.win), format!("{:.0}%", s.win * 100.0)),
        (MoveBot::Kiter, PickBot::Random) => {
            out += &bar("KITER+RANDOM win 15-35%", (b.kiter_random_win.0..=b.kiter_random_win.1).contains(&s.win), format!("{:.0}%", s.win * 100.0))
        }
        (MoveBot::Oracle, _) => out += &bar("ORACLE win >= 95%", s.win >= b.oracle_win_min, format!("{:.0}%", s.win * 100.0)),
        (MoveBot::Kiter, PickBot::Skip) => out += &bar("KITER+NONE median death <= 4:30", s.median_s <= b.none_median_max_s, mmss(s.median_s)),
        (MoveBot::Still, _) => out += &bar("STILL median death <= 5:00 (N1)", s.median_s <= b.still_median_max_s, mmss(s.median_s)),
        _ => {}
    }
    if s.win > 0.0 {
        out += &bar("any skill in winning builds <= 60%", s.share_max.1 <= b.skill_share_max, format!("{} {:.0}%", s.share_max.0, s.share_max.1 * 100.0));
    }
    out += &format!("  degenerate flags: {} (top optional dmg share: {} {:.0}%)\n", if s.flagged.is_empty() { "none".into() } else { s.flagged.join(", ") }, s.top_dmg.0, s.top_dmg.1 * 100.0);
    let mut all: Vec<(String, usize, usize)> = Vec::new();
    for (t, a, r) in rs.iter().flat_map(|r| r.srcs.iter()) {
        match all.iter_mut().find(|x| x.0 == *t) {
            Some(x) => (x.1, x.2) = (x.1 + a, x.2 + r),
            None => all.push((t.clone(), *a, *r)),
        }
    }
    out += &format!("  cmds by src (applied/rejected): {}\n", all.iter().map(|(t, a, r)| format!("{t} {a}/{r}")).collect::<Vec<_>>().join("  "));
    for b in c.bosses.iter().map(|b| &c.fam(b.fam).name) {
        let t: Vec<f64> = rs.iter().flat_map(|r| r.bosses.iter()).filter(|(n, _)| n == b).map(|(_, s)| *s).collect();
        out += &format!("  boss {b}: killed in {}/{} runs, median fight {:.0} s\n", t.len(), rs.len(), median(t.clone()));
    }
    let fp: Vec<f64> = rs.iter().filter_map(|r| r.first_pick_s).collect();
    let army: Vec<f64> = rs.iter().map(|r| r.dealt_hijack as f64 / (r.dealt.iter().sum::<i64>() + r.dealt_hijack).max(1) as f64).collect();
    out += &format!("  first pick median {:.1} s (bar 8-15 s) | median level {:.0} | hijacked-army share of damage {:.0}%\n", median(fp), median(rs.iter().map(|r| f64::from(r.level)).collect()), median(army) * 100.0);
    out
}

/// Negative controls (design sec 14): each must FIRE. Returns (report, all fired).
pub fn controls(c: &Content, seeds: u64) -> (String, bool) {
    let mut out = String::new();
    let mut all = true;
    let mut line = |name: &str, fired: bool, got: String| {
        all &= fired;
        out += &format!("CONTROL {name:<44} {got:<40} {}\n", if fired { "FIRED (ok)" } else { "DID NOT FIRE (bad)" });
    };
    let base = runs(c, seeds, MoveBot::Kiter, PickBot::Tag, None, None);
    let none = runs(c, seeds, MoveBot::Kiter, PickBot::Skip, None, None);
    let (mb, mn) = (median(base.iter().map(|r| r.secs).collect()), median(none.iter().map(|r| r.secs).collect()));
    let earlier = 1.0 - mn / mb.max(1.0);
    line("N2 no-pick bot dies >= 40% earlier", earlier >= c.bars.n2_earlier_min, format!("tag {} none {} = {:.0}% earlier", mmss(mb), mmss(mn), earlier * 100.0));
    let still = runs(c, seeds, MoveBot::Still, PickBot::Tag, None, None);
    let ms = median(still.iter().map(|r| r.secs).collect());
    line("N1 stand-still + autofire dies before 5:00", ms <= c.bars.still_median_max_s, format!("median {}", mmss(ms)));
    let plain = summarize(c, &runs(c, seeds, MoveBot::Kiter, PickBot::Random, None, None));
    let target = c.skill_by_key("chain").or_else(|| c.skill_ids().last());
    if let Some(t) = target {
        let hot = summarize(c, &runs(c, seeds, MoveBot::Kiter, PickBot::Random, Some(Mutant::Plus500(t)), None));
        let key = &c.skill(t).key;
        let hit = hot.flagged.iter().any(|f| f.starts_with(key.as_str()));
        line(&format!("+500% {key} gets flagged (and not without)"), hit && !plain.flagged.iter().any(|f| f.starts_with(key.as_str())), format!("with: [{}] without: [{}]", hot.flagged.join(","), plain.flagged.join(",")));
    }
    let oracle = summarize(c, &runs(c, seeds, MoveBot::Oracle, PickBot::Dps, None, None));
    let zero = summarize(c, &runs(c, seeds, MoveBot::Oracle, PickBot::Dps, Some(Mutant::ZeroTelegraph), None));
    line("0-telegraph boss breaks ORACLE (< 95% win)", zero.win < c.bars.oracle_win_min && zero.win < oracle.win, format!("oracle win {:.0}% -> {:.0}% with 0 lead", oracle.win * 100.0, zero.win * 100.0));
    let rho3 = |rs: &[Run]| median(rs.iter().filter_map(|r| r.rho.iter().find(|(t, _)| *t >= c.hz().get() * 180).map(|(_, v)| f64::from(*v))).collect());
    let short = 30;
    let a = runs(c, short.min(seeds), MoveBot::Kiter, PickBot::Skip, None, None);
    let d = runs(c, short.min(seeds), MoveBot::Kiter, PickBot::Skip, Some(Mutant::DoubleTrashHp), None);
    let ratio = rho3(&d) / rho3(&a).max(1.0);
    line("meter sanity: 2x trash HP raises rho@3 >= 1.5x", ratio >= c.bars.meter_ratio_min, format!("rho@3 {:.2} -> {:.2} (x{ratio:.2})", rho3(&a) / 1000.0, rho3(&d) / 1000.0));
    (out, all)
}

fn tally(sim: &Sim) -> Vec<(String, usize, usize)> {
    let mut out: Vec<(String, usize, usize)> = Vec::new();
    for r in &sim.log {
        let tag = r.src.tag();
        let k = match out.iter().position(|(t, _, _)| *t == tag) {
            Some(k) => k,
            None => {
                out.push((tag, 0, 0));
                out.len() - 1
            }
        };
        if r.ok.is_ok() { out[k].1 += 1 } else { out[k].2 += 1 }
    }
    out
}
