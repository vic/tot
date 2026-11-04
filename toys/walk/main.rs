//! T6 toy "walk" (vic 2026-11-03: walk + map + enemies + all UI, check resolution, small bodies,
//! rich texture). Generated dungeon floor, fog + explored memory, map overlay, horde + autofire,
//! XP shards, 4 bars + 3-row strip + which-key. Voxel bodies + walls = pre-baked sprites (T1 law)
//! at sub-cell resolution: HALF 1x2 / QUAD 2x2 / SEXT 2x3 per cell.
//! std only; f64 toy sim. Run: ./walk [--seed N --view-m M --cap N]   headless check: ./walk shot
//! keys: wasd move · q/e turn · 1/2/3 cell mode · +/- zoom · space = leader (m map, r rim, c cut, p pause) · x quit
use std::collections::HashMap;
use std::f64::consts::{FRAC_1_SQRT_2, SQRT_2, TAU};
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
    fn mix(self, o: Rgb, t: f64) -> Rgb {
        let m = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).clamp(0.0, 255.0) as u8;
        Rgb(m(self.0, o.0), m(self.1, o.1), m(self.2, o.2))
    }
    fn d2(self, o: Rgb) -> i32 {
        let d = |a: u8, b: u8| a as i32 - b as i32;
        d(self.0, o.0).pow(2) + d(self.1, o.1).pow(2) + d(self.2, o.2).pow(2)
    }
}
const VOID: Rgb = Rgb::hex(0x0b0620);
const FLOOR: Rgb = Rgb::hex(0x1c1438);
const WALL: Rgb = Rgb::hex(0x5a4a8a);
const TRIM: Rgb = Rgb::hex(0x19f3ff);
const C_HP: Rgb = Rgb::hex(0xffffff);
const C_FX: Rgb = Rgb::hex(0x5b8cff);
const C_RG: Rgb = Rgb::hex(0xff8a1f);
const C_XP: Rgb = Rgb::hex(0xc0c0d8);
const C_UI: Rgb = Rgb::hex(0xc0c0d8);
const C_STRIP: Rgb = Rgb::hex(0x140c2c);

// ---------- cell modes ----------
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Mode {
    Half,
    Quad,
    Sext,
}
impl Mode {
    fn sub(self) -> (usize, usize) {
        match self {
            Mode::Half => (1, 2),
            Mode::Quad => (2, 2),
            Mode::Sext => (2, 3),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Mode::Half => "HALF 1x2",
            Mode::Quad => "QUAD 2x2",
            Mode::Sext => "SEXT 2x3",
        }
    }
    /// bits: subpixel index r*sx + c set = fg
    fn glyph(self, bits: u32) -> char {
        const QUAD: [char; 16] = [' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█'];
        match self {
            Mode::Half => [' ', '▀', '▄', '█'][bits as usize & 3],
            Mode::Quad => QUAD[bits as usize & 15],
            Mode::Sext => match bits & 63 {
                0 => ' ',
                21 => '▌',
                42 => '▐',
                63 => '█',
                n => char::from_u32(0x1FB00 + n - 1 - u32::from(n > 21) - u32::from(n > 42)).unwrap_or('?'),
            },
        }
    }
}

// ---------- projection: true 2:1 iso (T1 O-proj), output in subpixels ----------
const CE: f64 = 0.866_025_403_784_438_6;
const SE: f64 = 0.5;
const ZK: f64 = 1.224_744_871_391_589;
#[derive(Clone, Copy)]
struct Proj {
    th: f64,
    kc: f64,
    aspect: f64,
    cx: f64,
    cy: f64,
    ox: f64,
    oy: f64,
    sx: f64,
    sy: f64,
}
const FACES: [(f64, f64, f64); 5] = [(1., 0., 0.), (-1., 0., 0.), (0., 1., 0.), (0., -1., 0.), (0., 0., 1.)];
const FACE_L: [f64; 3] = [1.0, 0.62, 0.36];
impl Proj {
    fn rot(&self, x: f64, y: f64) -> (f64, f64) {
        let (s, c) = self.th.sin_cos();
        let (dx, dy) = (x - self.cx, y - self.cy);
        (c * dx - s * dy, s * dx + c * dy)
    }
    fn p(&self, x: f64, y: f64, z: f64) -> (f64, f64, f64) {
        let (u, v) = self.rot(x, y);
        let py = (u + v) * self.kc / 2.0 - z * self.kc * ZK;
        (self.ox + (u - v) * self.kc * self.sx, self.oy + py * self.sy / self.aspect, (u + v) * CE * FRAC_1_SQRT_2 + z * SE)
    }
    fn face(&self, f: usize) -> Option<usize> {
        let (nx, ny, nz) = FACES[f];
        if nz > 0.0 {
            return Some(0);
        }
        let (s, c) = self.th.sin_cos();
        let (nu, nv) = (c * nx - s * ny, s * nx + c * ny);
        if (nu + nv) <= 1e-6 {
            return None;
        }
        Some(if nu - nv < 0.0 { 1 } else { 2 })
    }
}

