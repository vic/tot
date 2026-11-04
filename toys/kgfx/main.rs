//! T13 probe "kgfx" (vic 2026-11-04: "why not draw everything as kitty pixel images? we need to measure that now").
//! Closed-loop meter (proto1 style): each frame ends with DSR `CSI 5n`; next frame only after the `CSI 0n` reply
//! => fps = frames the terminal fully parsed + executed [M-parse]. Errors from the terminal (q=1) are counted.
//! Scenes (each --secs):
//!   TEXT n      : n moving glyphs on a text grid (diff), baseline
//!   FULL d      : whole window as ONE RGBA image every frame, (W/d)x(H/d) px, shm transfer, scaled to the window
//!   SPRITE n    : atlas uploaded once, n moving placements per frame (source rect + pixel offset), text world below
//! std only. run inside the terminal: ./kgfx [--secs S] [--out FILE]
use std::fs::File;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

type Res<T> = Result<T, String>;

fn stty(args: &[&str]) -> Res<String> {
    let tty = File::open("/dev/tty").map_err(|e| e.to_string())?;
    let out = Command::new("stty").args(args).stdin(tty).output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
struct Raw(String);
impl Drop for Raw {
    fn drop(&mut self) {
        let _ = std::io::stdout().write_all(b"\x1b_Ga=d,d=A,q=2\x1b\\\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = std::io::stdout().flush();
        let _ = stty(&[self.0.as_str()]);
    }
}
/// terminal link: writer + reader thread; replies accumulate as text
struct Term {
    rx: mpsc::Receiver<u8>,
    acc: Vec<u8>,
    errors: u64,
    first_err: String,
}
impl Term {
    fn send(&mut self, b: &[u8]) -> Res<()> {
        let mut so = std::io::stdout();
        so.write_all(b).map_err(|e| e.to_string())?;
        so.flush().map_err(|e| e.to_string())
    }
    /// wait for `CSI 0n`; count kitty error replies (`ESC _ G ... ; <not OK>`) seen meanwhile
    fn wait_dsr(&mut self, timeout: Duration) -> bool {
        let t0 = Instant::now();
        loop {
            while let Ok(c) = self.rx.try_recv() {
                self.acc.push(c);
            }
            if let Some(p) = find(&self.acc, b"\x1b[0n") {
                let head: Vec<u8> = self.acc.drain(..p + 4).collect();
                self.scan(&head);
                return true;
            }
            if t0.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_micros(50));
        }
    }
    fn scan(&mut self, b: &[u8]) {
        let s = String::from_utf8_lossy(b);
        for part in s.split("\x1b_G").skip(1) {
            let body = part.split("\x1b\\").next().unwrap_or("");
            if !body.ends_with(";OK") {
                self.errors += 1;
                if self.first_err.is_empty() {
                    self.first_err = body.chars().take(120).collect();
                }
            }
        }
    }
    /// query reply `CSI <code> ; a ; b t`
    fn query_t(&mut self, q: &[u8], code: &str) -> Option<(u32, u32)> {
        let mut b = q.to_vec();
        b.extend_from_slice(b"\x1b[5n");
        self.send(&b).ok()?;
        self.wait_dsr(Duration::from_secs(2));
        None.or_else(|| {
            let s = String::from_utf8_lossy(&self.acc).to_string();
            self.acc.clear();
            let p = s.find(&format!("\x1b[{code};"))?;
            let r = &s[p + code.len() + 3..];
            let r = &r[..r.find('t')?];
            let mut it = r.split(';').filter_map(|x| x.parse().ok());
            Some((it.next()?, it.next()?))
        })
    }
}
fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}
fn dec(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(v.to_string().as_bytes());
}
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
fn base64(out: &mut Vec<u8>, data: &[u8]) {
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(c.get(1).copied().unwrap_or(0)) << 8) | u32::from(c.get(2).copied().unwrap_or(0));
        let idx = |s: u32| B64[((n >> s) & 63) as usize];
        out.extend_from_slice(&[idx(18), idx(12)]);
        out.push(if c.len() > 1 { idx(6) } else { b'=' });
        out.push(if c.len() > 2 { idx(0) } else { b'=' });
    }
}
/// xorshift
struct R(u64);
impl R {
    fn f(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}
// ---------- art: procedural armored figure (load stand-in, not final art) ----------
const SW: usize = 48;
const SH: usize = 72;
const NSPR: usize = 16;
fn atlas() -> (Vec<u8>, usize, usize) {
    let (aw, ah) = (SW * NSPR, SH);
    let mut px = vec![0u8; aw * ah * 4];
    for k in 0..NSPR {
        let hue = [(255.0, 60.0, 200.0), (40.0, 240.0, 255.0), (180.0, 255.0, 60.0), (255.0, 200.0, 80.0)][k % 4];
        for y in 0..SH {
            for x in 0..SW {
                let (fx, fy) = (x as f64 - 24.0, y as f64);
                // helm, torso, pauldrons, legs, cape: ellipses; shading by x
                let inside = |cx: f64, cy: f64, rx: f64, ry: f64| ((fx - cx) / rx).powi(2) + ((fy - cy) / ry).powi(2) <= 1.0;
                let step = (k as f64 * 0.7).sin() * 3.0;
                let part = if inside(0.0, 10.0, 6.0, 7.0) {
                    1
                } else if inside(-11.0, 20.0, 7.0, 5.0) || inside(11.0, 20.0, 7.0, 5.0) {
                    2
                } else if inside(0.0, 30.0, 10.0, 13.0) {
                    3
                } else if inside(-5.0 + step, 55.0, 4.0, 14.0) || inside(5.0 - step, 55.0, 4.0, 14.0) {
                    4
                } else {
                    0
                };
                if part == 0 {
                    continue;
                }
                let shade = 0.45 + 0.55 * (1.0 - (fx + 12.0) / 36.0).clamp(0.0, 1.0);
                let (r, g, b) = match part {
                    1 => (150.0, 160.0, 190.0),
                    2 => (210.0, 170.0, 80.0),
                    3 => (120.0, 125.0, 150.0),
                    _ => (60.0, 55.0, 80.0),
                };
                let glow = part == 1 && (fy - 10.0).abs() < 1.2 && fx.abs() < 4.0;
                let (r, g, b) = if glow { hue } else { (r * shade, g * shade, b * shade) };
                let i = (y * aw + k * SW + x) * 4;
                px[i..i + 4].copy_from_slice(&[r as u8, g as u8, b as u8, 255]);
            }
        }
    }
    (px, aw, ah)
}
struct Stat {
    name: String,
    frames: u64,
    bytes: u64,
    build_us: u64,
    ack_us: Vec<u64>,
    secs: f64,
    timeout: bool,
}
fn report(s: &mut Stat, errors: u64, first_err: &str) -> String {
    s.ack_us.sort_unstable();
    let p = |q: f64| s.ack_us.get(((s.ack_us.len().max(1) - 1) as f64 * q) as usize).copied().unwrap_or(0) as f64 / 1000.0;
    format!(
        "{{\"scene\":\"{}\",\"frames\":{},\"fps_acked\":{:.1},\"pty_bytes_per_frame\":{},\"build_ms\":{:.2},\"ack_ms_p50\":{:.2},\"ack_ms_p99\":{:.2},\"terminal_errors\":{},\"first_error\":{:?},\"timeout\":{}}}",
        s.name,
        s.frames,
        s.frames as f64 / s.secs.max(1e-9),
        s.bytes / s.frames.max(1),
        s.build_us as f64 / s.frames.max(1) as f64 / 1000.0,
        p(0.5),
        p(0.99),
        errors,
        first_err,
        s.timeout
    )
}
struct Opts {
    secs: f64,
    out: String,
}
fn run(o: Opts) -> Res<()> {
    let size = stty(&["size"])?;
    let mut it = size.split_whitespace().map(|s| s.parse::<usize>());
    let (Some(Ok(rows)), Some(Ok(cols))) = (it.next(), it.next()) else { return Err(format!("stty size: {size}")) };
    let _raw = Raw(stty(&["-g"])?);
    stty(&["raw", "-echo"])?;
    let (tx, rx) = mpsc::channel::<u8>();
    std::thread::spawn(move || {
        let mut b = [0u8; 4096];
        let mut i = std::io::stdin();
        while let Ok(n) = i.read(&mut b) {
            if n == 0 || b[..n].iter().any(|&c| tx.send(c).is_err()) {
                break;
            }
        }
    });
    let mut t = Term { rx, acc: vec![], errors: 0, first_err: String::new() };
    t.send(b"\x1b[?1049h\x1b[?25l\x1b[2J")?;
    let (wh, ww) = t.query_t(b"\x1b[14t", "4").unwrap_or((rows as u32 * 16, cols as u32 * 8));
    let (cw, ch) = (ww as usize / cols, wh as usize / rows);
    let mut lines = vec![format!("{{\"scene\":\"PROBE\",\"cols\":{cols},\"rows\":{rows},\"win_px\":[{ww},{wh}],\"cell_px\":[{cw},{ch}]}}")];
    let mut rng = R(0x9e3779b97f4a7c15);
    let dur = Duration::from_secs_f64(o.secs);
    let timeout = Duration::from_secs(3);
    // upload sprite atlas once (direct, chunked), check it
    let (apx, aw, ah) = atlas();
    let mut up = Vec::new();
    let mut enc = Vec::new();
    base64(&mut enc, &apx);
    let n = enc.chunks(4096).len();
    for (k, c) in enc.chunks(4096).enumerate() {
        if k == 0 {
            up.extend_from_slice(format!("\x1b_Ga=t,i=10,f=32,s={aw},v={ah},t=d,q=1,").as_bytes());
        } else {
            up.extend_from_slice(b"\x1b_G");
        }
        up.extend_from_slice(if k + 1 < n { b"m=1;" } else { b"m=0;" });
        up.extend_from_slice(c);
        up.extend_from_slice(b"\x1b\\");
    }
    up.extend_from_slice(b"\x1b[5n");
    t.send(&up)?;
    t.wait_dsr(timeout);
    // scene list
    let only = std::env::var("KGFX_ONLY").unwrap_or_default();
    let mut scenes: Vec<(String, u8, usize)> = vec![];
    for n in [300usize, 1500] {
        scenes.push((format!("TEXT {n}"), 0, n));
    }
    for d in [4usize, 2, 1] {
        scenes.push((format!("FULL 1/{d} ({}x{} px)", ww as usize / d, wh as usize / d), 1, d));
    }
    for n in [100usize, 300, 1000, 3000] {
        scenes.push((format!("SPRITE {n}"), 2, n));
    }
    let pid = std::process::id();
    for (name, kind, n) in scenes.into_iter().filter(|s| only.is_empty() || s.0.starts_with(only.as_str())) {
        t.errors = 0;
        t.first_err.clear();
        t.send(b"\x1b_Ga=d,d=a,q=2\x1b\\\x1b[0m\x1b[2J")?;
        if kind == 2 {
            // scene clear drops the atlas (ENOENT seen in run 1): upload again, ack-checked
            t.send(&up)?;
            t.wait_dsr(timeout);
        }
        let mut st = Stat { name, frames: 0, bytes: 0, build_us: 0, ack_us: vec![], secs: 0.0, timeout: false };
        // movers: positions in px
        let mut mv: Vec<[f64; 4]> = (0..n.max(1)).map(|_| [rng.f() * ww as f64, rng.f() * wh as f64, (rng.f() - 0.5) * 6.0, (rng.f() - 0.5) * 3.0]).collect();
        let mut grid = vec![b' '; cols * rows];
        let mut prev = vec![b' '; cols * rows];
        let (fw, fh) = if kind == 1 { (ww as usize / n, wh as usize / n) } else { (0, 0) };
        let mut frame = vec![0u8; fw * fh * 4];
        let t0 = Instant::now();
        let mut f = 0u64;
        while t0.elapsed() < dur {
            let tb = Instant::now();
            for m in mv.iter_mut() {
                m[0] = (m[0] + m[2]).rem_euclid(ww as f64);
                m[1] = (m[1] + m[3]).rem_euclid(wh as f64);
            }
            let mut b: Vec<u8> = Vec::with_capacity(1 << 16);
            b.extend_from_slice(b"\x1b[?2026h");
            match kind {
                0 => {
                    grid.fill(b' ');
                    for m in &mv {
                        let (c, r) = ((m[0] as usize / cw.max(1)).min(cols - 1), (m[1] as usize / ch.max(1)).min(rows - 1));
                        grid[r * cols + c] = b"gobr"[(c + r) % 4];
                    }
                    for i in 0..cols * rows {
                        if grid[i] != prev[i] {
                            b.extend_from_slice(format!("\x1b[{};{}H\x1b[38;2;{};200;120m", i / cols + 1, i % cols + 1, 80 + (i % 150)).as_bytes());
                            b.push(grid[i]);
                        }
                    }
                    prev.copy_from_slice(&grid);
                }
                1 => {
                    // CPU compose: gradient floor + sprites from the atlas, then one shm image
                    for y in 0..fh {
                        for x in 0..fw {
                            let i = (y * fw + x) * 4;
                            let v = ((x ^ y) & 31) as u8;
                            frame[i..i + 4].copy_from_slice(&[20 + v, 12, 40 + v, 255]);
                        }
                    }
                    let sc = n as f64;
                    for (k, m) in mv.iter().enumerate().take(300) {
                        let (ox, oy) = ((m[0] / sc) as usize, (m[1] / sc) as usize);
                        let s = k % NSPR;
                        for y in 0..SH / n {
                            for x in 0..SW / n {
                                let (sx, sy) = (s * SW + x * n, y * n);
                                let si = (sy * aw + sx) * 4;
                                if apx[si + 3] == 0 || ox + x >= fw || oy + y >= fh {
                                    continue;
                                }
                                let di = ((oy + y) * fw + ox + x) * 4;
                                frame[di..di + 4].copy_from_slice(&apx[si..si + 4]);
                            }
                        }
                    }
                    let path = format!("/dev/shm/tot-kgfx-{pid}-{f}");
                    std::fs::write(&path, &frame).map_err(|e| e.to_string())?;
                    let id = 100 + (f % 2);
                    b.extend_from_slice(format!("\x1b[1;1H\x1b_Ga=T,i={id},p=1,f=32,s={fw},v={fh},t=s,S={},c={cols},r={rows},C=1,z=1,q=1;", frame.len()).as_bytes());
                    base64(&mut b, path.trim_start_matches("/dev/shm").as_bytes());
                    b.extend_from_slice(b"\x1b\\");
                    b.extend_from_slice(format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", 100 + ((f + 1) % 2)).as_bytes());
                }
                _ => {
                    for (k, m) in mv.iter().enumerate() {
                        let (c, r) = ((m[0] as usize / cw.max(1)).min(cols - 1), (m[1] as usize / ch.max(1)).min(rows - 1));
                        let (ox, oy) = (m[0] as usize % cw.max(1), m[1] as usize % ch.max(1));
                        b.extend_from_slice(b"\x1b[");
                        dec(&mut b, r as u64 + 1);
                        b.push(b';');
                        dec(&mut b, c as u64 + 1);
                        b.extend_from_slice(b"H\x1b_Ga=p,i=10,p=");
                        dec(&mut b, k as u64 + 1);
                        b.extend_from_slice(b",x=");
                        dec(&mut b, ((k % NSPR) * SW) as u64);
                        b.extend_from_slice(format!(",y=0,w={SW},h={SH},X=").as_bytes());
                        dec(&mut b, ox as u64);
                        b.extend_from_slice(b",Y=");
                        dec(&mut b, oy as u64);
                        b.extend_from_slice(b",z=1,C=1,q=1\x1b\\");
                    }
                }
            }
            b.extend_from_slice(b"\x1b[?2026l\x1b[5n");
            st.build_us += tb.elapsed().as_micros() as u64;
            st.bytes += b.len() as u64;
            let ta = Instant::now();
            t.send(&b)?;
            if !t.wait_dsr(timeout) {
                st.timeout = true;
                break;
            }
            st.ack_us.push(ta.elapsed().as_micros() as u64);
            f += 1;
            st.frames += 1;
        }
        st.secs = t0.elapsed().as_secs_f64();
        for k in 0..f + 2 {
            let _ = std::fs::remove_file(format!("/dev/shm/tot-kgfx-{pid}-{k}"));
        }
        let (e, fe) = (t.errors, t.first_err.clone());
        lines.push(report(&mut st, e, &fe));
    }
    drop(_raw);
    let text = lines.join("\n") + "\n";
    std::fs::OpenOptions::new().create(true).append(true).open(&o.out).and_then(|mut f| f.write_all(text.as_bytes())).map_err(|e| e.to_string())?;
    Ok(())
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut o = Opts { secs: 4.0, out: "kgfx-results.jsonl".into() };
    for k in (1..a.len()).step_by(2) {
        match (a[k].as_str(), a.get(k + 1)) {
            ("--secs", Some(v)) => o.secs = v.parse().unwrap_or(4.0),
            ("--out", Some(v)) => o.out = v.clone(),
            _ => {}
        }
    }
    if let Err(e) = run(o) {
        eprintln!("kgfx: {e}");
        std::process::exit(1);
    }
}
