//! proto2: ToT FP1 playable survivor-like (Netrunner) + headless bot harness. Spec: vic/design.md.
mod bigtext;
mod bot;
mod cam;
mod data;
mod director;
mod face;
mod floorgen;
mod input;
mod iso;
mod json;
mod play;
mod render;
mod rng;
mod sim;
mod trig;
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

const USAGE: &str = "proto2 play [--data DIR] [--profile NAME] [--seed N] [--out DIR]
proto2 schema   (adventure schema docs: every shown message, art and field, where it shows)
proto2 snapshot [--seed N] [--secs S] [--cols C] [--rows R] [--ansi FILE] [--pick]
proto2 bot --seeds N --move kiter|oracle|still|random --pick tag|dps|random|none [--mutant zero_tele|plus500:<skill>|double_hp] [--cmds FILE --src mcp:NAME] [--quiet] [--data DIR]
proto2 controls --seeds N [--data DIR]";

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
    let dir = PathBuf::from(args.val("--data").unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/data")));
    let c = data::Content::load_profile(&dir, args.val("--profile"))?;
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
            play::play(&c, &dir, args.val("--seed").map(|_| args.num("--seed", 0)).transpose()?, &PathBuf::from(args.val("--out").unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/results"))), auto)
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
        eprintln!("proto2: {e}");
        std::process::exit(1);
    }
}