// ---------- voxel models: 0.125 m voxels (vic item 40: font 2 resolution buys detail) ----------
const VS: f64 = 0.125;
#[derive(Clone, Copy)]
enum Mat {
    Dark,
    Plate,
    Chrome,
    Neon,
}
struct Model {
    vox: Vec<([f64; 3], u8, Rgb)>,
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
    Behemoth,
}
const KINDS: [Kind; 5] = [Kind::Player, Kind::Stalker, Kind::Drone, Kind::Mech, Kind::Behemoth];
/// voxel set keyed by integer coords; later writes win (detail over base shape)
type Vset = HashMap<(i32, i32, i32), Mat>;
fn bx(v: &mut Vset, x: (i32, i32), y: (i32, i32), z: (i32, i32), m: Mat) {
    for i in x.0..=x.1 {
        for j in y.0..=y.1 {
            for k in z.0..=z.1 {
                v.insert((i, j, k), m);
            }
        }
    }
}
/// filled ellipsoid, centre + radii in voxel units
fn ell(v: &mut Vset, c: (f64, f64, f64), r: (f64, f64, f64), m: Mat) {
    let lo = |a: f64, b: f64| (a - b).floor() as i32;
    let hi = |a: f64, b: f64| (a + b).ceil() as i32;
    for i in lo(c.0, r.0)..=hi(c.0, r.0) {
        for j in lo(c.1, r.1)..=hi(c.1, r.1) {
            for k in lo(c.2, r.2)..=hi(c.2, r.2) {
                let d = ((i as f64 - c.0) / r.0).powi(2) + ((j as f64 - c.1) / r.1).powi(2) + ((k as f64 - c.2) / r.2).powi(2);
                if d <= 1.0 {
                    v.insert((i, j, k), m);
                }
            }
        }
    }
}
/// returns (voxels, core voxel, glyph, hue); frame 0/1 = walk cycle
fn build(kind: Kind, frame: i32) -> (Vset, (i32, i32, i32), char, Rgb) {
    let mut v = Vset::new();
    let st = if frame == 1 { 1 } else { -1 };
    match kind {
        Kind::Player => {
            bx(&mut v, (st, st + 1), (-2, -1), (1, 6), Mat::Dark);
            bx(&mut v, (-st, -st + 1), (1, 2), (1, 6), Mat::Dark);
            bx(&mut v, (st, st + 2), (-2, -1), (0, 0), Mat::Chrome);
            bx(&mut v, (-st, -st + 2), (1, 2), (0, 0), Mat::Chrome);
            bx(&mut v, (-1, 1), (-2, 2), (7, 7), Mat::Dark);
            ell(&mut v, (0.0, 0.0, 10.0), (1.6, 2.4, 2.6), Mat::Plate);
            bx(&mut v, (2, 2), (-1, 1), (10, 10), Mat::Neon);
            ell(&mut v, (0.0, 3.0, 12.0), (1.2, 1.2, 1.0), Mat::Chrome);
            ell(&mut v, (0.0, -3.0, 12.0), (1.2, 1.2, 1.0), Mat::Chrome);
            bx(&mut v, (0, 0), (-3, -3), (7, 11), Mat::Plate);
            bx(&mut v, (0, 0), (3, 3), (8, 11), Mat::Plate);
            bx(&mut v, (1, 4), (3, 3), (9, 9), Mat::Dark);
            v.insert((5, 3, 9), Mat::Neon);
            ell(&mut v, (0.0, 0.0, 14.5), (1.3, 1.3, 1.5), Mat::Chrome);
            bx(&mut v, (2, 2), (-1, 1), (14, 14), Mat::Neon);
            bx(&mut v, (-3, -2), (-1, 1), (9, 12), Mat::Dark);
            bx(&mut v, (-3, -3), (0, 0), (13, 14), Mat::Neon);
            (v, (2, 0, 14), '@', Rgb::hex(0xffffff))
        }
        Kind::Stalker => {
            bx(&mut v, (st, st), (-1, -1), (0, 4), Mat::Dark);
            bx(&mut v, (-st, -st), (1, 1), (0, 4), Mat::Dark);
            ell(&mut v, (0.5, 0.0, 7.5), (2.2, 1.6, 2.0), Mat::Plate);
            bx(&mut v, (-2, -2), (0, 0), (6, 10), Mat::Neon);
            ell(&mut v, (2.0, 0.0, 10.5), (1.1, 1.0, 1.0), Mat::Chrome);
            bx(&mut v, (1, 4), (-2, -2), (7, 7), Mat::Chrome);
            bx(&mut v, (1, 4), (2, 2), (7, 7), Mat::Chrome);
            v.insert((3, 0, 11), Mat::Neon);
            (v, (3, 0, 11), 's', Rgb::hex(0xb6ff3b))
        }
        Kind::Drone => {
            ell(&mut v, (0.0, 0.0, 9.0), (2.5, 2.5, 0.8), Mat::Plate);
            for k in 0..24 {
                let a = k as f64 / 24.0 * TAU + frame as f64 * 0.13;
                v.insert(((3.6 * a.cos()).round() as i32, (3.6 * a.sin()).round() as i32, 10), Mat::Dark);
            }
            bx(&mut v, (0, 0), (0, 0), (6 + frame, 7 + frame), Mat::Neon);
            v.insert((3, 0, 9), Mat::Neon);
            (v, (3, 0, 9), 'o', Rgb::hex(0x19f3ff))
        }
        Kind::Mech => {
            bx(&mut v, (st - 1, st + 1), (-4, -3), (0, 8), Mat::Dark);
            bx(&mut v, (-st - 1, -st + 1), (3, 4), (0, 8), Mat::Dark);
            ell(&mut v, (0.0, 0.0, 13.0), (3.0, 4.0, 3.2), Mat::Plate);
            bx(&mut v, (0, 7), (-5, -5), (13, 14), Mat::Chrome);
            bx(&mut v, (0, 7), (5, 5), (13, 14), Mat::Chrome);
            bx(&mut v, (8, 8), (-5, -5), (13, 14), Mat::Neon);
            bx(&mut v, (8, 8), (5, 5), (13, 14), Mat::Neon);
            for z in [11, 13, 15] {
                bx(&mut v, (-3, -3), (-2, 2), (z, z), Mat::Neon);
            }
            v.insert((3, 0, 16), Mat::Neon);
            (v, (3, 0, 16), 'M', Rgb::hex(0xff2bd6))
        }
        Kind::Behemoth => {
            for s in [-1.0, 1.0] {
                ell(&mut v, (st as f64 * s, 4.5 * s, 4.0), (1.8, 1.8, 4.2), Mat::Dark);
                ell(&mut v, (2.0, 7.5 * s, 11.0), (1.6, 1.6, 4.0), Mat::Plate);
                bx(&mut v, (3, 5), ((7.5 * s) as i32, (7.5 * s) as i32), (7, 7), Mat::Chrome);
            }
            ell(&mut v, (0.0, 0.0, 14.0), (5.0, 6.0, 6.0), Mat::Plate);
            ell(&mut v, (5.5, 0.0, 19.0), (2.6, 2.6, 2.2), Mat::Chrome);
            bx(&mut v, (7, 8), (-1, 1), (17, 17), Mat::Neon);
            for x in -4..=3 {
                v.insert((x, 0, 20 + (x + 4) % 2), Mat::Neon);
            }
            v.insert((8, 1, 20), Mat::Neon);
            v.insert((8, -1, 20), Mat::Neon);
            (v, (8, 1, 20), 'W', Rgb::hex(0x7a2cff))
        }
    }
}
/// per-voxel texture: hash noise +-12% luma, plate seams every 4 voxels
fn texel(m: Mat, hue: Rgb, x: i32, y: i32, z: i32) -> Rgb {
    let h = ((x as u32).wrapping_mul(73856093) ^ (y as u32).wrapping_mul(19349663) ^ (z as u32).wrapping_mul(83492791)) % 1000;
    let n = 0.88 + 0.24 * h as f64 / 1000.0;
    match m {
        Mat::Dark => Rgb::hex(0x2e2840).scale(n),
        Mat::Plate => hue.mix(Rgb::hex(0x6a6a7e), 0.55).scale(if z.rem_euclid(4) == 0 { 0.72 } else { n }),
        Mat::Chrome => Rgb::hex(0xc8d0e8).scale(n),
        Mat::Neon => hue,
    }
}
fn model(kind: Kind, yaw: i32, frame: i32) -> Model {
    let (raw, core, glyph, hue) = build(kind, frame);
    let r = |x: i32, y: i32| (0..yaw).fold((x, y), |(a, b), _| (-b, a));
    let rv: Vset = raw.iter().map(|(&(x, y, z), &m)| ((r(x, y).0, r(x, y).1, z), m)).collect();
    let nb = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1)];
    let vox = rv
        .iter()
        .filter_map(|(&(x, y, z), &mat)| {
            let mask = (0..5).fold(0u8, |m, f| {
                let (a, b, c) = nb[f];
                if rv.contains_key(&(x + a, y + b, z + c)) { m } else { m | 1 << f }
            });
            (mask != 0).then_some(([x as f64 * VS - VS / 2.0, y as f64 * VS - VS / 2.0, z as f64 * VS], mask, texel(mat, hue, x, y, z)))
        })
        .collect();
    let (cx, cy) = r(core.0, core.1);
    Model {
        vox,
        core: [cx as f64 * VS, cy as f64 * VS, core.2 as f64 * VS + VS / 2.0],
        glyph,
        hue,
    }
}

// ---------- sprites: bake once per (id, angle, mode), blit at integer subpixels ----------
const ANGLES: f64 = 64.0;
struct Bx {
    lo: [f64; 3],
    hi: [f64; 3],
    mask: u8,
    rgb: Rgb,
}
fn faces(p: &Proj, b: &Bx, mut put: impl FnMut(f64, f64, f64, Rgb, u8)) {
    let dens = 2.0 * p.kc * p.sx.max(p.sy);
    for f in 0..5 {
        if b.mask & (1 << f) == 0 {
            continue;
        }
        let Some(k) = p.face(f) else { continue };
        let (fix, fv, i, j) = match f {
            0 => (0, b.hi[0], 1, 2),
            1 => (0, b.lo[0], 1, 2),
            2 => (1, b.hi[1], 0, 2),
            3 => (1, b.lo[1], 0, 2),
            _ => (2, b.hi[2], 0, 1),
        };
        let mi = ((b.hi[i] - b.lo[i]) * dens).ceil().max(1.0) as usize;
        let mj = ((b.hi[j] - b.lo[j]) * dens).ceil().max(1.0) as usize;
        let col = b.rgb.scale(FACE_L[k]);
        for a in 0..mi {
            for c in 0..mj {
                let mut q = [0.0; 3];
                q[fix] = fv;
                q[i] = b.lo[i] + (a as f64 + 0.5) / mi as f64 * (b.hi[i] - b.lo[i]);
                q[j] = b.lo[j] + (c as f64 + 0.5) / mj as f64 * (b.hi[j] - b.lo[j]);
                let (x, y, n) = p.p(q[0], q[1], q[2]);
                put(x, y, n, col, k as u8);
            }
        }
    }
}
struct Sprite {
    px: Vec<(i32, i32, f32, Rgb, u8)>,
    core: Option<(i32, i32)>,
    rows: (i32, i32), // min, max dsub (band reject)
}
fn bake(p: &Proj, boxes: &[Bx], core: Option<[f64; 3]>) -> Sprite {
    let o = Proj {
        cx: 0.0,
        cy: 0.0,
        ox: 0.0,
        oy: 0.0,
        ..*p
    };
    let mut m: HashMap<(i32, i32), (f32, Rgb, u8)> = HashMap::new();
    for b in boxes {
        faces(&o, b, |c, s, n, col, k| {
            let e = m.entry((c.floor() as i32, s.floor() as i32)).or_insert((f32::MIN, col, k));
            if n as f32 > e.0 {
                *e = (n as f32, col, k);
            }
        });
    }
    let core = core.map(|q| {
        let (c, s, _) = o.p(q[0], q[1], q[2]);
        (c.floor() as i32, s.floor() as i32)
    });
    let px: Vec<(i32, i32, f32, Rgb, u8)> = m.into_iter().map(|((c, s), (n, col, k))| (c, s, n, col, k)).collect();
    let rows = px.iter().fold((i32::MAX, i32::MIN), |(a, b), q| (a.min(q.1), b.max(q.1)));
    Sprite { px, core, rows }
}
const WALL_H: [f64; 2] = [0.3, 2.0];
fn bake_id(models: &[[[Model; 2]; 4]], p: &Proj, id: u32) -> Sprite {
    let t = |lo, hi, rgb| Bx { lo, hi, mask: 1 << 4, rgb };
    match id {
        3000 => bake(p, &[t([0.0, 0.0, -0.02], [1.0, 1.0, 0.0], FLOOR)], None), // no grid lines (vic item 41)
        4000 => bake(
            p,
            &[Bx {
                lo: [-0.09, -0.09, 0.2],
                hi: [0.09, 0.09, 0.38],
                mask: 31,
                rgb: C_XP,
            }],
            None,
        ),
        4001 => bake(
            p,
            &[Bx {
                lo: [-0.1, -0.1, 0.9],
                hi: [0.1, 0.1, 1.1],
                mask: 31,
                rgb: Rgb::hex(0x7af9ff),
            }],
            None,
        ),
        2000..=2031 => {
            let (hc, mask) = (((id - 2000) / 16) as usize, ((id - 2000) % 16) as u8 | 1 << 4);
            let rgb = if hc == 0 { WALL.scale(0.8) } else { WALL };
            let mut v = vec![Bx {
                lo: [0.0; 3],
                hi: [1.0, 1.0, WALL_H[hc]],
                mask,
                rgb,
            }];
            if hc == 1 {
                v.push(Bx {
                    lo: [0.0, 0.0, 1.55],
                    hi: [1.0, 1.0, 1.62],
                    mask: mask & 15,
                    rgb: TRIM.scale(0.7),
                }); // neon trim line
            }
            bake(p, &v, None)
        }
        _ => {
            let k = (id - 1000) as usize;
            let m = &models[k / 8][(k % 8) / 2][k % 2];
            let boxes: Vec<Bx> = m
                .vox
                .iter()
                .map(|&(o, mask, rgb)| Bx {
                    lo: o,
                    hi: [o[0] + VS, o[1] + VS, o[2] + VS],
                    mask,
                    rgb,
                })
                .collect();
            bake(p, &boxes, Some(m.core))
        }
    }
}
fn aq(th: f64) -> u32 {
    ((th / TAU * ANGLES).round() as i64).rem_euclid(ANGLES as i64) as u32
}

