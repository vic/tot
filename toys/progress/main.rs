//! T4 toy (design v5 sec 22.1-22.5, 22.8): persistent hero over <= 30-min runs. Monte Carlo of
//! hero level, act / DEFCON progress, item upgrades, tree breakpoints. Fits the XP curve to targets.
//! std only; f64 model (design math, not sim). All numbers [E] = knobs.
const HEROES: usize = 200;
const RUNS: usize = 160;
const SLOTS: usize = 9;
const ACT_BASE: [f64; 5] = [1.0, 8.0, 15.0, 22.0, 29.0];
const DEFCON_OFF: [f64; 3] = [0.0, 30.0, 60.0];

struct R(u64);
impl R {
    fn f(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}
#[derive(Clone, Copy)]
struct Curve {
    base: f64,     // XP to go L1 -> L2
    g: [f64; 3],   // per-level growth in bands L1-10, L11-30, L31-60
}
impl Curve {
    fn need(&self, l: u32) -> f64 {
        let l = l as i32 - 1;
        self.base * self.g[0].powi(l.min(9)) * self.g[1].powi((l - 9).clamp(0, 20)) * self.g[2].powi((l - 29).max(0))
    }
}
/// kills by minute t (design sec 11.4 K(t))
fn kills(t: f64) -> f64 {
    60.0 * (t + 0.175 * t * t)
}
/// XP per kill grows with monster level; D2-shape penalty when hero outlevels monsters
fn xpk(mlvl: f64, clvl: f64) -> f64 {
    let pen = if clvl > mlvl + 5.0 { (1.0 - 0.1 * (clvl - mlvl - 5.0)).max(0.05) } else { 1.0 };
    1.07f64.powf(mlvl) * pen
}
#[derive(Default, Clone)]
struct Run {
    level: f64,
    hours: f64,
    defcon: f64,
    upgrades: f64,
    breakpoints: f64,
}
/// one hero's career; returns per-run snapshots
fn career(c: Curve, seed: u64) -> Vec<Run> {
    let mut r = R(seed.wrapping_mul(0x9e3779b97f4a7c15) | 1);
    let (mut lvl, mut xp, mut hours) = (1u32, 0.0f64, 0.0f64);
    let (mut act, mut defcon) = (0usize, 0usize);
    let mut gear = [0.0f64; SLOTS];
    let mut ranks = [0u32; 3]; // focused build: 3 main skills
    let mut out = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let mlvl = ACT_BASE[act] + 2.0 + DEFCON_OFF[defcon]; // mid-act floor
        // power: hero level + gear vs monster level => win chance (logistic)
        let gp = gear.iter().sum::<f64>() / SLOTS as f64;
        let power = lvl as f64 * 0.6 + gp * 0.4;
        let pwin = 1.0 / (1.0 + (-(power - mlvl - 1.0) / 2.5).exp());
        let win = r.f() < pwin;
        let t = if win { 30.0 } else { 8.0 + 20.0 * r.f() };
        hours += t / 60.0;
        let mut gain = kills(t) * xpk(mlvl, lvl as f64);
        let mut bps = 0;
        while gain > 0.0 && lvl < 60 {
            let n = c.need(lvl) - xp;
            if gain >= n {
                gain -= n;
                xp = 0.0;
                lvl += 1;
                // spend: lowest-rank main skill first; breakpoint at ranks 1, 5, 10, 15, 20
                if let Some(k) = (0..3).filter(|&k| ranks[k] < 20).min_by_key(|&k| ranks[k]) {
                    ranks[k] += 1;
                    if [1, 5, 10, 15, 20].contains(&ranks[k]) {
                        bps += 1;
                    }
                }
            } else {
                xp += gain;
                gain = 0.0;
            }
        }
        // drops (design sec 22.8 cadence): magic+ every ~60 s, rare ~3 min, unique per boss kill
        let mut ups = 0;
        let drops = [(t, 1.0), (t / 3.0, 1.25), (if win { 1.0 } else { 0.0 }, 1.5)];
        for (n, mult) in drops {
            let k = n.floor() as usize + usize::from(r.f() < n.fract());
            for _ in 0..k {
                let s = (r.f() * SLOTS as f64) as usize % SLOTS;
                let p = mlvl * mult * (0.8 + 0.2 * r.f());
                if p > gear[s] * 1.02 {
                    gear[s] = p;
                    ups += 1;
                }
            }
        }
        if win {
            if act < 4 {
                act += 1;
            } else if defcon < 2 {
                defcon += 1;
                act = 0;
            }
        }
        out.push(Run { level: lvl as f64, hours, defcon: defcon as f64 + 1.0, upgrades: ups as f64, breakpoints: bps as f64 });
    }
    out
}
fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}
/// median hours to reach level L (inf if never)
fn hours_to(all: &[Vec<Run>], l: f64) -> f64 {
    median(all.iter().map(|c| c.iter().find(|r| r.level >= l).map_or(f64::INFINITY, |r| r.hours)).collect())
}
fn sim(c: Curve) -> Vec<Vec<Run>> {
    (0..HEROES).map(|h| career(c, h as u64 + 1)).collect()
}
/// log error vs targets: L10 by 0.5 h (inside run 1), L30 by 8 h, L60 by 40 h
fn err(all: &[Vec<Run>]) -> f64 {
    [(10.0, 0.5), (30.0, 8.0), (60.0, 40.0)].iter().map(|&(l, h)| (hours_to(all, l) / h).ln().powi(2)).sum::<f64>()
}
/// coordinate descent over base + 3 band growths; flat = all growths 1.0 (NEG)
fn fit(flat: bool) -> (Curve, f64) {
    let mut c = Curve { base: 100.0, g: [1.0; 3] };
    let mut best = err(&sim(c));
    let bases: Vec<f64> = (0..48).map(|k| 5.0 * 1.25f64.powi(k)).collect();
    let gs: Vec<f64> = (0..=40).map(|k| 1.0 + 0.01 * k as f64).collect();
    for _ in 0..4 {
        for &b in &bases {
            let t = Curve { base: b, ..c };
            let e = err(&sim(t));
            if e < best {
                (c, best) = (t, e);
            }
        }
        for k in 0..3 {
            for &g in gs.iter().filter(|_| !flat) {
                let mut t = c;
                t.g[k] = g;
                let e = err(&sim(t));
                if e < best {
                    (c, best) = (t, e);
                }
            }
        }
    }
    (c, best)
}
fn main() {
    let (c, e) = fit(false);
    let all = sim(c);
    println!("FIT need(L) = {:.0} x growth per level {:.2} (L1-10) {:.2} (L11-30) {:.2} (L31-60)  log-err {e:.3}", c.base, c.g[0], c.g[1], c.g[2]);
    println!("hours to L10 {:.2} | L30 {:.1} | L60 {:.1}  (targets 0.5 / 8 / 40)", hours_to(&all, 10.0), hours_to(&all, 30.0), hours_to(&all, 60.0));
    let col = |f: fn(&Run) -> f64, i: usize| median(all.iter().map(|c| f(&c[i])).collect());
    println!("run | hours | level | DEFCON | upgrades this run | breakpoints this run   (medians of {HEROES} heroes)");
    for i in [0, 1, 2, 4, 9, 14, 19, 29, 39, 59, 79, 99, 119, 159] {
        println!("{:>3} | {:>5.1} | {:>5.0} | {:>6.0} | {:>17.0} | {:>4.0}", i + 1, col(|r| r.hours, i), col(|r| r.level, i), col(|r| r.defcon, i), col(|r| r.upgrades, i), col(|r| r.breakpoints, i));
    }
    // means over windows (medians of integer counts hide cadence)
    let win = |a: usize, b: usize, f: fn(&Run) -> f64| all.iter().map(|c| c[a..b].iter().map(f).sum::<f64>() / (b - a) as f64).sum::<f64>() / HEROES as f64;
    let (u_early, u_late) = (win(0, 10, |r| r.upgrades), win(100, 120, |r| r.upgrades));
    let bp_h = |a: usize, b: usize| {
        let bp: f64 = all.iter().map(|c| c[a..b].iter().map(|r| r.breakpoints).sum::<f64>()).sum::<f64>() / HEROES as f64;
        let h: f64 = all.iter().map(|c| c[b - 1].hours - c[a].hours).sum::<f64>() / HEROES as f64;
        h / bp.max(1e-9)
    };
    let d2 = median(all.iter().map(|c| c.iter().find(|r| r.defcon >= 2.0).map_or(f64::INFINITY, |r| r.hours)).collect());
    let d1 = median(all.iter().map(|c| c.iter().find(|r| r.defcon >= 3.0).map_or(f64::INFINITY, |r| r.hours)).collect());
    println!("upgrades/run mean: runs 1-10 {u_early:.2} | runs 101-120 {u_late:.2}   (bands >= 2.0 / >= 0.5)");
    println!("hours per tree breakpoint: runs 1-20 {:.2} | runs 21-120 {:.2}   (band late <= 1.5)", bp_h(0, 20), bp_h(20, 120));
    println!("hours to unlock DEFCON 2 {d2:.1} | DEFCON 1 {d1:.1}");
    let ok = e < 0.15 && u_early >= 2.0 && u_late >= 0.5 && bp_h(20, 120) <= 1.5;
    println!("BANDS {}", if ok { "PASS" } else { "FAIL" });
    // NEG control: flat curve (growth 1.00 only) must fail the level fit
    let (cf, ef) = fit(true);
    let af = sim(cf);
    println!("NEG flat need {:.0}: hours L10 {:.2} L30 {:.1} L60 {:.1} log-err {ef:.3} => {}", cf.base, hours_to(&af, 10.0), hours_to(&af, 30.0), hours_to(&af, 60.0), if ef < 0.15 { "PASS (control NOT armed)" } else { "FAIL (armed)" });
}
