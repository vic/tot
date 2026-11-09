//! Human play loop + session telemetry (results/play-<unix>.jsonl).
use crate::bot::{Autopilot, MoveBot, PickBot, cause_name};
use crate::data::Content;
use crate::director::Director;
use crate::input::{Event, Mode, Ui};
use crate::render::{Frame, View};
use crate::sim::{Outcome, PickChoice, Sim};
use crate::tty::Tty;
use crate::{Error, Result};
use serde_json::json;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Telemetry {
    file: std::fs::File,
    decide_ms: Vec<u64>,
}

impl Telemetry {
    fn line(&mut self, v: &serde_json::Value) -> Result<()> {
        let mut s = serde_json::to_string(v)?;
        s.push('\n');
        self.file.write_all(s.as_bytes())?;
        Ok(())
    }

    fn run_end(&mut self, c: &Content, sim: &Sim, run: u32, seed: u64, how: &str) -> Result<()> {
        let hz = f64::from(c.hz().get());
        let secs = f64::from(sim.tick) / hz;
        let picks = sim.stats.picks.iter().filter(|p| p.skill.is_some()).count();
        let in_min = |a: f64, b: f64| sim.stats.picks.iter().filter(|p| (a..b).contains(&(f64::from(p.tick) / hz / 60.0))).count();
        let mut d = std::mem::take(&mut self.decide_ms);
        d.sort_unstable();
        let (cause, foreseen) = match sim.outcome {
            Some(o @ Outcome::Dead { foreseen, .. }) => (cause_name(c, &o), Some(foreseen)),
            _ => ("-".into(), None),
        };
        let rows: Vec<serde_json::Value> = sim.log.iter().map(|r| json!([r.tick, r.src.tag(), format!("{:?}", r.cmd), r.ok.map_or_else(|e| format!("{e:?}"), |()| "ok".into())])).collect();
        self.line(&json!({"t": "runlog", "run": run, "rows": rows}))?;
        self.line(&json!({
            "t": "run", "run": run, "seed": format!("{seed:016x}"), "end": how, "run_s": secs, "cause": cause, "death_foreseen": foreseen,
            "time_to_first_pick_s": sim.stats.first_pick.map(|t| f64::from(t) / hz),
            "picks": picks, "picks_per_min": picks as f64 / (secs / 60.0).max(1.0 / 60.0),
            "picks_min_0_1": in_min(0.0, 1.0), "picks_min_1_4": in_min(1.0, 4.0), "picks_min_9_15": in_min(9.0, 15.0),
            "pick_decide_median_s": d.get(d.len() / 2).map(|m| *m as f64 / 1000.0),
            "kills": sim.stats.kills, "level": sim.p.level, "hijacked_peak": sim.stats.hijacked_peak,
            "rarity_taken": ([0u8, 1, 2, 3].map(|r| sim.stats.picks.iter().filter(|p| p.skill.is_some() && p.rarity == r).count())),
            "arena": format!("{:?}", sim.arena().template),
            "dmg_taken": sim.stats.taken / 1000,
            "rho": sim.stats.rho.iter().map(|(t, r)| [f64::from(*t) / hz, f64::from(*r) / 1000.0]).collect::<Vec<_>>(),
            "options": {"pick_pauses": c.opt.pick_pauses, "tick_hz": c.hz().get(), "rewind": format!("{:?}", c.opt.rewind), "rewind_charges": c.opt.rewind_charges, "backend": format!("{:?}", c.opt.backend), "hot_numbers": format!("{:?}", c.opt.hot_numbers), "whichkey_ms": c.opt.whichkey_ms},
            "state_hash": format!("{:016x}", sim.hash()),
            "cmds": sim.log.len(), "cmds_rejected": sim.log.iter().filter(|r| r.ok.is_err()).count(),
        }))
    }
}

