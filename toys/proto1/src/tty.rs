//! /dev/tty in raw mode (via stty), reader thread, reply parser, ack wait with timeout.
use crate::grid::Dims;
use crate::{Error, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::process::Command;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyId {
    Code(u32),
    Up,
    Down,
    Right,
    Left,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    /// `CSI 0 n`: DSR status reply = everything before it parsed.
    Dsr,
    /// `CSI ? 2026 ; v $ y`
    SyncMode(u8),
    /// `CSI ? flags u`
    KbdFlags(u32),
    Gfx { id: Option<u32>, placement: Option<u32>, ok: bool, raw: String },
    Key { key: KeyId, kind: KeyKind },
    Other,
}

#[derive(Default)]
pub struct Parser {
    buf: Vec<u8>,
}

fn field(s: &str, i: usize) -> Option<&str> {
    s.split(';').nth(i)
}
fn first_num(s: &str) -> Option<u32> {
    field(s, 0)?.split(':').next()?.parse().ok()
}

fn csi(params: &str, inter: &str, fin: u8) -> Event {
    let kind = || match field(params, 1).and_then(|m| m.split(':').nth(1)) {
        Some("2") => KeyKind::Repeat,
        Some("3") => KeyKind::Release,
        _ => KeyKind::Press,
    };
    match (fin, inter) {
        (b'n', "") if params == "0" => Event::Dsr,
        (b'y', "$") => params
            .strip_prefix("?2026;")
            .and_then(|v| v.parse().ok())
            .map_or(Event::Other, Event::SyncMode),
        (b'u', "") => match params.strip_prefix('?') {
            Some(f) => f.parse().map_or(Event::Other, Event::KbdFlags),
            None => first_num(params).map_or(Event::Other, |c| Event::Key { key: KeyId::Code(c), kind: kind() }),
        },
        (b'A' | b'B' | b'C' | b'D', "") => {
            let key = match fin {
                b'A' => KeyId::Up,
                b'B' => KeyId::Down,
                b'C' => KeyId::Right,
                _ => KeyId::Left,
            };
            Event::Key { key, kind: kind() }
        }
        _ => Event::Other,
    }
}

fn apc(body: &str) -> Event {
    let Some(rest) = body.strip_prefix('G') else { return Event::Other };
    let (keys, msg) = rest.split_once(';').unwrap_or((rest, ""));
    let get = |k: &str| keys.split(',').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('=')?.parse().ok());
    Event::Gfx { id: get("i"), placement: get("p"), ok: msg == "OK", raw: rest.to_string() }
}

impl Parser {
    /// Feed bytes, return every complete event; keep an incomplete tail.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.buf.extend_from_slice(bytes);
        let mut evs = Vec::new();
        let mut i = 0;
        let b = &self.buf;
        while i < b.len() {
            if b[i] != 0x1b {
                i += 1;
                continue;
            }
            let Some(&kind) = b.get(i + 1) else { break };
            match kind {
                b'[' => {
                    let Some(end) = b[i + 2..].iter().position(|c| (0x40..=0x7e).contains(c)) else { break };
                    let body = &b[i + 2..i + 2 + end];
                    let split = body.iter().position(|c| (0x20..=0x2f).contains(c)).unwrap_or(body.len());
                    let s = |x: &[u8]| String::from_utf8_lossy(x).into_owned();
                    evs.push(csi(&s(&body[..split]), &s(&body[split..]), b[i + 2 + end]));
                    i += 3 + end;
                }
                b'_' => {
                    let Some(end) = b[i + 2..].windows(2).position(|w| w == b"\x1b\\") else { break };
                    evs.push(apc(&String::from_utf8_lossy(&b[i + 2..i + 2 + end])));
                    i += 4 + end;
                }
                _ => i += 1,
            }
        }
        self.buf.drain(..i);
        evs
    }
}

pub struct Tty {
    w: File,
    rx: Receiver<Vec<u8>>,
    parser: Parser,
    saved: String,
    /// write(2) calls made by `send` in total
    pub writes: u64,
}

