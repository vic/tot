//! K1 held keys + L1 key->parse-ack latency. Needs a human at the keyboard.
use crate::suite::pct;
use crate::tty::{Event, KeyId, KeyKind, Tty};
use crate::Result;
use serde_json::json;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

const KEYS: [(KeyId, &str); 9] = [
    (KeyId::Code(119), "w"),
    (KeyId::Code(97), "a"),
    (KeyId::Code(115), "s"),
    (KeyId::Code(100), "d"),
    (KeyId::Code(27), "Esc"),
    (KeyId::Up, "Up"),
    (KeyId::Down, "Down"),
    (KeyId::Left, "Left"),
    (KeyId::Right, "Right"),
];

pub fn run(secs: Duration, out: Option<&Path>) -> Result<()> {
    let mut tty = Tty::open()?;
    // kitty keyboard flags 1|2: disambiguate + report event types (press/repeat/release)
    tty.send(b"\x1b[>3u\x1b[?u\x1b[5n")?;
    let flags = tty.wait_dsr(Duration::from_secs(2)).unwrap_or_default().iter().find_map(|e| match e {
        Event::KbdFlags(f) => Some(*f),
        _ => None,
    });
    tty.send(b"\x1b[2J\x1b[1;1HK1: press, hold 1 s, release each: w a s d Esc Up Down Left Right. Ends when all released or on timeout.\r\n")?;
    let mut counts = [[0u32; 3]; 9];
    let mut lat_us = Vec::new();
    let mut pending: Option<Instant> = None;
    let deadline = Instant::now() + secs;
    let mut frame = 0u64;
    while Instant::now() < deadline && counts.iter().any(|c| c[2] == 0) {
        for e in tty.poll(Duration::from_millis(50)) {
            match e {
                Event::Key { key, kind } => {
                    let t = Instant::now();
                    if let Some(i) = KEYS.iter().position(|k| k.0 == key) {
                        counts[i][kind as usize] += 1;
                    }
                    if kind == KeyKind::Press && pending.is_none() {
                        // L1: one small frame + DSR per press, time key-parsed -> terminal ack
                        frame += 1;
                        let mut b = b"\x1b[3;1H\x1b[0mframe ".to_vec();
                        crate::grid::dec(&mut b, frame);
                        b.extend_from_slice(b"\x1b[5n");
                        tty.send(&b)?;
                        pending = Some(t);
                    }
                }
                Event::Dsr => {
                    if let Some(t) = pending.take() {
                        lat_us.push(t.elapsed().as_micros() as u64);
                    }
                }
                _ => {}
            }
        }
    }
    tty.send(b"\x1b[<u")?;
    drop(tty);
    lat_us.sort_unstable();
    let all_released = counts.iter().all(|c| c[2] > 0);
    let table: serde_json::Map<String, serde_json::Value> =
        KEYS.iter().zip(&counts).map(|((_, n), c)| (n.to_string(), json!({"press": c[0], "repeat": c[1], "release": c[2]}))).collect();
    let term = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let rev = std::env::var("TERM_PROGRAM_VERSION").unwrap_or_default();
    let ms = |p| pct(&lat_us, p) as f64 / 1000.0;
    let k1 = json!({"term": term, "rev": rev, "scene": "K1", "kbd_flags": flags, "keys": table, "ok": all_released});
    let l1 = json!({"term": term, "rev": rev, "scene": "L1", "frames": lat_us.len(), "ack_ms_p50": (!lat_us.is_empty()).then(|| ms(50)),
        "ack_ms_p99": (!lat_us.is_empty()).then(|| ms(99)), "ok": !lat_us.is_empty(), "meter": "M-parse"});
    let mut text = String::new();
    println!("key     press repeat release");
    for ((_, n), c) in KEYS.iter().zip(&counts) {
        println!("{n:<7} {:>5} {:>6} {:>7}", c[0], c[1], c[2]);
    }
    println!("K1 {} (kbd flags reply {flags:?}); L1 key->ack ms p50/p99 = {:?}/{:?} over {} presses",
        if all_released { "PASS" } else { "FAIL" }, l1["ack_ms_p50"], l1["ack_ms_p99"], lat_us.len());
    for v in [k1, l1] {
        text.push_str(&serde_json::to_string(&v)?);
        text.push('\n');
    }
    if let Some(p) = out {
        std::fs::OpenOptions::new().create(true).append(true).open(p)?.write_all(text.as_bytes())?;
    }
    Ok(())
}
