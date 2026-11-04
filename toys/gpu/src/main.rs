//! T14 probe "gpu" (vic 2026-11-04: "rust+wgpu+winit for t14", "effulgence like art").
//! Everything is a glowing glyph placed freely in 2:1 iso world space (no cell grid): glyph-field floor,
//! glyph-stack walls, glyph-cloud bodies (dense bright core, sparse dim rim), glyph-chain fx, HDR + bloom.
//! Horde sim on the CPU (spatial grid separation), instances uploaded per frame, one instanced draw.
//! keys (native press/release): wasd / arrows move, q/e turn (smooth), +/- zoom, 1..5 horde size, x quit.
//! bench: GPU_BENCH=1 ./gpu  -> gpu-bench.jsonl (scripted, uncapped present mode when available)
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

type Res<T> = Result<T, String>;

// ---------- tiny std block_on (no extra crate) ----------
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    struct Wake(std::thread::Thread);
    impl std::task::Wake for Wake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(Wake(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

// ---------- PSF2 bitmap font -> glyph atlas ----------
struct Font {
    w: u32,
    h: u32,
    per_row: u32,
    atlas_w: u32,
    atlas_h: u32,
    pixels: Vec<u8>, // R8
    map: std::collections::HashMap<char, u32>,
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn load_font(b: &[u8]) -> Res<Font> {
    if b.len() < 32 || b[..4] != [0x72, 0xb5, 0x4a, 0x86] {
        return Err("not a PSF2 font".into());
    }
    let (hs, flags, n, cs, h, w) = (u32le(b, 8) as usize, u32le(b, 12), u32le(b, 16), u32le(b, 20) as usize, u32le(b, 24), u32le(b, 28));
    let bpr = w.div_ceil(8) as usize;
    let per_row = 32u32;
    let (aw, ah) = (per_row * w, n.div_ceil(per_row) * h);
    let mut px = vec![0u8; (aw * ah) as usize];
    for g in 0..n {
        let base = hs + g as usize * cs;
        let (ox, oy) = ((g % per_row) * w, (g / per_row) * h);
        for y in 0..h as usize {
            for x in 0..w as usize {
                if b.get(base + y * bpr + x / 8).is_some_and(|byte| byte & (0x80 >> (x % 8)) != 0) {
                    px[((oy as usize + y) * aw as usize) + ox as usize + x] = 255;
                }
            }
        }
    }
    let mut map = std::collections::HashMap::new();
    if flags & 1 != 0 {
        // unicode table: per glyph, UTF-8 sequences separated, 0xFF terminates
        let mut p = hs + n as usize * cs;
        for g in 0..n {
            let start = p;
            while p < b.len() && b[p] != 0xff {
                p += 1;
            }
            for s in String::from_utf8_lossy(&b[start..p]).chars() {
                map.entry(s).or_insert(g);
            }
            p += 1;
        }
    }
    let mut f = Font { w, h, per_row, atlas_w: aw, atlas_h: ah, pixels: px, map };
    f.synth("█▓▒░▀▄▌▐═║╣╠╬◘▲▼◄►■");
    Ok(f)
}
/// procedural box-drawing / block glyphs (Terminus v32b lacks them): coverage test per pixel
fn synth_px(c: char, x: u32, y: u32, w: u32, h: u32) -> bool {
    let (fx, fy) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
    let hl = |a: f32| (fy - a).abs() < 0.07;
    let vl = |a: f32| (fx - a).abs() < 0.12;
    let dither = |k: u32| (x + y * 3).is_multiple_of(k);
    match c {
        '█' => true,
        '▓' => !dither(4),
        '▒' => (x + y).is_multiple_of(2),
        '░' => dither(4),
        '▀' => fy < 0.5,
        '▄' => fy >= 0.5,
        '▌' => fx < 0.5,
        '▐' => fx >= 0.5,
        '═' => hl(0.42) || hl(0.58),
        '║' => vl(0.35) || vl(0.65),
        '╣' => vl(0.65) || (fx < 0.35 && (hl(0.42) || hl(0.58))) || (vl(0.35) && (fy < 0.42 || fy > 0.58)),
        '╠' => vl(0.35) || (fx > 0.65 && (hl(0.42) || hl(0.58))) || (vl(0.65) && (fy < 0.42 || fy > 0.58)),
        '╬' => ((vl(0.35) || vl(0.65)) && !(fy > 0.42 && fy < 0.58)) || ((hl(0.42) || hl(0.58)) && !(fx > 0.35 && fx < 0.65)),
        '◘' => (fx - 0.5).powi(2) + ((fy - 0.5) * 0.5).powi(2) >= 0.06,
        '▲' => fy > 0.3 && fy < 0.75 && (fx - 0.5).abs() < (fy - 0.3) * 1.0,
        '▼' => fy > 0.3 && fy < 0.75 && (fx - 0.5).abs() < (0.75 - fy) * 1.0,
        '◄' => fy > 0.25 && fy < 0.75 && fx > 0.15 && (fy - 0.5).abs() < (fx - 0.15) * 0.4,
        '►' => fy > 0.25 && fy < 0.75 && fx < 0.85 && (fy - 0.5).abs() < (0.85 - fx) * 0.4,
        '■' => fx > 0.15 && fx < 0.85 && fy > 0.3 && fy < 0.7,
        _ => false,
    }
}
impl Font {
    /// append procedural glyphs for chars the PSF lacks; grows the atlas by whole rows
    fn synth(&mut self, chars: &str) {
        let need: Vec<char> = chars.chars().filter(|c| !self.map.contains_key(c)).collect();
        let n0 = (self.atlas_w / self.w) * (self.atlas_h / self.h);
        let total = n0 + need.len() as u32;
        let ah = total.div_ceil(self.per_row) * self.h;
        self.pixels.resize((self.atlas_w * ah) as usize, 0);
        self.atlas_h = ah;
        for (k, &c) in need.iter().enumerate() {
            let g = n0 + k as u32;
            let (ox, oy) = ((g % self.per_row) * self.w, (g / self.per_row) * self.h);
            for y in 0..self.h {
                for x in 0..self.w {
                    if synth_px(c, x, y, self.w, self.h) {
                        self.pixels[((oy + y) * self.atlas_w + ox + x) as usize] = 255;
                    }
                }
            }
            self.map.insert(c, g);
        }
    }
}
impl Font {
    fn uv(&self, c: char) -> [f32; 4] {
        let g = self.map.get(&c).or(self.map.get(&'?')).copied().unwrap_or(0);
        let (x, y) = ((g % self.per_row) * self.w, (g / self.per_row) * self.h);
        let (aw, ah) = (self.atlas_w as f32, self.atlas_h as f32);
        [x as f32 / aw, y as f32 / ah, (x + self.w) as f32 / aw, (y + self.h) as f32 / ah]
    }
}

// ---------- world ----------
struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}
const ARENA: f32 = 120.0;
#[derive(Clone, Copy)]
struct Kind {
    hue: [f32; 3],
    speed: f32,
    shape: &'static [(f32, f32, f32, char, f32)], // (side, fwd, height m, glyph, brightness)
}
/// glyph-cloud bodies: (lateral, forward, height, glyph, brightness 0..1.5)
const GHOUL: &[(f32, f32, f32, char, f32)] = &[
    (0.0, 0.0, 1.55, 'Ö', 1.4),
    (0.0, 0.05, 1.25, '#', 1.0),
    (-0.18, 0.05, 1.2, '/', 0.8),
    (0.18, 0.05, 1.2, '\\', 0.8),
    (0.0, 0.0, 0.95, '¥', 0.9),
    (-0.1, 0.0, 0.55, '|', 0.6),
    (0.1, 0.0, 0.55, '|', 0.6),
    (-0.12, 0.0, 0.15, '^', 0.5),
    (0.12, 0.0, 0.15, '^', 0.5),
    (-0.3, 0.2, 0.9, '‡', 0.7),
    (0.3, 0.2, 0.9, '‡', 0.7),
];
const DRONE: &[(f32, f32, f32, char, f32)] = &[
    (0.0, 0.0, 1.3, '◘', 1.5),
    (-0.3, 0.0, 1.3, '◄', 0.8),
    (0.3, 0.0, 1.3, '►', 0.8),
    (0.0, 0.0, 1.6, '▲', 0.7),
    (0.0, 0.0, 1.0, '▼', 0.7),
    (0.0, 0.0, 0.6, '¦', 0.4),
    (0.0, 0.0, 0.3, '∙', 0.3),
];
const BRUTE: &[(f32, f32, f32, char, f32)] = &[
    (0.0, 0.1, 2.25, 'Θ', 1.4),
    (-0.35, 0.0, 1.95, '▓', 0.9),
    (0.35, 0.0, 1.95, '▓', 0.9),
    (0.0, 0.0, 1.75, '█', 1.0),
    (0.0, 0.0, 1.4, '█', 0.9),
    (-0.5, 0.2, 1.5, '╣', 0.8),
    (0.55, 0.35, 1.6, '═', 1.3),
    (0.85, 0.5, 1.6, '»', 1.5),
    (-0.2, 0.0, 0.9, '║', 0.7),
    (0.2, 0.0, 0.9, '║', 0.7),
    (-0.2, 0.0, 0.45, '║', 0.6),
    (0.2, 0.0, 0.45, '║', 0.6),
    (-0.25, 0.1, 0.1, '▀', 0.5),
    (0.25, 0.1, 0.1, '▀', 0.5),
];
const HERO: &[(f32, f32, f32, char, f32)] = &[
    (0.0, 0.05, 1.75, 'Ω', 1.2),
    (0.0, 0.15, 1.68, '═', 1.6),
    (-0.28, 0.0, 1.5, '▄', 1.0),
    (0.28, 0.0, 1.5, '▄', 1.0),
    (0.0, 0.0, 1.42, '▼', 1.1),
    (0.0, 0.12, 1.3, '♦', 1.6),
    (0.0, 0.0, 1.15, '█', 0.9),
    (-0.32, 0.05, 1.2, '▌', 0.8),
    (0.34, 0.15, 1.2, '▐', 0.8),
    (0.5, 0.45, 1.25, '─', 0.9),
    (0.75, 0.6, 1.25, '═', 1.0),
    (1.0, 0.75, 1.25, '*', 1.6),
    (0.0, 0.0, 0.98, '═', 1.0),
    (-0.12, 0.0, 0.7, '║', 0.8),
    (0.12, 0.0, 0.7, '║', 0.8),
    (-0.12, 0.0, 0.35, '║', 0.7),
    (0.12, 0.0, 0.35, '║', 0.7),
    (-0.14, 0.08, 0.08, '▀', 0.8),
    (0.14, 0.08, 0.08, '▀', 0.8),
    (-0.05, -0.2, 1.3, '▒', 0.5),
    (0.05, -0.25, 1.0, '▒', 0.45),
    (0.0, -0.28, 0.7, '░', 0.4),
];
const KINDS: [Kind; 3] = [
    Kind { hue: [0.71, 1.0, 0.23], speed: 3.0, shape: GHOUL },
    Kind { hue: [0.1, 0.95, 1.0], speed: 3.8, shape: DRONE },
    Kind { hue: [1.0, 0.17, 0.84], speed: 2.4, shape: BRUTE },
];
struct World {
    px: f32,
    py: f32,
    pface: f32,
    ex: Vec<f32>,
    ey: Vec<f32>,
    ek: Vec<u8>,
    ehit: Vec<f32>,
    pillars: Vec<(f32, f32)>,
    bolts: Vec<[f32; 5]>,
    sparks: Vec<[f32; 6]>,
    t: f32,
    fire: f32,
    rng: Rng,
    grid: Vec<Vec<u32>>,
}
const GC: f32 = 2.0;
const GN: usize = (ARENA / GC) as usize + 1;
impl World {
    fn new(n: usize) -> World {
        let mut w = World {
            px: ARENA / 2.0,
            py: ARENA / 2.0,
            pface: 0.0,
            ex: vec![],
            ey: vec![],
            ek: vec![],
            ehit: vec![],
            pillars: vec![],
            bolts: vec![],
            sparks: vec![],
            t: 0.0,
            fire: 0.0,
            rng: Rng(0x2545F4914F6CDD1D),
            grid: vec![vec![]; GN * GN],
        };
        for y in (8..ARENA as i32 - 4).step_by(12) {
            for x in (8..ARENA as i32 - 4).step_by(12) {
                w.pillars.push((x as f32 + 0.5, y as f32 + 0.5));
            }
        }
        w.resize(n);
        w
    }
    fn resize(&mut self, n: usize) {
        while self.ex.len() < n {
            let a = self.rng.f() * std::f32::consts::TAU;
            let r = 15.0 + self.rng.f() * 45.0;
            self.ex.push((self.px + a.cos() * r).clamp(1.0, ARENA - 1.0));
            self.ey.push((self.py + a.sin() * r).clamp(1.0, ARENA - 1.0));
            self.ek.push((self.rng.f() * 3.0) as u8 % 3);
            self.ehit.push(0.0);
        }
        self.ex.truncate(n);
        self.ey.truncate(n);
        self.ek.truncate(n);
        self.ehit.truncate(n);
    }
    fn step(&mut self, dt: f32, mv: (f32, f32)) {
        self.t += dt;
        let l = (mv.0 * mv.0 + mv.1 * mv.1).sqrt();
        if l > 0.0 {
            self.px = (self.px + mv.0 / l * 5.0 * dt).clamp(1.0, ARENA - 1.0);
            self.py = (self.py + mv.1 / l * 5.0 * dt).clamp(1.0, ARENA - 1.0);
            self.pface = mv.1.atan2(mv.0);
        }
        for c in self.grid.iter_mut() {
            c.clear();
        }
        let cell = |x: f32, y: f32| ((y / GC) as usize).min(GN - 1) * GN + ((x / GC) as usize).min(GN - 1);
        for i in 0..self.ex.len() {
            let c = cell(self.ex[i], self.ey[i]);
            self.grid[c].push(i as u32);
        }
        // seek + separation (grid neighbours) + pillar push
        for i in 0..self.ex.len() {
            let (x, y) = (self.ex[i], self.ey[i]);
            let (dx, dy) = (self.px - x, self.py - y);
            let d = (dx * dx + dy * dy).sqrt().max(1e-3);
            let sp = KINDS[self.ek[i] as usize].speed;
            let (mut vx, mut vy) = if d > 0.8 { (dx / d * sp, dy / d * sp) } else { (0.0, 0.0) };
            let (gx, gy) = ((x / GC) as i32, (y / GC) as i32);
            for oy in -1..=1 {
                for ox in -1..=1 {
                    let (cx, cy) = (gx + ox, gy + oy);
                    if cx < 0 || cy < 0 || cx >= GN as i32 || cy >= GN as i32 {
                        continue;
                    }
                    for &j in &self.grid[cy as usize * GN + cx as usize] {
                        if j as usize == i {
                            continue;
                        }
                        let (ex, ey) = (x - self.ex[j as usize], y - self.ey[j as usize]);
                        let dd = ex * ex + ey * ey;
                        if dd < 0.49 && dd > 1e-6 {
                            let k = (0.7 - dd.sqrt()) * 6.0 / dd.sqrt();
                            vx += ex * k;
                            vy += ey * k;
                        }
                    }
                }
            }
            self.ex[i] = (x + vx * dt).clamp(0.5, ARENA - 0.5);
            self.ey[i] = (y + vy * dt).clamp(0.5, ARENA - 0.5);
            self.ehit[i] = (self.ehit[i] - dt).max(0.0);
        }
        // autofire: 5 bolts / s at nearest within 14 m
        self.fire -= dt;
        if self.fire <= 0.0 {
            self.fire = 0.2;
            let mut best = (196.0f32, usize::MAX);
            let (gx, gy) = ((self.px / GC) as i32, (self.py / GC) as i32);
            for oy in -7..=7 {
                for ox in -7..=7 {
                    let (cx, cy) = (gx + ox, gy + oy);
                    if cx < 0 || cy < 0 || cx >= GN as i32 || cy >= GN as i32 {
                        continue;
                    }
                    for &j in &self.grid[cy as usize * GN + cx as usize] {
                        let dd = (self.ex[j as usize] - self.px).powi(2) + (self.ey[j as usize] - self.py).powi(2);
                        if dd < best.0 {
                            best = (dd, j as usize);
                        }
                    }
                }
            }
            if best.1 != usize::MAX {
                let j = best.1;
                let a = (self.ey[j] - self.py).atan2(self.ex[j] - self.px);
                self.bolts.push([self.px, self.py, a.cos() * 22.0, a.sin() * 22.0, 0.7]);
            }
        }
        let mut sp = vec![];
        for b in &mut self.bolts {
            b[0] += b[2] * dt;
            b[1] += b[3] * dt;
            b[4] -= dt;
            let c = cell(b[0], b[1]);
            if let Some(&j) = self.grid[c].iter().find(|&&j| (self.ex[j as usize] - b[0]).powi(2) + (self.ey[j as usize] - b[1]).powi(2) < 0.36) {
                self.ehit[j as usize] = 0.15;
                b[4] = 0.0;
                sp.push((b[0], b[1]));
            }
        }
        self.bolts.retain(|b| b[4] > 0.0);
        for (x, y) in sp {
            for _ in 0..10 {
                let a = self.rng.f() * std::f32::consts::TAU;
                let s = 2.0 + self.rng.f() * 4.0;
                self.sparks.push([x, y, 1.0, a.cos() * s, a.sin() * s, 0.4 + self.rng.f() * 0.3]);
            }
        }
        for s in &mut self.sparks {
            s[0] += s[3] * dt;
            s[1] += s[4] * dt;
            s[2] = (s[2] + dt * 1.5 - s[5] * 0.5 * dt).max(0.0);
            s[5] -= dt;
        }
        self.sparks.retain(|s| s[5] > 0.0);
    }
}

// ---------- instances: 12 floats = rect px (x,y,w,h), uv (4), rgb + glow (4) ----------
struct Cam {
    cx: f32,
    cy: f32,
    th: f32,
    s: f32, // px per metre along screen x
    sw: f32,
    sh: f32,
}
impl Cam {
    fn p(&self, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        let (sn, cs) = self.th.sin_cos();
        let (dx, dy) = (x - self.cx, y - self.cy);
        let (u, v) = (cs * dx - sn * dy, sn * dx + cs * dy);
        let a = self.s * std::f32::consts::FRAC_1_SQRT_2;
        (self.sw / 2.0 + (u - v) * a, self.sh * 0.55 + (u + v) * a / 2.0 - z * a * 1.224_745, u + v)
    }
}
fn push(out: &mut Vec<f32>, (font, cam): (&Font, &Cam), (x, y, z): (f32, f32, f32), size_m: f32, c: char, rgb: [f32; 3], glow: f32) {
    let (sx, sy, _) = cam.p(x, y, z);
    let h = size_m * cam.s;
    let w = h * font.w as f32 / font.h as f32;
    if sx < -w || sy < -h || sx > cam.sw + w || sy > cam.sh + h {
        return;
    }
    let uv = font.uv(c);
    out.extend_from_slice(&[sx - w / 2.0, sy - h, w, h, uv[0], uv[1], uv[2], uv[3], rgb[0], rgb[1], rgb[2], glow]);
}
fn build(w: &World, font: &Font, cam: &Cam, out: &mut Vec<f32>) {
    out.clear();
    let view = cam.sw / cam.s * 0.8;
    let (x0, x1) = ((w.px - view).max(0.0) as i32, (w.px + view).min(ARENA) as i32);
    let (y0, y1) = ((w.py - view).max(0.0) as i32, (w.py + view).min(ARENA) as i32);
    // floor glyph field: one dim glyph per m2, hash-picked, fades with distance (Effulgence bg <= 35% luma)
    let field = ['·', '∙', ':', '÷', '¨', '°', '·', ' '];
    for ty in y0..y1 {
        for tx in x0..x1 {
            let h = (tx as u32).wrapping_mul(73856093) ^ (ty as u32).wrapping_mul(19349663);
            let c = field[(h % 8) as usize];
            if c == ' ' {
                continue;
            }
            let d = ((tx as f32 - w.px).powi(2) + (ty as f32 - w.py).powi(2)).sqrt();
            let k = (0.35 - d * 0.012).max(0.06);
            push(out, (font, cam), (tx as f32 + 0.5, ty as f32 + 0.5, 0.0), 0.5, c, [0.48 * k, 0.17 * k, 1.0 * k], 0.0);
        }
    }
    // depth-sorted drawables: pillars, enemies, hero, bolts, sparks
    let mut ds: Vec<(f32, u8, u32)> = Vec::with_capacity(w.ex.len() + 64);
    for (i, p) in w.pillars.iter().enumerate() {
        if (p.0 - w.px).abs() < view && (p.1 - w.py).abs() < view {
            ds.push((cam.p(p.0, p.1, 0.0).2, 0, i as u32));
        }
    }
    for i in 0..w.ex.len() {
        if (w.ex[i] - w.px).abs() < view && (w.ey[i] - w.py).abs() < view {
            ds.push((cam.p(w.ex[i], w.ey[i], 0.0).2, 1, i as u32));
        }
    }
    ds.push((cam.p(w.px, w.py, 0.0).2, 2, 0));
    ds.sort_by(|a, b| a.0.total_cmp(&b.0));
    let body = |out: &mut Vec<f32>, x: f32, y: f32, face: f32, shape: &[(f32, f32, f32, char, f32)], hue: [f32; 3], flash: bool, bob: f32| {
        let (fs, fc) = face.sin_cos();
        // shadow
        push(out, (font, cam), (x, y, 0.0), 0.5, '▄', [0.02, 0.0, 0.05], 0.0);
        for &(side, fwd, z, c, br) in shape {
            let (ox, oy) = (fc * fwd - fs * side, fs * fwd + fc * side);
            let rgb = if flash { [1.0, 1.0, 1.0] } else { [hue[0] * br.min(1.0) + 0.15, hue[1] * br.min(1.0) + 0.12, hue[2] * br.min(1.0) + 0.2] };
            push(out, (font, cam), (x + ox, y + oy, z + bob), 0.34, c, rgb, (br - 1.0).max(0.0) * 2.5);
        }
    };
    for &(_, kind, i) in &ds {
        let i = i as usize;
        match kind {
            0 => {
                let (x, y) = w.pillars[i];
                for k in 0..10 {
                    let c = if k % 3 == 2 { '╬' } else { '▓' };
                    let glow = if k % 3 == 2 { 1.2 } else { 0.0 };
                    let rgb = if k % 3 == 2 { [0.1, 0.95, 1.0] } else { [0.35, 0.29, 0.54] };
                    push(out, (font, cam), (x, y, k as f32 * 0.3), 0.4, c, rgb, glow);
                }
            }
            1 => {
                let k = KINDS[w.ek[i] as usize];
                let face = (w.py - w.ey[i]).atan2(w.px - w.ex[i]);
                let bob = (w.t * 8.0 + i as f32).sin().abs() * 0.04;
                body(out, w.ex[i], w.ey[i], face, k.shape, k.hue, w.ehit[i] > 0.0, bob);
            }
            _ => body(out, w.px, w.py, w.pface, HERO, [0.85, 0.9, 1.0], false, (w.t * 9.0).sin().abs() * 0.03),
        }
    }
    for b in &w.bolts {
        for k in 0..5 {
            let t = k as f32 * 0.04;
            let g = 1.0 - k as f32 * 0.18;
            push(out, (font, cam), (b[0] - b[2] * t, b[1] - b[3] * t, 1.25), 0.3 * g, if k == 0 { '◆' } else { '∙' }, [0.1 * g, 0.95 * g, 1.0 * g], 2.0 * g);
        }
    }
    for s in &w.sparks {
        push(out, (font, cam), (s[0], s[1], s[2]), 0.18, '*', [1.0, 0.8, 0.3], 1.5 * s[5] / 0.7);
    }
}

// ---------- GPU ----------
const SHADER: &str = r#"
struct U { screen: vec2<f32>, bloom: f32, _pad: f32 };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) col: vec4<f32> };
@vertex fn vs_glyph(@builtin(vertex_index) vi: u32, @location(0) rect: vec4<f32>, @location(1) uvr: vec4<f32>, @location(2) col: vec4<f32>) -> VOut {
  var c = array<vec2<f32>, 6>(vec2(0.,0.), vec2(1.,0.), vec2(0.,1.), vec2(0.,1.), vec2(1.,0.), vec2(1.,1.));
  let k = c[vi];
  let p = rect.xy + k * rect.zw;
  var o: VOut;
  o.pos = vec4(p.x / u.screen.x * 2.0 - 1.0, 1.0 - p.y / u.screen.y * 2.0, 0.0, 1.0);
  o.uv = mix(uvr.xy, uvr.zw, k);
  o.col = col;
  return o;
}
@fragment fn fs_glyph(i: VOut) -> @location(0) vec4<f32> {
  let a = textureSample(atlas, samp, i.uv).r;
  let hdr = i.col.rgb * (1.0 + i.col.a);
  return vec4(hdr * a, a);
}
struct FOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_full(@builtin(vertex_index) vi: u32) -> FOut {
  let p = vec2(f32((vi << 1u) & 2u), f32(vi & 2u));
  var o: FOut;
  o.pos = vec4(p * 2.0 - 1.0, 0.0, 1.0);
  o.uv = vec2(p.x, 1.0 - p.y);
  return o;
}
@group(1) @binding(0) var src: texture_2d<f32>;
@group(1) @binding(1) var src2: texture_2d<f32>;
@fragment fn fs_bright(i: FOut) -> @location(0) vec4<f32> {
  let c = textureSample(src, samp, i.uv).rgb;
  let l = max(c.r, max(c.g, c.b));
  return vec4(c * smoothstep(0.7, 1.6, l), 1.0);
}
fn blur(i: FOut, d: vec2<f32>) -> vec4<f32> {
  let w = array<f32, 5>(0.227027, 0.1945946, 0.1216216, 0.054054, 0.016216);
  var acc = textureSample(src, samp, i.uv).rgb * w[0];
  for (var k = 1; k < 5; k++) {
    let o = d * f32(k) * 1.5;
    acc += textureSample(src, samp, i.uv + o).rgb * w[k];
    acc += textureSample(src, samp, i.uv - o).rgb * w[k];
  }
  return vec4(acc, 1.0);
}
@fragment fn fs_blur_h(i: FOut) -> @location(0) vec4<f32> { return blur(i, vec2(1.0 / (u.screen.x / 4.0), 0.0)); }
@fragment fn fs_blur_v(i: FOut) -> @location(0) vec4<f32> { return blur(i, vec2(0.0, 1.0 / (u.screen.y / 4.0))); }
@fragment fn fs_comp(i: FOut) -> @location(0) vec4<f32> {
  let base = textureSample(src, samp, i.uv).rgb + vec3(0.043, 0.024, 0.125);
  let glow = textureSample(src2, samp, i.uv).rgb * u.bloom;
  let c = base + glow;
  return vec4(c / (1.0 + c * 0.35), 1.0);
}
"#;
struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    glyph: wgpu::RenderPipeline,
    bright: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    comp: wgpu::RenderPipeline,
    g0: wgpu::BindGroup,
    g1_layout: wgpu::BindGroupLayout,
    ubuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    icap: u64,
    targets: Option<Targets>,
    present: &'static str,
}
struct Targets {
    hdr: wgpu::TextureView,
    a: wgpu::TextureView,
    b: wgpu::TextureView,
    g_hdr: wgpu::BindGroup,
    g_a: wgpu::BindGroup,
    g_b: wgpu::BindGroup,
    g_comp: wgpu::BindGroup,
}
const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
impl Gpu {
    fn new(window: Arc<Window>, font: &Font, uncapped: bool) -> Res<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, force_fallback_adapter: false, compatible_surface: Some(&surface), apply_limit_buckets: false })).map_err(|e| e.to_string())?;
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| e.to_string())?;
        let size = window.inner_size();
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).ok_or("no surface config")?;
        let caps = surface.get_capabilities(&adapter);
        let want = if uncapped { [wgpu::PresentMode::Immediate, wgpu::PresentMode::Mailbox, wgpu::PresentMode::Fifo] } else { [wgpu::PresentMode::Fifo; 3] };
        config.present_mode = want.into_iter().find(|m| caps.present_modes.contains(m)).unwrap_or(wgpu::PresentMode::Fifo);
        let present = match config.present_mode {
            wgpu::PresentMode::Immediate => "immediate",
            wgpu::PresentMode::Mailbox => "mailbox",
            _ => "fifo",
        };
        config.format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(config.format);
        surface.configure(&device, &config);
        // atlas texture (R8)
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d { width: font.atlas_w, height: font.atlas_h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &font.pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(font.atlas_w), rows_per_image: Some(font.atlas_h) },
            wgpu::Extent3d { width: font.atlas_w, height: font.atlas_h, depth_or_array_layers: 1 },
        );
        let atlas_view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let samp = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("u"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let vis = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: vis, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None };
        let g0_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("g0"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: vis, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                tex_entry(1),
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: vis, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let g1_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("g1"), entries: &[tex_entry(0), tex_entry(1)] });
        let g0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("g0"),
            layout: &g0_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&samp) },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let lay0 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&g0_layout)], ..Default::default() });
        let lay1 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&g0_layout), Some(&g1_layout)], ..Default::default() });
        let attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
        let premul = wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::OVER };
        let pipe = |lay: &wgpu::PipelineLayout, vs: &str, fs: &str, fmt: wgpu::TextureFormat, inst: bool, blend: Option<wgpu::BlendState>| {
            let buffers = [Some(wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Instance, attributes: &attrs })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(lay),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), buffers: if inst { &buffers } else { &[] }, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(fs), targets: &[Some(wgpu::ColorTargetState { format: fmt, blend, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let glyph = pipe(&lay0, "vs_glyph", "fs_glyph", HDR, true, Some(premul));
        let bright = pipe(&lay1, "vs_full", "fs_bright", HDR, false, None);
        let blur_h = pipe(&lay1, "vs_full", "fs_blur_h", HDR, false, None);
        let blur_v = pipe(&lay1, "vs_full", "fs_blur_v", HDR, false, None);
        let comp = pipe(&lay1, "vs_full", "fs_comp", config.format, false, None);
        let icap = 1 << 22;
        let ibuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("inst"), size: icap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut g = Gpu { window, surface, device, queue, config, glyph, bright, blur_h, blur_v, comp, g0, g1_layout, ubuf, ibuf, icap, targets: None, present };
        g.make_targets();
        Ok(g)
    }
    fn make_targets(&mut self) {
        let (w, h) = (self.config.width.max(1), self.config.height.max(1));
        let mk = |w: u32, h: u32| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: HDR,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let (hdr, a, b) = (mk(w, h), mk(w / 4, h / 4), mk(w / 4, h / 4));
        let bg = |x: &wgpu::TextureView, y: &wgpu::TextureView| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.g1_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(x) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(y) }],
            })
        };
        let (g_hdr, g_a, g_b, g_comp) = (bg(&hdr, &hdr), bg(&a, &a), bg(&b, &b), bg(&hdr, &a));
        self.targets = Some(Targets { hdr, a, b, g_hdr, g_a, g_b, g_comp });
    }
    fn resize(&mut self, w: u32, h: u32) {
        self.config.width = w.max(1);
        self.config.height = h.max(1);
        self.surface.configure(&self.device, &self.config);
        self.make_targets();
    }
    fn draw(&mut self, inst: &[f32]) -> Res<()> {
        let bytes: Vec<u8> = inst.iter().flat_map(|f| f.to_le_bytes()).collect();
        if bytes.len() as u64 > self.icap {
            self.icap = (bytes.len() as u64).next_power_of_two();
            self.ibuf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("inst"), size: self.icap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        }
        self.queue.write_buffer(&self.ibuf, 0, &bytes);
        let ub: Vec<u8> = [self.config.width as f32, self.config.height as f32, 1.1, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        self.queue.write_buffer(&self.ubuf, 0, &ub);
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            _ => return Ok(()),
        };
        let out = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let t = self.targets.as_ref().ok_or("no targets")?;
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let pass = |enc: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, pipe: &wgpu::RenderPipeline, g1: Option<&wgpu::BindGroup>, n_inst: u32| {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            rp.set_pipeline(pipe);
            rp.set_bind_group(0, &self.g0, &[]);
            match g1 {
                Some(g) => {
                    rp.set_bind_group(1, g, &[]);
                    rp.draw(0..3, 0..1);
                }
                None => {
                    rp.set_vertex_buffer(0, self.ibuf.slice(..));
                    rp.draw(0..6, 0..n_inst);
                }
            }
        };
        pass(&mut enc, &t.hdr, &self.glyph, None, (inst.len() / 12) as u32);
        pass(&mut enc, &t.a, &self.bright, Some(&t.g_hdr), 0);
        pass(&mut enc, &t.b, &self.blur_h, Some(&t.g_a), 0);
        pass(&mut enc, &t.a, &self.blur_v, Some(&t.g_b), 0);
        pass(&mut enc, &out, &self.comp, Some(&t.g_comp), 0);
        self.queue.submit([enc.finish()]);
        self.queue.present(frame);
        Ok(())
    }
}

