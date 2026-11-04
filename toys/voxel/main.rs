//! T1 toy (design v5 sec 22.6, 22.7, 22.11): true 2:1 iso projection, voxel bodies + voxel walls in
//! terminal cells, horde bench, oracles, live view. std only. Floats OK: render-only toy.
//! modes: oracle | bench | still [--out DIR] | view   flags: --cols --rows --view-m --n --aspect --mode half|glyph
use std::collections::HashSet;
use std::f64::consts::FRAC_1_SQRT_2;
use std::fs::File;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// ablation knobs (env, probe only): TOT_CAM_STATIC=1 TOT_GRID=0 TOT_FOG=0 TOT_BODIES=0 TOT_Q=<step> TOT_DIRECT=1 (old per-frame splat path)
struct Knobs {
    cam_static: bool,
    grid: bool,
    fog: bool,
    bodies: bool,
    q: u8,
    direct: bool,
    body_div: usize,
    prebake: bool,
}
fn kn() -> &'static Knobs {
    static K: OnceLock<Knobs> = OnceLock::new();
    K.get_or_init(|| {
        let e = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        Knobs { cam_static: e("TOT_CAM_STATIC", "0") == "1", grid: e("TOT_GRID", "1") == "1", fog: e("TOT_FOG", "1") == "1", bodies: e("TOT_BODIES", "1") == "1", q: e("TOT_Q", "1").parse().unwrap_or(1).max(1), direct: e("TOT_DIRECT", "0") == "1", body_div: e("TOT_BODY_DIV", "1").parse().unwrap_or(1).max(1), prebake: e("TOT_PREBAKE", "0") == "1" }
    })
}

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
    fn mix(self, o: Rgb, t: f64) -> Rgb {
        let m = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).clamp(0.0, 255.0) as u8;
        Rgb(m(self.0, o.0), m(self.1, o.1), m(self.2, o.2))
    }
    fn luma(self) -> f64 {
        (0.2126 * self.0 as f64 + 0.7152 * self.1 as f64 + 0.0722 * self.2 as f64) / 255.0
    }
}
const VOID: Rgb = Rgb::hex(0x0b0620);
const FLOOR: Rgb = Rgb::hex(0x1a1234);
const GRID: Rgb = Rgb::hex(0x3a2a6a);
const WALL: Rgb = Rgb::hex(0x5a4a8a);
const TRIM: Rgb = Rgb::hex(0x19f3ff);

// ---------- projection: orthographic, camera yaw th + elevation 30 deg => pixel y = pixel x / 2 ----------
const CE: f64 = 0.866_025_403_784_438_6; // cos 30
const SE: f64 = 0.5; // sin 30
const ZK: f64 = 1.224_744_871_391_589; // sqrt2 * cos30: vertical px per metre, in units of A
#[derive(Clone, Copy)]
struct Proj {
    th: f64,
    kc: f64,     // cols per unit of (u - v)
    aspect: f64, // cell pixel h / w
    cx: f64,
    cy: f64,
    ox: f64,
    oy: f64,    // screen origin: col, subrow (2 subrows per cell row)
    flat: bool, // design v4 sec 5.1 formula (top-down rotated 45 deg), for the NEG control
}
const FACES: [(f64, f64, f64); 5] = [(1., 0., 0.), (-1., 0., 0.), (0., 1., 0.), (0., -1., 0.), (0., 0., 1.)];
const FACE_L: [f64; 3] = [1.0, 0.62, 0.36]; // top, screen-left, screen-right
impl Proj {
    fn rot(&self, x: f64, y: f64) -> (f64, f64) {
        let (s, c) = self.th.sin_cos();
        let (dx, dy) = (x - self.cx, y - self.cy);
        (c * dx - s * dy, s * dx + c * dy)
    }
    /// (col, subrow, near); larger near = closer to the camera
    fn p(&self, x: f64, y: f64, z: f64) -> (f64, f64, f64) {
        let (u, v) = self.rot(x, y);
        let py = if self.flat { (u + v) * self.kc * self.aspect / 2.0 } else { (u + v) * self.kc / 2.0 - z * self.kc * ZK };
        let near = if self.flat { z } else { (u + v) * CE * FRAC_1_SQRT_2 + z * SE };
        (self.ox + (u - v) * self.kc, self.oy + 2.0 * py / self.aspect, near)
    }
    /// subpixel -> ground point (z = 0)
    fn ground(&self, col: f64, sub: f64) -> (f64, f64) {
        let a = (col - self.ox) / self.kc;
        let b = (sub - self.oy) * self.aspect / self.kc;
        let (u, v) = ((a + b) / 2.0, (b - a) / 2.0);
        let (s, c) = self.th.sin_cos();
        (c * u + s * v + self.cx, -s * u + c * v + self.cy)
    }
    /// None = back face; Some(0 top | 1 screen-left | 2 screen-right)
    fn face(&self, f: usize) -> Option<usize> {
        let (nx, ny, nz) = FACES[f];
        if nz > 0.0 {
            return Some(0);
        }
        if self.flat {
            return None;
        }
        let (s, c) = self.th.sin_cos();
        let (nu, nv) = (c * nx - s * ny, s * nx + c * ny);
        if (nu + nv) * CE * FRAC_1_SQRT_2 <= 1e-6 {
            return None;
        }
        Some(if nu - nv < 0.0 { 1 } else { 2 })
    }
}

