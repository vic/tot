//! T12 toy "swarm" (vic 2026-11-04: "graphics are not as detailed, nethack like, because we need to prove huge
//! hordes are capable of rendering on combat and motion"). One glyph per body on a true 2:1 iso cell grid,
//! generated floor (showcase rules), 1500 live horde (tile cap 3, flow field, out-of-LOS spawns), heavy autofire
//! (bolts, chain arcs, orbit blades, nova), shards, camera = whole-cell steps + terminal scroll (T10 law),
//! kitty keyboard press/release (held keys, no autorepeat stutter). std only, f64 toy sim at 30 Hz, render 60 fps.
//! run: ./swarm [--live N --k Z --seed S]   headless: ./swarm shot
//! keys: wasd / arrows move, q/e turn, g god mode, +/- zoom, x quit
use std::f64::consts::{FRAC_1_SQRT_2, TAU};
use std::fs::File;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

type Res<T> = Result<T, String>;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
struct Rgb(u8, u8, u8);
impl Rgb {
    const fn hex(h: u32) -> Rgb {
        Rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
    }
    fn scale(self, f: f64) -> Rgb {
        let c = |v: u8| (v as f64 * f).clamp(0.0, 255.0) as u8;
        Rgb(c(self.0), c(self.1), c(self.2))
    }
}
#[derive(Clone, Copy, PartialEq, Debug)]
struct Cell {
    ch: char,
    fg: Rgb,
    bg: Rgb,
}
const VOID: Rgb = Rgb::hex(0x0b0620);
const BLANK: Cell = Cell { ch: ' ', fg: VOID, bg: VOID };
const FLOORC: Rgb = Rgb::hex(0x1a1234);
const WALLC: Rgb = Rgb::hex(0x5a4a8a);
const TRIM: Rgb = Rgb::hex(0x19f3ff);
const UI: Rgb = Rgb::hex(0xc0c0d8);
const STRIP: Rgb = Rgb::hex(0x140c2c);
const ZK: f64 = 1.224_744_871_391_589;
// ---------- floor gen: Diablo-ish rooms + halls + props (toys/gen rules, tighter rooms) ----------
const G: usize = 7;
const CW: usize = 24;
const W: usize = G * CW; // 168 m
struct X([u64; 4]);
fn sm(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}
impl X {
    fn new(seed: u64) -> X {
        let mut s = seed;
        X([sm(&mut s), sm(&mut s), sm(&mut s), sm(&mut s)])
    }
    fn next(&mut self) -> u64 {
        let s = &mut self.0;
        let r = (s[0].wrapping_add(s[3])).rotate_left(23).wrapping_add(s[0]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        r
    }
    fn below(&mut self, n: u32) -> u32 {
        let z = (u32::MAX - n + 1) % n;
        loop {
            let r = (self.next() >> 32) as u32;
            if r >= z {
                return r % n;
            }
        }
    }
    fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}
/// tile codes
const ROCK: u8 = 0;
const OPEN: u8 = 1;
const PROP: u8 = 2;
const PILLAR: u8 = 3;
#[derive(Clone, Copy, PartialEq)]
enum PropK {
    Brazier,
    Rack,
    Crate,
}
struct Floor {
    t: Vec<u8>,
    start: usize,
    props: Vec<(f64, f64, PropK)>,
}
fn carve(t: &mut [u8], x0: usize, y0: usize, x1: usize, y1: usize) {
    for y in y0..y1 {
        for x in x0..x1 {
            t[y * W + x] = OPEN;
        }
    }
}
fn floorgen(seed: u64, room: (u32, u32)) -> Floor {
    let nb = |c: usize| {
        let (x, y) = (c % G, c / G);
        let mut v = vec![];
        if x > 0 {
            v.push(c - 1)
        }
        if x + 1 < G {
            v.push(c + 1)
        }
        if y > 0 {
            v.push(c - G)
        }
        if y + 1 < G {
            v.push(c + G)
        }
        v
    };
    let mut m = X::new(seed);
    let mut used = [false; G * G];
    let mut e = vec![];
    let start = [0usize, G - 1, G * (G - 1), G * G - 1][m.below(4) as usize];
    used[start] = true;
    let mut cells = vec![start];
    let n = 16 + m.below(8) as usize;
    while cells.len() < n {
        let fr: Vec<(usize, usize)> = cells.iter().flat_map(|&c| nb(c).into_iter().filter(|&d| !used[d]).map(move |d| (c, d))).collect();
        let (a, b) = fr[m.below(fr.len() as u32) as usize];
        used[b] = true;
        cells.push(b);
        e.push((a, b));
    }
    let mut cand: Vec<(usize, usize)> = cells
        .iter()
        .flat_map(|&c| nb(c).into_iter().map(move |d| (c, d)))
        .filter(|&(c, d)| d > c && used[d] && !e.contains(&(c, d)) && !e.contains(&(d, c)))
        .collect();
    for _ in 0..3 {
        if cand.is_empty() {
            break;
        }
        let i = m.below(cand.len() as u32) as usize;
        e.push(cand.swap_remove(i));
    }
    let mut t = vec![ROCK; W * W];
    let mut props = vec![];
    let (lo, hi) = (room.0.min(CW as u32 - 4), room.1.min(CW as u32 - 4).max(room.0));
    let mut boxes = vec![];
    for &c in &cells {
        let (ox, oy) = ((c % G) * CW, (c / G) * CW);
        let (w, h) = ((lo + m.below(hi - lo + 1)) as usize, (lo + m.below(hi - lo + 1)) as usize);
        let (x0, y0) = (ox + (CW - w) / 2, oy + (CW - h) / 2);
        carve(&mut t, x0, y0, x0 + w, y0 + h);
        boxes.push((x0, y0, w, h, m.below(3)));
    }
    for &(a, b) in &e {
        let (ax, ay) = ((a % G) * CW + CW / 2, (a / G) * CW + CW / 2);
        let (bx, by) = ((b % G) * CW + CW / 2, (b / G) * CW + CW / 2);
        carve(&mut t, ax.min(bx) - 2, ay.min(by) - 2, ax.max(bx) + 2, ay.max(by) + 2);
    }
    for (k, &(x0, y0, w, h, style)) in boxes.iter().enumerate() {
        let is_start = cells[k] == start;
        // braziers in inner corners
        for (x, y) in [(x0 + 1, y0 + 1), (x0 + w - 2, y0 + 1), (x0 + 1, y0 + h - 2), (x0 + w - 2, y0 + h - 2)] {
            t[y * W + x] = PROP;
            props.push((x as f64 + 0.5, y as f64 + 0.5, PropK::Brazier));
        }
        if style == 1 && w >= 12 && h >= 12 && !is_start {
            // nave: two pillar rows
            for x in (x0 + 3..x0 + w - 3).step_by(4) {
                for y in [y0 + 3, y0 + h - 4] {
                    t[y * W + x] = PILLAR;
                }
            }
        }
        if style == 2 && !is_start {
            // crypt: rack row in the middle, gaps every 3
            let y = y0 + h / 2;
            for x in (x0 + 3..x0 + w - 3).filter(|x| (x - x0) % 3 != 0) {
                t[y * W + x] = PROP;
                props.push((x as f64 + 0.5, y as f64 + 0.5, PropK::Rack));
            }
        }
        for _ in 0..m.below(4) {
            let (x, y) = (x0 + 2 + m.below(w as u32 - 4) as usize, y0 + 2 + m.below(h as u32 - 4) as usize);
            if t[y * W + x] == OPEN && !is_start {
                t[y * W + x] = PROP;
                props.push((x as f64 + 0.5, y as f64 + 0.5, PropK::Crate));
            }
        }
    }
    let s = ((start % G) * CW + CW / 2) + ((start / G) * CW + CW / 2) * W;
    t[s] = OPEN;
    Floor { t, start: s, props }
}

fn los_t(t: &[u8], a: usize, b: usize) -> bool {
    let (mut x, mut y) = ((a % W) as i32, (a / W) as i32);
    let (x1, y1) = ((b % W) as i32, (b / W) as i32);
    let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
    let (sx, sy) = ((x1 - x).signum(), (y1 - y).signum());
    let mut e = dx + dy;
    loop {
        if x == x1 && y == y1 {
            return true;
        }
        let c = t[y as usize * W + x as usize];
        if (c == ROCK || c == PILLAR) && (x, y) != ((a % W) as i32, (a / W) as i32) {
            return false;
        }
        let e2 = 2 * e;
        if e2 >= dy {
            e += dy;
            x += sx;
        }
        if e2 <= dx {
            e += dx;
            y += sy;
        }
    }
}
fn num(o: &mut Vec<u8>, mut v: usize) {
    let mut b = [0u8; 20];
    let mut i = 20;
    loop {
        i -= 1;
        b[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    o.extend_from_slice(&b[i..]);
}
fn rgb(o: &mut Vec<u8>, c: Rgb) {
    num(o, c.0 as usize);
    o.push(b';');
    num(o, c.1 as usize);
    o.push(b';');
    num(o, c.2 as usize);
}
/// terminal-side pan (Ghostty: DECSTBM + SU/SD, ICH/DCH): shift the play area by (dc, dr) cells and shift
/// prev the same way; exposed cells get a sentinel so the diff repaints them
fn scroll(prev: &mut [Cell], cols: usize, prow: usize, (dc, dr): (i64, i64), o: &mut Vec<u8>) {
    o.extend_from_slice(b"\x1b[?2026h\x1b[0m");
    if dr != 0 {
        o.extend_from_slice(b"\x1b[1;");
        num(o, prow);
        o.push(b'r');
        o.extend_from_slice(b"\x1b[");
        num(o, dr.unsigned_abs() as usize);
        o.push(if dr < 0 { b'S' } else { b'T' });
        o.extend_from_slice(b"\x1b[r");
    }
    if dc != 0 {
        for r in 0..prow {
            o.extend_from_slice(b"\x1b[");
            num(o, r + 1);
            o.extend_from_slice(b";1H\x1b[");
            num(o, dc.unsigned_abs() as usize);
            o.push(if dc < 0 { b'P' } else { b'@' });
        }
    }
    let old = prev[..cols * prow].to_vec();
    const GONE: Cell = Cell { ch: '\0', fg: VOID, bg: VOID };
    for r in 0..prow as i64 {
        for c in 0..cols as i64 {
            let (sr, sc) = (r - dr, c - dc);
            prev[r as usize * cols + c as usize] = if sr >= 0 && sc >= 0 && sr < prow as i64 && sc < cols as i64 {
                old[sr as usize * cols + sc as usize]
            } else {
                GONE
            };
        }
    }
}
fn emit(prev: &[Cell], cur: &[Cell], w: usize, force: bool, o: &mut Vec<u8>) {
    o.extend_from_slice(b"\x1b[?2026h");
    let mut at = None;
    let (mut lf, mut lb) = (None, None);
    for (i, &c) in cur.iter().enumerate() {
        if !force && prev[i] == c {
            continue;
        }
        if at != Some(i) {
            o.extend_from_slice(b"\x1b[");
            num(o, i / w + 1);
            o.push(b';');
            num(o, i % w + 1);
            o.push(b'H');
        }
        let need_fg = lf != Some(c.fg) && c.ch != ' ';
        if need_fg || lb != Some(c.bg) {
            o.extend_from_slice(b"\x1b[");
            if need_fg {
                o.extend_from_slice(b"38;2;");
                rgb(o, c.fg);
                lf = Some(c.fg);
            }
            if lb != Some(c.bg) {
                if need_fg {
                    o.push(b';');
                }
                o.extend_from_slice(b"48;2;");
                rgb(o, c.bg);
                lb = Some(c.bg);
            }
            o.push(b'm');
        }
        let mut u = [0u8; 4];
        o.extend_from_slice(c.ch.encode_utf8(&mut u).as_bytes());
        at = if (i + 1) % w == 0 { None } else { Some(i + 1) };
    }
    o.extend_from_slice(b"\x1b[0m\x1b[?2026l");
}

fn stty(args: &[&str]) -> Res<String> {
    let tty = File::open("/dev/tty").map_err(|e| e.to_string())?;
    let out = Command::new("stty").args(args).stdin(tty).output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
struct Raw(String);
impl Drop for Raw {
    fn drop(&mut self) {
        let _ = std::io::stdout().write_all(b"\x1b[<u\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = std::io::stdout().flush();
        let _ = stty(&[self.0.as_str()]);
    }
}

// ---------- game ----------
#[derive(Clone, Copy, PartialEq)]
enum Fam {
    Ghoul,
    Drone,
    Brute,
    Runner,
}
const FAMS: [(Fam, char, Rgb, f64, f64); 4] = [
    (Fam::Ghoul, 'g', Rgb::hex(0xb6ff3b), 3.0, 1.0),
    (Fam::Drone, 'o', Rgb::hex(0x19f3ff), 3.6, 1.0),
    (Fam::Brute, 'B', Rgb::hex(0xff2bd6), 2.4, 6.0),
    (Fam::Runner, 'r', Rgb::hex(0xffd75e), 4.4, 0.6),
];
fn fam(f: Fam) -> (char, Rgb, f64, f64) {
    let e = FAMS[FAMS.iter().position(|q| q.0 == f).unwrap_or(0)];
    (e.1, e.2, e.3, e.4)
}
/// SoA horde
struct Horde {
    x: Vec<f64>,
    y: Vec<f64>,
    hp: Vec<f64>,
    fam: Vec<Fam>,
    blade_cd: Vec<f32>,
    hit_cd: Vec<f32>,
    flash: Vec<f32>,
}
struct Bolt {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    life: f64,
    pierce: u8,
}
struct Game {
    t: Vec<u8>,
    props: Vec<(f64, f64, PropK)>,
    wall: Vec<bool>,
    vis: Vec<bool>,
    explored: Vec<bool>,
    dist: Vec<u16>,
    ring: Vec<u32>,
    occ: Vec<u8>,
    cstart: Vec<u32>,
    order: Vec<u32>,
    tile_at: usize,
    px: f64,
    py: f64,
    h: Horde,
    bolts: Vec<Bolt>,
    arcs: Vec<(Vec<(f64, f64)>, f64)>,
    deaths: Vec<(f64, f64, f64)>,
    shards: Vec<(f64, f64)>,
    nova: Option<f64>,
    blade_a: f64,
    cd: [f64; 3],
    hp: f64,
    god: bool,
    level: u32,
    xp: u32,
    kills: u64,
    kills_sec: u64,
    time: f64,
    live: usize,
    dmg: f64,
    arena: bool,
    rng: X,
}
const PMAX: f64 = 500.0;
fn tl(x: f64, y: f64) -> usize {
    (y.clamp(0.0, W as f64 - 1.0) as usize) * W + x.clamp(0.0, W as f64 - 1.0) as usize
}
impl Game {
    fn new(seed: u64, live: usize, arena: bool) -> Game {
        let mut f = floorgen(seed, (14, 22));
        if arena {
            // one 70 x 70 m hall, pillar grid every 9 m, start in the centre
            f.t.fill(ROCK);
            let o = (W - 70) / 2;
            carve(&mut f.t, o, o, o + 70, o + 70);
            for y in (o + 5..o + 66).step_by(9) {
                for x in (o + 5..o + 66).step_by(9) {
                    f.t[y * W + x] = PILLAR;
                    f.t[y * W + x + 1] = PILLAR;
                }
            }
            f.props.clear();
            f.start = (W / 2) * W + W / 2 + 2;
        }
        let t = f.t.clone();
        let wall = (0..W * W)
            .map(|c| {
                t[c] == PILLAR
                    || (t[c] == ROCK
                        && (-1i32..=1).any(|dy| {
                            (-1i32..=1).any(|dx| {
                                let (x, y) = ((c % W) as i32 + dx, (c / W) as i32 + dy);
                                x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < W && t[y as usize * W + x as usize] != ROCK
                            })
                        }))
            })
            .collect();
        let mut g = Game {
            props: f.props,
            wall,
            vis: vec![false; W * W],
            explored: vec![false; W * W],
            dist: vec![u16::MAX; W * W],
            ring: vec![],
            occ: vec![0; W * W],
            cstart: vec![0; W * W + 2],
            order: vec![],
            tile_at: usize::MAX,
            px: (f.start % W) as f64 + 0.5,
            py: (f.start / W) as f64 + 0.5,
            h: Horde {
                x: vec![],
                y: vec![],
                hp: vec![],
                fam: vec![],
                blade_cd: vec![],
                hit_cd: vec![],
                flash: vec![],
            },
            bolts: vec![],
            arcs: vec![],
            deaths: vec![],
            shards: vec![],
            nova: None,
            blade_a: 0.0,
            cd: [0.0; 3],
            hp: PMAX,
            god: true,
            level: 1,
            xp: 0,
            kills: 0,
            kills_sec: 0,
            time: 0.0,
            live,
            dmg: 1.0,
            arena,
            rng: X::new(seed ^ 0x5eed),
            t,
        };
        g.retile();
        g
    }
    fn walk(&self, x: f64, y: f64) -> bool {
        x >= 0.0 && y >= 0.0 && x < W as f64 && y < W as f64 && self.t[tl(x, y)] == OPEN
    }
    fn retile(&mut self) {
        let s = tl(self.px, self.py);
        if s == self.tile_at {
            return;
        }
        self.tile_at = s;
        self.vis.fill(false);
        let (cx, cy) = ((s % W) as i32, (s / W) as i32);
        let rr: i32 = if self.arena { 45 } else { 22 }; // sight radius m
        for y in (cy - rr).max(0)..=(cy + rr).min(W as i32 - 1) {
            for x in (cx - rr).max(0)..=(cx + rr).min(W as i32 - 1) {
                let c = y as usize * W + x as usize;
                if (x - cx).pow(2) + (y - cy).pow(2) <= rr * rr && (self.t[c] != ROCK || self.wall[c]) && los_t(&self.t, s, c) {
                    self.vis[c] = true;
                    self.explored[c] = true;
                }
            }
        }
        self.dist.fill(u16::MAX);
        self.ring.clear();
        let mut q = vec![s as u32];
        self.dist[s] = 0;
        let mut i = 0;
        while i < q.len() {
            let c = q[i] as usize;
            i += 1;
            let d = self.dist[c];
            if (14..=40).contains(&d) && (!self.vis[c] || self.arena && d >= 20) {
                self.ring.push(c as u32);
            }
            for o in [c.wrapping_sub(1), c + 1, c.wrapping_sub(W), c + W] {
                if o < W * W && self.t[o] == OPEN && self.dist[o] == u16::MAX {
                    self.dist[o] = d + 1;
                    q.push(o as u32);
                }
            }
        }
    }
    fn spawn(&mut self) {
        let mut tries = 0;
        while self.h.x.len() < self.live && !self.ring.is_empty() && tries < 200 {
            tries += 1;
            let c = self.ring[self.rng.below(self.ring.len() as u32) as usize] as usize;
            if self.occ[c] >= 3 {
                continue;
            }
            self.occ[c] += 1;
            let r = self.rng.below(100);
            let f = if r < 6 {
                Fam::Brute
            } else if r < 22 {
                Fam::Runner
            } else if r < 45 {
                Fam::Drone
            } else {
                Fam::Ghoul
            };
            let hpm = fam(f).3 * (1.0 + 0.1 * self.time / 60.0);
            let h = &mut self.h;
            h.x.push((c % W) as f64 + 0.2 + 0.6 * self.rng.f());
            h.y.push((c / W) as f64 + 0.2 + 0.6 * self.rng.f());
            h.hp.push(30.0 * hpm);
            h.fam.push(f);
            h.blade_cd.push(0.0);
            h.hit_cd.push(0.0);
            h.flash.push(0.0);
        }
    }
    /// bucket enemies by tile (counting sort) for local queries
    fn bucket(&mut self) {
        let n = self.h.x.len();
        self.cstart.fill(0);
        for i in 0..n {
            self.cstart[tl(self.h.x[i], self.h.y[i]) + 1] += 1;
        }
        for k in 0..W * W {
            self.cstart[k + 1] += self.cstart[k];
        }
        self.order.resize(n, 0);
        let mut fill = self.cstart.clone(); // toy shortcut: 1 alloc/tick
        for i in 0..n {
            let c = tl(self.h.x[i], self.h.y[i]);
            self.order[fill[c] as usize] = i as u32;
            fill[c] += 1;
        }
        for k in 0..W * W {
            self.occ[k] = (self.cstart[k + 1] - self.cstart[k]).min(255) as u8;
        }
    }
    /// enemies within r of (x, y)
    fn near(&self, x: f64, y: f64, r: f64, out: &mut Vec<usize>) {
        out.clear();
        let (x0, x1) = ((x - r).max(0.0) as usize, ((x + r) as usize).min(W - 1));
        let (y0, y1) = ((y - r).max(0.0) as usize, ((y + r) as usize).min(W - 1));
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                let k = ty * W + tx;
                for &j in &self.order[self.cstart[k] as usize..self.cstart[k + 1] as usize] {
                    let j = j as usize;
                    if (self.h.x[j] - x).powi(2) + (self.h.y[j] - y).powi(2) <= r * r {
                        out.push(j);
                    }
                }
            }
        }
    }
    fn damage(&mut self, i: usize, d: f64) {
        self.h.hp[i] -= d;
        self.h.flash[i] = 0.12;
    }
    fn step(&mut self, dt: f64, mv: (f64, f64)) {
        self.time += dt;
        let l = (mv.0 * mv.0 + mv.1 * mv.1).sqrt();
        if l > 0.0 {
            let (dx, dy) = (mv.0 / l * 5.0 * dt, mv.1 / l * 5.0 * dt);
            if self.walk(self.px + dx + dx.signum() * 0.3, self.py) {
                self.px += dx;
            }
            if self.walk(self.px, self.py + dy + dy.signum() * 0.3) {
                self.py += dy;
            }
        }
        self.retile();
        self.bucket();
        self.spawn();
        // move: flow-field descent + tile cap 3 (T3 law)
        let n = self.h.x.len();
        let mut contact = 0.0;
        for i in 0..n {
            let (x, y) = (self.h.x[i], self.h.y[i]);
            let c = tl(x, y);
            let mut best = (c, self.dist[c]);
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let o = c as i32 + dx + dy * W as i32;
                if o < 0 || o as usize >= W * W {
                    continue;
                }
                let ok = dx == 0 || dy == 0 || (self.t[(c as i32 + dx) as usize] == OPEN && self.t[(c as i32 + dy * W as i32) as usize] == OPEN);
                if ok && self.dist[o as usize] < best.1 {
                    best = (o as usize, self.dist[o as usize]);
                }
            }
            let (tx, ty) = if best.0 == c {
                (self.px, self.py)
            } else {
                ((best.0 % W) as f64 + 0.5, (best.0 / W) as f64 + 0.5)
            };
            let (dx, dy) = (tx - x, ty - y);
            let d = (dx * dx + dy * dy).sqrt().max(1e-6);
            let sp = fam(self.h.fam[i]).2;
            let pd = ((self.px - x).powi(2) + (self.py - y).powi(2)).sqrt();
            self.h.hit_cd[i] = (self.h.hit_cd[i] - dt as f32).max(0.0);
            self.h.blade_cd[i] = (self.h.blade_cd[i] - dt as f32).max(0.0);
            self.h.flash[i] = (self.h.flash[i] - dt as f32).max(0.0);
            if pd > 0.7 {
                let (nx, ny) = (x + dx / d * sp * dt, y + dy / d * sp * dt);
                let nt = tl(nx, ny);
                if self.walk(nx, ny) && (nt == c || self.occ[nt] < 3) {
                    self.occ[c] = self.occ[c].saturating_sub(1);
                    self.occ[nt] = self.occ[nt].saturating_add(1);
                    (self.h.x[i], self.h.y[i]) = (nx, ny);
                }
            } else if self.h.hit_cd[i] == 0.0 {
                self.h.hit_cd[i] = 0.5;
                contact += 4.0 * fam(self.h.fam[i]).3.sqrt();
            }
        }
        if !self.god {
            self.hp -= contact;
            if self.hp <= 0.0 {
                self.hp = PMAX;
            }
        }
        self.bucket();
        let pw = self.dmg * (1.0 + 0.12 * (self.level - 1) as f64);
        let mut buf = vec![];
        // weapon 1: bolts x3 spread at nearest visible within 16 m, pierce 2
        self.cd[0] -= dt;
        if self.cd[0] <= 0.0 {
            self.cd[0] = 0.12;
            self.near(self.px, self.py, 16.0, &mut buf);
            let tgt = buf.iter().filter(|&&j| self.vis[tl(self.h.x[j], self.h.y[j])]).min_by(|&&a, &&b| {
                let da = (self.h.x[a] - self.px).powi(2) + (self.h.y[a] - self.py).powi(2);
                let db = (self.h.x[b] - self.px).powi(2) + (self.h.y[b] - self.py).powi(2);
                da.total_cmp(&db)
            });
            if let Some(&j) = tgt {
                let a0 = (self.h.y[j] - self.py).atan2(self.h.x[j] - self.px);
                for k in [-1.0, 0.0, 1.0] {
                    let a = a0 + 0.12 * k;
                    self.bolts.push(Bolt {
                        x: self.px,
                        y: self.py,
                        vx: a.cos() * 24.0,
                        vy: a.sin() * 24.0,
                        life: 0.7,
                        pierce: 2,
                    });
                }
            }
        }
        // weapon 2: chain arc every 0.6 s, 8 jumps within 5 m
        self.cd[1] -= dt;
        if self.cd[1] <= 0.0 {
            self.cd[1] = 0.6;
            let (mut cx, mut cy) = (self.px, self.py);
            let mut path = vec![(cx, cy)];
            let mut hit: Vec<usize> = vec![];
            for _ in 0..8 {
                self.near(cx, cy, 5.0, &mut buf);
                let Some(&j) = buf
                    .iter()
                    .filter(|j| !hit.contains(j))
                    .min_by(|&&a, &&b| ((self.h.x[a] - cx).powi(2) + (self.h.y[a] - cy).powi(2)).total_cmp(&((self.h.x[b] - cx).powi(2) + (self.h.y[b] - cy).powi(2))))
                else {
                    break;
                };
                hit.push(j);
                (cx, cy) = (self.h.x[j], self.h.y[j]);
                path.push((cx, cy));
                self.damage(j, 45.0 * pw);
            }
            if path.len() > 1 {
                self.arcs.push((path, 0.12));
            }
        }
        // weapon 3: 4 orbit blades r 2.5 m
        self.blade_a += dt * 4.0;
        for k in 0..4 {
            let a = self.blade_a + k as f64 * TAU / 4.0;
            let (bx, by) = (self.px + 2.5 * a.cos(), self.py + 2.5 * a.sin());
            self.near(bx, by, 0.7, &mut buf);
            for &j in &buf {
                if self.h.blade_cd[j] == 0.0 {
                    self.h.blade_cd[j] = 0.3;
                    self.h.hp[j] -= 25.0 * pw;
                    self.h.flash[j] = 0.12;
                }
            }
        }
        // nova every 3 s, ring to 9 m in 0.45 s
        self.cd[2] -= dt;
        if self.cd[2] <= 0.0 {
            self.cd[2] = 3.0;
            self.nova = Some(0.0);
        }
        if let Some(age) = self.nova {
            let (r0, r1) = (9.0 * age / 0.45, 9.0 * (age + dt) / 0.45);
            self.near(self.px, self.py, r1, &mut buf);
            for &j in &buf {
                let d = ((self.h.x[j] - self.px).powi(2) + (self.h.y[j] - self.py).powi(2)).sqrt();
                if d >= r0 {
                    self.h.hp[j] -= 60.0 * pw;
                    self.h.flash[j] = 0.12;
                }
            }
            self.nova = if age + dt > 0.45 { None } else { Some(age + dt) };
        }
        // bolts
        let mut bolts = std::mem::take(&mut self.bolts);
        for b in &mut bolts {
            b.x += b.vx * dt;
            b.y += b.vy * dt;
            b.life -= dt;
            if !self.walk(b.x, b.y) {
                b.life = 0.0;
                continue;
            }
            self.near(b.x, b.y, 0.5, &mut buf);
            if let Some(&j) = buf.first() {
                self.h.hp[j] -= 22.0 * pw;
                self.h.flash[j] = 0.12;
                if b.pierce == 0 { b.life = 0.0 } else { b.pierce -= 1 }
            }
        }
        bolts.retain(|b| b.life > 0.0);
        self.bolts = bolts;
        for a in &mut self.arcs {
            a.1 -= dt;
        }
        self.arcs.retain(|a| a.1 > 0.0);
        for d in &mut self.deaths {
            d.2 -= dt;
        }
        self.deaths.retain(|d| d.2 > 0.0);
        // deaths: swap-remove (reverse order)
        let mut i = self.h.x.len();
        while i > 0 {
            i -= 1;
            if self.h.hp[i] <= 0.0 {
                let h = &mut self.h;
                self.deaths.push((h.x[i], h.y[i], 0.15));
                if self.shards.len() < 3000 {
                    self.shards.push((h.x[i], h.y[i]));
                }
                for v in [&mut h.x, &mut h.y, &mut h.hp] {
                    v.swap_remove(i);
                }
                h.fam.swap_remove(i);
                h.blade_cd.swap_remove(i);
                h.hit_cd.swap_remove(i);
                h.flash.swap_remove(i);
                self.kills += 1;
                self.kills_sec += 1;
            }
        }
        // shards: magnet 4 m
        let (px, py) = (self.px, self.py);
        let mut got = 0;
        self.shards.retain_mut(|s| {
            let (dx, dy) = (px - s.0, py - s.1);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 0.5 {
                got += 1;
                return false;
            }
            if d < 4.0 {
                s.0 += dx / d * 12.0 * dt;
                s.1 += dy / d * 12.0 * dt;
            }
            true
        });
        self.xp += got;
        while self.xp >= 20 + 10 * self.level {
            self.xp -= 20 + 10 * self.level;
            self.level += 1;
        }
    }
}

// ---------- render: one glyph per body on a 2:1 iso cell grid ----------
struct View {
    k: f64, // cols per unit (u - v)
    aspect: f64,
    heading: i32,
}
struct Cam {
    cx: f64,
    cy: f64,
    ox: f64,
    oy: f64,
    k: f64,
    asp: f64,
    h: i32,
}
impl Cam {
    fn rot(&self, x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = (x - self.cx, y - self.cy);
        match self.h.rem_euclid(4) {
            0 => (dx, dy),
            1 => (-dy, dx),
            2 => (-dx, -dy),
            _ => (dy, -dx),
        }
    }
    fn unrot(&self, u: f64, v: f64) -> (f64, f64) {
        let (dx, dy) = match self.h.rem_euclid(4) {
            0 => (u, v),
            1 => (v, -u),
            2 => (-u, -v),
            _ => (-v, u),
        };
        (dx + self.cx, dy + self.cy)
    }
    /// (col, row) of world point at height z
    fn p(&self, x: f64, y: f64, z: f64) -> (f64, f64) {
        let (u, v) = self.rot(x, y);
        (self.ox + (u - v) * self.k, self.oy + ((u + v) * self.k / 2.0 - z * self.k * ZK) / self.asp)
    }
}
fn cam_for(g: &Game, v: &View, cols: usize, prow: usize) -> (Cam, (i64, i64)) {
    let mut c = Cam {
        cx: g.px,
        cy: g.py,
        ox: cols as f64 / 2.0,
        oy: prow as f64 * 0.55,
        k: v.k,
        asp: v.aspect,
        h: v.heading,
    };
    // snap world origin to whole cells: pan = whole-cell steps (terminal scroll)
    let (c0, r0) = c.p(0.0, 0.0, 0.0);
    c.ox -= c0 - c0.floor();
    c.oy -= r0 - r0.floor();
    let (c1, r1) = c.p(0.0, 0.0, 0.0);
    (c, (c1.round() as i64, r1.round() as i64))
}
struct Frame {
    cells: Vec<Cell>,
    depth: Vec<f32>,
    onscreen: usize,
}
fn render(g: &Game, v: &View, cols: usize, prow: usize, fr: &mut Frame) -> (i64, i64) {
    let (cam, org) = cam_for(g, v, cols, prow);
    fr.cells[..cols * prow].fill(BLANK);
    fr.depth.fill(f32::MIN);
    // floor: per cell inverse projection (z = 0)
    for r in 0..prow {
        for c in 0..cols {
            let a = (c as f64 + 0.5 - cam.ox) / cam.k;
            let b = (r as f64 + 0.5 - cam.oy) * cam.asp * 2.0 / cam.k;
            let (x, y) = cam.unrot((a + b) / 2.0, (b - a) / 2.0);
            if x < 0.0 || y < 0.0 || x >= W as f64 || y >= W as f64 {
                continue;
            }
            let i = tl(x, y);
            if !g.explored[i] || g.t[i] == ROCK && !g.wall[i] {
                continue;
            }
            let dot = (x.fract() - 0.5).abs() < 0.5 / cam.k && (y.fract() - 0.5).abs() < 0.25;
            let lit = if g.vis[i] {
                1.0 - 0.55 * (((x - g.px).powi(2) + (y - g.py).powi(2)).sqrt() / 22.0).min(1.0)
            } else {
                0.35
            };
            let lit = (lit * 6.0).round() / 6.0;
            fr.cells[r * cols + c] = Cell {
                ch: if dot { '·' } else { ' ' },
                fg: FLOORC.scale(2.6 * lit),
                bg: FLOORC.scale(lit),
            };
        }
    }
    // walls: back to front by u + v, footprint cells + 2 m face
    let rad = (cols as f64 / cam.k * 0.6) as i32 + 3;
    let (tx0, ty0) = (g.px as i32, g.py as i32);
    let mut walls: Vec<(f64, usize)> = vec![];
    for ty in (ty0 - rad).max(0)..(ty0 + rad).min(W as i32) {
        for tx in (tx0 - rad).max(0)..(tx0 + rad).min(W as i32) {
            let i = ty as usize * W + tx as usize;
            if g.wall[i] && g.explored[i] {
                let (u, vv) = cam.rot(tx as f64 + 0.5, ty as f64 + 0.5);
                walls.push((u + vv, i));
            }
        }
    }
    walls.sort_by(|a, b| a.0.total_cmp(&b.0));
    let hrows = (2.0 * cam.k * ZK / cam.asp).round().max(1.0) as i64;
    for &(_, i) in &walls {
        let (x, y) = ((i % W) as f64 + 0.5, (i / W) as f64 + 0.5);
        let (c, r) = cam.p(x, y, 0.0);
        let lit = if g.vis[i] {
            1.0 - 0.5 * (((x - g.px).powi(2) + (y - g.py).powi(2)).sqrt() / 22.0).min(1.0)
        } else {
            0.4
        };
        let lit = (lit * 6.0).round() / 6.0;
        let half = (cam.k * 0.75).max(0.5);
        for cc in (c - half).round() as i64..(c + half).round() as i64 {
            for k in 0..=hrows {
                let rr = r.round() as i64 - k;
                if cc < 0 || rr < 0 || cc >= cols as i64 || rr >= prow as i64 {
                    continue;
                }
                let top = k == hrows;
                let face = if top {
                    WALLC.scale(1.4 * lit)
                } else if (cc as f64) < c {
                    WALLC.scale(0.8 * lit)
                } else {
                    WALLC.scale(0.5 * lit)
                };
                let ch = if top {
                    '▀'
                } else if k == hrows / 2 && g.t[i] == PILLAR {
                    '═'
                } else {
                    '█'
                };
                fr.cells[rr as usize * cols + cc as usize] = Cell {
                    ch,
                    fg: if ch == '═' { TRIM } else { face },
                    bg: if top { face.scale(0.7) } else { face },
                };
            }
        }
    }
    // entities: one glyph per body; >= 3 in a cell => density glyph (design sec 5.5)
    let mut count = vec![0u8; cols * prow];
    let put = |fr: &mut Frame, x: f64, y: f64, z: f64, ch: char, fg: Rgb, pri: f32| -> bool {
        let (c, r) = cam.p(x, y, z);
        let (c, r) = (c.floor() as i64, r.floor() as i64);
        if c < 0 || r < 0 || c >= cols as i64 || r >= prow as i64 {
            return false;
        }
        let i = r as usize * cols + c as usize;
        if pri >= fr.depth[i] {
            fr.depth[i] = pri;
            let bg = fr.cells[i].bg;
            fr.cells[i] = Cell { ch, fg, bg };
        }
        true
    };
    for &(x, y, k) in &g.props {
        if g.explored[tl(x, y)] {
            let (ch, fg) = match k {
                PropK::Brazier => ('Ψ', Rgb::hex(0xff2bd6)),
                PropK::Rack => ('▥', TRIM),
                PropK::Crate => ('■', Rgb::hex(0x8a6a40)),
            };
            put(fr, x, y, 0.0, ch, fg, 1.5);
        }
    }
    for s in &g.shards {
        if g.vis[tl(s.0, s.1)] {
            put(fr, s.0, s.1, 0.0, '∙', Rgb::hex(0x7af9ff), 1.0);
        }
    }
    fr.onscreen = 0;
    for i in 0..g.h.x.len() {
        let (x, y) = (g.h.x[i], g.h.y[i]);
        if !g.vis[tl(x, y)] {
            continue;
        }
        let (ch, hue, _, _) = fam(g.h.fam[i]);
        let fg = if g.h.flash[i] > 0.0 { Rgb::hex(0xffffff) } else { hue };
        let (c, r) = cam.p(x, y, 0.0);
        if c >= 0.0 && r >= 0.0 && (c as usize) < cols && (r as usize) < prow {
            let k = r as usize * cols + c as usize;
            count[k] = count[k].saturating_add(1);
            let glyph = match count[k] {
                0..=2 => ch,
                3 => '▪',
                _ => '▓',
            };
            if put(fr, x, y, 0.0, glyph, fg, 2.0) {
                fr.onscreen += 1;
            }
        }
    }
    for d in &g.deaths {
        put(fr, d.0, d.1, 0.0, '*', Rgb::hex(0xffffff).scale(0.5 + d.2 / 0.3), 3.0);
    }
    for b in &g.bolts {
        put(fr, b.x, b.y, 0.5, '•', TRIM, 4.0);
    }
    for (path, _) in &g.arcs {
        for w in path.windows(2) {
            let ((x0, y0), (x1, y1)) = (w[0], w[1]);
            let n = (((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt() * 3.0).ceil().max(1.0) as usize;
            for s in 0..=n {
                let t = s as f64 / n as f64;
                put(fr, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, 0.6, if s % 2 == 0 { '~' } else { '-' }, Rgb::hex(0x9ab8ff), 4.0);
            }
        }
    }
    for k in 0..4 {
        let a = g.blade_a + k as f64 * TAU / 4.0;
        put(fr, g.px + 2.5 * a.cos(), g.py + 2.5 * a.sin(), 0.3, '+', Rgb::hex(0xff8a1f), 5.0);
    }
    if let Some(age) = g.nova {
        let r = 9.0 * age / 0.45;
        for k in 0..(r * 8.0) as usize + 8 {
            let a = k as f64 / ((r * 8.0) as usize + 8) as f64 * TAU;
            put(fr, g.px + r * a.cos(), g.py + r * a.sin(), 0.2, '°', TRIM, 4.5);
        }
    }
    put(fr, g.px, g.py, 0.0, '@', Rgb::hex(0xffffff), 9.0);
    org
}

// ---------- UI strip (3 rows) ----------
fn text(cells: &mut [Cell], cols: usize, row: usize, col: usize, s: &str, fg: Rgb) -> usize {
    let mut c = col;
    for ch in s.chars().take(cols.saturating_sub(col)) {
        cells[row * cols + c] = Cell { ch, fg, bg: STRIP };
        c += 1;
    }
    c
}
fn ui(g: &Game, cells: &mut [Cell], cols: usize, rows: usize, perf: &str) {
    let base = rows - 3;
    for c in cells[base * cols..].iter_mut() {
        *c = Cell { ch: ' ', fg: UI, bg: STRIP };
    }
    let hpbar: String = (0..20).map(|i| if (i as f64) < g.hp / PMAX * 20.0 { '█' } else { '░' }).collect();
    text(
        cells,
        cols,
        base,
        1,
        &format!(
            "HP {hpbar} {:.0}{}  LV {}  {}:{:02}  K {}  live {}  bolts {}",
            g.hp,
            if g.god { " GOD" } else { "" },
            g.level,
            (g.time / 60.0) as u32,
            g.time as u32 % 60,
            g.kills,
            g.h.x.len(),
            g.bolts.len()
        ),
        UI,
    );
    text(cells, cols, base + 1, 1, perf, UI.scale(0.75));
    text(
        cells,
        cols,
        base + 2,
        1,
        "wasd/arrows move (kitty kb: held keys)   q/e turn   g god   +/- zoom   x quit",
        UI.scale(0.55),
    );
}

// ---------- frame + loop ----------
struct Screen {
    cols: usize,
    rows: usize,
    fr: Frame,
    prev: Vec<Cell>,
    out: Vec<u8>,
    org: Option<(i64, i64, i32)>,
}
impl Screen {
    fn new(cols: usize, rows: usize) -> Screen {
        Screen {
            cols,
            rows,
            fr: Frame {
                cells: vec![BLANK; cols * rows],
                depth: vec![f32::MIN; cols * rows],
                onscreen: 0,
            },
            prev: vec![BLANK; cols * rows],
            out: Vec::with_capacity(1 << 20),
            org: None,
        }
    }
    /// render + ui + (terminal scroll) + diff emit; returns render us
    fn frame(&mut self, g: &Game, v: &View, force: bool, perf: &str) -> u128 {
        let t = Instant::now();
        let (cols, rows) = (self.cols, self.rows);
        let prow = rows - 3;
        let org = render(g, v, cols, prow, &mut self.fr);
        ui(g, &mut self.fr.cells, cols, rows, perf);
        let us = t.elapsed().as_micros();
        self.out.clear();
        let now = (org.0, org.1, v.heading);
        if let Some(o) = self.org {
            let pan = (now.0 - o.0, now.1 - o.1);
            if !force && o.2 == now.2 && pan != (0, 0) && pan.0.unsigned_abs() < cols as u64 / 3 && pan.1.unsigned_abs() < prow as u64 / 3 {
                scroll(&mut self.prev, cols, prow, pan, &mut self.out);
            }
        }
        self.org = Some(now);
        emit(&self.prev, &self.fr.cells, cols, force, &mut self.out);
        self.prev.copy_from_slice(&self.fr.cells);
        us
    }
}
struct Opts {
    seed: u64,
    live: usize,
    k: f64,
    dmg: f64,
    arena: bool,
}
fn opts(a: &[String]) -> Res<Opts> {
    let mut o = Opts { seed: 7, live: 1500, k: 2.0, dmg: 0.25, arena: false };
    for i in (0..a.len()).step_by(2) {
        let v = a.get(i + 1).ok_or(format!("missing value for {}", a[i]))?;
        let n = if a[i] == "--arena" { 1.0 } else { v.parse::<f64>().map_err(|e| format!("{}: {e}", a[i]))? };
        match a[i].as_str() {
            "--seed" => o.seed = n as u64,
            "--live" => o.live = n as usize,
            "--k" => o.k = n,
            "--dmg" => o.dmg = n,
            "--arena" => o.arena = v == "1",
            x => return Err(format!("unknown flag {x}")),
        }
    }
    Ok(o)
}
/// screen dirs (up, left, down, right) -> world move vector for heading h
fn move_vec(held: [bool; 4], h: i32) -> (f64, f64) {
    let dirs = [(-1.0, -1.0), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)];
    let (mut su, mut sv) = (0.0f64, 0.0f64);
    for k in 0..4 {
        if held[k] {
            su += dirs[k].0;
            sv += dirs[k].1;
        }
    }
    let cam = Cam {
        cx: 0.0,
        cy: 0.0,
        ox: 0.0,
        oy: 0.0,
        k: 1.0,
        asp: 1.0,
        h,
    };
    cam.unrot(su * FRAC_1_SQRT_2, sv * FRAC_1_SQRT_2)
}
enum Key {
    Dir(usize, bool),
    Char(char),
}
/// kitty keyboard (flags 1|2|8): CSI code[:alt];mods[:event] u, arrows CSI 1;mods[:event] A-D; event 3 = release
fn parse_csi(s: &str) -> Option<Key> {
    let body = s.strip_prefix("\x1b[")?;
    let fin = body.chars().last()?;
    let params = &body[..body.len() - fin.len_utf8()];
    let mut it = params.split(';');
    let code: u32 = it.next().and_then(|p| p.split(':').next()).and_then(|p| p.parse().ok()).unwrap_or(1);
    let mods_ev = it.next().unwrap_or("1");
    let mut me = mods_ev.split(':');
    let mods: u32 = me.next().and_then(|p| p.parse().ok()).unwrap_or(1);
    let ev: u32 = me.next().and_then(|p| p.parse().ok()).unwrap_or(1);
    let down = ev != 3;
    let dir = |k| Some(Key::Dir(k, down));
    match (fin, code) {
        ('A', _) => dir(0),
        ('D', _) => dir(1),
        ('B', _) => dir(2),
        ('C', _) => dir(3),
        ('u', 119) => dir(0),
        ('u', 97) => dir(1),
        ('u', 115) => dir(2),
        ('u', 100) => dir(3),
        ('u', 99) if (mods - 1) & 4 != 0 && down => Some(Key::Char('x')),
        ('u', c) if down && ev == 1 => char::from_u32(c).map(Key::Char),
        _ => None,
    }
}
fn play(o: Opts) -> Res<String> {
    let size = stty(&["size"])?;
    let mut it = size.split_whitespace().map(|s| s.parse::<usize>());
    let (Some(Ok(rows)), Some(Ok(cols))) = (it.next(), it.next()) else {
        return Err(format!("stty size: {size}"));
    };
    if cols < 80 || rows < 24 {
        return Err(format!("terminal {cols}x{rows} too small"));
    }
    let _raw = Raw(stty(&["-g"])?);
    stty(&["raw", "-echo"])?;
    let mut so = std::io::stdout();
    so.write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[>11u\x1b[16t").map_err(|e| e.to_string())?;
    so.flush().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel::<u8>();
    std::thread::spawn(move || {
        let mut b = [0u8; 256];
        let mut i = std::io::stdin();
        while let Ok(n) = i.read(&mut b) {
            if n == 0 || b[..n].iter().any(|&c| tx.send(c).is_err()) {
                break;
            }
        }
    });
    let mut g = Game::new(o.seed, o.live, o.arena);
    g.dmg = o.dmg;
    let mut v = View { k: o.k, aspect: 2.0, heading: 0 };
    let mut sc = Screen::new(cols, rows);
    let _ = std::fs::write("swarm-perf.log", "");
    let (mut held, mut legacy) = ([false; 4], [0.0f64; 4]);
    let mut esc: Vec<u8> = vec![];
    let mut force = true;
    let mut perf = String::new();
    let (mut acc_b, mut acc_r, mut acc_s, mut nfr, mut nticks, mut tick) = (0usize, 0u128, 0u128, 0u32, 0u32, Instant::now());
    let (mut acc_t, sim_dt) = (0.0f64, 1.0 / 30.0);
    let mut last = Instant::now();
    loop {
        let t0 = Instant::now();
        let mut keys: Vec<Key> = vec![];
        while let Ok(c) = rx.try_recv() {
            if !esc.is_empty() || c == 0x1b {
                esc.push(c);
                if esc.len() > 2 && (0x40..=0x7e).contains(&c) && c != b'[' {
                    let s = String::from_utf8_lossy(&esc).to_string();
                    if let Some(p) = s.strip_prefix("\x1b[6;").and_then(|r| r.strip_suffix('t')) {
                        let n: Vec<f64> = p.split(';').filter_map(|x| x.parse().ok()).collect();
                        if n.len() == 2 && n[1] > 0.0 {
                            v.aspect = n[0] / n[1];
                            force = true;
                        }
                    } else if let Some(k) = parse_csi(&s) {
                        keys.push(k);
                    }
                    esc.clear();
                } else if esc.len() > 24 {
                    esc.clear();
                }
                continue;
            }
            // legacy bytes (terminal without kitty keyboard): timers bridge the autorepeat delay
            match c {
                b'w' => legacy[0] = 0.6,
                b'a' => legacy[1] = 0.6,
                b's' => legacy[2] = 0.6,
                b'd' => legacy[3] = 0.6,
                3 => keys.push(Key::Char('x')),
                _ => keys.push(Key::Char(c as char)),
            }
        }
        for k in keys {
            match k {
                Key::Dir(d, down) => held[d] = down,
                Key::Char('x') => return Ok(perf),
                Key::Char('q') => v.heading -= 1,
                Key::Char('e') => v.heading += 1,
                Key::Char('g') => g.god = !g.god,
                Key::Char('+') | Key::Char('=') => v.k = (v.k * 1.25).min(8.0),
                Key::Char('-') => v.k = (v.k / 1.25).max(0.5),
                Key::Char(_) => {}
            }
        }
        // fixed 30 Hz sim, render every frame
        let now = Instant::now();
        acc_t = (acc_t + now.duration_since(last).as_secs_f64()).min(0.25);
        last = now;
        let dt = 1.0 / 60.0;
        let mut eff = held;
        for (k, l) in legacy.iter_mut().enumerate() {
            if *l > 0.0 {
                eff[k] = true;
                *l -= dt;
            }
        }
        while acc_t >= sim_dt {
            acc_t -= sim_dt;
            let ts = Instant::now();
            g.step(sim_dt, move_vec(eff, v.heading));
            acc_s += ts.elapsed().as_micros();
            nticks += 1;
        }
        let us = sc.frame(&g, &v, force, &perf);
        force = false;
        so.write_all(&sc.out).map_err(|e| e.to_string())?;
        so.flush().map_err(|e| e.to_string())?;
        acc_b += sc.out.len();
        acc_r += us;
        nfr += 1;
        if tick.elapsed() >= Duration::from_secs(1) {
            let n = nfr.max(1) as f64;
            perf = format!(
                "{cols}x{rows} asp {:.2} k {:.2} | live {} onscreen {} kills/s {} | {:.1} KB/f render {:.2} ms sim {:.2} ms/tick | {} fps",
                v.aspect,
                v.k,
                g.h.x.len(),
                sc.fr.onscreen,
                g.kills_sec,
                acc_b as f64 / n / 1024.0,
                acc_r as f64 / n / 1000.0,
                acc_s as f64 / nticks.max(1) as f64 / 1000.0,
                nfr
            );
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("swarm-perf.log") {
                let _ = writeln!(f, "{:.0}s | {perf}", g.time);
            }
            g.kills_sec = 0;
            (acc_b, acc_r, acc_s, nfr, nticks, tick) = (0, 0, 0, 0, 0, Instant::now());
        }
        if let Some(rest) = Duration::from_secs_f64(dt).checked_sub(t0.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}
/// headless: scripted kiting 40 s (1200 ticks, 2400 frames) per (grid, live); prints sim + render + bytes
fn shot(o: Opts) -> Res<()> {
    for (cols, rows) in [(160usize, 50usize), (249, 111)] {
        for live in [600usize, 1500, 3000] {
            let mut g = Game::new(o.seed, live, o.arena);
            g.dmg = o.dmg;
            let v = View { k: o.k, aspect: 2.3, heading: 0 };
            let mut sc = Screen::new(cols, rows);
            let (mut bytes, mut rus, mut sus, mut scr) = (vec![], vec![], vec![], vec![]);
            for f in 0..2400 {
                if f % 2 == 0 {
                    let a = f as f64 / 400.0;
                    let ts = Instant::now();
                    g.step(1.0 / 30.0, (a.cos(), a.sin()));
                    sus.push(ts.elapsed().as_micros() as u64);
                }
                let us = sc.frame(&g, &v, f == 0, "");
                if f > 600 {
                    bytes.push(sc.out.len() as u64);
                    rus.push(us as u64);
                    scr.push(sc.fr.onscreen as u64);
                }
            }
            let pc = |v: &mut Vec<u64>, q: f64| {
                v.sort_unstable();
                v[((v.len() - 1) as f64 * q) as usize]
            };
            println!(
                "{cols}x{rows} live {live:>4} | onscreen p50 {:>4} | kills {:>5} | sim us p50 {:>5} p99 {:>5} | render us p50 {:>5} p99 {:>5} | bytes mean {:>6} p50 {:>6} p99 {:>6}",
                pc(&mut scr, 0.5),
                g.kills,
                pc(&mut sus, 0.5),
                pc(&mut sus, 0.99),
                pc(&mut rus, 0.5),
                pc(&mut rus, 0.99),
                bytes.iter().sum::<u64>() / bytes.len() as u64,
                pc(&mut bytes, 0.5),
                pc(&mut bytes, 0.99)
            );
        }
    }
    Ok(())
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let r = match a.get(1).map(String::as_str) {
        Some("shot") => opts(&a[2..]).and_then(shot),
        _ => opts(&a[1..]).and_then(play).map(|p| println!("last perf: {p}")),
    };
    if let Err(e) = r {
        eprintln!("swarm: {e}");
        std::process::exit(1);
    }
}