// ---------- app ----------
struct App {
    font: Font,
    gpu: Option<Gpu>,
    world: World,
    held: [bool; 4],
    th: f32,
    th_target: f32,
    zoom: f32,
    inst: Vec<f32>,
    last: Instant,
    bench: Option<Bench>,
    stat: (u32, f64, f64, Instant),
    log: String,
    err: Option<String>,
}
struct Bench {
    sizes: Vec<usize>,
    k: usize,
    t0: Instant,
    frames: u32,
    cpu: f64,
    lines: Vec<String>,
}
impl App {
    fn frame(&mut self) -> Res<()> {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.05);
        self.last = now;
        let dirs = [(-1.0f32, -1.0f32), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)];
        let (mut su, mut sv) = (0.0f32, 0.0f32);
        let scripted = self.bench.is_some();
        let held = if scripted { [true, false, false, (self.world.t * 0.6).sin() > 0.0] } else { self.held };
        for k in 0..4 {
            if held[k] {
                su += dirs[k].0;
                sv += dirs[k].1;
            }
        }
        let (sn, cs) = self.th.sin_cos();
        let mv = (cs * su + sn * sv, -sn * su + cs * sv);
        let t0 = Instant::now();
        self.world.step(dt, mv);
        self.th += (self.th_target - self.th) * (1.0 - (-dt * 12.0).exp());
        let Some(gpu) = self.gpu.as_mut() else { return Ok(()) };
        let size = gpu.window.inner_size();
        // smooth camera: follow is exact (float), no cell snapping exists here
        let cam = Cam { cx: self.world.px, cy: self.world.py, th: self.th, s: self.zoom * size.height as f32 / 40.0, sw: size.width as f32, sh: size.height as f32 };
        build(&self.world, &self.font, &cam, &mut self.inst);
        let cpu = t0.elapsed().as_secs_f64() * 1000.0;
        gpu.draw(&self.inst)?;
        self.stat.0 += 1;
        self.stat.1 += cpu;
        self.stat.2 += t0.elapsed().as_secs_f64() * 1000.0;
        if self.stat.3.elapsed() >= Duration::from_secs(1) {
            let n = self.stat.0.max(1) as f64;
            self.log = format!(
                "{}x{} {} | enemies {} glyphs {} | {} fps | cpu sim+build {:.2} ms | cpu frame {:.2} ms",
                size.width,
                size.height,
                gpu.present,
                self.world.ex.len(),
                self.inst.len() / 12,
                self.stat.0,
                self.stat.1 / n,
                self.stat.2 / n
            );
            gpu.window.set_title(&format!("ToT gpu probe | {}", self.log));
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("gpu-perf.log") {
                use std::io::Write;
                let _ = writeln!(f, "{:.0}s | {}", self.world.t, self.log);
            }
            self.stat = (0, 0.0, 0.0, Instant::now());
        }
        if let Some(b) = self.bench.as_mut() {
            b.frames += 1;
            b.cpu += cpu;
            if b.t0.elapsed() >= Duration::from_secs(5) {
                let secs = b.t0.elapsed().as_secs_f64();
                b.lines.push(format!(
                    "{{\"enemies\":{},\"glyphs\":{},\"present\":\"{}\",\"px\":[{},{}],\"fps\":{:.1},\"cpu_sim_build_ms\":{:.2}}}",
                    self.world.ex.len(),
                    self.inst.len() / 12,
                    gpu.present,
                    size.width,
                    size.height,
                    b.frames as f64 / secs,
                    b.cpu / b.frames.max(1) as f64
                ));
                b.k += 1;
                b.frames = 0;
                b.cpu = 0.0;
                b.t0 = Instant::now();
                match b.sizes.get(b.k) {
                    Some(&n) => self.world.resize(n),
                    None => {
                        let text = b.lines.join("\n") + "\n";
                        std::fs::write("gpu-bench.jsonl", text).map_err(|e| e.to_string())?;
                        return Err("bench done".into());
                    }
                }
            }
        }
        Ok(())
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("ToT gpu probe").with_fullscreen(Some(Fullscreen::Borderless(None)));
        let r = el.create_window(attrs).map_err(|e| e.to_string()).and_then(|w| Gpu::new(Arc::new(w), &self.font, self.bench.is_some()));
        match r {
            Ok(g) => {
                g.window.request_redraw();
                self.gpu = Some(g);
            }
            Err(e) => {
                self.err = Some(e);
                el.exit();
            }
        }
    }
    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => {
                if let Some(g) = self.gpu.as_mut() {
                    g.resize(s.width, s.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let down = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(k) = event.physical_key {
                    let dir = match k {
                        KeyCode::KeyW | KeyCode::ArrowUp => Some(0),
                        KeyCode::KeyA | KeyCode::ArrowLeft => Some(1),
                        KeyCode::KeyS | KeyCode::ArrowDown => Some(2),
                        KeyCode::KeyD | KeyCode::ArrowRight => Some(3),
                        _ => None,
                    };
                    if let Some(d) = dir {
                        self.held[d] = down;
                    } else if down && !event.repeat {
                        match k {
                            KeyCode::KeyX | KeyCode::Escape => el.exit(),
                            KeyCode::KeyQ => self.th_target -= std::f32::consts::FRAC_PI_2,
                            KeyCode::KeyE => self.th_target += std::f32::consts::FRAC_PI_2,
                            KeyCode::Equal | KeyCode::NumpadAdd => self.zoom = (self.zoom * 1.2).min(6.0),
                            KeyCode::Minus | KeyCode::NumpadSubtract => self.zoom = (self.zoom / 1.2).max(0.3),
                            KeyCode::Digit1 => self.world.resize(300),
                            KeyCode::Digit2 => self.world.resize(1000),
                            KeyCode::Digit3 => self.world.resize(3000),
                            KeyCode::Digit4 => self.world.resize(10000),
                            KeyCode::Digit5 => self.world.resize(30000),
                            _ => {}
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.frame() {
                    if e != "bench done" {
                        self.err = Some(e);
                    }
                    el.exit();
                    return;
                }
                if let Some(g) = self.gpu.as_ref() {
                    g.window.request_redraw();
                }
            }
            _ => {}
        }
    }
}
fn main() {
    let r = (|| -> Res<()> {
        let font = load_font(include_bytes!("../assets/ter-v32b.psf"))?;
        let used: String = [GHOUL, DRONE, BRUTE, HERO].iter().flat_map(|s| s.iter().map(|q| q.3)).chain("·∙:÷¨°▄╬▓◆*".chars()).collect();
        let missing: String = used.chars().filter(|c| !font.map.contains_key(c)).collect();
        if !missing.is_empty() {
            eprintln!("glyphs missing in font: {missing}");
        }
        let bench = std::env::var("GPU_BENCH").is_ok().then(|| Bench { sizes: vec![1000, 5000, 20000, 50000], k: 0, t0: Instant::now(), frames: 0, cpu: 0.0, lines: vec![] });
        let n = bench.as_ref().map_or(1000, |b| b.sizes[0]);
        let _ = std::fs::write("gpu-perf.log", "");
        let mut app = App { font, gpu: None, world: World::new(n), held: [false; 4], th: 0.0, th_target: 0.0, zoom: 1.0, inst: Vec::with_capacity(1 << 20), last: Instant::now(), bench, stat: (0, 0.0, 0.0, Instant::now()), log: String::new(), err: None };
        let el = EventLoop::new().map_err(|e| e.to_string())?;
        el.set_control_flow(ControlFlow::Poll);
        el.run_app(&mut app).map_err(|e| e.to_string())?;
        match app.err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    })();
    if let Err(e) = r {
        eprintln!("gpu: {e}");
        std::process::exit(1);
    }
}