// ---------- voxel models (0.25 m voxels, forward = +x) ----------
const VS: f64 = 0.25;
#[derive(Clone, Copy)]
enum Mat {
    Dark,
    Plate,
    Chrome,
    Neon,
}
struct Vox {
    o: [f64; 3],
    mask: u8,
    mat: Mat,
}
struct Model {
    vox: Vec<Vox>,
    core: [f64; 3],
    glyph: char,
    hue: Rgb,
}
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Player,
    Stalker,
    Drone,
    Mech,
    Boss,
}
const KINDS: [Kind; 5] = [Kind::Player, Kind::Stalker, Kind::Drone, Kind::Mech, Kind::Boss];
type V = (i32, i32, i32, Mat);
fn bx(v: &mut Vec<V>, x: (i32, i32), y: (i32, i32), z: (i32, i32), m: Mat) {
    for i in x.0..=x.1 {
        for j in y.0..=y.1 {
            for k in z.0..=z.1 {
                v.push((i, j, k, m));
            }
        }
    }
}
fn shell(v: &mut Vec<V>, r: i32, z: (i32, i32), m: Mat) {
    for i in -r..=r {
        for j in -r..=r {
            for k in z.0..=z.1 {
                if i.abs() == r || j.abs() == r || k == z.1 {
                    v.push((i, j, k, m));
                }
            }
        }
    }
}
/// returns (voxels, core voxel, glyph, hue)
fn build(kind: Kind, frame: i32) -> (Vec<V>, (i32, i32, i32), char, Rgb) {
    let mut v = Vec::new();
    let st = if frame == 1 { 1 } else { 0 };
    match kind {
        Kind::Player | Kind::Stalker => {
            bx(&mut v, (st, st), (-1, -1), (0, 2), Mat::Dark); // left leg
            bx(&mut v, (-st, -st), (1, 1), (0, 2), Mat::Dark); // right leg
            bx(&mut v, (-1, 0), (-1, 1), (3, 5), Mat::Plate); // torso
            bx(&mut v, (0, 0), (-2, -2), (3, 5), Mat::Plate); // left arm
            bx(&mut v, (0, 0), (2, 2), (4, 5), Mat::Plate); // right arm
            bx(&mut v, (1, 2), (2, 2), (4, 4), Mat::Dark); // gun
            bx(&mut v, (-1, 0), (0, 0), (6, 6), Mat::Chrome); // head
            v.push((1, 0, 6, Mat::Neon)); // visor = core
            let (g, h) = if kind == Kind::Player { ('@', Rgb::hex(0xffffff)) } else { ('s', Rgb::hex(0xb6ff3b)) };
            (v, (1, 0, 6), g, h)
        }
        Kind::Drone => {
            bx(&mut v, (-1, 1), (-1, 1), (4, 4), Mat::Plate);
            bx(&mut v, (-2, 2), (0, 0), (4, 4), Mat::Dark);
            bx(&mut v, (0, 0), (-2, 2), (4, 4), Mat::Dark);
            v.push((0, 0, 3 - frame, Mat::Neon)); // thruster bob
            v.push((2, 0, 5, Mat::Neon)); // eye
            (v, (2, 0, 5), 'o', Rgb::hex(0x19f3ff))
        }
        Kind::Mech => {
            bx(&mut v, (st - 1, st), (-2, -2), (0, 3), Mat::Dark);
            bx(&mut v, (-st - 1, -st), (2, 2), (0, 3), Mat::Dark);
            bx(&mut v, (-2, 2), (-3, 3), (4, 7), Mat::Plate);
            bx(&mut v, (0, 5), (-4, -4), (6, 6), Mat::Chrome); // cannon L
            bx(&mut v, (0, 5), (4, 4), (6, 6), Mat::Chrome); // cannon R
            bx(&mut v, (3, 3), (-1, 1), (7, 7), Mat::Neon);
            (v, (3, 0, 7), 'M', Rgb::hex(0xff2bd6))
        }
        Kind::Boss => {
            shell(&mut v, 6, (0, 1), Mat::Dark);
            shell(&mut v, 4, (2, 12), Mat::Plate);
            shell(&mut v, 2, (13, 16), Mat::Chrome);
            bx(&mut v, (5, 5), (-1, 1), (9, 10), Mat::Neon);
            (v, (5, 0, 10), 'W', Rgb::hex(0x7a2cff))
        }
    }
}
fn model(kind: Kind, yaw: i32, frame: i32) -> Model {
    let (raw, core, glyph, hue) = build(kind, frame);
    let r = |x: i32, y: i32| (0..yaw).fold((x, y), |(a, b), _| (-b, a));
    let rv: Vec<V> = raw.iter().map(|&(x, y, z, m)| (r(x, y).0, r(x, y).1, z, m)).collect();
    let occ: HashSet<(i32, i32, i32)> = rv.iter().map(|&(x, y, z, _)| (x, y, z)).collect();
    let nb = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1)];
    let vox = rv
        .iter()
        .filter_map(|&(x, y, z, mat)| {
            let mask = (0..5).fold(0u8, |m, f| {
                let (a, b, c) = nb[f];
                if occ.contains(&(x + a, y + b, z + c)) { m } else { m | 1 << f }
            });
            (mask != 0).then_some(Vox { o: [x as f64 * VS - VS / 2.0, y as f64 * VS - VS / 2.0, z as f64 * VS], mask, mat })
        })
        .collect();
    let (cx, cy) = r(core.0, core.1);
    Model { vox, core: [cx as f64 * VS, cy as f64 * VS, core.2 as f64 * VS + VS / 2.0], glyph, hue }
}
fn mat_rgb(m: Mat, hue: Rgb) -> Rgb {
    match m {
        Mat::Dark => Rgb::hex(0x2e2840),
        Mat::Plate => hue.mix(Rgb::hex(0x6a6a7e), 0.55),
        Mat::Chrome => Rgb::hex(0xc8d0e8),
        Mat::Neon => hue,
    }
}

// ---------- canvas ----------
#[derive(Clone, Copy)]
struct Px {
    near: f32,
    rgb: Rgb,
    face: u8, // 0..2 face kind, 3 floor, 4 void
    ent: u16, // body id + 1; 0 = none
}
const EMPTY: Px = Px { near: f32::MIN, rgb: VOID, face: 4, ent: 0 };
#[derive(Clone, Copy, PartialEq)]
struct Cell {
    ch: char,
    fg: Rgb,
    bg: Rgb,
}
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Half,
    Glyph,
}
struct Canvas {
    w: usize,
    h: usize, // subrows
    scene: Vec<Px>,
    body: Vec<Px>,
    hues: Vec<Rgb>,
    cores: Vec<(usize, usize, char, Rgb, u16)>, // col, subrow, glyph, hue, ent
    samples: u64,
    vis: u64,
    ph: [u64; 3], // us: floor, walls, bodies
}
impl Canvas {
    fn new(cols: usize, rows: usize) -> Canvas {
        Canvas { w: cols, h: rows * 2, scene: vec![EMPTY; cols * rows * 2], body: vec![EMPTY; cols * rows * 2], hues: vec![], cores: vec![], samples: 0, vis: 0, ph: [0; 3] }
    }
}
fn fog(p: &Proj, x: f64, y: f64) -> f64 {
    if !kn().fog {
        return 1.0;
    }
    let d = ((x - p.cx).powi(2) + (y - p.cy).powi(2)).sqrt();
    1.0 - 0.65 * ((d - 8.0) / 22.0).clamp(0.0, 1.0)
}
struct Bx {
    lo: [f64; 3],
    hi: [f64; 3],
    mask: u8,
    rgb: Rgb,
    ent: u16,
}
fn splat(p: &Proj, buf: &mut [Px], w: usize, h: usize, bx: Bx, samples: &mut u64) {
    let f0 = fog(p, (bx.lo[0] + bx.hi[0]) / 2.0, (bx.lo[1] + bx.hi[1]) / 2.0);
    let ent = bx.ent;
    faces(p, &bx, f0, |c, s, n, col, k| {
        *samples += 1;
        if c < 0.0 || s < 0.0 || c >= w as f64 || s >= h as f64 {
            return;
        }
        let px = &mut buf[s as usize * w + c as usize];
        if (n as f32) > px.near {
            *px = Px { near: n as f32, rgb: col, face: k, ent };
        }
    });
}
/// visits every visible face sample of a box: put(col, subrow, near, rgb, face kind)
fn faces(p: &Proj, bx: &Bx, f0: f64, mut put: impl FnMut(f64, f64, f64, Rgb, u8)) {
    let Bx { lo, hi, mask, rgb, .. } = *bx;
    let dens = 2.2 * p.kc;
    for f in 0..5 {
        if mask & (1 << f) == 0 {
            continue;
        }
        let Some(k) = p.face(f) else { continue };
        let (fix, fv, i, j) = match f {
            0 => (0, hi[0], 1, 2),
            1 => (0, lo[0], 1, 2),
            2 => (1, hi[1], 0, 2),
            3 => (1, lo[1], 0, 2),
            _ => (2, hi[2], 0, 1),
        };
        let mi = ((hi[i] - lo[i]) * dens).ceil().max(1.0) as usize;
        let mj = ((hi[j] - lo[j]) * dens).ceil().max(1.0) as usize;
        let col = rgb.scale(FACE_L[k] * f0);
        for a in 0..mi {
            for b in 0..mj {
                let mut q = [0.0; 3];
                q[fix] = fv;
                q[i] = lo[i] + (a as f64 + 0.5) / mi as f64 * (hi[i] - lo[i]);
                q[j] = lo[j] + (b as f64 + 0.5) / mj as f64 * (hi[j] - lo[j]);
                let (c, s, n) = p.p(q[0], q[1], q[2]);
                put(c, s, n, col, k as u8);
            }
        }
    }
}

