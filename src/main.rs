//! tot: ToT FP1 playable survivor-like (Netrunner) + headless bot harness. Spec: vic/design.md.
mod agentic;
mod bigtext;
mod bot;
mod crew;
mod cam;
mod data;
mod director;
mod face;
mod floorgen;
mod input;
mod iso;
mod json;
mod legacy;
mod peer;
mod play;
mod render;
mod rng;
mod sim;
mod trig;
mod skill;
mod text;
mod tty;

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(serde_json::Error),
    Usage(String),
    /// Content error with blame path: file:line .path: msg
    Data { file: String, line: usize, path: String, msg: String },
    Tty(String),
    TooSmall { need: (u16, u16), have: (u16, u16) },
    NoKitty,
    ControlsFailed,
    /// local Claude session link (cross-session socket protocol)
    Peer(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Json(e) => write!(f, "json: {e}"),
            Error::Usage(s) => write!(f, "usage: {s}\n{USAGE}"),
            Error::Data { file, line, path, msg } => write!(f, "{file}:{line} {path}: {msg}"),
            Error::Tty(s) => write!(f, "tty: {s}"),
            Error::TooSmall { need, have } => write!(f, "terminal {}x{} is smaller than {}x{}; enlarge it", have.0, have.1, need.0, need.1),
            Error::NoKitty => write!(f, "this terminal does not speak the kitty keyboard protocol (needed for held keys). Use ghostty, kitty or rio."),
            Error::ControlsFailed => write!(f, "a negative control did not fire"),
            Error::Peer(s) => write!(f, "claude link: {s}"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

const USAGE: &str = "tot play [--data DIR] [--profile NAME] [--seed N] [--out DIR]
tot schema   (adventure schema docs: every shown message, art and field, where it shows)
tot skill [--data DIR]   (agent skill: write and update adventures; schema + every shipped adventure as examples)
tot agentic [--to PID --brief TEXT --wait-s S]   (no --to: list live local Claude sessions; else ask one for an adventure)
tot batch --to PID --briefs FILE --wait-s S   (ask the session for one adventure per line of FILE, one at a time; nothing is played)
tot crew --to PID --say TEXT [--secs S] [--seed N] [--ansi FILE]   (headless run: your line goes to a live Claude session, its crew of pals and foes joins the fight)
tot snapshot [--seed N] [--secs S] [--cols C] [--rows R] [--ansi FILE] [--pick]
tot bot --seeds N --move kiter|oracle|still|random --pick tag|dps|random|none [--mutant zero_tele|plus500:<skill>|double_hp] [--cmds FILE --src mcp:NAME] [--quiet] [--data DIR]
tot controls --seeds N [--data DIR]";

struct Args(Vec<String>);

impl Args {
    fn val(&self, k: &str) -> Option<&str> {
        self.0.iter().position(|a| a == k).and_then(|i| self.0.get(i + 1)).map(String::as_str)
    }
    fn num(&self, k: &str, d: u64) -> Result<u64> {
        self.val(k).map_or(Ok(d), |v| v.parse().map_err(|_| Error::Usage(format!("{k} {v}: not a number"))))
    }
    fn has(&self, k: &str) -> bool {
        self.0.iter().any(|a| a == k)
    }
}

fn run() -> Result<()> {
    let args = Args(std::env::args().skip(1).collect());
    if matches!(args.0.first().map(String::as_str), Some("--help" | "-h")) {
        println!("{USAGE}");
        return Ok(());
    }
    let dir = PathBuf::from(args.val("--data").unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/data")));
    let lib = data::Library::with_user(&dir);
    let c = lib.load(args.val("--profile"))?;
    match args.0.first().map(String::as_str) {
        Some("bot") => {
            let mv = bot::MoveBot::parse(args.val("--move").unwrap_or("kiter"))?;
            let pk = bot::PickBot::parse(args.val("--pick").unwrap_or("tag"))?;
            let mutant = match args.val("--mutant") {
                None => None,
                Some("zero_tele") => Some(sim::Mutant::ZeroTelegraph),
                Some("double_hp") => Some(sim::Mutant::DoubleTrashHp),
                Some(m) => match m.strip_prefix("plus500:").and_then(|k| c.skill_by_key(k)) {
                    Some(s) => Some(sim::Mutant::Plus500(s)),
                    None => return Err(Error::Usage(format!("--mutant {m}"))),
                },
            };
            let inj = match args.val("--cmds") {
                None => None,
                Some(p) => {
                    let src = args.val("--src").unwrap_or("mcp:file");
                    let src = sim::Src::parse(src).ok_or_else(|| Error::Usage(format!("--src {src}: builtin|fallback|human|mcp:<a-z0-9_->")))?;
                    let mut cmds = director::parse_cmds(&c, &std::fs::read_to_string(p)?)?;
                    cmds.sort_by_key(|(t, _)| *t);
                    Some(bot::Inject { cmds, src })
                }
            };
            let rs = bot::runs(&c, args.num("--seeds", 10)?, mv, pk, mutant, inj.as_ref());
            if !args.has("--quiet") {
                print!("{}", bot::table(&c, &rs));
            }
            print!("{}", bot::summary_text(&c, &rs, mv, pk));
            Ok(())
        }
        Some("play") => {
            let auto = if args.has("--auto") {
                Some((bot::MoveBot::parse(args.val("--move").unwrap_or("kiter"))?, bot::PickBot::parse(args.val("--pick").unwrap_or("tag"))?))
            } else {
                None
            };
            play::play(&c, &lib, args.val("--seed").map(|_| args.num("--seed", 0)).transpose()?, &PathBuf::from(args.val("--out").unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/results"))), auto)
        }
        Some("agentic" | "batch") => agentic_cli(&lib, &args),
        Some("crew") => crew_cli(&c, &args),
        Some("skill") => {
            print!("{}", skill::skill(&dir)?);
            Ok(())
        }
        Some("schema") => {
            print!("{}", data::schema_doc());
            Ok(())
        }
        Some("snapshot") => {
            let (mut sim, mut dir) = bot::sim_at(&c, args.num("--seed", 1)?, args.num("--secs", 60)? as u32);
            let mut ui = input::Ui::new(true);
            // --pick: play on (auto-fire, no picks) until the next power-up offer, at most 2 minutes, and show it open
            if args.has("--pick") {
                let until = sim.secs() + 120;
                let mut pilot = bot::Autopilot::new(args.num("--seed", 1)?, bot::MoveBot::Kiter, bot::PickBot::Skip);
                while sim.offer.is_none() && sim.outcome.is_none() && sim.secs() < until {
                    dir.tick(&mut sim);
                    let (mv, dodge) = pilot.walk(&sim);
                    sim.step(&sim::Input { mv, dodge, autofire: true, ..sim::Input::default() });
                }
                ui.sync(sim.offer.is_some());
                if ui.offer_waiting {
                    ui.mode = input::Mode::Pick;
                }
            }
            for _ in 0..args.num("--cam", 0)? % 4 {
                ui.heading = ui.heading.right();
            }
            let mut f = render::Frame::new(&c, args.num("--cols", 160)? as u16, args.num("--rows", 45)? as u16);
            f.compose(&sim, &render::View { ui: &ui, now_ms: 0, alpha: 1.0, phase: dir.phase_name(), last_bytes: 0 });
            // --turn MS: press e at 0 ms, show the frame MS into the animated turn
            if let Some(ms) = args.val("--turn").map(|_| args.num("--turn", 0)).transpose()? {
                ui.heading = ui.heading.right();
                for now_ms in [1, ms + 1] {
                    f.compose(&sim, &render::View { ui: &ui, now_ms, alpha: 1.0, phase: dir.phase_name(), last_bytes: 0 });
                }
                f.invalidate();
            }
            let n = f.emit();
            match args.val("--ansi") {
                Some(p) => std::fs::write(p, &f.out)?,
                None => print!("{}", f.plain()),
            }
            eprintln!("t={}s live={} cells={n} bytes={}", sim.secs(), sim.live(), f.out.len());
            Ok(())
        }
        Some("controls") => {
            let (text, ok) = bot::controls(&c, args.num("--seeds", 20)?);
            print!("{text}");
            if ok { Ok(()) } else { Err(Error::ControlsFailed) }
        }
        _ => Err(Error::Usage("no command".into())),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("tot: {e}");
        std::process::exit(1);
    }
}

/// `agentic`: list the live local Claude sessions, or ask one (`--to PID --brief TEXT --wait-s S`) for an
/// adventure and wait for it, as the start screen does.
/// `tot crew --to PID --say TEXT --secs S [--seed N] [--ansi FILE]`: a headless run (the autopilot plays the hero in
/// real time) where the player's line goes to a live local Claude session and its crew joins the fight. Prints the
/// chat and the crew as it goes; --ansi writes the last frame.
fn crew_cli(c: &data::Content, args: &Args) -> Result<()> {
    let (reg, socks) = peer::default_dirs().ok_or_else(|| Error::Usage("HOME and XDG_RUNTIME_DIR must be set".into()))?;
    let pid = args.val("--to").ok_or_else(|| Error::Usage("--to PID (tot agentic lists the live sessions)".into()))?;
    let to = peer::sessions(&reg, std::path::Path::new("/proc")).into_iter().find(|s| s.pid.to_string() == pid).ok_or_else(|| Error::Usage(format!("no live Claude session with pid {pid}")))?;
    let say = args.val("--say").ok_or_else(|| Error::Usage("--say TEXT".into()))?;
    let seed = args.num("--seed", 1)?;
    let (mut sim, mut dir) = bot::sim_at(c, seed, 0);
    let mut pilot = bot::Autopilot::new(seed, bot::MoveBot::Kiter, bot::PickBot::Tag);
    let mut link = crew::Link::open(&socks)?;
    link.prompt(&to, say, &sim)?;
    println!("you: {say}");
    let tick = std::time::Duration::from_micros(1_000_000 / u64::from(c.hz().get()));
    let end = sim.secs() + args.num("--secs", 120)? as u32;
    while sim.secs() < end && sim.outcome.is_none() {
        dir.tick(&mut sim);
        let (mv, dodge) = pilot.walk(&sim);
        let pick = sim.offer.map(|_| pilot.choose(&sim));
        sim.step(&sim::Input { mv, dodge, autofire: true, pick, ..sim::Input::default() });
        for h in link.poll(&mut sim) {
            // headless: a scene card is printed and its first choice taken (there is no player to ask)
            if let crew::Heard::Applied(crew::Order::Scene(s)) = &h {
                println!("[scene {}] {}: {}", s.key.as_str(), s.title.as_str(), s.lines.iter().map(|l| l.as_str()).collect::<Vec<_>>().join(" / "));
                link.chose(s.key, s.choices.first().map(|c| c.0));
            }
            match h {
                crew::Heard::Applied(o) => match crew::chat_line(&sim, &o) {
                    Some((who, text)) => println!("{who}: {text}"),
                    None => println!("applied: {o:?}"),
                },
                crew::Heard::Refused(r) => println!("refused: {r}"),
            }
        }
        for e in sim.crew_ev.iter().filter(|e| matches!(e, sim::CrewEv::Joined(_) | sim::CrewEv::Down(_) | sim::CrewEv::Raised { .. })) {
            println!("[{}s] {e:?}", sim.secs());
        }
        // what the agents hear in play too: quests done, levels entered, people met
        for k in std::mem::take(&mut sim.quest_ev) {
            if let Some(q) = sim.quests.iter().find(|q| q.key == k) {
                println!("[{}s] quest done: {}", sim.secs(), q.title.as_str());
                link.quest_done(k.as_str(), q.title.as_str());
            }
        }
        for l in std::mem::take(&mut sim.level_ev) {
            link.entered(&render::level_name(&sim, l));
        }
        for k in std::mem::take(&mut sim.people_ev) {
            if let Some(p) = sim.people.iter().find(|p| p.key == k) {
                link.met(p.name.as_str());
            }
        }
        if link.tick(&mut sim)? {
            println!("[{}s] report sent: {}", sim.secs(), crew::snapshot(&sim));
        }
        std::thread::sleep(tick);
    }
    println!("end at {}s: {:?} kills {} crew {}", sim.secs(), sim.outcome, sim.stats.kills, crew::snapshot(&sim));
    if let Some(p) = args.val("--ansi") {
        let ui = input::Ui::new(true);
        let mut f = render::Frame::new(c, 160, 45);
        f.compose(&sim, &render::View { ui: &ui, now_ms: 0, alpha: 1.0, phase: dir.phase_name(), last_bytes: 0 });
        f.emit();
        std::fs::write(p, &f.out)?;
    }
    Ok(())
}

fn agentic_cli(lib: &data::Library, args: &Args) -> Result<()> {
    let (reg, socks) = peer::default_dirs().ok_or_else(|| Error::Usage("HOME and XDG_RUNTIME_DIR must be set".into()))?;
    let live = peer::sessions(&reg, std::path::Path::new("/proc"));
    let Some(pid) = args.val("--to") else {
        for s in &live {
            println!("{} {}{} {}", s.pid, s.name, if s.busy { " (busy)" } else { "" }, s.cwd);
        }
        return Ok(());
    };
    let to = live.into_iter().find(|s| s.pid.to_string() == pid).ok_or_else(|| Error::Usage(format!("no live Claude session with pid {pid}")))?;
    let wait = std::time::Duration::from_secs(args.val("--wait-s").ok_or_else(|| Error::Usage("--wait-s S".into()))?.parse().map_err(|_| Error::Usage("--wait-s: not a number".into()))?);
    let Some(file) = args.val("--briefs") else {
        let brief = args.val("--brief").ok_or_else(|| Error::Usage("--brief TEXT or --briefs FILE".into()))?;
        println!("{}", fetch(lib, &socks, to, brief, wait, true)?);
        return Ok(());
    };
    // one adventure per non-empty line, each asked only once the one before is installed or failed
    let text = std::fs::read_to_string(file)?;
    let briefs: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let mut failed = 0;
    for (n, brief) in briefs.iter().enumerate() {
        eprintln!("[{}/{}] {brief}", n + 1, briefs.len());
        match fetch(lib, &socks, to.clone(), brief, wait, n == 0) {
            Ok(id) => println!("{id}"),
            Err(e) => {
                failed += 1;
                eprintln!("[{}/{}] failed: {e}", n + 1, briefs.len());
            }
        }
    }
    if failed > 0 { Err(Error::Peer(format!("{failed} of {} adventures failed", briefs.len()))) } else { Ok(()) }
}

/// Ask `to` for the adventure `brief` describes and wait up to `wait` for it to install. Ok(its id).
fn fetch(lib: &data::Library, socks: &std::path::Path, to: peer::Session, brief: &str, wait: std::time::Duration, with_skill: bool) -> Result<String> {
    let mut link = agentic::Link::ask(lib, socks, to, brief, with_skill)?;
    eprintln!("asked; accept the request in that Claude Code session");
    let start = std::time::Instant::now();
    while start.elapsed() < wait {
        match link.poll(lib)? {
            agentic::Heard::Installed(id) => return Ok(id),
            agentic::Heard::Refused(why) => eprintln!("refused, sent back: {why}"),
            agentic::Heard::Nothing => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    }
    Err(Error::Peer(format!("no adventure within {} s", wait.as_secs())))
}