// ---------- canvas + compose ----------
#[derive(Clone, Copy)]
struct Px {
    near: f32,
    rgb: Rgb,
    ent: u16,
}
const EMPTY: Px = Px { near: f32::MIN, rgb: VOID, ent: 0 };
#[derive(Clone, Copy, PartialEq)]
struct Cell {
    ch: char,
    fg: Rgb,
    bg: Rgb,
}
const BLANK: Cell = Cell { ch: ' ', fg: VOID, bg: VOID };
struct Canvas {
    w: usize,
    h: usize,
    scene: Vec<Px>,
    body: Vec<Px>,
    hues: Vec<Rgb>,
    cores: Vec<(usize, usize, char, Rgb, u16)>,
}
/// one blit into a band of subrows [s0, s0 + h) of a w-wide buffer
struct Item {
    key: (u32, u32, Mode),
    o: (i32, i32, f32),
    fog: f64,
    ent: u16,
}
fn blit(buf: &mut [Px], w: usize, s0: i32, h: usize, sp: &Sprite, it: &Item) {
    let (o, fog, ent) = (it.o, it.fog, it.ent);
    if o.1 + sp.rows.1 < s0 || o.1 + sp.rows.0 >= s0 + h as i32 {
        return;
    }
    for &(dc, ds, dn, col, _) in &sp.px {
        let (c, s) = (o.0 + dc, o.1 + ds - s0);
        if c < 0 || s < 0 || c as usize >= w || s as usize >= h {
            continue;
        }
        let px = &mut buf[s as usize * w + c as usize];
        let n = o.2 + dn;
        if n > px.near {
            *px = Px { near: n, rgb: col.scale(fog), ent };
        }
    }
}
fn compose(cv: &Canvas, mode: Mode, rim: bool, cols: usize, rows: usize, out: &mut [Cell]) {
    let (sx, sy) = mode.sub();
    let w = cv.w;
    let sub = |i: usize| -> Rgb {
        let b = cv.body[i];
        if b.ent == 0 {
            return cv.scene[i].rgb;
        }
        let mut c = b.rgb;
        if rim && [i.wrapping_sub(1), i + 1, i.wrapping_sub(w), i + w].iter().any(|&j| cv.body.get(j).is_none_or(|q| q.ent != b.ent)) {
            c = c.mix(cv.hues[b.ent as usize - 1], 0.55);
        }
        if cv.scene[i].near > b.near + 0.05 { c.scale(0.5) } else { c }
    };
    // pure per cell => rows split over threads (toy: scoped spawn per frame; product = persistent pool)
    let nt = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
    let chunk = rows.div_ceil(nt).max(1);
    std::thread::scope(|s| {
        for (ci, part) in out[..rows * cols].chunks_mut(chunk * cols).enumerate() {
            let sub = &sub;
            s.spawn(move || {
                let mut buf = [Rgb::default(); 6];
                for (rr, row) in part.chunks_mut(cols).enumerate() {
                    let r = ci * chunk + rr;
                    for (c, cell) in row.iter_mut().enumerate() {
                        *cell = cell_of(mode, sx, sy, &mut buf, |a, b| sub((r * sy + a) * w + c * sx + b));
                    }
                }
            });
        }
    });
    for &(c, s, g, hue, ent) in &cv.cores {
        if cv.body[s * w + c].ent == ent {
            let i = (s / sy) * cols + c / sx;
            if i < out.len() {
                out[i] = Cell {
                    ch: g,
                    fg: hue.mix(Rgb::hex(0xffffff), 0.3),
                    bg: out[i].bg,
                };
            }
        }
    }
}
/// one cell from its sx x sy subpixels: 2-colour split (farthest pair) -> block glyph
fn cell_of(mode: Mode, sx: usize, sy: usize, buf: &mut [Rgb; 6], sub: impl Fn(usize, usize) -> Rgb) -> Cell {
    let k = sx * sy;
    for a in 0..sy {
        for b in 0..sx {
            buf[a * sx + b] = sub(a, b);
        }
    }
    let px = &buf[..k];
    if px.iter().all(|q| *q == px[0]) {
        Cell { ch: ' ', fg: px[0], bg: px[0] }
    } else {
        let (mut ia, mut ib, mut best) = (0, 1, -1);
        for i in 0..k {
            for j in i + 1..k {
                let d = px[i].d2(px[j]);
                if d > best {
                    (ia, ib, best) = (i, j, d);
                }
            }
        }
        let (mut bits, mut fa, mut fb, mut na, mut nb) = (0u32, [0u32; 3], [0u32; 3], 0u32, 0u32);
        for (i, q) in px.iter().enumerate() {
            let acc = if q.d2(px[ia]) <= q.d2(px[ib]) {
                bits |= 1 << i;
                na += 1;
                &mut fa
            } else {
                nb += 1;
                &mut fb
            };
            acc[0] += q.0 as u32;
            acc[1] += q.1 as u32;
            acc[2] += q.2 as u32;
        }
        let avg = |a: [u32; 3], n: u32| Rgb((a[0] / n.max(1)) as u8, (a[1] / n.max(1)) as u8, (a[2] / n.max(1)) as u8);
        Cell {
            ch: mode.glyph(bits),
            fg: avg(fa, na),
            bg: avg(fb, nb),
        }
    }
}

// ---------- floor gen (toys/gen rules) ----------
const G: usize = 6;
const CW: usize = 32;
const W: usize = G * CW;
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
fn carve(t: &mut [u8], x0: usize, y0: usize, x1: usize, y1: usize) {
    for y in y0..y1 {
        for x in x0..x1 {
            t[y * W + x] = 1;
        }
    }
}
fn floorgen(seed: u64) -> (Vec<u8>, usize) {
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
    let n = 10 + m.below(5) as usize;
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
    for _ in 0..2 {
        if cand.is_empty() {
            break;
        }
        let i = m.below(cand.len() as u32) as usize;
        e.push(cand.swap_remove(i));
    }
    let mut t = vec![0u8; W * W];
    for &c in &cells {
        let (ox, oy) = ((c % G) * CW, (c / G) * CW);
        let k = m.below(3);
        let (w, h) = (16 + m.below(13) as usize, 16 + m.below(13) as usize);
        let (x0, y0) = (ox + (CW - w) / 2, oy + (CW - h) / 2);
        carve(&mut t, x0, y0, x0 + w, y0 + h);
        if k == 1 {
            for y in (y0 + 3..y0 + h - 3).step_by(5) {
                for x in (x0 + 3..x0 + w - 3).step_by(5) {
                    t[y * W + x] = 0;
                }
            }
        }
    }
    for &(a, b) in &e {
        let (ax, ay) = ((a % G) * CW + CW / 2, (a / G) * CW + CW / 2);
        let (bx, by) = ((b % G) * CW + CW / 2, (b / G) * CW + CW / 2);
        carve(&mut t, ax.min(bx) - 2, ay.min(by) - 2, ax.max(bx) + 2, ay.max(by) + 2);
    }
    (t, ((start % G) * CW + CW / 2) + ((start / G) * CW + CW / 2) * W)
}