// ---------- sprite path: bake each shape once per (shape, angle step); blit at integer cells ----------
const ANGLES: f64 = 64.0;
struct Sprite {
    px: Vec<(i32, i32, f32, Rgb, u8)>, // dcol, dsub, dnear, rgb, face
    core: Option<(i32, i32)>,
}
#[derive(Default)]
struct Cache {
    map: std::collections::HashMap<(u32, u32), Sprite>,
    baked: u64,
}
fn aq(th: f64) -> u32 {
    ((th / std::f64::consts::TAU * ANGLES).round() as i64).rem_euclid(ANGLES as i64) as u32
}
fn bake(p: &Proj, boxes: &[Bx], core: Option<[f64; 3]>) -> Sprite {
    let o = Proj { cx: 0.0, cy: 0.0, ox: 0.0, oy: 0.0, ..*p };
    let mut m: std::collections::HashMap<(i32, i32), (f32, Rgb, u8)> = std::collections::HashMap::new();
    for bx in boxes {
        faces(&o, bx, 1.0, |c, s, n, col, k| {
            let e = m.entry((c.floor() as i32, s.floor() as i32)).or_insert((f32::MIN, col, k));
            if n as f32 > e.0 {
                *e = (n as f32, col, k);
            }
        });
    }
    let px = m.into_iter().map(|((c, s), (n, col, k))| (c, s, n, col, k)).collect();
    let core = core.map(|q| {
        let (c, s, _) = o.p(q[0], q[1], q[2]);
        (c.floor() as i32, s.floor() as i32)
    });
    Sprite { px, core }
}
struct Blit {
    oc: i32,
    os: i32,
    on: f32,
    fog: f64,
    ent: u16,
    face: Option<u8>,
}
fn blit(buf: &mut [Px], w: usize, h: usize, sp: &Sprite, b: &Blit) {
    for &(dc, ds, dn, col, k) in &sp.px {
        let (c, s) = (b.oc + dc, b.os + ds);
        if c < 0 || s < 0 || c as usize >= w || s as usize >= h {
            continue;
        }
        let px = &mut buf[s as usize * w + c as usize];
        let n = b.on + dn;
        if n > px.near {
            *px = Px { near: n, rgb: col.scale(b.fog), face: b.face.unwrap_or(k), ent: b.ent };
        }
    }
}
fn draw(w: &World, p: &Proj, cv: &mut Canvas, cut: bool, cache: &mut Cache) {
    if kn().direct { render(w, p, cv, cut) } else { render_sprites(w, p, cv, cut, cache) }
}
const HS: [f64; 4] = [0.3, 0.5, 2.0, 2.5];
/// every sprite id: 3000 floor tile, 2000 + hclass*16 + side mask = wall, 1000 + kind*8 + yaw*2 + frame = body
fn bake_id(w: &World, p: &Proj, id: u32) -> Sprite {
    let b = |lo: [f64; 3], hi: [f64; 3], rgb| Bx { lo, hi, mask: 1 << 4, rgb, ent: 0 };
    if id == 3000 {
        let mut v = vec![b([0.0, 0.0, -0.02], [1.0, 1.0, 0.0], FLOOR)];
        if kn().grid {
            v.push(b([0.0, 0.0, -0.01], [0.06, 1.0, 0.001], GRID));
            v.push(b([0.0, 0.0, -0.01], [1.0, 0.06, 0.001], GRID));
        }
        bake(p, &v, None)
    } else if id >= 2000 {
        let hc = ((id - 2000) / 16) as usize;
        let mask = ((id - 2000) % 16) as u8 | 1 << 4;
        let rgb = if HS[hc] < 1.0 { Rgb::hex(0x6a5a3a) } else if HS[hc] > 2.2 { WALL.mix(TRIM, 0.25) } else { WALL };
        bake(p, &[Bx { lo: [0.0; 3], hi: [1.0, 1.0, HS[hc]], mask, rgb, ent: 0 }], None)
    } else {
        let k = (id - 1000) as usize;
        let m = &w.models[k / 8][(k % 8) / 2][k % 2];
        let boxes: Vec<Bx> = m.vox.iter().map(|v| Bx { lo: v.o, hi: [v.o[0] + VS, v.o[1] + VS, v.o[2] + VS], mask: v.mask, rgb: mat_rgb(v.mat, m.hue), ent: 0 }).collect();
        bake(p, &boxes, Some(m.core))
    }
}
fn all_ids() -> Vec<u32> {
    let mut v = vec![3000];
    v.extend((0..4 * 16).map(|k| 2000 + k));
    v.extend((0..KINDS.len() as u32 * 8).map(|k| 1000 + k));
    v
}
impl Cache {
    fn ensure(&mut self, w: &World, p: &Proj, key: (u32, u32)) {
        if !self.map.contains_key(&key) {
            self.baked += 1;
            self.map.insert(key, bake_id(w, p, key.0));
        }
    }
    /// bake every id at all 64 angles (floor-load step); p gives kc + aspect
    fn prebake(&mut self, w: &World, p: &Proj) {
        for a in 0..ANGLES as u32 {
            let q = Proj { th: a as f64 / ANGLES * std::f64::consts::TAU, ..*p };
            for id in all_ids() {
                self.ensure(w, &q, (id, a));
            }
        }
    }
}
fn render_sprites(w: &World, p0: &Proj, cv: &mut Canvas, cutaway: bool, cache: &mut Cache) {
    let mut p = Proj { th: aq(p0.th) as f64 / ANGLES * std::f64::consts::TAU, ..*p0 };
    // snap world origin to an integer subpixel: static layer + bodies move in whole cells only
    let (c0, s0, _) = p.p(0.0, 0.0, 0.0);
    p.ox -= c0 - c0.floor();
    p.oy -= s0 - s0.floor();
    let a = aq(p.th);
    cv.scene.fill(EMPTY);
    cv.body.fill(EMPTY);
    cv.cores.clear();
    cv.hues.clear();
    cv.hues.resize(w.bodies.len(), VOID);
    cv.vis = 0;
    cv.samples = 0;
    let (cw, ch) = (cv.w, cv.h);
    let t0 = Instant::now();
    let band = |f: f64| (f * 16.0).round() / 16.0;
    let pl = &w.bodies[0];
    let (pu, pv) = p.rot(pl.x, pl.y);
    let eff = |x: i32, y: i32| {
        let h = w.map.h(x, y);
        if !cutaway || h == 0.0 {
            return h;
        }
        let (u, v) = p.rot(x as f64 + 0.5, y as f64 + 0.5);
        let ahead = (u + v) - (pu + pv);
        if ahead > 0.3 && ahead < 7.0 && ((u - v) - (pu - pv)).abs() < 3.5 { h.min(0.3) } else { h }
    };
    let rad = (cw as f64 / p.kc * 0.6) as i32 + 4;
    let (tx0, ty0) = (p.cx as i32, p.cy as i32);
    for ty in (ty0 - rad).max(0)..(ty0 + rad).min(w.map.w) {
        for tx in (tx0 - rad).max(0)..(tx0 + rad).min(w.map.w) {
            let h = eff(tx, ty);
            let (oc, os, on) = p.p(tx as f64, ty as f64, 0.0);
            if oc < -40.0 || os < -80.0 || oc > cw as f64 + 40.0 || os > ch as f64 + 80.0 {
                continue;
            }
            let fogf = band(fog(&p, tx as f64 + 0.5, ty as f64 + 0.5));
            let (id, face) = if h == 0.0 {
                (3000, Some(3))
            } else {
                let hc = HS.iter().position(|&x| (x - h).abs() < 1e-9).unwrap_or(2) as u32;
                let nb = [(1, 0), (-1, 0), (0, 1), (0, -1)];
                let mask = nb.iter().enumerate().fold(0u32, |m, (f, (dx, dy))| if eff(tx + dx, ty + dy) < h { m | 1 << f } else { m });
                (2000 + hc * 16 + mask, None)
            };
            let key = (id, a);
            cache.ensure(w, &p, key);
            if let Some(sp) = cache.map.get(&key) {
                blit(&mut cv.scene, cw, ch, sp, &Blit { oc: oc.round() as i32, os: os.round() as i32, on: on as f32, fog: fogf, ent: 0, face });
            }
        }
    }
    let t1 = Instant::now();
    // shadows: screen ellipse of a ground circle (exact for orthographic projection)
    for b in &w.bodies {
        let r = if b.kind == Kind::Boss { 1.8 } else if b.kind == Kind::Mech { 0.9 } else { 0.45 };
        let (c0, s0, _) = p.p(b.x, b.y, 0.0);
        let (ra, rb) = (r * std::f64::consts::SQRT_2 * p.kc, r * std::f64::consts::SQRT_2 * p.kc / p.aspect);
        for s in (s0 - rb).floor() as i64..=(s0 + rb).ceil() as i64 {
            for c in (c0 - ra).floor() as i64..=(c0 + ra).ceil() as i64 {
                if c < 0 || s < 0 || c >= cw as i64 || s >= ch as i64 {
                    continue;
                }
                let (dx, dy) = ((c as f64 + 0.5 - c0) / ra, (s as f64 + 0.5 - s0) / rb);
                let px = &mut cv.scene[s as usize * cw + c as usize];
                if px.face == 3 && dx * dx + dy * dy < 1.0 {
                    px.rgb = px.rgb.scale(0.45);
                }
            }
        }
    }
    let t2 = Instant::now();
    for (i, b) in w.bodies.iter().enumerate().filter(|_| kn().bodies) {
        let (oc, os, on) = p.p(b.x, b.y, 0.0);
        if oc < -40.0 || os < -80.0 || oc > cw as f64 + 40.0 || os > ch as f64 + 120.0 {
            continue;
        }
        let (yaw, fr) = (b.yaw as usize, (b.phase * 2.0) as usize % 2);
        let m = &w.models[b.kind as usize][yaw][fr];
        let key = (1000 + b.kind as u32 * 8 + yaw as u32 * 2 + fr as u32, a);
        cache.ensure(w, &p, key);
        let ent = (i + 1) as u16;
        cv.hues[i] = m.hue;
        cv.vis += 1;
        if let Some(sp) = cache.map.get(&key) {
            let (oc, os) = (oc.round() as i32, os.round() as i32);
            blit(&mut cv.body, cw, ch, sp, &Blit { oc, os, on: on as f32, fog: band(fog(&p, b.x, b.y)), ent, face: None });
            if let Some((dc, ds)) = sp.core {
                let (c, s) = (oc + dc, os + ds);
                if c >= 0 && s >= 0 && (c as usize) < cw && (s as usize) < ch {
                    cv.cores.push((c as usize, s as usize, m.glyph, m.hue, ent));
                }
            }
        }
    }
    cv.ph = [t1.duration_since(t0).as_micros() as u64, t2.duration_since(t1).as_micros() as u64, t2.elapsed().as_micros() as u64];
}

