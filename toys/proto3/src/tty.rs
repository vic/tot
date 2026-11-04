//! /dev/tty raw mode (via stty), reader thread, kitty keyboard probe (from proto1/src/tty.rs).
use crate::input::{Event, Parser};
use crate::{Error, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::process::Command;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel};
use std::time::{Duration, Instant};

pub struct Tty {
    w: File,
    rx: Receiver<Vec<u8>>,
    parser: Parser,
    saved: String,
    kitty: bool,
    /// write(2) calls in total
    pub writes: u64,
}

fn stty(args: &[&str]) -> Result<String> {
    let out = Command::new("stty").args(args).stdin(File::open("/dev/tty")?).output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(Error::Tty(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

/// kitty flags: 1 disambiguate | 2 event types | 4 alternate keys | 8 all keys as escapes.
/// (design sec 6.1 says 11; +4 gives the shifted key, so `:` works on any layout)
const KITTY_PUSH: &[u8] = b"\x1b[>15u";

impl Tty {
    pub fn open() -> Result<Tty> {
        let w = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let mut r = w.try_clone()?;
        let saved = stty(&["-g"])?;
        stty(&["raw", "-echo"])?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut b = [0u8; 4096];
            while let Ok(n) = r.read(&mut b) {
                if n == 0 || tx.send(b[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut t = Tty { w, rx, parser: Parser::default(), saved, kitty: false, writes: 0 };
        t.send(b"\x1b[?1049h\x1b[?25l\x1b[2J")?;
        Ok(t)
    }

    /// Probe: `CSI ? u` then DA1. Only a DA reply => no kitty protocol (design sec 6.1).
    pub fn probe_kitty(&mut self) -> Result<bool> {
        self.send(b"\x1b[?u\x1b[c")?;
        let deadline = Instant::now() + Duration::from_millis(1500);
        let mut flags = false;
        while Instant::now() < deadline {
            for e in self.wait(deadline.saturating_duration_since(Instant::now())) {
                match e {
                    Event::KbdFlags(_) => flags = true,
                    Event::DeviceAttrs => {
                        if flags {
                            self.send(KITTY_PUSH)?;
                            self.kitty = true;
                        }
                        return Ok(flags);
                    }
                    _ => {}
                }
            }
        }
        Ok(false)
    }

    pub fn size(&self) -> Result<(u16, u16)> {
        let s = stty(&["size"])?;
        let mut it = s.split_whitespace().map(str::parse::<u16>);
        match (it.next(), it.next()) {
            (Some(Ok(r)), Some(Ok(c))) => Ok((c, r)),
            _ => Err(Error::Tty(format!("stty size: {s}"))),
        }
    }

    pub fn send(&mut self, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() {
            let k = self.w.write(buf)?;
            if k == 0 {
                return Err(Error::Io(std::io::ErrorKind::WriteZero.into()));
            }
            buf = &buf[k..];
            self.writes += 1;
        }
        Ok(())
    }

    /// Events already received (non-blocking).
    pub fn events(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(b) => out.extend(self.parser.feed(&b)),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return out,
            }
        }
    }

    /// Block until input arrives or `timeout` passes.
    pub fn wait(&mut self, timeout: Duration) -> Vec<Event> {
        match self.rx.recv_timeout(timeout) {
            Ok(b) => {
                let mut v = self.parser.feed(&b);
                v.extend(self.events());
                v
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Vec::new(),
        }
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        // best effort restore; Drop has no caller to report to
        let pop: &[u8] = if self.kitty { b"\x1b[<u" } else { b"" };
        let _ = self.w.write_all(pop);
        let _ = self.w.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = stty(&[self.saved.as_str()]);
    }
}
