//! proto1: ToT terminal render-budget prototype (throwaway). Plan: vic/notes/proto1-plan.md.
mod analyze;
mod grid;
mod keys;
mod kitty;
mod scene;
mod suite;
mod tty;

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(serde_json::Error),
    Usage(String),
    ZRange(i32),
    Dims { cols: u16, rows: u16 },
    TooSmall { need: (u16, u16), have: (u16, u16) },
    Stty(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Json(e) => write!(f, "json: {e}"),
            Error::Usage(s) => write!(f, "usage: {s}\n{USAGE}"),
            Error::ZRange(z) => write!(f, "z-index {z} outside below-text range"),
            Error::Dims { cols, rows } => write!(f, "bad grid {cols}x{rows} (1..=1000)"),
            Error::TooSmall { need, have } => {
                write!(f, "terminal {}x{} smaller than {}x{}", have.0, have.1, need.0, need.1)
            }
            Error::Stty(s) => write!(f, "stty: {s}"),
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

const USAGE: &str = "proto1 suite <all|S0..S8|N1..N3> [--cols 200] [--rows 60] [--secs 5] [--seed 1] [--run 1] [--null] [--out FILE]
proto1 keys [--secs 30] [--out FILE]
proto1 analyze FILE...";

struct Flags {
    cols: u16,
    rows: u16,
    secs: u64,
    seed: u64,
    run: u32,
    null: bool,
    out: Option<PathBuf>,
    rest: Vec<String>,
}

fn num<T: std::str::FromStr>(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<T> {
    let v = it.next().ok_or_else(|| Error::Usage(format!("{flag} needs a value")))?;
    v.parse().map_err(|_| Error::Usage(format!("{flag}: bad value {v}")))
}

fn flags(args: impl Iterator<Item = String>, default_secs: u64) -> Result<Flags> {
    let mut f = Flags { cols: 200, rows: 60, secs: default_secs, seed: 1, run: 1, null: false, out: None, rest: vec![] };
    let mut it = args;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--cols" => f.cols = num(&mut it, "--cols")?,
            "--rows" => f.rows = num(&mut it, "--rows")?,
            "--secs" => f.secs = num(&mut it, "--secs")?,
            "--seed" => f.seed = num(&mut it, "--seed")?,
            "--run" => f.run = num(&mut it, "--run")?,
            "--null" => f.null = true,
            "--out" => f.out = Some(PathBuf::from(it.next().ok_or_else(|| Error::Usage("--out needs a path".into()))?)),
            _ if a.starts_with("--") => return Err(Error::Usage(format!("unknown flag {a}"))),
            _ => f.rest.push(a),
        }
    }
    Ok(f)
}

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().ok_or_else(|| Error::Usage("no command".into()))?;
    match cmd.as_str() {
        "suite" => {
            let f = flags(args, 5)?;
            let which = f.rest.first().ok_or_else(|| Error::Usage("suite needs a scene".into()))?;
            let scenes = scene::Scene::select(which)?;
            let o = suite::Opts {
                dims: grid::Dims::new(f.cols, f.rows)?,
                secs: Duration::from_secs(f.secs),
                seed: f.seed,
                run: f.run,
                null: f.null,
                term: if f.null { "null".into() } else { env_or("TERM_PROGRAM", "unknown") },
                rev: if f.null { "none".into() } else { env_or("TERM_PROGRAM_VERSION", "unknown") },
                out: f.out,
            };
            suite::run(&scenes, &o)
        }
        "keys" => {
            let f = flags(args, 30)?;
            keys::run(Duration::from_secs(f.secs), f.out.as_deref())
        }
        "analyze" => {
            let f = flags(args, 0)?;
            print!("{}", analyze::report(&f.rest)?);
            Ok(())
        }
        _ => Err(Error::Usage(format!("unknown command {cmd}"))),
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("proto1: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
