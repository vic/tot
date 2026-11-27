//! The browser build's terminal: stdout is the screen, stdin carries the key bytes the page sends (kitty
//! protocol, key releases included), and `/term/size` holds "rows cols", rewritten by the page on resize.
//! No raw mode, no reader thread: the page's WASI shim returns 0 bytes from an empty stdin and wakes a
//! sleeping `poll_oneoff` as soon as input arrives, so `wait` is a sleep followed by a read.
use crate::input::{Event, Parser};
use crate::{Error, Result};
use std::io::{Read, Write};
use std::time::Duration;

const SIZE_FILE: &str = "/term/size";
const KITTY_PUSH: &[u8] = b"\x1b[>15u";

pub struct Tty {
    parser: Parser,
    /// write calls in total
    pub writes: u64,
}

impl Tty {
    pub fn open() -> Result<Tty> {
        let mut t = Tty { parser: Parser::default(), writes: 0 };
        t.send(b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[?1000h\x1b[?1006h")?;
        Ok(t)
    }

    /// The page always speaks the kitty protocol: push the flags, no probe round trip.
    pub fn probe_kitty(&mut self) -> Result<bool> {
        self.send(KITTY_PUSH)?;
        Ok(true)
    }

    pub fn size(&self) -> Result<(u16, u16)> {
        let s = std::fs::read_to_string(SIZE_FILE)?;
        let mut it = s.split_whitespace().map(str::parse::<u16>);
        match (it.next(), it.next()) {
            (Some(Ok(r)), Some(Ok(c))) => Ok((c, r)),
            _ => Err(Error::Tty(format!("{SIZE_FILE}: {s}"))),
        }
    }

    pub fn send(&mut self, buf: &[u8]) -> Result<()> {
        let mut o = std::io::stdout().lock();
        o.write_all(buf)?;
        o.flush()?;
        self.writes += 1;
        Ok(())
    }

    /// Events already received (non-blocking).
    pub fn events(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let mut b = [0u8; 4096];
        while let Ok(n @ 1..) = std::io::stdin().read(&mut b) {
            out.extend(self.parser.feed(&b[..n]));
        }
        out
    }

    /// Wait for input or `timeout`; the sleep ends early when the page sends keys.
    pub fn wait(&mut self, timeout: Duration) -> Vec<Event> {
        let v = self.events();
        if !v.is_empty() {
            return v;
        }
        std::thread::sleep(timeout);
        self.events()
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        let _ = self.send(b"\x1b[<u\x1b[?1006l\x1b[?1000l\x1b[0m\x1b[?25h\x1b[?1049l");
    }
}