fn stty(args: &[&str]) -> Result<String> {
    let out = Command::new("stty").args(args).stdin(File::open("/dev/tty")?).output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(Error::Stty(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

impl Tty {
    pub fn open() -> Result<Self> {
        let w = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let mut r = w.try_clone()?;
        let saved = stty(&["-g"])?;
        stty(&["raw", "-echo"])?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut b = [0u8; 65536];
            while let Ok(n) = r.read(&mut b) {
                if n == 0 || tx.send(b[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut t = Tty { w, rx, parser: Parser::default(), saved, writes: 0 };
        t.send(b"\x1b[?1049h\x1b[?25l\x1b[2J")?;
        Ok(t)
    }

    pub fn size(&self) -> Result<Dims> {
        let s = stty(&["size"])?;
        let mut it = s.split_whitespace().map(str::parse::<u16>);
        match (it.next(), it.next()) {
            (Some(Ok(r)), Some(Ok(c))) => Dims::new(c, r),
            _ => Err(Error::Stty(format!("size: {s}"))),
        }
    }

    /// Write the whole buffer; returns the write(2) count (1 unless the pty takes a partial write).
    pub fn send(&mut self, mut buf: &[u8]) -> Result<u32> {
        let mut n = 0;
        while !buf.is_empty() {
            let k = self.w.write(buf)?;
            if k == 0 {
                return Err(Error::Io(std::io::ErrorKind::WriteZero.into()));
            }
            buf = &buf[k..];
            n += 1;
        }
        self.writes += u64::from(n);
        Ok(n)
    }

    /// Events that arrive within `timeout` (returns after the first chunk or at timeout).
    pub fn poll(&mut self, timeout: Duration) -> Vec<Event> {
        match self.rx.recv_timeout(timeout) {
            Ok(b) => self.parser.feed(&b),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => vec![],
        }
    }

    /// Wait for the DSR reply. Ok(Some(events before it)) or Ok(None) on timeout.
    pub fn wait_dsr(&mut self, timeout: Duration) -> Option<Vec<Event>> {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            for e in self.poll(left) {
                if e == Event::Dsr {
                    return Some(seen);
                }
                seen.push(e);
            }
        }
    }

    /// Drop replies for `d` (between scenes).
    pub fn drain(&mut self, d: Duration) {
        let deadline = Instant::now() + d;
        while Instant::now() < deadline {
            self.poll(deadline.saturating_duration_since(Instant::now()));
        }
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        // best effort restore; nothing to propagate to from Drop
        let _ = self.w.write_all(b"\x1b_Ga=d,d=A\x1b\\\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = stty(&[self.saved.as_str()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_replies_across_chunks() {
        let mut p = Parser::default();
        assert!(p.feed(b"\x1b_Gi=31,p=1;O").is_empty());
        let e = p.feed(b"K\x1b\\\x1b[0");
        assert_eq!(e, vec![Event::Gfx { id: Some(31), placement: Some(1), ok: true, raw: "i=31,p=1;OK".into() }]);
        assert_eq!(p.feed(b"n\x1b[?2026;2$y\x1b[?3u"), vec![Event::Dsr, Event::SyncMode(2), Event::KbdFlags(3)]);
    }

    #[test]
    fn gfx_error_and_no_placement() {
        let mut p = Parser::default();
        let e = p.feed(b"\x1b_Gi=31;ENOENT:no such\x1b\\");
        assert_eq!(e, vec![Event::Gfx { id: Some(31), placement: None, ok: false, raw: "i=31;ENOENT:no such".into() }]);
    }

    #[test]
    fn decodes_kitty_keys() {
        let mut p = Parser::default();
        let k = |key, kind| Event::Key { key, kind };
        assert_eq!(
            p.feed(b"\x1b[119u\x1b[119;1:2u\x1b[119;1:3u\x1b[27u\x1b[1;1:3A\x1b[D"),
            vec![
                k(KeyId::Code(119), KeyKind::Press),
                k(KeyId::Code(119), KeyKind::Repeat),
                k(KeyId::Code(119), KeyKind::Release),
                k(KeyId::Code(27), KeyKind::Press),
                k(KeyId::Up, KeyKind::Release),
                k(KeyId::Left, KeyKind::Press),
            ]
        );
    }

    #[test]
    fn not_dsr_controls() {
        // CPR and DSR-3 must not count as an ack
        let mut p = Parser::default();
        assert_eq!(p.feed(b"\x1b[3n\x1b[12;4R"), vec![Event::Other, Event::Other]);
    }
}