// ---------- game ----------
struct En {
    x: f64,
    y: f64,
    kind: Kind,
    yaw: i32,
    phase: f64,
    hp: f64,
    cd: f64,
}
struct Game {
    t: Vec<u8>,
    wall: Vec<bool>,
    vis: Vec<bool>,
    explored: Vec<bool>,
    walk_total: usize,
    walk_seen: usize,
    dist: Vec<u16>,
    ring: Vec<u32>,
    occ: Vec<u8>,
    tile_at: usize,
    px: f64,
    py: f64,
    pyaw: i32,
    pphase: f64,
    en: Vec<En>,
    shards: Vec<(f64, f64, u32)>,
    bolts: Vec<[f64; 5]>,
    hp: f64,
    flux: f64,
    rage: f64,
    xp: u32,
    level: u32,
    kills: u32,
    time: f64,
    fire_cd: f64,
    spawn_acc: f64,
    cap: usize,
    rng: X,
    msg: (String, f64),
    models: Vec<[[Model; 2]; 4]>,
}
const PMAX: f64 = 320.0;
fn need(l: u32) -> u32 {
    match l {
        0..=5 => 14,
        6..=11 => 56,
        12..=19 => 123,
        _ => 312,
    }
}
fn tl(x: f64, y: f64) -> usize {
    (y.clamp(0.0, W as f64 - 1.0) as usize) * W + x.clamp(0.0, W as f64 - 1.0) as usize
}
impl Game {
    fn new(seed: u64, cap: usize) -> Game {
        let (t, s) = floorgen(seed);
        let wall = (0..W * W)
            .map(|c| {
                t[c] == 0
                    && [-1i32, 0, 1].iter().any(|&dy| {
                        [-1i32, 0, 1].iter().any(|&dx| {
                            let (x, y) = ((c % W) as i32 + dx, (c / W) as i32 + dy);
                            x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < W && t[y as usize * W + x as usize] == 1
                        })
                    })
            })
            .collect();
        let walk_total = t.iter().filter(|&&v| v == 1).count();
        let models = KINDS.iter().map(|&k| [0, 1, 2, 3].map(|y| [model(k, y, 0), model(k, y, 1)])).collect();
        let mut g = Game {
            t,
            wall,
            vis: vec![false; W * W],
            explored: vec![false; W * W],
            walk_total,
            walk_seen: 0,
            dist: vec![u16::MAX; W * W],
            ring: vec![],
            occ: vec![0; W * W],
            tile_at: usize::MAX,
            px: (s % W) as f64 + 0.5,
            py: (s / W) as f64 + 0.5,
            pyaw: 0,
            pphase: 0.0,
            en: vec![],
            shards: vec![],
            bolts: vec![],
            hp: PMAX,
            flux: 100.0,
            rage: 0.0,
            xp: 0,
            level: 1,
            kills: 0,
            time: 0.0,
            fire_cd: 0.0,
            spawn_acc: 0.0,
            cap,
            rng: X::new(seed ^ 0xabcdef),
            msg: ("entered floor 1 - find the uplink".into(), 4.0),
            models,
        };
        g.retile();
        g
    }
    fn walk(&self, x: f64, y: f64) -> bool {
        x >= 0.0 && y >= 0.0 && x < W as f64 && y < W as f64 && self.t[tl(x, y)] == 1
    }
    fn los(&self, a: usize, b: usize) -> bool {
        let (mut x, mut y) = ((a % W) as i32, (a / W) as i32);
        let (x1, y1) = ((b % W) as i32, (b / W) as i32);
        let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
        let (sx, sy) = ((x1 - x).signum(), (y1 - y).signum());
        let mut e = dx + dy;
        loop {
            if x == x1 && y == y1 {
                return true;
            }
            if self.t[y as usize * W + x as usize] == 0 {
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
    /// player changed tile: FOV (R 14, LOS), explored memory, flow field, spawn ring (out of LOS, path 14-32)
    fn retile(&mut self) {
        let s = tl(self.px, self.py);
        if s == self.tile_at {
            return;
        }
        self.tile_at = s;
        self.vis.fill(false);
        let (cx, cy) = ((s % W) as i32, (s / W) as i32);
        for y in (cy - 14).max(0)..=(cy + 14).min(W as i32 - 1) {
            for x in (cx - 14).max(0)..=(cx + 14).min(W as i32 - 1) {
                let c = y as usize * W + x as usize;
                if (x - cx).pow(2) + (y - cy).pow(2) <= 196 && (self.t[c] == 1 || self.wall[c]) && self.los(s, c) {
                    self.vis[c] = true;
                    if !self.explored[c] {
                        self.explored[c] = true;
                        self.walk_seen += usize::from(self.t[c] == 1);
                    }
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
            if (14..=32).contains(&d) && !self.vis[c] {
                self.ring.push(c as u32);
            }
            for o in [c.wrapping_sub(1), c + 1, c.wrapping_sub(W), c + W] {
                if o < W * W && self.t[o] == 1 && self.dist[o] == u16::MAX {
                    self.dist[o] = d + 1;
                    q.push(o as u32);
                }
            }
        }
    }
    fn hurt(&mut self, d: f64) {
        self.hp -= d;
        self.rage = (self.rage + 4.0 * d / PMAX * 100.0).min(100.0);
        if self.hp <= 0.0 {
            self.hp = PMAX;
            self.en.clear();
            self.bolts.clear();
            self.msg = ("FLATLINE - rebooted (toy)".into(), 3.0);
        }
    }
    fn step(&mut self, dt: f64, mv: (f64, f64)) {
        self.time += dt;
        let m = self.time / 60.0;
        // player
        if mv.0 != 0.0 || mv.1 != 0.0 {
            let (nx, ny) = (self.px + mv.0 * 5.0 * dt, self.py + mv.1 * 5.0 * dt);
            if self.walk(nx + mv.0.signum() * 0.3, self.py) {
                self.px = nx;
            }
            if self.walk(self.px, ny + mv.1.signum() * 0.3) {
                self.py = ny;
            }
            self.pyaw = if mv.0.abs() > mv.1.abs() {
                if mv.0 > 0.0 { 0 } else { 2 }
            } else if mv.1 > 0.0 {
                1
            } else {
                3
            };
            self.pphase = (self.pphase + dt * 2.5).fract();
        }
        self.retile();
        // spawn (pincer rule: out of LOS, fog hides them)
        let target = self.cap.min(25 + (12.0 * m) as usize);
        self.spawn_acc += dt * (2.0 + 0.6 * m);
        while self.spawn_acc >= 1.0 && self.en.len() < target && !self.ring.is_empty() {
            self.spawn_acc -= 1.0;
            let c = self.ring[self.rng.below(self.ring.len() as u32) as usize] as usize;
            let r = self.rng.below(100);
            let kind = if r < 8 {
                Kind::Mech
            } else if r < 30 {
                Kind::Drone
            } else {
                Kind::Stalker
            };
            let hp = 30.0 * 1.12f64.powf(m) * if kind == Kind::Mech { 8.0 } else { 1.0 };
            let (x, y) = ((c % W) as f64 + 0.2 + 0.6 * self.rng.f(), (c / W) as f64 + 0.2 + 0.6 * self.rng.f());
            self.en.push(En {
                x,
                y,
                kind,
                yaw: 0,
                phase: self.rng.f(),
                hp,
                cd: 0.0,
            });
        }
        self.spawn_acc = self.spawn_acc.min(5.0);
        // enemies: flow descent + tile cap 3 (T3 law) + contact
        self.occ.fill(0);
        for e in &self.en {
            self.occ[tl(e.x, e.y)] = self.occ[tl(e.x, e.y)].saturating_add(1);
        }
        let mut dmg = 0.0;
        for i in 0..self.en.len() {
            let (x, y) = (self.en[i].x, self.en[i].y);
            let c = tl(x, y);
            let mut best = (c, self.dist[c]);
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let o = c as i32 + dx + dy * W as i32;
                if o < 0 || o as usize >= W * W {
                    continue;
                }
                let ok = dx == 0 || dy == 0 || (self.t[(c as i32 + dx) as usize] == 1 && self.t[(c as i32 + dy * W as i32) as usize] == 1);
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
            let e = &self.en[i];
            let sp = match e.kind {
                Kind::Drone => 4.0,
                Kind::Mech => 2.4,
                _ => 3.0,
            };
            let pd = ((self.px - x).powi(2) + (self.py - y).powi(2)).sqrt();
            if pd > 0.75 {
                let (nx, ny) = (x + dx / d * sp * dt, y + dy / d * sp * dt);
                let nt = tl(nx, ny);
                if self.walk(nx, ny) && (nt == c || self.occ[nt] < 3) {
                    self.occ[c] -= 1;
                    self.occ[nt] = self.occ[nt].saturating_add(1);
                    let e = &mut self.en[i];
                    (e.x, e.y) = (nx, ny);
                    e.yaw = if dx.abs() > dy.abs() {
                        if dx > 0.0 { 0 } else { 2 }
                    } else if dy > 0.0 {
                        1
                    } else {
                        3
                    };
                    e.phase = (e.phase + dt * 2.0).fract();
                }
            }
            let e = &mut self.en[i];
            e.cd = (e.cd - dt).max(0.0);
            if pd < 0.9 && e.cd == 0.0 {
                e.cd = 0.5;
                dmg += 6.0 * (1.0 + 0.04 * m) * if e.kind == Kind::Mech { 2.0 } else { 1.0 };
            }
        }
        if dmg > 0.0 {
            self.hurt(dmg);
        }
        // autofire PACKET BOLT: nearest VISIBLE enemy within 12 m, 25 m/s bolt
        self.fire_cd -= dt;
        if self.fire_cd <= 0.0 {
            self.fire_cd = 0.35;
            let tgt = self
                .en
                .iter()
                .filter(|e| self.vis[tl(e.x, e.y)])
                .map(|e| ((e.x - self.px).powi(2) + (e.y - self.py).powi(2), e.x, e.y))
                .filter(|q| q.0 < 144.0)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((d2, x, y)) = tgt {
                let d = d2.sqrt().max(1e-6);
                self.bolts.push([self.px, self.py, (x - self.px) / d * 25.0, (y - self.py) / d * 25.0, 0.6]);
            }
        }
        let cc = 0.10 + 0.005 * self.rage;
        let mut kills = vec![];
        let mut keep = Vec::with_capacity(self.bolts.len());
        for mut b in std::mem::take(&mut self.bolts) {
            b[0] += b[2] * dt;
            b[1] += b[3] * dt;
            b[4] -= dt;
            if b[4] <= 0.0 || !self.walk(b[0], b[1]) {
                continue;
            }
            if let Some(i) = self.en.iter().position(|e| (e.x - b[0]).powi(2) + (e.y - b[1]).powi(2) < 0.36) {
                let crit = if self.rng.f() < cc { 2.0 } else { 1.0 };
                self.en[i].hp -= 40.0 * 1.142f64.powi(self.level as i32 - 1) * crit;
                if self.en[i].hp <= 0.0 && !kills.contains(&i) {
                    kills.push(i);
                }
                continue;
            }
            keep.push(b);
        }
        self.bolts = keep;
        kills.sort_unstable_by(|a, b| b.cmp(a));
        for i in kills {
            let e = self.en.swap_remove(i);
            self.kills += 1;
            self.shards.push((e.x, e.y, if e.kind == Kind::Mech { 10 } else { 1 }));
        }
        // shards: magnet 3 m
        let (px, py) = (self.px, self.py);
        let mut got = 0;
        self.shards.retain_mut(|s| {
            let (dx, dy) = (px - s.0, py - s.1);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 0.4 {
                got += s.2;
                return false;
            }
            if d < 3.0 {
                s.0 += dx / d * 8.0 * dt;
                s.1 += dy / d * 8.0 * dt;
            }
            true
        });
        self.xp += got;
        while self.xp >= need(self.level) {
            self.xp -= need(self.level);
            self.level += 1;
            self.msg = (format!("LEVEL {} - +1 tree point", self.level), 2.5);
        }
        self.hp = (self.hp + 0.2 * dt).min(PMAX);
        self.flux = (self.flux + 2.0 * dt).min(100.0);
        self.rage = (self.rage - 5.0 * dt).max(0.0);
        self.msg.1 -= dt;
    }
}

// ---------- render ----------
struct View {
    mode: Mode,
    view_m: f64,
    aspect: f64,
    th: f64,
    rim: bool,
    cut: bool,
    map: bool,
    ui_scale: u8, // 0 = cell text; 1..=4 = pixel font, font px = ui_scale subpixels tall
}
type Cache = HashMap<(u32, u32, Mode), Sprite>;
fn render(g: &Game, v: &View, cols: usize, prow: usize, cv: &mut Canvas, cache: &mut Cache) {
    let (sx, sy) = v.mode.sub();
    let kc = cols as f64 / (SQRT_2 * v.view_m);
    let mut p = Proj {
        th: aq(v.th) as f64 / ANGLES * TAU,
        kc,
        aspect: v.aspect,
        cx: g.px,
        cy: g.py,
        ox: (cols * sx) as f64 / 2.0,
        oy: (prow * sy) as f64 * 0.56,
        sx: sx as f64,
        sy: sy as f64,
    };
    let (c0, s0, _) = p.p(0.0, 0.0, 0.0);
    // snap world origin to whole CELLS (sx x sy subpixels): pan shifts whole cells, no sub-cell re-split
    let (fx, fy) = (sx as f64, sy as f64);
    p.ox -= c0 - (c0 / fx).floor() * fx;
    p.oy -= s0 - (s0 / fy).floor() * fy;
    let a = aq(p.th);
    cv.cores.clear();
    cv.hues.clear();
    let (w, h) = (cv.w, cv.h);
    let (pu, pv) = p.rot(g.px, g.py);
    let eff = |x: i32, y: i32| -> usize {
        if x < 0 || y < 0 || x >= W as i32 || y >= W as i32 || !g.wall[y as usize * W + x as usize] {
            return 0;
        }
        if v.cut {
            let (u, vv) = p.rot(x as f64 + 0.5, y as f64 + 0.5);
            let ahead = (u + vv) - (pu + pv);
            if ahead > 0.3 && ahead < 6.0 && ((u - vv) - (pu - pv)).abs() < 3.0 {
                return 1;
            }
        }
        2
    };
    let band = |f: f64| (f * 12.0).round() / 12.0;
    let nofog = std::env::var("TOT_NOFOG").is_ok();
    let fogd = |x: f64, y: f64| {
        if nofog {
            1.0
        } else {
            1.0 - 0.6 * ((((x - g.px).powi(2) + (y - g.py).powi(2)).sqrt() - 6.0) / 12.0).clamp(0.0, 1.0)
        }
    };
    let onscreen = |o: (f64, f64, f64)| o.0 >= -60.0 && o.1 >= -120.0 && o.0 <= w as f64 + 60.0 && o.1 <= h as f64 + 160.0;
    let at = |o: (f64, f64, f64)| (o.0.round() as i32, o.1.round() as i32, o.2 as f32);
    // 1. item lists (serial; bakes missing sprites into the cache)
    let mut scene: Vec<Item> = Vec::with_capacity(4096);
    let rad = (v.view_m * 0.85) as i32;
    let (tx0, ty0) = (g.px as i32, g.py as i32);
    for ty in (ty0 - rad).max(0)..(ty0 + rad).min(W as i32) {
        for tx in (tx0 - rad).max(0)..(tx0 + rad).min(W as i32) {
            let c = ty as usize * W + tx as usize;
            let o = p.p(tx as f64, ty as f64, 0.0);
            if !g.explored[c] || !onscreen(o) {
                continue;
            }
            let fog = if g.vis[c] { band(fogd(tx as f64 + 0.5, ty as f64 + 0.5)) } else { 0.3 };
            let hh = eff(tx, ty);
            let id = if hh == 0 {
                3000
            } else {
                let nb = [(1, 0), (-1, 0), (0, 1), (0, -1)];
                let mask = nb.iter().enumerate().fold(0u32, |m, (f, (dx, dy))| if eff(tx + dx, ty + dy) < hh { m | 1 << f } else { m });
                2000 + (hh as u32 - 1) * 16 + mask
            };
            sprite(cache, &g.models, &p, a, v.mode, id);
            scene.push(Item {
                key: (id, a, v.mode),
                o: at(o),
                fog,
                ent: 0,
            });
        }
    }
    let mut bodies: Vec<(f64, f64, Kind, i32, f64)> = vec![(g.px, g.py, Kind::Player, g.pyaw, g.pphase)];
    bodies.extend(g.en.iter().filter(|e| g.vis[tl(e.x, e.y)]).map(|e| (e.x, e.y, e.kind, e.yaw, e.phase)));
    let shadows: Vec<(f64, f64, f64)> = bodies
        .iter()
        .map(|b| {
            let (c0, s0, _) = p.p(b.0, b.1, 0.0);
            (c0, s0, if b.2 == Kind::Mech { 0.8 } else { 0.4 })
        })
        .collect();
    let mut src: Vec<(u32, f64, f64, Rgb, Option<char>)> = g.shards.iter().filter(|s| g.vis[tl(s.0, s.1)]).map(|s| (4000, s.0, s.1, C_XP, None)).collect();
    src.extend(g.bolts.iter().map(|b| (4001, b[0], b[1], TRIM, None)));
    for b in bodies {
        let fr = (b.4 * 2.0) as u32 % 2;
        let m = &g.models[b.2 as usize][b.3 as usize][fr as usize];
        src.push((1000 + b.2 as u32 * 8 + b.3 as u32 * 2 + fr, b.0, b.1, m.hue, Some(m.glyph)));
    }
    let mut items: Vec<Item> = Vec::with_capacity(src.len());
    for (id, x, y, hue, glyph) in src {
        let o = p.p(x, y, 0.0);
        if !onscreen(o) {
            continue;
        }
        let ent = items.len() as u16 + 1;
        cv.hues.push(hue);
        let sp = sprite(cache, &g.models, &p, a, v.mode, id);
        let oi = at(o);
        if let (Some(gl), Some((dc, ds))) = (glyph, sp.core) {
            let (c, s) = (oi.0 + dc, oi.1 + ds);
            if c >= 0 && s >= 0 && (c as usize) < w && (s as usize) < h {
                cv.cores.push((c as usize, s as usize, gl, hue, ent));
            }
        }
        items.push(Item {
            key: (id, a, v.mode),
            o: oi,
            fog: band(fogd(x, y)),
            ent,
        });
    }
    // 2. bands in parallel: clear, blit scene, shadows, blit bodies (pure per band; cache read-only)
    let cache: &Cache = cache;
    let nt = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
    let rows_per = h.div_ceil(nt).max(1);
    let (ra, rb) = (SQRT_2 * kc * p.sx, SQRT_2 * kc / v.aspect * p.sy / 2.0);
    std::thread::scope(|s| {
        for (bi, (sc, bd)) in cv.scene.chunks_mut(rows_per * w).zip(cv.body.chunks_mut(rows_per * w)).enumerate() {
            let (scene, items, shadows) = (&scene, &items, &shadows);
            s.spawn(move || {
                let (s0, bh) = ((bi * rows_per) as i32, sc.len() / w);
                sc.fill(EMPTY);
                bd.fill(EMPTY);
                for it in scene {
                    if let Some(sp) = cache.get(&it.key) {
                        blit(sc, w, s0, bh, sp, it);
                    }
                }
                for &(c0, sh0, r) in shadows {
                    let (a2, b2) = (r * ra, r * rb);
                    let lo = ((sh0 - b2).floor() as i64).max(s0 as i64);
                    let hi = ((sh0 + b2).ceil() as i64).min(s0 as i64 + bh as i64 - 1);
                    for sy in lo..=hi {
                        for c in ((c0 - a2).floor() as i64).max(0)..=((c0 + a2).ceil() as i64).min(w as i64 - 1) {
                            let (dx, dy) = ((c as f64 + 0.5 - c0) / a2, (sy as f64 + 0.5 - sh0) / b2);
                            if dx * dx + dy * dy < 1.0 {
                                let px = &mut sc[(sy - s0 as i64) as usize * w + c as usize];
                                px.rgb = px.rgb.scale(0.5);
                            }
                        }
                    }
                }
                for it in items {
                    if let Some(sp) = cache.get(&it.key) {
                        blit(bd, w, s0, bh, sp, it);
                    }
                }
            });
        }
    });
}
fn sprite<'c>(cache: &'c mut Cache, models: &[[[Model; 2]; 4]], p: &Proj, a: u32, mode: Mode, id: u32) -> &'c Sprite {
    cache.entry((id, a, mode)).or_insert_with(|| bake_id(models, p, id))
}

// ---------- UI: 3-row strip + map overlay ----------
/// UI target: x and widths in CHARACTER units, line 0..3 of the strip
trait Ink {
    fn text(&mut self, line: usize, x: usize, s: &str, fg: Rgb) -> usize;
    fn bar(&mut self, line: usize, x: usize, width: usize, frac: f64, fg: Rgb) -> usize;
    fn width(&self) -> usize;
}
/// plain cells (normal font sizes)
struct CellInk<'a> {
    cells: &'a mut [Cell],
    cols: usize,
    base: usize,
}
impl Ink for CellInk<'_> {
    fn text(&mut self, line: usize, x: usize, s: &str, fg: Rgb) -> usize {
        let mut c = x;
        for ch in s.chars().take(self.cols.saturating_sub(x)) {
            self.cells[(self.base + line) * self.cols + c] = Cell { ch, fg, bg: C_STRIP };
            c += 1;
        }
        c
    }
    fn bar(&mut self, line: usize, x: usize, width: usize, frac: f64, fg: Rgb) -> usize {
        const E: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
        let full = (frac.clamp(0.0, 1.0) * width as f64 * 8.0).round() as usize;
        for i in 0..width.min(self.cols.saturating_sub(x)) {
            let v = full.saturating_sub(i * 8).min(8);
            let cell = if v == 0 {
                Cell {
                    ch: '░',
                    fg: fg.scale(0.3),
                    bg: C_STRIP,
                }
            } else {
                Cell { ch: E[v - 1], fg, bg: C_STRIP }
            };
            self.cells[(self.base + line) * self.cols + x + i] = cell;
        }
        x + width
    }
    fn width(&self) -> usize {
        self.cols
    }
}
/// 5x7 pixel font drawn into the strip's subpixel buffer (tiny fonts); pixel = xs x ys subpixels
struct PixInk {
    px: Vec<Rgb>,
    w: usize,
    xs: usize,
    ys: usize,
}
fn glyph5x7(ch: char) -> &'static str {
    match ch.to_ascii_uppercase() {
        'A' => ".###. #...# #...# ##### #...# #...# #...#",
        'B' => "####. #...# #...# ####. #...# #...# ####.",
        'C' => ".###. #...# #.... #.... #.... #...# .###.",
        'D' => "####. #...# #...# #...# #...# #...# ####.",
        'E' => "##### #.... #.... ####. #.... #.... #####",
        'F' => "##### #.... #.... ####. #.... #.... #....",
        'G' => ".###. #...# #.... #.### #...# #...# .####",
        'H' => "#...# #...# #...# ##### #...# #...# #...#",
        'I' => ".###. ..#.. ..#.. ..#.. ..#.. ..#.. .###.",
        'J' => "..### ...#. ...#. ...#. ...#. #..#. .##..",
        'K' => "#...# #..#. #.#.. ##... #.#.. #..#. #...#",
        'L' => "#.... #.... #.... #.... #.... #.... #####",
        'M' => "#...# ##.## #.#.# #.#.# #...# #...# #...#",
        'N' => "#...# #...# ##..# #.#.# #..## #...# #...#",
        'O' => ".###. #...# #...# #...# #...# #...# .###.",
        'P' => "####. #...# #...# ####. #.... #.... #....",
        'Q' => ".###. #...# #...# #...# #.#.# #..#. .##.#",
        'R' => "####. #...# #...# ####. #.#.. #..#. #...#",
        'S' => ".#### #.... #.... .###. ....# ....# ####.",
        'T' => "##### ..#.. ..#.. ..#.. ..#.. ..#.. ..#..",
        'U' => "#...# #...# #...# #...# #...# #...# .###.",
        'V' => "#...# #...# #...# #...# #...# .#.#. ..#..",
        'W' => "#...# #...# #...# #.#.# #.#.# #.#.# .#.#.",
        'X' => "#...# #...# .#.#. ..#.. .#.#. #...# #...#",
        'Y' => "#...# #...# .#.#. ..#.. ..#.. ..#.. ..#..",
        'Z' => "##### ....# ...#. ..#.. .#... #.... #####",
        '0' => ".###. #...# #..## #.#.# ##..# #...# .###.",
        '1' => "..#.. .##.. ..#.. ..#.. ..#.. ..#.. .###.",
        '2' => ".###. #...# ....# ...#. ..#.. .#... #####",
        '3' => "####. ....# ....# .###. ....# ....# ####.",
        '4' => "...#. ..##. .#.#. #..#. ##### ...#. ...#.",
        '5' => "##### #.... ####. ....# ....# #...# .###.",
        '6' => "..##. .#... #.... ####. #...# #...# .###.",
        '7' => "##### ....# ...#. ..#.. .#... .#... .#...",
        '8' => ".###. #...# #...# .###. #...# #...# .###.",
        '9' => ".###. #...# #...# .#### ....# ...#. .##..",
        ':' => "..... ..#.. ..#.. ..... ..#.. ..#.. .....",
        '/' => "....# ....# ...#. ..#.. .#... #.... #....",
        '%' => "##..# ##..# ...#. ..#.. .#... #..## #..##",
        '.' => "..... ..... ..... ..... ..... .##.. .##..",
        '-' => "..... ..... ..... .###. ..... ..... .....",
        '+' => "..... ..#.. ..#.. ##### ..#.. ..#.. .....",
        '=' => "..... ..... ##### ..... ##### ..... .....",
        '(' => "...#. ..#.. .#... .#... .#... ..#.. ...#.",
        ')' => ".#... ..#.. ...#. ...#. ...#. ..#.. .#...",
        '▴' => "..... ..... ..#.. .###. ##### ..... .....",
        '·' => "..... ..... ..... ..#.. ..... ..... .....",
        ' ' => "..... ..... ..... ..... ..... ..... .....",
        _ => ".###. #...# ....# ...#. ..#.. ..... ..#..",
    }
}
impl PixInk {
    /// advance 6 font px per char, line pitch 10 font px (7 glyph + 3 gap)
    fn at(&self, line: usize, x: usize) -> (usize, usize) {
        (x * 6 * self.xs + self.xs, line * 10 * self.ys + 2 * self.ys)
    }
    fn dot(&mut self, x: usize, y: usize, c: Rgb) {
        for yy in y..y + self.ys {
            for xx in x..x + self.xs {
                if xx < self.w && yy * self.w + xx < self.px.len() {
                    self.px[yy * self.w + xx] = c;
                }
            }
        }
    }
}
impl Ink for PixInk {
    fn text(&mut self, line: usize, x: usize, s: &str, fg: Rgb) -> usize {
        let mut c = x;
        for ch in s.chars() {
            let (x0, y0) = self.at(line, c);
            for (r, row) in glyph5x7(ch).split(' ').enumerate() {
                for (k, b) in row.bytes().enumerate() {
                    if b == b'#' {
                        self.dot(x0 + k * self.xs, y0 + r * self.ys, fg);
                    }
                }
            }
            c += 1;
        }
        c
    }
    fn bar(&mut self, line: usize, x: usize, width: usize, frac: f64, fg: Rgb) -> usize {
        let (x0, y0) = self.at(line, x);
        let wpx = width * 6;
        let full = (frac.clamp(0.0, 1.0) * wpx as f64).round() as usize;
        for k in 0..wpx {
            for r in 1..6 {
                self.dot(x0 + k * self.xs, y0 + r * self.ys, if k < full { fg } else { fg.scale(0.22) });
            }
        }
        x + width
    }
    fn width(&self) -> usize {
        self.w / (6 * self.xs)
    }
}
/// strip content, written once for both inks
fn strip(ink: &mut dyn Ink, g: &Game, v: &View, leader: bool, perf: &str) {
    let wide = ink.width() >= 200;
    let (wh, wf, wr, wx) = if wide { (24, 16, 16, 24) } else { (16, 10, 10, 12) };
    let mut c = ink.text(0, 1, "HP ", C_HP);
    c = ink.bar(0, c, wh, g.hp / PMAX, if g.hp < PMAX * 0.25 { Rgb::hex(0xff3860) } else { C_HP });
    c = ink.text(0, c, &format!(" {:.0}/{:.0}  FX ", g.hp, PMAX), C_HP);
    c = ink.bar(0, c, wf, g.flux / 100.0, C_FX);
    c = ink.text(0, c, &format!(" {:.0}  RG ", g.flux), C_FX);
    c = ink.bar(0, c, wr, g.rage / 100.0, C_RG);
    let pips = "▴".repeat((g.rage / 25.0) as usize);
    c = ink.text(0, c, &format!(" {pips:<3} crit {:.0}%  LV {} ", (0.10 + 0.005 * g.rage) * 100.0, g.level), C_RG);
    c = ink.bar(0, c, wx, g.xp as f64 / need(g.level) as f64, C_XP);
    ink.text(0, c, &format!(" {}:{:02}  K {}", (g.time / 60.0) as u32, g.time as u32 % 60, g.kills), C_XP);
    let pct = g.walk_seen * 100 / g.walk_total.max(1);
    let mut c = ink.text(1, 1, &format!("MAP {pct}% · RELAY 0/3 · EXIT ? · live {} · ", g.en.len()), C_UI);
    if g.msg.1 > 0.0 {
        c = ink.text(1, c, &format!("{} · ", g.msg.0), Rgb::hex(0xffd75e));
    }
    ink.text(1, c, &format!("{} view {:.0}m {perf}", v.mode.name(), v.view_m), C_UI.scale(0.7));
    let hint = if leader {
        "-- LEADER --  m map   r rim   c cutaway   p pause   any other key = cancel"
    } else {
        "wasd move  q/e turn  1/2/3 HALF/QUAD/SEXT  +/- zoom  [/] ui size  space leader  x quit"
    };
    ink.text(2, 1, hint, if leader { Rgb::hex(0xffd75e) } else { C_UI.scale(0.6) });
}
/// strip height in cells: 3 text rows, or 3 pixel-font lines of 10 font px at ui_scale
fn strip_rows(v: &View) -> usize {
    let (_, sy) = v.mode.sub();
    if v.ui_scale == 0 { 3 } else { (30 * v.ui_scale as usize + 2).div_ceil(sy) }
}
fn ui(g: &Game, v: &View, cells: &mut [Cell], cols: usize, rows: usize, leader: bool, perf: &str) {
    let base = rows - strip_rows(v);
    for c in cells[base * cols..].iter_mut() {
        *c = Cell { ch: ' ', fg: C_UI, bg: C_STRIP };
    }
    if v.ui_scale == 0 {
        strip(&mut CellInk { cells, cols, base }, g, v, leader, perf);
    } else {
        let (sx, sy) = v.mode.sub();
        let s = v.ui_scale as usize;
        // square font pixels: subpixel w:h = (1/sx) : (aspect/sy)
        let xs = ((s as f64 * v.aspect * sx as f64 / sy as f64).round() as usize).max(1);
        let n = rows - base;
        let mut ink = PixInk {
            px: vec![C_STRIP; cols * sx * n * sy],
            w: cols * sx,
            xs,
            ys: s,
        };
        strip(&mut ink, g, v, leader, perf);
        let mut buf = [Rgb::default(); 6];
        for r in 0..n {
            for c in 0..cols {
                cells[(base + r) * cols + c] = cell_of(v.mode, sx, sy, &mut buf, |a, b| ink.px[(r * sy + a) * ink.w + c * sx + b]);
            }
        }
    }
    if v.map {
        // iso overlay (design sec 19.12): marked cells only, centred on player, rotates with heading
        let (s, cth) = v.th.sin_cos();
        let km = (cols as f64 / 300.0).max(0.5);
        let (mc, mr) = (cols as f64 / 2.0, base as f64 * 0.5);
        for y in 0..W {
            for x in 0..W {
                let i = y * W + x;
                if !g.explored[i] {
                    continue;
                }
                let (dx, dy) = (x as f64 + 0.5 - g.px, y as f64 + 0.5 - g.py);
                let (u, vv) = (cth * dx - s * dy, s * dx + cth * dy);
                let (cc, rr) = ((mc + (u - vv) * km).round() as i64, (mr + (u + vv) * km / 2.0 / v.aspect).round() as i64);
                if cc < 0 || rr < 0 || cc as usize >= cols || rr as usize >= base {
                    continue;
                }
                let cell = &mut cells[rr as usize * cols + cc as usize];
                if g.wall[i] {
                    *cell = Cell {
                        ch: '█',
                        fg: WALL.mix(TRIM, 0.4),
                        bg: cell.bg,
                    };
                } else {
                    *cell = Cell {
                        ch: '█',
                        fg: TRIM.scale(0.35),
                        bg: cell.bg,
                    };
                }
            }
        }
        let i = (mr.round() as usize) * cols + mc.round() as usize;
        cells[i] = Cell {
            ch: '█',
            fg: Rgb::hex(0xffffff),
            bg: cells[i].bg,
        };
    }
}