// ---------- world ----------
struct Map {
    w: i32,
    ht: Vec<f64>,
}
impl Map {
    fn h(&self, x: i32, y: i32) -> f64 {
        if x < 0 || y < 0 || x >= self.w || y >= self.w { 0.0 } else { self.ht[(y * self.w + x) as usize] }
    }
    fn solid(&self, x: f64, y: f64) -> bool {
        x < 1.0 || y < 1.0 || x >= (self.w - 1) as f64 || y >= (self.w - 1) as f64 || self.h(x as i32, y as i32) > 0.0
    }
}
fn map() -> Map {
    let w = 48;
    let mut ht = vec![0.0; (w * w) as usize];
    let mut set = |x: i32, y: i32, h: f64| ht[(y * w + x) as usize] = h;
    for i in 0..w {
        for (x, y) in [(i, 0), (i, w - 1), (0, i), (w - 1, i)] {
            set(x, y, 2.0);
        }
        if !(10..14).contains(&i) && !(30..34).contains(&i) {
            set(16, i, 2.0);
            set(32, i, 2.0);
        }
        if !(5..9).contains(&i) && !(22..27).contains(&i) && !(39..43).contains(&i) {
            set(i, 24, 2.0);
        }
    }
    for (x, y) in [(7, 7), (40, 7), (7, 40), (40, 40), (24, 36), (24, 11)] {
        for (a, b) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            set(x + a, y + b, 2.5);
        }
    }
    for (x, y) in [(20, 6), (21, 6), (28, 40), (28, 41), (10, 30), (36, 18)] {
        set(x, y, 0.5);
    }
    Map { w, ht }
}
struct Body {
    x: f64,
    y: f64,
    kind: Kind,
    yaw: i32,
    phase: f64,
}
struct World {
    map: Map,
    bodies: Vec<Body>, // [0] = player
    models: Vec<[[Model; 2]; 4]>,
    rng: u64,
}
impl World {
    fn new(n: usize, seed: u64) -> World {
        let mut w = World { map: map(), bodies: vec![], models: vec![], rng: seed | 1 };
        w.models = KINDS.iter().map(|&k| [0, 1, 2, 3].map(|y| [model(k, y, 0), model(k, y, 1)])).collect();
        w.bodies.push(Body { x: 24.5, y: 18.5, kind: Kind::Player, yaw: 0, phase: 0.0 });
        w.bodies.push(Body { x: 26.0, y: 30.0, kind: Kind::Boss, yaw: 2, phase: 0.0 });
        for i in 0..n {
            let kind = match i % 10 {
                0 => Kind::Mech,
                1..=3 => Kind::Drone,
                _ => Kind::Stalker,
            };
            let (x, y) = loop {
                let (x, y) = (1.5 + w.rnd() * 45.0, 1.5 + w.rnd() * 45.0);
                if !w.map.solid(x, y) {
                    break (x, y);
                }
            };
            let phase = w.rnd();
            w.bodies.push(Body { x, y, kind, yaw: 0, phase });
        }
        w
    }
    fn rnd(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }
    fn step(&mut self, dt: f64) {
        let (px, py) = (self.bodies[0].x, self.bodies[0].y);
        for i in 2..self.bodies.len() {
            let jitter = (self.rnd() - 0.5) * 0.8;
            let b = &mut self.bodies[i];
            let sp = if b.kind == Kind::Drone { 3.6 } else { 2.8 };
            let (dx, dy) = (px - b.x, py - b.y);
            let d = (dx * dx + dy * dy).sqrt().max(0.01);
            if d < 1.2 {
                continue;
            }
            let (vx, vy) = (dx / d * sp + jitter, dy / d * sp - jitter);
            let (nx, ny) = (b.x + vx * dt, b.y + vy * dt);
            if !self.map.solid(nx, b.y) {
                b.x = nx;
            }
            if !self.map.solid(b.x, ny) {
                b.y = ny;
            }
            b.yaw = if vx.abs() > vy.abs() { if vx > 0.0 { 0 } else { 2 } } else if vy > 0.0 { 1 } else { 3 };
            b.phase = (b.phase + dt * 2.0).fract();
        }
    }
    fn move_player(&mut self, dx: f64, dy: f64) {
        let b = &mut self.bodies[0];
        let (nx, ny) = (b.x + dx, b.y + dy);
        if !self.map.solid(nx, b.y) {
            b.x = nx;
        }
        if !self.map.solid(b.x, ny) {
            b.y = ny;
        }
        if dx.abs() + dy.abs() > 0.0 {
            b.yaw = if dx.abs() > dy.abs() { if dx > 0.0 { 0 } else { 2 } } else if dy > 0.0 { 1 } else { 3 };
            b.phase = (b.phase + 0.25).fract();
        }
    }
}