/// Run seed from OS entropy (design sec 7.2: runs never equal; replay reads the saved seed).
pub fn entropy_seed() -> Result<u64> {
    let mut b = [0u8; 8];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

pub fn play(c: &Content, seed0: Option<u64>, out_dir: &Path, auto: Option<(MoveBot, PickBot)>) -> Result<()> {
    let mut tty = Tty::open()?;
    if !tty.probe_kitty()? {
        return Err(Error::NoKitty);
    }
    let need = (c.view.min_cols, c.view.min_rows);
    let mut size = tty.size()?;
    if size.0 < need.0 || size.1 < need.1 {
        return Err(Error::TooSmall { need, have: size });
    }
    std::fs::create_dir_all(out_dir)?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let path = out_dir.join(format!("play-{ts}.jsonl"));
    let mut tel = Telemetry { file: std::fs::File::create(&path)?, decide_ms: Vec::new() };
    let start = Instant::now();
    let now_ms = || start.elapsed().as_millis() as u64;
    let tick = Duration::from_micros(1_000_000 / u64::from(c.hz().get()));
    let mut run = 1;
    let mut seed = match seed0 {
        Some(s) => s,
        None => entropy_seed()?,
    };
    tel.line(&json!({"t": "head", "schema_version": 1, "file": "play", "run_seed": format!("{seed:016x}"), "seed_source": if seed0.is_some() { "flag" } else { "entropy" }}))?;
    let mut sim = Sim::new(c, seed, None);
    let mut dir = Director::new(seed);
    let mut ui = Ui::new(c.opt.autofire);
    // demo: `--auto` plays itself, restarts a new run 4 s after each end
    let (auto_mv, auto_pk) = auto.unwrap_or((MoveBot::Kiter, PickBot::Skip));
    let mut pilot = Autopilot::new(seed, auto_mv, auto_pk);
    ui.autowalk = auto.is_some();
    ui.autofire |= auto.is_some();
    let mut ended_at = Instant::now();
    let mut frame = Frame::new(c, size.0, size.1);
    let mut next = Instant::now();
    // render clock: up to view.fps, decoupled from the fixed sim tick (render interpolates)
    let frame_dt = Duration::from_micros(1_000_000 / u64::from(c.view.fps));
    let mut next_frame = Instant::now();
    let mut last_bytes = 0;
    let mut last_size_check = Instant::now();
    let mut ended = false;
    let mut pending: Vec<Event> = Vec::new();
    loop {
        pending.extend(tty.events());
        let mut restart = auto.is_some() && ended && ended_at.elapsed() > Duration::from_secs(4);
        for e in pending.drain(..) {
            if let Event::Click(x, y) = e
                && let Some(url) = frame.link_at(x, y)
            {
                ui.msg = open_url(url);
            }
            if let Event::Key(k) = e {
                if ended && k.kind == crate::input::Kind::Press {
                    match k.key {
                        crate::input::Key::Char('r') => restart = true,
                        crate::input::Key::Char('q') | crate::input::Key::Esc => ui.quit = true,
                        _ => {}
                    }
                } else {
                    ui.key(k, c, now_ms());
                }
            }
        }
        if restart {
            run += 1;
            seed = entropy_seed()?;
            tel.line(&json!({"t": "seed", "run": run, "run_seed": format!("{seed:016x}")}))?;
            sim = Sim::new(c, seed, None);
            dir = Director::new(seed);
            ui = ui.fresh();
            pilot = Autopilot::new(seed, auto_mv, auto_pk);
            ended = false;
            frame.reset_run();
            frame.invalidate();
            tty.send(b"\x1b[2J")?;
        }
        if ui.quit {
            if !ended {
                tel.run_end(c, &sim, run, seed, "quit")?;
            }
            break;
        }
        if last_size_check.elapsed() > Duration::from_secs(1) {
            last_size_check = Instant::now();
            let s = tty.size()?;
            if s != size && s.0 >= need.0 && s.1 >= need.1 {
                size = s;
                frame = Frame::new(c, size.0, size.1);
                tty.send(b"\x1b[2J")?;
            }
        }
        // shortcut: points = levels gained, owned nodes live in Ui until the sim holds them
        ui.tree_earned = sim.p.level.saturating_sub(1);
        ui.sync(sim.offer.is_some(), now_ms());
        ui.sync_shop(sim.world.open_shop.and_then(|i| sim.world.shops.get(i)).map(Vec::len));
        if Instant::now() >= next {
            next += tick;
            if Instant::now() > next + tick * 4 {
                next = Instant::now() + tick; // fell behind (suspend etc): do not spiral
            }
            if !ended {
                if let Some(cmd) = ui.human.take() {
                    ui.msg = match sim.command(crate::sim::Src::Human, cmd) {
                        Ok(()) => "applied (src human)".into(),
                        Err(e) => format!("rejected: {e:?}"),
                    };
                }
                let pick_open = ui.pick_open_ms;
                dir.tick(&mut sim);
                let mut inp = ui.input(c.opt.pick_pauses);
                // autowalk: bot movement unless the human holds a direction
                if ui.autowalk && inp.mv == (0, 0) {
                    let (mv, dodge) = pilot.walk(&sim);
                    inp.mv = mv;
                    inp.dodge |= dodge;
                }
                if auto.is_some() && sim.offer.is_some() {
                    inp.pick = Some(pilot.choose(&sim));
                }
                // upgrade timeout: no choice inside the time limit takes the highlighted card
                if ui.mode == Mode::Pick && sim.offer.is_some() && inp.pick.is_none() && now_ms().saturating_sub(ui.pick_open_ms) >= sim.pick_timeout_ms() {
                    inp.pick = Some(PickChoice::Card(ui.sel));
                }
                if let (Some(ch), Some(offer)) = (inp.pick, sim.offer) {
                    let card = match ch {
                        crate::sim::PickChoice::Card(i) => offer.cards.get(usize::from(i)).copied().flatten(),
                        crate::sim::PickChoice::Skip => None,
                    };
                    let ms = now_ms().saturating_sub(pick_open);
                    tel.decide_ms.push(ms);
                    tel.line(&json!({
                        "t": "pick", "run": run, "at_s": f64::from(sim.tick) / f64::from(c.hz().get()),
                        "skill": card.map(|k| c.skill(k.skill).key.clone()), "rarity": card.map(|k| k.rarity),
                        "rank": card.map(|k| k.new_rank), "chest": offer.chest, "decide_s": ms as f64 / 1000.0,
                    }))?;
                }
                sim.step(&inp);
                if sim.tick.is_multiple_of(c.hz().get() * 30) {
                    tel.line(&json!({"t": "alive", "run": run, "run_s": sim.secs(), "kills": sim.stats.kills, "level": sim.p.level, "hp": sim.p.hp / 1000}))?;
                }
                if let Some(o) = sim.outcome {
                    ended = true;
                    ended_at = Instant::now();
                    tel.run_end(c, &sim, run, seed, if o == Outcome::Win { "win" } else { "end" })?;
                    ui.msg = match o {
                        Outcome::Win => "SWEEPER-9 DOWN. you win. r = new run, q = quit".into(),
                        _ => format!("DEAD: {}. r = new run, q = quit", cause_name(c, &o)),
                    };
                }
            }
        }
        if Instant::now() >= next_frame {
            next_frame = (next_frame + frame_dt).max(Instant::now());
            // place of this frame between the last tick (next - tick) and the coming one (next)
            let alpha = 1.0 - next.saturating_duration_since(Instant::now()).as_secs_f64() / tick.as_secs_f64();
            frame.compose(&sim, &View { ui: &ui, now_ms: now_ms(), alpha, phase: dir.phase_name(), last_bytes });
            if frame.emit() > 0 {
                last_bytes = frame.out.len();
                tty.send(&frame.out)?;
            }
        }
        let wait = next.min(next_frame).saturating_duration_since(Instant::now());
        pending = tty.wait(wait);
    }
    drop(tty);
    println!("telemetry: {}", path.display());
    Ok(())
}

/// Openers tried in order: the desktop's default, then browsers by name.
const OPENERS: [&str; 7] = ["xdg-open", "open", "firefox", "chromium", "chromium-browser", "google-chrome", "chrome"];

/// Open `url` with the first opener found in PATH, detached (no shell, output discarded). Returns the status line.
fn open_url(url: &str) -> String {
    let Some(bin) = opener(&std::env::var_os("PATH").unwrap_or_default()) else {
        return format!("no browser found: open {url}");
    };
    let null = std::process::Stdio::null;
    match std::process::Command::new(&bin).arg(url).stdin(null()).stdout(null()).stderr(null()).spawn() {
        Ok(_) => format!("opening {url} - thank you!"),
        Err(e) => format!("could not open {url}: {e}"),
    }
}

/// First of OPENERS that exists as a file in the PATH list `path`.
fn opener(path: &std::ffi::OsStr) -> Option<std::path::PathBuf> {
    OPENERS.iter().find_map(|o| std::env::split_paths(path).map(|d| d.join(o)).find(|p| p.is_file()))
}

#[cfg(test)]
mod open_tests {
    use super::*;

    #[test]
    fn opener_prefers_xdg_open_and_falls_back_to_browsers() {
        let dir = std::env::temp_dir().join(format!("tot-open-{}", std::process::id()));
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("firefox"), "").unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();
        assert_eq!(opener(&path), Some(b.join("firefox")), "falls back to a browser");
        std::fs::write(a.join("xdg-open"), "").unwrap();
        assert_eq!(opener(&path), Some(a.join("xdg-open")), "xdg-open first");
        assert_eq!(opener(std::ffi::OsStr::new("")), None, "NEG: empty PATH");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