// ---------- emit ----------
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

// ---------- terminal ----------
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
struct Opts {
    seed: u64,
    view_m: f64,
    cap: usize,
}
fn opts(a: &[String]) -> Res<Opts> {
    let mut o = Opts { seed: 7, view_m: 40.0, cap: 300 };
    for k in (0..a.len()).step_by(2) {
        let v = a.get(k + 1).ok_or(format!("missing value for {}", a[k]))?;
        let n = v.parse::<f64>().map_err(|e| format!("{}: {e}", a[k]))?;
        match a[k].as_str() {
            "--seed" => o.seed = n as u64,
            "--view-m" => o.view_m = n,
            "--cap" => o.cap = n as usize,
            x => return Err(format!("unknown flag {x}")),
        }
    }
    Ok(o)
}
/// screen buffers: canvas (subpixels), sprite cache, cell front/back, out bytes
struct Screen {
    cols: usize,
    rows: usize,
    cv: Canvas,
    cache: Cache,
    cur: Vec<Cell>,
    prev: Vec<Cell>,
    out: Vec<u8>,
    ph: [u128; 4], // us: render, compose, ui, emit (sum over frames)
}
impl Screen {
    fn new(cols: usize, rows: usize) -> Screen {
        Screen {
            cols,
            rows,
            cv: Canvas {
                w: 0,
                h: 0,
                scene: vec![],
                body: vec![],
                hues: vec![],
                cores: vec![],
            },
            cache: HashMap::new(),
            cur: vec![BLANK; cols * rows],
            prev: vec![BLANK; cols * rows],
            out: Vec::with_capacity(1 << 21),
            ph: [0; 4],
        }
    }
    /// render + compose + ui + emit into self.out, swap buffers; returns compose us
    fn frame(&mut self, g: &Game, v: &View, force: bool, leader: bool, perf: &str) -> u128 {
        let t = Instant::now();
        let (cols, rows) = (self.cols, self.rows);
        let (sx, sy) = v.mode.sub();
        let prow = rows - strip_rows(v);
        if self.cv.w != cols * sx || self.cv.h != prow * sy {
            self.cv = Canvas {
                w: cols * sx,
                h: prow * sy,
                scene: vec![EMPTY; cols * sx * prow * sy],
                body: vec![EMPTY; cols * sx * prow * sy],
                hues: vec![],
                cores: vec![],
            };
        }
        render(g, v, cols, prow, &mut self.cv, &mut self.cache);
        let t1 = Instant::now();
        compose(&self.cv, v.mode, v.rim, cols, prow, &mut self.cur[..cols * prow]);
        let t2 = Instant::now();
        ui(g, v, &mut self.cur, cols, rows, leader, perf);
        let t3 = Instant::now();
        let us = t.elapsed().as_micros();
        self.out.clear();
        emit(&self.prev, &self.cur, cols, force, &mut self.out);
        for (k, d) in [t1 - t, t2 - t1, t3 - t2, t3.elapsed()].iter().enumerate() {
            self.ph[k] += d.as_micros();
        }
        std::mem::swap(&mut self.prev, &mut self.cur);
        us
    }
}
fn play(o: Opts) -> Res<String> {
    let size = stty(&["size"])?;
    let mut it = size.split_whitespace().map(|s| s.parse::<usize>());
    let (Some(Ok(rows)), Some(Ok(cols))) = (it.next(), it.next()) else {
        return Err(format!("stty size: {size}"));
    };
    if cols < 100 || rows < 30 {
        return Err(format!("terminal {cols}x{rows} too small (min 100x30); use a smaller font"));
    }
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
    let mut g = Game::new(o.seed, o.cap);
    let tiny = cols >= 400; // font ~2-3: SEXT world + pixel-font UI
    let mut v = View {
        mode: if tiny { Mode::Sext } else { Mode::Half },
        view_m: o.view_m,
        aspect: 2.0,
        th: 0.0,
        rim: true,
        cut: true,
        map: false,
        ui_scale: if tiny { 2 } else { 0 },
    };
    let _ = std::fs::write("walk-perf.log", "");
    let mut sc = Screen::new(cols, rows);
    let (mut target, mut from, mut turn) = (0.0f64, 0.0f64, 1.0f64);
    let (mut leader, mut pause, mut force) = (false, false, true);
    let mut held = [0.0f64; 4]; // w a s d hold timers (s); autorepeat refreshes
    let mut esc: Vec<u8> = vec![];
    let (mut acc_b, mut acc_us, mut nfr, mut tick) = (0usize, 0u128, 0u32, Instant::now());
    let mut perf = String::new();
    let dt = 1.0 / 60.0;
    loop {
        let t0 = Instant::now();
        while let Ok(c) = rx.try_recv() {
            if !esc.is_empty() || c == 0x1b {
                esc.push(c);
                if esc.len() > 2 && (0x40..=0x7e).contains(&c) {
                    let s = String::from_utf8_lossy(&esc).to_string();
                    if let Some(p) = s.strip_prefix("\x1b[6;").and_then(|r| r.strip_suffix('t')) {
                        let n: Vec<f64> = p.split(';').filter_map(|x| x.parse().ok()).collect();
                        if n.len() == 2 && n[1] > 0.0 {
                            v.aspect = n[0] / n[1];
                            sc.cache.clear();
                        }
                    }
                    esc.clear();
                } else if esc.len() > 16 {
                    esc.clear();
                }
                continue;
            }
            if leader {
                leader = false;
                match c {
                    b'm' => v.map = !v.map,
                    b'r' => v.rim = !v.rim,
                    b'c' => v.cut = !v.cut,
                    b'p' => pause = !pause,
                    _ => {}
                }
                continue;
            }
            match c {
                b'x' | 3 => return Ok(perf),
                b' ' => leader = true,
                b'q' | b'e' => {
                    from = v.th;
                    target += if c == b'q' { -std::f64::consts::FRAC_PI_2 } else { std::f64::consts::FRAC_PI_2 };
                    turn = 0.0;
                }
                b'w' => held[0] = 0.15,
                b'a' => held[1] = 0.15,
                b's' => held[2] = 0.15,
                b'd' => held[3] = 0.15,
                b'1' | b'2' | b'3' => {
                    v.mode = [Mode::Half, Mode::Quad, Mode::Sext][(c - b'1') as usize];
                    force = true;
                }
                b'[' | b']' => {
                    v.ui_scale = if c == b'[' { v.ui_scale.saturating_sub(1) } else { (v.ui_scale + 1).min(4) };
                    force = true;
                }
                b'+' | b'=' => v.view_m = (v.view_m * 0.85).max(10.0),
                b'-' => v.view_m = (v.view_m / 0.85).min(100.0),
                _ => {}
            }
        }
        // screen dirs -> world: up = -(u+v), right = (u-v)
        let (mut su, mut sv) = (0.0f64, 0.0f64);
        let dirs = [(-1.0, -1.0), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)];
        for (k, h) in held.iter_mut().enumerate() {
            if *h > 0.0 {
                su += dirs[k].0;
                sv += dirs[k].1;
                *h -= dt;
            }
        }
        let l = (su * su + sv * sv).sqrt();
        let (s, c2) = v.th.sin_cos();
        let mv = if l > 0.0 { ((c2 * su + s * sv) / l, (-s * su + c2 * sv) / l) } else { (0.0, 0.0) };
        if !pause {
            g.step(dt, mv);
        }
        if turn < 1.0 {
            turn = (turn + dt / 0.25).min(1.0);
            let e = turn * turn * (3.0 - 2.0 * turn);
            v.th = from + (target - from) * e;
        }
        let us = sc.frame(&g, &v, force, leader, &perf);
        force = false;
        so.write_all(&sc.out).map_err(|e| e.to_string())?;
        so.flush().map_err(|e| e.to_string())?;
        acc_b += sc.out.len();
        acc_us += us;
        nfr += 1;
        if tick.elapsed() >= Duration::from_secs(1) {
            let n = nfr.max(1) as f64;
            perf = format!(
                "{cols}x{rows} asp {:.2} | {:.1} KB/f {:.2} ms {} fps",
                v.aspect,
                acc_b as f64 / n / 1024.0,
                acc_us as f64 / n / 1000.0,
                nfr
            );
            // survives the window closing: one line per second, tail it after quit
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("walk-perf.log") {
                let _ = writeln!(f, "{:.0}s {} {} ui{} | {perf}", g.time, v.mode.name(), if pause { "PAUSE" } else { "run" }, v.ui_scale);
            }
            (acc_b, acc_us, nfr, tick) = (0, 0, 0, Instant::now());
        }
        if let Some(rest) = Duration::from_secs_f64(dt).checked_sub(t0.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}
/// headless check: scripted walk 20 s per mode at 320x96 + 149x79; prints perf + writes a luma txt
fn shot(o: Opts) -> Res<()> {
    let sizes: Vec<(usize, usize)> = std::env::var("TOT_SIZES")
        .ok()
        .map(|s| s.split(',').filter_map(|p| p.split_once('x')).filter_map(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))).collect())
        .unwrap_or(vec![(149, 79), (320, 96)]);
    for (cols, rows) in sizes {
        for mode in [Mode::Half, Mode::Quad, Mode::Sext] {
            let mut g = Game::new(o.seed, o.cap);
            let v = View {
                mode,
                view_m: o.view_m,
                aspect: 2.1,
                th: 0.0,
                rim: true,
                cut: true,
                map: false,
                ui_scale: if cols >= 400 { 2 } else { 0 },
            };
            let mut sc = Screen::new(cols, rows);
            let (mut bytes, mut times) = (vec![], vec![]);
            for f in 0..1200 {
                let a = f as f64 / 300.0;
                let mv = if std::env::var("TOT_STILL").is_ok() { (0.0, 0.0) } else { (a.cos(), a.sin()) };
                g.step(1.0 / 60.0, mv);
                let us = sc.frame(&g, &v, f == 0, false, "");
                if f > 0 {
                    bytes.push(sc.out.len());
                    times.push(us);
                }
            }
            bytes.sort_unstable();
            times.sort_unstable();
            let n = bytes.len();
            println!(
                "{cols}x{rows} {:<8} live {:>3} kills {:>3} lv {:>2} map {:>2}% | bytes p50 {:>6} p99 {:>6} | frame us p50 {:>5} p99 {:>5} | sprites {} | us/frame render compose ui emit {:?}",
                mode.name(),
                g.en.len(),
                g.kills,
                g.level,
                g.walk_seen * 100 / g.walk_total,
                bytes[n / 2],
                bytes[n * 99 / 100],
                times[n / 2],
                times[n * 99 / 100],
                sc.cache.len(),
                sc.ph.map(|v| v / 1199)
            );
            if mode == Mode::Sext {
                let txt: String = sc.prev.chunks(cols).map(|r| r.iter().map(|c| c.ch).collect::<String>() + "\n").collect();
                std::fs::write(format!("shot-{cols}x{rows}-SEXT.txt"), txt).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let shot_mode = a.get(1).map(String::as_str) == Some("shot");
    let rest = &a[if shot_mode { 2 } else { 1 }..];
    let r = opts(rest).and_then(|o| if shot_mode { shot(o) } else { play(o).map(|perf| println!("last perf: {perf}")) });
    if let Err(e) = r {
        eprintln!("walk: {e}");
        std::process::exit(1);
    }
}