// ---------- render ----------
fn render(w: &World, p: &Proj, cv: &mut Canvas, cutaway: bool) {
    cv.scene.fill(EMPTY);
    cv.body.fill(EMPTY);
    cv.cores.clear();
    cv.hues.clear();
    cv.hues.resize(w.bodies.len(), VOID);
    cv.vis = 0;
    cv.samples = 0;
    let t0 = Instant::now();
    let (cw, ch) = (cv.w, cv.h);
    // floor
    for s in 0..ch {
        for c in 0..cw {
            let (x, y) = p.ground(c as f64 + 0.5, s as f64 + 0.5);
            if x < 0.0 || y < 0.0 || x >= w.map.w as f64 || y >= w.map.w as f64 {
                continue;
            }
            let line = kn().grid && (x.fract() < 0.06 || y.fract() < 0.06);
            let (u, v) = p.rot(x, y);
            cv.scene[s * cw + c] = Px { near: ((u + v) * CE * FRAC_1_SQRT_2) as f32, rgb: (if line { GRID } else { FLOOR }).scale(fog(p, x, y)), face: 3, ent: 0 };
        }
    }
    let t1 = Instant::now();
    // shadows
    for b in &w.bodies {
        let r = if b.kind == Kind::Boss { 1.8 } else if b.kind == Kind::Mech { 0.9 } else { 0.45 };
        let (c0, s0, _) = p.p(b.x, b.y, 0.0);
        let rc = (r * p.kc * 1.5) as i64 + 1;
        for s in (s0 as i64 - rc)..=(s0 as i64 + rc) {
            for c in (c0 as i64 - rc)..=(c0 as i64 + rc) {
                if c < 0 || s < 0 || c >= cw as i64 || s >= ch as i64 {
                    continue;
                }
                let (x, y) = p.ground(c as f64 + 0.5, s as f64 + 0.5);
                let px = &mut cv.scene[s as usize * cw + c as usize];
                if px.face == 3 && (x - b.x).powi(2) + (y - b.y).powi(2) < r * r {
                    px.rgb = px.rgb.scale(0.45);
                }
            }
        }
    }
    // walls (cutaway: near-camera walls in front of the player shrink to a stub)
    let pl = &w.bodies[0];
    let (pu, pv) = p.rot(pl.x, pl.y);
    let eff = |x: i32, y: i32| {
        let h = w.map.h(x, y);
        if !cutaway || h == 0.0 {
            return h;
        }
        let (u, v) = p.rot(x as f64 + 0.5, y as f64 + 0.5);
        let ahead = (u + v) - (pu + pv);
        if ahead > 0.3 && ahead < 7.0 && ((u - v) - (pu - pv)).abs() < 3.5 { h.min(0.3) } else { h }
    };
    let rad = (cw as f64 / p.kc * 0.6) as i32 + 4;
    let (tx0, ty0) = (p.cx as i32, p.cy as i32);
    for ty in (ty0 - rad).max(0)..(ty0 + rad).min(w.map.w) {
        for tx in (tx0 - rad).max(0)..(tx0 + rad).min(w.map.w) {
            let h = eff(tx, ty);
            if h == 0.0 {
                continue;
            }
            let nb = [(1, 0), (-1, 0), (0, 1), (0, -1)];
            let mut mask = 1 << 4;
            for (f, (a, b)) in nb.iter().enumerate() {
                if eff(tx + a, ty + b) < h {
                    mask |= 1 << f;
                }
            }
            let rgb = if h < 1.0 { Rgb::hex(0x6a5a3a) } else if h > 2.2 { WALL.mix(TRIM, 0.25) } else { WALL };
            let lo = [tx as f64, ty as f64, 0.0];
            splat(p, &mut cv.scene, cw, ch, Bx { lo, hi: [lo[0] + 1.0, lo[1] + 1.0, h], mask, rgb, ent: 0 }, &mut cv.samples);
        }
    }
    let t2 = Instant::now();
    // bodies
    for (i, b) in w.bodies.iter().enumerate().filter(|_| kn().bodies) {
        let (c0, s0, _) = p.p(b.x, b.y, 0.0);
        if c0 < -40.0 || s0 < -80.0 || c0 > cw as f64 + 40.0 || s0 > ch as f64 + 120.0 {
            continue;
        }
        let m = &w.models[b.kind as usize][b.yaw as usize][(b.phase * 2.0) as usize % 2];
        let ent = (i + 1) as u16;
        cv.hues[i] = m.hue;
        cv.vis += 1;
        for v in &m.vox {
            let lo = [b.x + v.o[0], b.y + v.o[1], v.o[2]];
            splat(p, &mut cv.body, cw, ch, Bx { lo, hi: [lo[0] + VS, lo[1] + VS, lo[2] + VS], mask: v.mask, rgb: mat_rgb(v.mat, m.hue), ent }, &mut cv.samples);
        }
        let (cc, cs, _) = p.p(b.x + m.core[0], b.y + m.core[1], m.core[2]);
        if cc >= 0.0 && cs >= 0.0 && (cc as usize) < cw && (cs as usize) < ch {
            cv.cores.push((cc as usize, cs as usize, m.glyph, m.hue, ent));
        }
    }
    cv.ph = [t1.duration_since(t0).as_micros() as u64, t2.duration_since(t1).as_micros() as u64, t2.elapsed().as_micros() as u64];
}
fn tex(face: u8, mode: Mode) -> char {
    match (mode, face) {
        (Mode::Glyph, 0) => '▓',
        (Mode::Glyph, 1) => '▒',
        (Mode::Glyph, 2) => '░',
        _ => ' ',
    }
}
/// subpixel colour: body (x-ray dimmed when behind scene), else scene
fn sub(cv: &Canvas, i: usize, rim: bool) -> (Rgb, u8, u16) {
    let b = cv.body[i];
    if b.ent == 0 {
        let s = cv.scene[i];
        return (s.rgb, s.face, 0);
    }
    let mut c = b.rgb;
    if rim {
        let w = cv.w;
        let edge = [i.wrapping_sub(1), i + 1, i.wrapping_sub(w), i + w].iter().any(|&j| cv.body.get(j).is_none_or(|q| q.ent != b.ent));
        if edge {
            c = c.mix(cv.hues[b.ent as usize - 1], 0.55);
        }
    }
    if cv.scene[i].near > b.near + 0.05 {
        c = c.scale(0.5);
    }
    (c, b.face, b.ent)
}
fn compose(cv: &Canvas, mode: Mode, rim: bool, out: &mut [Cell]) {
    let w = cv.w;
    for r in 0..cv.h / 2 {
        for c in 0..w {
            let (t, tf, te) = sub(cv, 2 * r * w + c, rim);
            let (b, bf, be) = sub(cv, (2 * r + 1) * w + c, rim);
            out[r * w + c] = match mode {
                Mode::Half if t == b => Cell { ch: ' ', fg: t, bg: t },
                Mode::Half => Cell { ch: '▀', fg: t, bg: b },
                Mode::Glyph => {
                    let (col, f, _) = if te != 0 || be == 0 { (t, tf, te) } else { (b, bf, be) };
                    let ch = if f == 3 && col == GRID.scale(1.0) { '·' } else { tex(f, mode) };
                    Cell { ch, fg: col, bg: col.scale(0.35) }
                }
            };
        }
    }
    for &(c, s, g, hue, ent) in &cv.cores {
        if cv.body[s * w + c].ent == ent {
            let i = (s / 2) * w + c;
            out[i] = Cell { ch: g, fg: hue.mix(Rgb::hex(0xffffff), 0.3), bg: out[i].bg };
        }
    }
    let q = kn().q;
    if q > 1 {
        let f = |c: Rgb| Rgb(c.0 / q * q, c.1 / q * q, c.2 / q * q);
        for c in out.iter_mut() {
            (c.fg, c.bg) = (f(c.fg), f(c.bg));
        }
    }
}

// ---------- emit (diff, sync 2026, hand-formatted numbers) ----------
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
    for (k, x) in [c.0, c.1, c.2].into_iter().enumerate() {
        if k > 0 {
            o.push(b';');
        }
        num(o, x as usize);
    }
}
fn emit(prev: &[Cell], cur: &[Cell], w: usize, rows: usize, force: bool, o: &mut Vec<u8>) {
    o.extend_from_slice(b"\x1b[?2026h");
    let mut at: Option<usize> = None;
    let (mut lf, mut lb) = (None, None);
    for i in 0..w * rows {
        let c = cur[i];
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
        // half-block space shows bg only: skip fg
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

// ---------- args ----------
struct Opts {
    cols: usize,
    rows: usize,
    view_m: f64,
    n: usize,
    aspect: f64,
    mode: Mode,
    out: String,
}
fn opts(a: &[String]) -> Res<Opts> {
    let mut o = Opts { cols: 320, rows: 96, view_m: 40.0, n: 300, aspect: 2.0, mode: Mode::Half, out: "out".into() };
    let mut i = 0;
    while i < a.len() {
        let v = a.get(i + 1).ok_or(format!("missing value for {}", a[i]))?;
        let f = |s: &str| s.parse::<f64>().map_err(|e| format!("{}: {e}", a[i]));
        match a[i].as_str() {
            "--cols" => o.cols = f(v)? as usize,
            "--rows" => o.rows = f(v)? as usize,
            "--view-m" => o.view_m = f(v)?,
            "--n" => o.n = f(v)? as usize,
            "--aspect" => o.aspect = f(v)?,
            "--mode" => o.mode = if v == "glyph" { Mode::Glyph } else { Mode::Half },
            "--out" => o.out = v.clone(),
            x => return Err(format!("unknown flag {x}")),
        }
        i += 2;
    }
    Ok(o)
}
fn proj_for(o: &Opts, rows: usize, w: &World, th: f64) -> Proj {
    let b = &w.bodies[0];
    Proj { th, kc: o.cols as f64 / (std::f64::consts::SQRT_2 * o.view_m), aspect: o.aspect, cx: b.x, cy: b.y, ox: o.cols as f64 / 2.0, oy: rows as f64 * 2.0 * 0.58, flat: false }
}

// ---------- oracle ----------
fn oracle() -> Res<()> {
    let mut bad = 0;
    let mut check = |name: &str, ok: bool, want: bool, info: String| {
        let tag = if ok == want { "OK  " } else { "BAD " };
        if ok != want {
            bad += 1;
        }
        println!("{tag}{name}: {} (want {}) {info}", if ok { "PASS" } else { "FAIL" }, if want { "PASS" } else { "FAIL" });
    };
    // O-proj: ground square pixel bbox w:h
    for (flat, want) in [(false, true), (true, false)] {
        let p = Proj { th: 0.0, kc: 10.0, aspect: 2.0, cx: 0.0, cy: 0.0, ox: 0.0, oy: 0.0, flat };
        let pts: Vec<(f64, f64)> = [(0., 0.), (1., 0.), (1., 1.), (0., 1.)].iter().map(|&(x, y)| {
            let (c, s, _) = p.p(x, y, 0.0);
            (c, s / 2.0 * p.aspect)
        }).collect();
        let span = |f: fn(&(f64, f64)) -> f64| pts.iter().map(f).fold(f64::MIN, f64::max) - pts.iter().map(f).fold(f64::MAX, f64::min);
        let r = span(|q| q.0) / span(|q| q.1);
        check(&format!("O-proj flat={flat}"), (r - 2.0).abs() < 0.05, want, format!("w:h = {r:.3}"));
    }
    // O-body: each model alone, 4 headings; NEG = flat projection
    let o = Opts { cols: 120, rows: 60, view_m: 40.0 * 120.0 / 320.0, n: 0, aspect: 2.0, mode: Mode::Half, out: String::new() };
    for flat in [false, true] {
        for &k in &KINDS {
            let mut world = World::new(0, 1);
            world.bodies.truncate(1);
            world.bodies[0].kind = k;
            world.map.ht.fill(0.0);
            let mut masks = vec![];
            let mut fails = vec![];
            for hd in 0..4 {
                let mut p = proj_for(&o, o.rows, &world, hd as f64 * std::f64::consts::FRAC_PI_2);
                p.flat = flat;
                let mut cv = Canvas::new(o.cols, o.rows);
                render(&world, &p, &mut cv, false);
                let cells: Vec<usize> = (0..cv.body.len()).filter(|&i| cv.body[i].ent != 0).collect();
                let cols: HashSet<usize> = cells.iter().map(|i| i % cv.w).collect();
                let rows: HashSet<usize> = cells.iter().map(|i| i / cv.w / 2).collect();
                let faces: HashSet<u8> = cells.iter().map(|&i| cv.body[i].face).collect();
                let cores = cv.cores.iter().filter(|c| cv.body[c.1 * cv.w + c.0].ent == c.4).count();
                if cols.len() < 2 || rows.len() < 3 || faces.len() < 3 || cores != 1 {
                    fails.push(format!("h{hd} cols {} rows {} faces {} cores {cores}", cols.len(), rows.len(), faces.len()));
                }
                masks.push(cells.iter().map(|i| (i % cv.w) as i64 - (cv.w / 2) as i64 + (i / cv.w * 10000) as i64).collect::<HashSet<i64>>());
            }
            if masks[0] == masks[2] && k != Kind::Boss {
                fails.push("heading 0 == 2 silhouette".into());
            }
            check(&format!("O-body {:?} flat={flat}", k as u8), fails.is_empty(), !flat, fails.join("; "));
        }
    }
    if bad > 0 { Err(format!("{bad} oracle rows not as expected")) } else { Ok(()) }
}

// ---------- bench ----------
fn bench(o: &Opts) -> Res<()> {
    for (cols, rows) in [(120usize, 40usize), (200, 60), (o.cols, o.rows)] {
        let oo = Opts { cols, rows, view_m: o.view_m, n: o.n, aspect: o.aspect, mode: o.mode, out: String::new() };
        let mut world = World::new(o.n, 7);
        let mut cv = Canvas::new(cols, rows);
        let mut cache = Cache::default();
        if kn().prebake {
            let t = Instant::now();
            cache.prebake(&world, &proj_for(&oo, rows, &world, 0.0));
            let px: usize = cache.map.values().map(|s| s.px.len()).sum();
            eprintln!("prebake {cols}x{rows}: {} sprites, {} px (~{} KB at 20 B/px), {} ms", cache.map.len(), px, px * 20 / 1024, t.elapsed().as_millis());
            cache.baked = 0;
        }
        let (mut prev, mut cur) = (vec![Cell { ch: ' ', fg: VOID, bg: VOID }; cols * rows], vec![Cell { ch: ' ', fg: VOID, bg: VOID }; cols * rows]);
        let mut out = Vec::with_capacity(1 << 21);
        let (mut times, mut bytes, mut samples, mut vis) = (vec![], vec![], vec![], vec![]);
        let mut phs: [Vec<u64>; 5] = Default::default();
        let mut first = 0;
        for f in 0..360 {
            let a = f as f64 / 360.0 * std::f64::consts::TAU;
            if !kn().cam_static {
                world.move_player(a.cos() * 0.05, a.sin() * 0.05);
            }
            if f % kn().body_div == 0 {
                world.step(kn().body_div as f64 / 60.0);
            }
            let th = if (120..136).contains(&f) { (f - 120) as f64 / 15.0 * std::f64::consts::FRAC_PI_2 } else if f >= 136 { std::f64::consts::FRAC_PI_2 } else { 0.0 };
            let p = proj_for(&oo, rows, &world, th);
            let t = Instant::now();
            draw(&world, &p, &mut cv, true, &mut cache);
            let tc = Instant::now();
            compose(&cv, o.mode, true, &mut cur);
            let te = Instant::now();
            out.clear();
            emit(&prev, &cur, cols, rows, f == 0, &mut out);
            times.push(t.elapsed().as_micros() as u64);
            for (k, v) in cv.ph.iter().chain([te.duration_since(tc).as_micros() as u64, te.elapsed().as_micros() as u64].iter()).enumerate() {
                phs[k].push(*v);
            }
            if f == 0 {
                first = out.len();
            } else {
                bytes.push(out.len() as u64);
            }
            samples.push(cv.samples);
            vis.push(cv.vis);
            std::mem::swap(&mut prev, &mut cur);
        }
        let pc = |v: &mut Vec<u64>, q: f64| {
            v.sort_unstable();
            v[((v.len() - 1) as f64 * q) as usize]
        };
        println!(
            "{{\"cols\":{cols},\"rows\":{rows},\"n\":{},\"mode\":\"{}\",\"visible_bodies_p50\":{},\"frame_us_p50\":{},\"frame_us_p99\":{},\"bytes_full\":{first},\"bytes_p50\":{},\"bytes_p99\":{},\"bytes_max\":{},\"samples_p50\":{},\"us_p50_scene_shadow_bodies_compose_emit\":{:?},\"baked\":{}}}",
            o.n,
            if o.mode == Mode::Half { "half" } else { "glyph" },
            pc(&mut vis, 0.5),
            pc(&mut times, 0.5),
            pc(&mut times, 0.99),
            pc(&mut bytes, 0.5),
            pc(&mut bytes, 0.99),
            pc(&mut bytes, 1.0),
            pc(&mut samples, 0.5),
            phs.iter_mut().map(|v| pc(v, 0.5)).collect::<Vec<_>>(),
            cache.baked
        );
    }
    Ok(())
}

// ---------- still: .ans for vic (cat in Ghostty), .txt luma ramp for agents ----------
fn still(o: &Opts) -> Res<()> {
    std::fs::create_dir_all(&o.out).map_err(|e| e.to_string())?;
    let world = World::new(o.n, 7);
    let mut cache = Cache::default();
    for hd in 0..4 {
        let p = proj_for(o, o.rows, &world, hd as f64 * std::f64::consts::FRAC_PI_2);
        let mut cv = Canvas::new(o.cols, o.rows);
        draw(&world, &p, &mut cv, true, &mut cache);
        let mut cells = vec![Cell { ch: ' ', fg: VOID, bg: VOID }; o.cols * o.rows];
        compose(&cv, o.mode, true, &mut cells);
        let mut ans = b"\x1b[2J".to_vec();
        emit(&cells, &cells, o.cols, o.rows, true, &mut ans);
        ans.extend_from_slice(b"\x1b[0m\n");
        let ramp: Vec<char> = " .:-=+*#%@".chars().collect();
        let mut txt = String::new();
        for r in 0..o.rows {
            for c in 0..o.cols {
                let cell = cells[r * o.cols + c];
                let ch = if cell.ch.is_ascii_alphabetic() || cell.ch == '@' { cell.ch } else { ramp[((cell.fg.luma().max(cell.bg.luma())) * 9.0).round() as usize] };
                txt.push(ch);
            }
            txt.push('\n');
        }
        let base = format!("{}/still-h{hd}", o.out);
        std::fs::write(format!("{base}.ans"), &ans).map_err(|e| e.to_string())?;
        std::fs::write(format!("{base}.txt"), txt).map_err(|e| e.to_string())?;
    }
    println!("wrote {}/still-h0..3.{{ans,txt}}", o.out);
    Ok(())
}

// ---------- view: live, raw tty ----------
fn stty(args: &[&str]) -> Res<String> {
    let tty = File::open("/dev/tty").map_err(|e| e.to_string())?;
    let out = Command::new("stty").args(args).stdin(tty).output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
struct Raw(String);
impl Drop for Raw {
    fn drop(&mut self) {
        let _ = std::io::stdout().write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = std::io::stdout().flush();
        let _ = stty(&[self.0.as_str()]);
    }
}
fn view(mut o: Opts) -> Res<()> {
    let size = stty(&["size"])?;
    let mut it = size.split_whitespace().map(|s| s.parse::<usize>());
    let (Some(Ok(rows)), Some(Ok(cols))) = (it.next(), it.next()) else { return Err(format!("stty size: {size}")) };
    o.cols = cols;
    o.rows = rows;
    let _raw = Raw(stty(&["-g"])?);
    stty(&["raw", "-echo"])?;
    let mut so = std::io::stdout();
    so.write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[16t").map_err(|e| e.to_string())?;
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
    let wr = rows - 1;
    let mut world = World::new(o.n, 7);
    let mut cv = Canvas::new(cols, wr);
    let mut cache = Cache::default();
    let blank = Cell { ch: ' ', fg: VOID, bg: VOID };
    let (mut prev, mut cur) = (vec![blank; cols * rows], vec![blank; cols * rows]);
    let mut out = Vec::with_capacity(1 << 21);
    let (mut th, mut target, mut turn_from, mut turn_t) = (0.0f64, 0.0f64, 0.0f64, 1.0f64);
    let (mut rim, mut cut, mut pause, mut force) = (true, true, false, true);
    let mut esc: Vec<u8> = vec![];
    let mut stat = (0usize, 0u128, 0u32, Instant::now(), 0.0f64, 0usize, 0.0f64);
    let frame = Duration::from_micros(16_667);
    loop {
        let t0 = Instant::now();
        while let Ok(c) = rx.try_recv() {
            if !esc.is_empty() || c == 0x1b {
                esc.push(c);
                if esc.len() > 2 && (0x40..=0x7e).contains(&c) {
                    let s = String::from_utf8_lossy(&esc).to_string();
                    if let Some(p) = s.strip_prefix("\x1b[6;").and_then(|r| r.strip_suffix('t')) {
                        let v: Vec<f64> = p.split(';').filter_map(|x| x.parse().ok()).collect();
                        if v.len() == 2 && v[1] > 0.0 {
                            o.aspect = v[0] / v[1];
                        }
                    }
                    esc.clear();
                }
                continue;
            }
            let step = 0.35;
            let (s, c2) = th.sin_cos();
            // screen dirs -> world: up = -(u+v), right = (u-v)
            let mv = |su: f64, sv: f64| (c2 * su + s * sv, -s * su + c2 * sv);
            match c {
                b'x' | 3 => return Ok(()),
                b'q' | b'e' => {
                    turn_from = th;
                    target += if c == b'q' { -std::f64::consts::FRAC_PI_2 } else { std::f64::consts::FRAC_PI_2 };
                    turn_t = 0.0;
                }
                b'w' | b'a' | b's' | b'd' => {
                    let (su, sv) = match c {
                        b'w' => (-FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
                        b's' => (FRAC_1_SQRT_2, FRAC_1_SQRT_2),
                        b'a' => (-FRAC_1_SQRT_2, FRAC_1_SQRT_2),
                        _ => (FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
                    };
                    let (dx, dy) = mv(su * step, sv * step);
                    world.move_player(dx, dy);
                }
                b'm' => {
                    o.mode = if o.mode == Mode::Half { Mode::Glyph } else { Mode::Half };
                    force = true;
                }
                b'r' => rim = !rim,
                b'c' => cut = !cut,
                b'p' => pause = !pause,
                b'+' | b'=' => o.view_m = (o.view_m * 0.85).max(8.0),
                b'-' => o.view_m = (o.view_m / 0.85).min(120.0),
                _ => {}
            }
        }
        if !pause {
            world.step(1.0 / 60.0);
        }
        if turn_t < 1.0 {
            turn_t = (turn_t + 16.667 / 250.0).min(1.0);
            let e = turn_t * turn_t * (3.0 - 2.0 * turn_t);
            th = turn_from + (target - turn_from) * e;
        }
        let p = proj_for(&o, wr, &world, th);
        let tc = Instant::now();
        draw(&world, &p, &mut cv, cut, &mut cache);
        compose(&cv, o.mode, rim, &mut cur[..cols * wr]);
        let us = tc.elapsed().as_micros();
        let line = format!(
            " {} {cols}x{rows} view {:.0}m asp {:.2} bodies {} | {:.0} KB/f {:.2} ms compose {:.0} fps | q/e turn wasd move m mode r rim c cut p pause +/- zoom x quit",
            if o.mode == Mode::Half { "HALF" } else { "GLYPH" },
            o.view_m,
            o.aspect,
            cv.vis,
            stat.4,
            stat.6,
            stat.5 as f64
        );
        for (i, cell) in cur[cols * wr..].iter_mut().enumerate() {
            *cell = Cell { ch: line.chars().nth(i).unwrap_or(' '), fg: Rgb::hex(0xc0c0d8), bg: Rgb::hex(0x140c2c) };
        }
        out.clear();
        emit(&prev, &cur, cols, rows, force, &mut out);
        force = false;
        so.write_all(&out).map_err(|e| e.to_string())?;
        so.flush().map_err(|e| e.to_string())?;
        std::mem::swap(&mut prev, &mut cur);
        stat.0 += out.len();
        stat.1 += us;
        stat.2 += 1;
        if stat.3.elapsed() >= Duration::from_secs(1) {
            let n = stat.2.max(1) as f64;
            stat = (0, 0, 0, Instant::now(), stat.0 as f64 / n / 1024.0, stat.2 as usize, stat.1 as f64 / n / 1000.0);
        }
        if let Some(rest) = frame.checked_sub(t0.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let r = match (a.get(1).map(String::as_str), opts(a.get(2..).unwrap_or(&[]))) {
        (_, Err(e)) => Err(e),
        (Some("oracle"), _) => oracle(),
        (Some("bench"), Ok(o)) => bench(&o),
        (Some("still"), Ok(o)) => still(&o),
        (Some("view"), Ok(o)) => view(o),
        _ => Err("usage: voxel oracle | bench | still | view [--cols N --rows N --view-m M --n N --aspect A --mode half|glyph --out DIR]".into()),
    };
    if let Err(e) = r {
        eprintln!("voxel: {e}");
        std::process::exit(1);
    }
}
