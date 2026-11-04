//! T10 toy "showcase" (vic 2026-11-04: "visually appealing like a real game ... Diablo themed, arenas ... enough room to move like in diablo").
//! Cyber-gothic cathedral floor: per-pixel light (darkness, hero lamp, flickering braziers with LOS, glowing fx),
//! textured stone + brick voxel walls + neon trims, props, 0.125 m voxel bodies, boss in the far room, loot beams,
//! sparks + death bursts + decals, nova power (f), pixel-font UI. std only, f64 toy.
//! keys: wasd move, q/e turn, f nova, 1/2/3 HALF/QUAD/SEXT, +/- zoom, [ ] ui size, space leader (m map r rim c cut p pause), x quit
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

// ---------- body kinds (smooth SDF models: see bake_sdf) ----------
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Player,
    Stalker,
    Drone,
    Mech,
    Behemoth,
}
const KINDS: [Kind; 5] = [Kind::Player, Kind::Stalker, Kind::Drone, Kind::Mech, Kind::Behemoth];

// ---------- sprites: bake once per (id, angle, mode), blit at integer subpixels ----------
const ANGLES: f64 = 64.0;
struct Bx {
    lo: [f64; 3],
    hi: [f64; 3],
    mask: u8,
    rgb: Rgb,
    emit: bool, // emissive = ignores lighting (neon, fx)
}
const fn bxe(lo: [f64; 3], hi: [f64; 3], mask: u8, rgb: Rgb) -> Bx {
    Bx { lo, hi, mask, rgb, emit: false }
}
/// visits every visible face sample: put(col, sub, near, rgb, face|0x80 if emissive, world x, world y)
fn faces(p: &Proj, b: &Bx, mut put: impl FnMut(f64, f64, f64, Rgb, u8, f64, f64)) {
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
        let col = if b.emit { b.rgb } else { b.rgb.scale(FACE_L[k]) };
        let fk = k as u8 | if b.emit { 0x80 } else { 0 };
        for a in 0..mi {
            for c in 0..mj {
                let mut q = [0.0; 3];
                q[fix] = fv;
                q[i] = b.lo[i] + (a as f64 + 0.5) / mi as f64 * (b.hi[i] - b.lo[i]);
                q[j] = b.lo[j] + (c as f64 + 0.5) / mj as f64 * (b.hi[j] - b.lo[j]);
                let (x, y, n) = p.p(q[0], q[1], q[2]);
                put(x, y, n, col, fk, q[0], q[1]);
            }
        }
    }
}
/// baked px: dcol, dsub, dnear, rgb, face|emit, world dx, world dy (for per-pixel light)
type SPx = (i32, i32, f32, Rgb, u8, f32, f32);
/// bake scratch per screen subpixel: near, rgb, face, world dx, world dy
type BakePx = (f32, Rgb, u8, f32, f32);
struct Sprite {
    px: Vec<SPx>,
    core: Option<(i32, i32)>,
    rows: (i32, i32),
}
fn bake(p: &Proj, boxes: &[Bx], core: Option<[f64; 3]>) -> Sprite {
    let o = Proj {
        cx: 0.0,
        cy: 0.0,
        ox: 0.0,
        oy: 0.0,
        ..*p
    };
    let mut m: HashMap<(i32, i32), BakePx> = HashMap::new();
    for b in boxes {
        faces(&o, b, |c, s, n, col, k, wx, wy| {
            let e = m.entry((c.floor() as i32, s.floor() as i32)).or_insert((f32::MIN, col, k, 0.0, 0.0));
            if n as f32 > e.0 {
                *e = (n as f32, col, k, wx as f32, wy as f32);
            }
        });
    }
    let core = core.map(|q| {
        let (c, s, _) = o.p(q[0], q[1], q[2]);
        (c.floor() as i32, s.floor() as i32)
    });
    let px: Vec<SPx> = m.into_iter().map(|((c, s), (n, col, k, wx, wy))| (c, s, n, col, k, wx, wy)).collect();
    let rows = px.iter().fold((i32::MAX, i32::MIN), |(a, b), q| (a.min(q.1), b.max(q.1)));
    Sprite { px, core, rows }
}
/// voxel-built tile sprite (0.25 m voxels) with texture fn; mask bit f set = side f exposed to open floor
fn tile_vox(p: &Proj, h: f64, mask: u8, tex: impl Fn(i32, i32, i32, i32) -> (Rgb, bool)) -> Sprite {
    let (n, nz) = (4i32, (h / 0.25).round().max(1.0) as i32);
    let mut boxes = vec![];
    for z in 0..nz {
        for y in 0..n {
            for x in 0..n {
                let side = |f: usize, inside: bool| -> bool { if inside { false } else { mask & (1 << f) != 0 } };
                let m = u8::from(side(0, x + 1 < n)) | u8::from(side(1, x > 0)) << 1 | u8::from(side(2, y + 1 < n)) << 2 | u8::from(side(3, y > 0)) << 3 | u8::from(z + 1 == nz) << 4;
                if m == 0 {
                    continue;
                }
                let (rgb, emit) = tex(x, y, z, nz);
                let lo = [x as f64 * 0.25, y as f64 * 0.25, z as f64 * 0.25];
                boxes.push(Bx {
                    lo,
                    hi: [lo[0] + 0.25, lo[1] + 0.25, lo[2] + 0.25],
                    mask: m,
                    rgb,
                    emit,
                });
            }
        }
    }
    bake(p, &boxes, None)
}
fn hash3(x: i32, y: i32, z: i32) -> f64 {
    (((x as u32).wrapping_mul(73856093) ^ (y as u32).wrapping_mul(19349663) ^ (z as u32).wrapping_mul(83492791)) % 1000) as f64 / 1000.0
}
const WALL_H: [f64; 3] = [0.3, 2.5, 3.25];
const STONE: Rgb = Rgb::hex(0x4a3e66);
const NEON_M: Rgb = Rgb::hex(0xff2bd6);
/// sprite ids: 1000 + kind*8 + yaw*2 + frame body · 2000 + variant*16 + mask wall (0 stub, 1 brick, 2 pillar)
/// · 3000..3003 floor slab · 3100 decal · 4000 shard · 4001 bolt · 4002 particle · 4003 gem · 4004 beam
/// · 5000 + k prop (0/1 brazier flicker, 2 rack, 3 crate)
fn bake_id(p: &Proj, id: u32) -> Sprite {
    let cube = |r: f64, z0: f64, z1: f64, rgb: Rgb| Bx {
        lo: [-r, -r, z0],
        hi: [r, r, z1],
        mask: 31,
        rgb,
        emit: true,
    };
    match id {
        3000..=3003 => {
            let var = (id - 3000) as i32;
            tile_vox(p, 0.25, 0, |x, y, _, _| {
                let n = hash3(x + var * 7, y, var);
                let crack = var == 3 && (x == y || x + 1 == y);
                if kn_flag("TOT_FLATFLOOR") {
                    return (FLOOR, false);
                }
                (FLOOR.scale(if crack { 0.6 } else { 0.85 + 0.3 * n }), false)
            })
        }
        3100 => bake(p, &[bxe([-0.6, -0.4, 0.001], [0.5, 0.6, 0.002], 1 << 4, Rgb::hex(0x140a1e))], None),
        4000 => bake(p, &[cube(0.08, 0.2, 0.36, C_XP)], None),
        4001 => bake(p, &[cube(0.1, 0.9, 1.1, Rgb::hex(0xffffff))], None),
        4002 => bake(p, &[cube(0.05, -0.05, 0.05, Rgb::hex(0xffffff))], None),
        4003 => bake(p, &[cube(0.12, 0.05, 0.3, Rgb::hex(0xffffff))], None),
        4004 => bake(
            p,
            &[Bx {
                lo: [-0.04, -0.04, 0.3],
                hi: [0.04, 0.04, 3.0],
                mask: 15,
                rgb: Rgb::hex(0xffffff),
                emit: true,
            }],
            None,
        ),
        2000..=2047 => {
            let (var, mask) = (((id - 2000) / 16) as usize, ((id - 2000) % 16) as u8);
            tile_vox(p, WALL_H[var], mask, |x, y, z, nz| {
                let n = hash3(x, y, z);
                match var {
                    0 => (STONE.scale(0.7 + 0.2 * n), false),
                    1 if z == nz - 3 => (TRIM, true),
                    1 => {
                        // bricks 2 voxels long, rows offset; mortar darker
                        let mortar = (x + y + if z % 2 == 0 { 0 } else { 1 }) % 2 == 0 && n < 0.35;
                        (
                            STONE.scale(if mortar { 0.55 } else { 0.8 + 0.35 * n }).mix(Rgb::hex(0x6a5a9a), if z == nz - 1 { 0.5 } else { 0.0 }),
                            false,
                        )
                    }
                    _ if z % 4 == 2 => (NEON_M, true),
                    _ => (Rgb::hex(0x2a2240).scale(0.8 + 0.4 * n), false),
                }
            })
        }
        5000 | 5001 => {
            let fl = id == 5001;
            let mut v = vec![
                bxe([-0.15, -0.15, 0.0], [0.15, 0.15, 0.9], 31, Rgb::hex(0x3a3050)),
                bxe([-0.3, -0.3, 0.9], [0.3, 0.3, 1.0], 31, Rgb::hex(0x6a6080)),
            ];
            for k in 0..6 {
                let (a, z) = (k as f64 * 1.047 + if fl { 0.5 } else { 0.0 }, 1.0 + 0.12 * (k % 3) as f64 + if fl { 0.1 } else { 0.0 });
                let (x, y) = (0.12 * a.cos(), 0.12 * a.sin());
                v.push(Bx {
                    lo: [x - 0.08, y - 0.08, z],
                    hi: [x + 0.08, y + 0.08, z + 0.25],
                    mask: 31,
                    rgb: if k % 2 == 0 { NEON_M } else { Rgb::hex(0xffb0f0) },
                    emit: true,
                });
            }
            bake(p, &v, None)
        }
        5002 => {
            let mut v = vec![bxe([-0.45, -0.3, 0.0], [0.45, 0.3, 1.9], 31, Rgb::hex(0x24203a))];
            for z in 0..6 {
                v.push(Bx {
                    lo: [0.45, -0.22, 0.25 + z as f64 * 0.27],
                    hi: [0.47, 0.22, 0.3 + z as f64 * 0.27],
                    mask: 31,
                    rgb: if z % 2 == 0 { TRIM } else { Rgb::hex(0xb6ff3b) },
                    emit: true,
                });
            }
            bake(p, &v, None)
        }
        5003 => bake(
            p,
            &[
                bxe([-0.35, -0.35, 0.0], [0.35, 0.35, 0.6], 31, Rgb::hex(0x5a4430)),
                bxe([-0.36, -0.05, 0.0], [0.36, 0.05, 0.61], 31, Rgb::hex(0x3a2a20)),
            ],
            None,
        ),
        _ => bake_sdf(p, id),
    }
}
fn aq(th: f64) -> u32 {
    ((th / TAU * ANGLES).round() as i64).rem_euclid(ANGLES as i64) as u32
}

// ---------- canvas, light, blit ----------
#[derive(Clone, Copy)]
struct Px {
    near: f32,
    rgb: Rgb,
    ent: u16,
}
const EMPTY: Px = Px { near: f32::MIN, rgb: VOID, ent: 0 };
#[derive(Clone, Copy, PartialEq, Debug)]
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
    org: (i64, i64, u32), // world origin in screen CELLS + angle step (pan = terminal scroll by the delta)
}
/// probe knobs (env): TOT_NOFLICKER TOT_NOLAMP TOT_NONOVA TOT_BANDS=n
/// read once (env lookup per pixel cost 57 ms/frame [M])
fn kn_flag(k: &str) -> bool {
    static K: std::sync::OnceLock<[bool; 7]> = std::sync::OnceLock::new();
    let v = K.get_or_init(|| ["TOT_NOFLICKER", "TOT_NOLAMP", "TOT_NONOVA", "TOT_FLATFLOOR", "TOT_NOSCROLL", "TOT_NEGSCROLL", "TOT_NEGHSCROLL"].map(|n| std::env::var(n).is_ok()));
    ["TOT_NOFLICKER", "TOT_NOLAMP", "TOT_NONOVA", "TOT_FLATFLOOR", "TOT_NOSCROLL", "TOT_NEGSCROLL", "TOT_NEGHSCROLL"]
        .iter()
        .position(|n| *n == k)
        .is_some_and(|i| v[i])
}
fn kn_bands() -> f32 {
    static B: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *B.get_or_init(|| std::env::var("TOT_BANDS").ok().and_then(|v| v.parse().ok()).unwrap_or(16.0))
}
/// light per tile centre (W x W), sampled bilinear per pixel, quantized to 1/16
struct Light(Vec<[f32; 3]>);
impl Light {
    fn at(&self, x: f32, y: f32) -> [f32; 3] {
        let (fx, fy) = ((x - 0.5).clamp(0.0, W as f32 - 1.001), (y - 0.5).clamp(0.0, W as f32 - 1.001));
        let (ix, iy) = (fx as usize, fy as usize);
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let g = |x: usize, y: usize| self.0[y.min(W - 1) * W + x.min(W - 1)];
        let (a, b, c, d) = (g(ix, iy), g(ix + 1, iy), g(ix, iy + 1), g(ix + 1, iy + 1));
        let mut o = [0.0; 3];
        for k in 0..3 {
            let v = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
            o[k] = (v * kn_bands()).round() / kn_bands();
        }
        o
    }
}
/// one blit: world origin (light lookup), screen origin, tint (255 = x1), lit = apply light
struct Item {
    key: (u32, u32, Mode),
    o: (i32, i32, f32),
    wo: (f32, f32),
    tint: Rgb,
    lit: bool,
    ent: u16,
}
fn blit(buf: &mut [Px], w: usize, s0: i32, h: usize, sp: &Sprite, it: &Item, light: &Light) {
    let o = it.o;
    if o.1 + sp.rows.1 < s0 || o.1 + sp.rows.0 >= s0 + h as i32 {
        return;
    }
    let t = [it.tint.0 as f32 / 255.0, it.tint.1 as f32 / 255.0, it.tint.2 as f32 / 255.0];
    for &(dc, ds, dn, col, k, wx, wy) in &sp.px {
        let (c, s) = (o.0 + dc, o.1 + ds - s0);
        if c < 0 || s < 0 || c as usize >= w || s as usize >= h {
            continue;
        }
        let px = &mut buf[s as usize * w + c as usize];
        let n = o.2 + dn;
        if n > px.near {
            let l = if it.lit && k & 0x80 == 0 { light.at(it.wo.0 + wx, it.wo.1 + wy) } else { [1.0; 3] };
            let ch = |v: u8, i: usize| (v as f32 * t[i] * l[i]).clamp(0.0, 255.0) as u8;
            *px = Px {
                near: n,
                rgb: Rgb(ch(col.0, 0), ch(col.1, 1), ch(col.2, 2)),
                ent: it.ent,
            };
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
    rooms: Vec<(usize, usize)>,
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
    let mut rooms = vec![];
    let (lo, hi) = (room.0.min(CW as u32 - 4), room.1.min(CW as u32 - 4).max(room.0));
    let mut boxes = vec![];
    for &c in &cells {
        let (ox, oy) = ((c % G) * CW, (c / G) * CW);
        let (w, h) = ((lo + m.below(hi - lo + 1)) as usize, (lo + m.below(hi - lo + 1)) as usize);
        let (x0, y0) = (ox + (CW - w) / 2, oy + (CW - h) / 2);
        carve(&mut t, x0, y0, x0 + w, y0 + h);
        boxes.push((x0, y0, w, h, m.below(3)));
        rooms.push((x0 + w / 2, y0 + h / 2));
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
    Floor { t, start: s, props, rooms }
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
    nova_hit: bool,
    awake: bool,
}
struct Part {
    p: [f64; 3],
    v: [f64; 3],
    life: f64,
    tint: Rgb,
}
struct Loot {
    x: f64,
    y: f64,
    tier: usize,
    name: &'static str,
}
const TIER: [(&str, Rgb); 3] = [("MAGIC", Rgb::hex(0x5b8cff)), ("RARE", Rgb::hex(0xffd75e)), ("UNIQUE", Rgb::hex(0xff8a1f))];
const NAMES: [&str; 8] = ["smartgun", "optic rig", "exo-plating", "grip servos", "kinetic boots", "neural pendant", "chrome ring", "cyberdeck"];
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
    parts: Vec<Part>,
    decals: Vec<(f64, f64)>,
    loot: Vec<Loot>,
    flashes: Vec<(f64, f64, Rgb, f64, f64)>, // x y colour radius life
    props: Vec<(f64, f64, PropK)>,
    torch: Vec<Vec<(u32, f32)>>, // per brazier: lit tiles (tile, falloff), LOS-checked at load
    nova: Option<f64>,
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
impl Game {
    fn new(seed: u64, cap: usize, room: (u32, u32)) -> Game {
        let f = floorgen(seed, room);
        let t = f.t;
        let solid = |c: u8| c == ROCK;
        let wall: Vec<bool> = (0..W * W)
            .map(|c| {
                t[c] == PILLAR
                    || (solid(t[c])
                        && (-1i32..=1).any(|dy| {
                            (-1i32..=1).any(|dx| {
                                let (x, y) = ((c % W) as i32 + dx, (c / W) as i32 + dy);
                                x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < W && !solid(t[y as usize * W + x as usize])
                            })
                        }))
            })
            .collect();
        let walk_total = t.iter().filter(|&&v| v == OPEN).count();
        let torch = f
            .props
            .iter()
            .filter(|p| p.2 == PropK::Brazier)
            .map(|&(x, y, _)| {
                let c = tl(x, y);
                let mut v = vec![];
                for dy in -8i32..=8 {
                    for dx in -8i32..=8 {
                        let (tx, ty) = ((c % W) as i32 + dx, (c / W) as i32 + dy);
                        let d = ((dx * dx + dy * dy) as f32).sqrt();
                        if tx < 0 || ty < 0 || tx as usize >= W || ty as usize >= W || d > 8.0 {
                            continue;
                        }
                        let o = ty as usize * W + tx as usize;
                        if (t[o] != ROCK || wall[o]) && los_t(&t, c, o) {
                            v.push((o as u32, (1.0 - d / 8.0).powi(2)));
                        }
                    }
                }
                v
            })
            .collect();
        let mut g = Game {
            wall,
            vis: vec![false; W * W],
            explored: vec![false; W * W],
            walk_total,
            walk_seen: 0,
            dist: vec![u16::MAX; W * W],
            ring: vec![],
            occ: vec![0; W * W],
            tile_at: usize::MAX,
            px: (f.start % W) as f64 + 0.5,
            py: (f.start / W) as f64 + 0.5,
            pyaw: 0,
            pphase: 0.0,
            en: vec![],
            shards: vec![],
            bolts: vec![],
            parts: vec![],
            decals: vec![],
            loot: vec![],
            flashes: vec![],
            props: f.props,
            torch,
            nova: None,
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
            msg: ("CATHEDRAL OF THE DEAD NET - find the uplink".into(), 5.0),
            t,
        };
        // boss: farthest room centre
        let far = f.rooms.iter().max_by_key(|&&(x, y)| ((x as f64 - g.px).powi(2) + (y as f64 - g.py).powi(2)) as i64).copied();
        if let Some((x, y)) = far {
            g.en.push(En {
                x: x as f64 + 0.5,
                y: y as f64 + 0.5,
                kind: Kind::Behemoth,
                yaw: 8,
                phase: 0.0,
                hp: 4000.0,
                cd: 0.0,
                nova_hit: false,
                awake: false,
            });
        }
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
        for y in (cy - 16).max(0)..=(cy + 16).min(W as i32 - 1) {
            for x in (cx - 16).max(0)..=(cx + 16).min(W as i32 - 1) {
                let c = y as usize * W + x as usize;
                if (x - cx).pow(2) + (y - cy).pow(2) <= 256 && (self.t[c] != ROCK || self.wall[c]) && los_t(&self.t, s, c) {
                    self.vis[c] = true;
                    if !self.explored[c] {
                        self.explored[c] = true;
                        self.walk_seen += usize::from(self.t[c] == OPEN);
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
                if o < W * W && self.t[o] == OPEN && self.dist[o] == u16::MAX {
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
            self.en.retain(|e| e.kind == Kind::Behemoth);
            self.bolts.clear();
            self.msg = ("FLATLINE - rebooted (toy)".into(), 3.0);
        }
    }
    fn burst(&mut self, x: f64, y: f64, z: f64, n: usize, tint: Rgb, speed: f64) {
        for _ in 0..n {
            let (a, up) = (self.rng.f() * TAU, self.rng.f());
            let s = speed * (0.4 + 0.6 * self.rng.f());
            self.parts.push(Part {
                p: [x, y, z],
                v: [a.cos() * s, a.sin() * s, 1.5 + 3.0 * up],
                life: 0.35 + 0.4 * self.rng.f(),
                tint,
            });
        }
    }
    fn nova(&mut self) {
        if self.nova.is_none() && self.flux >= 30.0 {
            self.flux -= 30.0;
            self.nova = Some(0.0);
            for e in &mut self.en {
                e.nova_hit = false;
            }
            self.flashes.push((self.px, self.py, TRIM, 9.0, 0.45));
        }
    }
    fn kill(&mut self, i: usize) {
        let e = self.en.swap_remove(i);
        self.kills += 1;
        let hue = hue_of(e.kind);
        self.burst(e.x, e.y, 0.9, if e.kind == Kind::Behemoth { 80 } else { 16 }, hue, if e.kind == Kind::Behemoth { 7.0 } else { 4.0 });
        self.flashes.push((e.x, e.y, hue, if e.kind == Kind::Behemoth { 12.0 } else { 3.5 }, 0.25));
        self.decals.push((e.x, e.y));
        if self.decals.len() > 600 {
            self.decals.remove(0);
        }
        self.shards.push((
            e.x,
            e.y,
            match e.kind {
                Kind::Mech => 10,
                Kind::Behemoth => 200,
                _ => 1,
            },
        ));
        let r = self.rng.f();
        let tier = match e.kind {
            Kind::Behemoth => Some(2),
            Kind::Mech if r < 0.5 => Some(1),
            _ if r < 0.004 => Some(2),
            _ if r < 0.03 => Some(1),
            _ if r < 0.12 => Some(0),
            _ => None,
        };
        if let Some(tier) = tier {
            let name = NAMES[self.rng.below(NAMES.len() as u32) as usize];
            self.loot.push(Loot { x: e.x + 0.3, y: e.y, tier, name });
        }
    }
    fn step(&mut self, dt: f64, mv: (f64, f64)) {
        self.time += dt;
        let m = self.time / 60.0;
        if mv.0 != 0.0 || mv.1 != 0.0 {
            let (nx, ny) = (self.px + mv.0 * 5.0 * dt, self.py + mv.1 * 5.0 * dt);
            if self.walk(nx + mv.0.signum() * 0.3, self.py) {
                self.px = nx;
            }
            if self.walk(self.px, ny + mv.1.signum() * 0.3) {
                self.py = ny;
            }
            self.pyaw = yaw16(mv.0, mv.1);
            self.pphase = (self.pphase + dt * 2.5).fract();
        }
        self.retile();
        // spawns out of LOS (pincer rule, T5b)
        let target = self.cap.min(30 + (14.0 * m) as usize);
        self.spawn_acc += dt * (2.5 + 0.7 * m);
        while self.spawn_acc >= 1.0 && self.en.len() < target && !self.ring.is_empty() {
            self.spawn_acc -= 1.0;
            let c = self.ring[self.rng.below(self.ring.len() as u32) as usize] as usize;
            let r = self.rng.below(100);
            let kind = if r < 7 {
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
                nova_hit: false,
                awake: true,
            });
        }
        self.spawn_acc = self.spawn_acc.min(5.0);
        self.occ.fill(0);
        for e in &self.en {
            self.occ[tl(e.x, e.y)] = self.occ[tl(e.x, e.y)].saturating_add(1);
        }
        let mut dmg = 0.0;
        for i in 0..self.en.len() {
            let (x, y) = (self.en[i].x, self.en[i].y);
            let c = tl(x, y);
            if !self.en[i].awake {
                self.en[i].awake = self.vis[c];
                continue;
            }
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
            let kind = self.en[i].kind;
            let sp = match kind {
                Kind::Drone => 4.0,
                Kind::Mech => 2.4,
                Kind::Behemoth => 1.9,
                _ => 3.0,
            };
            let reach = if kind == Kind::Behemoth { 2.0 } else { 0.8 };
            let pd = ((self.px - x).powi(2) + (self.py - y).powi(2)).sqrt();
            if pd > reach {
                let (nx, ny) = (x + dx / d * sp * dt, y + dy / d * sp * dt);
                let nt = tl(nx, ny);
                if self.walk(nx, ny) && (nt == c || self.occ[nt] < 3 || kind == Kind::Behemoth) {
                    self.occ[c] = self.occ[c].saturating_sub(1);
                    self.occ[nt] = self.occ[nt].saturating_add(1);
                    let e = &mut self.en[i];
                    (e.x, e.y) = (nx, ny);
                    e.yaw = yaw16(dx, dy);
                    e.phase = (e.phase + dt * if kind == Kind::Behemoth { 1.0 } else { 2.0 }).fract();
                }
            }
            let e = &mut self.en[i];
            e.cd = (e.cd - dt).max(0.0);
            if pd < reach + 0.1 && e.cd == 0.0 {
                e.cd = if kind == Kind::Behemoth { 1.2 } else { 0.5 };
                dmg += 6.0
                    * (1.0 + 0.04 * m)
                    * match kind {
                        Kind::Mech => 2.0,
                        Kind::Behemoth => 6.0,
                        _ => 1.0,
                    };
            }
        }
        if dmg > 0.0 {
            self.hurt(dmg);
        }
        // autofire: nearest visible within 12 m
        self.fire_cd -= dt;
        if self.fire_cd <= 0.0 {
            self.fire_cd = 0.3;
            let tgt = self
                .en
                .iter()
                .filter(|e| e.awake && self.vis[tl(e.x, e.y)])
                .map(|e| ((e.x - self.px).powi(2) + (e.y - self.py).powi(2), e.x, e.y))
                .filter(|q| q.0 < 144.0)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((d2, x, y)) = tgt {
                let d = d2.sqrt().max(1e-6);
                self.bolts.push([self.px, self.py, (x - self.px) / d * 22.0, (y - self.py) / d * 22.0, 0.7]);
            }
        }
        let cc = 0.10 + 0.005 * self.rage;
        let mut dead = vec![];
        let mut keep = Vec::with_capacity(self.bolts.len());
        let mut sparks = vec![];
        for mut b in std::mem::take(&mut self.bolts) {
            b[0] += b[2] * dt;
            b[1] += b[3] * dt;
            b[4] -= dt;
            if b[4] <= 0.0 || !self.walk(b[0], b[1]) {
                sparks.push((b[0] - b[2] * dt, b[1] - b[3] * dt));
                continue;
            }
            let hr = |e: &En| if e.kind == Kind::Behemoth { 1.8 } else { 0.6 };
            if let Some(i) = self.en.iter().position(|e| (e.x - b[0]).powi(2) + (e.y - b[1]).powi(2) < hr(e) * hr(e)) {
                let crit = if self.rng.f() < cc { 2.0 } else { 1.0 };
                self.en[i].hp -= 40.0 * 1.142f64.powi(self.level as i32 - 1) * crit;
                sparks.push((b[0], b[1]));
                if self.en[i].hp <= 0.0 && !dead.contains(&i) {
                    dead.push(i);
                }
                continue;
            }
            keep.push(b);
        }
        self.bolts = keep;
        for (x, y) in sparks {
            self.burst(x, y, 1.0, 5, TRIM, 3.0);
        }
        // nova ring: 9 m in 0.45 s, 90 dmg once per enemy
        if let Some(age) = self.nova {
            let r = 9.0 * age / 0.45;
            for i in 0..self.en.len() {
                let e = &self.en[i];
                let d = ((e.x - self.px).powi(2) + (e.y - self.py).powi(2)).sqrt();
                if !e.nova_hit && d < r {
                    self.en[i].nova_hit = true;
                    self.en[i].hp -= 90.0 * 1.142f64.powi(self.level as i32 - 1);
                    if self.en[i].hp <= 0.0 && !dead.contains(&i) {
                        dead.push(i);
                    }
                }
            }
            self.nova = if age + dt > 0.45 { None } else { Some(age + dt) };
        }
        dead.sort_unstable_by(|a, b| b.cmp(a));
        for i in dead {
            self.kill(i);
        }
        // particles + flashes
        for p in &mut self.parts {
            p.v[2] -= 12.0 * dt;
            for k in 0..3 {
                p.p[k] += p.v[k] * dt;
            }
            if p.p[2] < 0.0 {
                p.p[2] = 0.0;
                p.v[2] *= -0.35;
                p.v[0] *= 0.6;
                p.v[1] *= 0.6;
            }
            p.life -= dt;
        }
        self.parts.retain(|p| p.life > 0.0);
        for f in &mut self.flashes {
            f.4 -= dt;
        }
        self.flashes.retain(|f| f.4 > 0.0);
        // pickups: shards (magnet 3 m), loot (walk over)
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
        if let Some(i) = self.loot.iter().position(|l| (l.x - px).powi(2) + (l.y - py).powi(2) < 0.5) {
            let l = self.loot.swap_remove(i);
            self.msg = (format!("picked {} {}", TIER[l.tier].0, l.name), 3.0);
        }
        self.xp += got;
        while self.xp >= need(self.level) {
            self.xp -= need(self.level);
            self.level += 1;
            self.msg = (format!("LEVEL {} - +1 tree point", self.level), 2.5);
        }
        self.hp = (self.hp + 0.2 * dt).min(PMAX);
        self.flux = (self.flux + 4.0 * dt).min(100.0);
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
const AMBIENT: [f32; 3] = [0.10, 0.09, 0.17];
const MEMORY: [f32; 3] = [0.16, 0.14, 0.24];
/// per-tile light: ambient + flickering braziers (LOS baked) + player lamp + bolts + nova + loot + flashes
fn lighting(g: &Game, out: &mut Light) {
    let add = |o: &mut [f32; 3], c: Rgb, k: f32| {
        o[0] += c.0 as f32 / 255.0 * k;
        o[1] += c.1 as f32 / 255.0 * k;
        o[2] += c.2 as f32 / 255.0 * k;
    };
    for (i, o) in out.0.iter_mut().enumerate() {
        *o = if g.vis[i] { AMBIENT } else { MEMORY };
    }
    for (k, lit) in g.torch.iter().enumerate() {
        let fl = if kn_flag("TOT_NOFLICKER") {
            1.0
        } else {
            0.85 + 0.15 * ((g.time * 9.0 + k as f64 * 1.7).sin() * (g.time * 3.1 + k as f64).cos()) as f32
        };
        for &(c, f) in lit {
            if g.vis[c as usize] {
                add(&mut out.0[c as usize], NEON_M, 0.9 * f * fl);
            }
        }
    }
    let mut point = |x: f64, y: f64, c: Rgb, r: f64, k: f32| {
        let (cx, cy, ri) = (x as i32, y as i32, r.ceil() as i32);
        for ty in (cy - ri).max(0)..=(cy + ri).min(W as i32 - 1) {
            for tx in (cx - ri).max(0)..=(cx + ri).min(W as i32 - 1) {
                let i = ty as usize * W + tx as usize;
                let d = ((tx as f64 + 0.5 - x).powi(2) + (ty as f64 + 0.5 - y).powi(2)).sqrt();
                if g.vis[i] && d < r {
                    add(&mut out.0[i], c, k * (1.0 - (d / r) as f32).powi(2));
                }
            }
        }
    };
    if !kn_flag("TOT_NOLAMP") {
        point(g.px, g.py, Rgb::hex(0xffe8d0), 11.0, 1.15);
    }
    for b in &g.bolts {
        point(b[0], b[1], TRIM, 2.5, 0.7);
    }
    for l in &g.loot {
        point(l.x, l.y, TIER[l.tier].1, 3.0, 0.9);
    }
    for f in &g.flashes {
        point(f.0, f.1, f.2, f.3, (f.4 / 0.3).min(1.0) as f32 * 1.4);
    }
    if let Some(age) = g.nova {
        let r = 9.0 * age / 0.45;
        for k in 0..24 {
            let a = k as f64 / 24.0 * TAU;
            point(g.px + r * a.cos(), g.py + r * a.sin(), TRIM, 2.0, 0.6);
        }
    }
}
fn render(g: &Game, v: &View, cols: usize, prow: usize, cv: &mut Canvas, cache: &mut Cache, light: &mut Light) {
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
    // snap world origin to whole CELLS: pan shifts whole cells (T6b)
    let (fx, fy) = (sx as f64, sy as f64);
    p.ox -= c0 - (c0 / fx).floor() * fx;
    p.oy -= s0 - (s0 / fy).floor() * fy;
    let (c1, s1, _) = p.p(0.0, 0.0, 0.0);
    cv.org = ((c1 / fx).round() as i64, (s1 / fy).round() as i64, aq(p.th));
    let a = aq(p.th);
    cv.cores.clear();
    cv.hues.clear();
    let (w, h) = (cv.w, cv.h);
    lighting(g, light);
    let (pu, pv) = p.rot(g.px, g.py);
    let cut = |x: f64, y: f64| {
        let (u, vv) = p.rot(x, y);
        let ahead = (u + vv) - (pu + pv);
        v.cut && ahead > 0.3 && ahead < 6.5 && ((u - vv) - (pu - pv)).abs() < 3.5
    };
    // wall variant: 0 stub (cutaway), 1 brick, 2 pillar; None = open floor
    let var = |x: i32, y: i32| -> Option<usize> {
        if x < 0 || y < 0 || x >= W as i32 || y >= W as i32 {
            return None;
        }
        let i = y as usize * W + x as usize;
        if !g.wall[i] {
            return None;
        }
        Some(if cut(x as f64 + 0.5, y as f64 + 0.5) {
            0
        } else if g.t[i] == PILLAR {
            2
        } else {
            1
        })
    };
    let onscreen = |o: (f64, f64, f64)| o.0 >= -80.0 && o.1 >= -160.0 && o.0 <= w as f64 + 80.0 && o.1 <= h as f64 + 200.0;
    let at = |o: (f64, f64, f64)| (o.0.round() as i32, o.1.round() as i32, o.2 as f32);
    let white = Rgb::hex(0xffffff);
    let mut scene: Vec<Item> = Vec::with_capacity(4096);
    let push = |cache: &mut Cache, list: &mut Vec<Item>, id: u32, x: f64, y: f64, tint: Rgb, lit: bool, ent: u16| -> Option<Option<(i32, i32)>> {
        let o = p.p(x, y, 0.0);
        if !onscreen(o) {
            return None;
        }
        let sp = sprite(cache, &p, a, v.mode, id);
        let oi = at(o);
        list.push(Item {
            key: (id, akey(id, a), v.mode),
            o: oi,
            wo: (x as f32, y as f32),
            tint,
            lit,
            ent,
        });
        Some(sp.core.map(|(dc, ds)| (oi.0 + dc, oi.1 + ds)))
    };
    let rad = (v.view_m * 0.9) as i32;
    let (tx0, ty0) = (g.px as i32, g.py as i32);
    for ty in (ty0 - rad).max(0)..(ty0 + rad).min(W as i32) {
        for tx in (tx0 - rad).max(0)..(tx0 + rad).min(W as i32) {
            let c = ty as usize * W + tx as usize;
            if !g.explored[c] {
                continue;
            }
            let id = match var(tx, ty) {
                None => 3000 + ((tx * 7 + ty * 13) as u32 % 11).min(3),
                Some(vr) => {
                    let nb = [(1, 0), (-1, 0), (0, 1), (0, -1)];
                    let me = WALL_H[vr];
                    let mask = nb
                        .iter()
                        .enumerate()
                        .fold(0u32, |m, (f, (dx, dy))| if var(tx + dx, ty + dy).is_none_or(|o| WALL_H[o] < me) { m | 1 << f } else { m });
                    2000 + vr as u32 * 16 + mask
                }
            };
            if g.t[c] != ROCK || g.wall[c] {
                push(cache, &mut scene, id, tx as f64, ty as f64, white, true, 0);
            }
        }
    }
    for &(x, y) in g.decals.iter().filter(|d| g.vis[tl(d.0, d.1)]) {
        push(cache, &mut scene, 3100, x, y, white, true, 0);
    }
    let fr = u32::from((g.time * 8.0) as u64 % 2 == 1);
    for &(x, y, k) in g.props.iter().filter(|q| g.explored[tl(q.0, q.1)]) {
        let id = match k {
            PropK::Brazier => 5000 + fr,
            PropK::Rack => 5002,
            PropK::Crate => 5003,
        };
        push(cache, &mut scene, id, x, y, white, true, 0);
    }
    // bodies + fx (ent ids index cv.hues)
    let mut items: Vec<Item> = Vec::with_capacity(2048);
    let mut shadows: Vec<(f64, f64, f64)> = vec![];
    let mut bodies: Vec<(f64, f64, Kind, i32, f64)> = vec![(g.px, g.py, Kind::Player, g.pyaw, g.pphase)];
    bodies.extend(g.en.iter().filter(|e| g.vis[tl(e.x, e.y)]).map(|e| (e.x, e.y, e.kind, e.yaw, e.phase)));
    for b in bodies {
        let f = (b.4 * 8.0) as u32 % 8;
        let hue = hue_of(b.2);
        let ent = cv.hues.len() as u16 + 1;
        if let Some(core) = push(
            cache,
            &mut items,
            6000 + b.2 as u32 * 128 + f * 16 + ((p.th / TAU * 16.0).round() as i32 - b.3).rem_euclid(16) as u32,
            b.0,
            b.1,
            white,
            true,
            ent,
        ) {
            cv.hues.push(hue);
            let (cx, cy, _) = p.p(b.0, b.1, 0.0);
            shadows.push((
                cx,
                cy,
                match b.2 {
                    Kind::Behemoth => 1.9,
                    Kind::Mech => 0.8,
                    _ => 0.4,
                },
            ));
            if let Some((c, s)) = core
                && c >= 0
                && s >= 0
                && (c as usize) < w
                && (s as usize) < h
            {
                cv.cores.push((c as usize, s as usize, '@', hue, ent));
            }
        }
    }
    let fxe = |cv: &mut Canvas, hue: Rgb| -> u16 {
        cv.hues.push(hue);
        cv.hues.len() as u16
    };
    for s in g.shards.iter().filter(|s| g.vis[tl(s.0, s.1)]) {
        let e = fxe(cv, C_XP);
        push(cache, &mut items, 4000, s.0, s.1, white, false, e);
    }
    for l in &g.loot {
        if g.explored[tl(l.x, l.y)] {
            let c = TIER[l.tier].1;
            let e = fxe(cv, c);
            push(cache, &mut items, 4003, l.x, l.y, c, false, e);
            let e = fxe(cv, c);
            push(cache, &mut items, 4004, l.x, l.y, c.scale(0.8), false, e);
        }
    }
    for b in &g.bolts {
        let e = fxe(cv, TRIM);
        push(cache, &mut items, 4001, b[0], b[1], TRIM, false, e);
        let e = fxe(cv, TRIM);
        push(cache, &mut items, 4002, b[0] - b[2] * 0.02, b[1] - b[3] * 0.02, TRIM.scale(0.7), false, e);
    }
    // particles: z lifts the screen origin (sprite baked at z 0)
    let zl = kc * ZK * p.sy / v.aspect;
    for q in &g.parts {
        let e = fxe(cv, q.tint);
        if push(cache, &mut items, 4002, q.p[0], q.p[1], q.tint, false, e).is_some()
            && let Some(it) = items.last_mut()
        {
            it.o.1 -= (q.p[2] * zl).round() as i32;
            it.o.2 += (q.p[2] * SE) as f32;
        }
    }
    if let Some(age) = g.nova {
        let r = 9.0 * age / 0.45;
        for k in 0..96 {
            let ang = k as f64 / 96.0 * TAU;
            let e = fxe(cv, TRIM);
            if push(cache, &mut items, 4002, g.px + r * ang.cos(), g.py + r * ang.sin(), if k % 3 == 0 { white } else { TRIM }, false, e).is_some()
                && let Some(it) = items.last_mut()
            {
                let z = 0.4 + 0.3 * ((k as f64 * 0.7 + age * 30.0).sin() + 1.0);
                it.o.1 -= (z * zl).round() as i32;
                it.o.2 += (z * SE) as f32;
            }
        }
    }
    // bands in parallel: clear, scene, shadows, bodies (cache + light read-only)
    let (cache, light): (&Cache, &Light) = (cache, light);
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
                        blit(sc, w, s0, bh, sp, it, light);
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
                                px.rgb = px.rgb.scale(0.45);
                            }
                        }
                    }
                }
                for it in items {
                    if let Some(sp) = cache.get(&it.key) {
                        blit(bd, w, s0, bh, sp, it, light);
                    }
                }
            });
        }
    });
}
/// body sprites (id >= 6000) depend only on facing relative to the camera: one shared slot for all camera angles
fn akey(id: u32, a: u32) -> u32 {
    if id >= 6000 { 0 } else { a }
}
fn sprite<'c>(cache: &'c mut Cache, p: &Proj, a: u32, mode: Mode, id: u32) -> &'c Sprite {
    cache.entry((id, akey(id, a), mode)).or_insert_with(|| bake_id(p, id))
}

// ---------- T11: smooth pre-rendered bodies (SDF ray-march at bake time; vic: "not squared bodies", "Diablo cyberpunk") ----------
type V3 = [f64; 3];
fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot3(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len3(a: V3) -> f64 {
    dot3(a, a).sqrt()
}
fn norm3(a: V3) -> V3 {
    let l = len3(a).max(1e-9);
    [a[0] / l, a[1] / l, a[2] / l]
}
fn sph(p: V3, c: V3, r: f64) -> f64 {
    len3(sub3(p, c)) - r
}
/// capsule a-b, radius ra at a, rb at b (tapered)
fn cap(p: V3, a: V3, b: V3, ra: f64, rb: f64) -> f64 {
    let (pa, ba) = (sub3(p, a), sub3(b, a));
    let h = (dot3(pa, ba) / dot3(ba, ba)).clamp(0.0, 1.0);
    len3(sub3(pa, [ba[0] * h, ba[1] * h, ba[2] * h])) - (ra + (rb - ra) * h)
}
/// rounded box centre c, half size e, rounding r
fn rbox(p: V3, c: V3, e: V3, r: f64) -> f64 {
    let q = [(p[0] - c[0]).abs() - e[0], (p[1] - c[1]).abs() - e[1], (p[2] - c[2]).abs() - e[2]];
    len3([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)]) + q[0].max(q[1]).max(q[2]).min(0.0) - r
}
/// ellipsoid (approx, Quilez)
fn ellip(p: V3, c: V3, r: V3) -> f64 {
    let q = sub3(p, c);
    let k0 = len3([q[0] / r[0], q[1] / r[1], q[2] / r[2]]);
    let k1 = len3([q[0] / (r[0] * r[0]), q[1] / (r[1] * r[1]), q[2] / (r[2] * r[2])]);
    k0 * (k0 - 1.0) / k1.max(1e-9)
}
/// materials: (base colour, metal 0..1, emissive)
#[derive(Clone, Copy, PartialEq)]
enum Sm {
    Armor,
    Trim,
    Under,
    Cloth,
    Skin,
    Glow,
    Glow2,
}
fn sm_col(m: Sm, hue: Rgb) -> (Rgb, f64, bool) {
    match m {
        Sm::Armor => (Rgb::hex(0x8a8fa8), 0.9, false),
        Sm::Trim => (Rgb::hex(0xc9a24a), 1.0, false),
        Sm::Under => (Rgb::hex(0x2a2636), 0.3, false),
        Sm::Cloth => (hue.scale(0.55).mix(Rgb::hex(0x401020), 0.5), 0.0, false),
        Sm::Skin => (Rgb::hex(0x5a5068), 0.5, false),
        Sm::Glow => (hue, 0.0, true),
        Sm::Glow2 => (Rgb::hex(0xff3860), 0.0, true),
    }
}
/// scene accumulator: nearest surface + its material
struct Sd(f64, Sm);
impl Sd {
    fn add(&mut self, d: f64, m: Sm) {
        if d < self.0 {
            *self = Sd(d, m);
        }
    }
}
/// walk cycle angle (rad) for frame 0..8
fn swing(frame: u32, off: f64) -> f64 {
    (frame as f64 / 8.0 * TAU + off).sin()
}
/// body SDF in local metres, forward +x, z up
fn body(kind: Kind, f: u32, p: V3) -> Sd {
    let mut s = Sd(f64::MAX, Sm::Under);
    let leg = |s: &mut Sd, side: f64, hip: V3, len: f64, ph: f64, r: f64, m: Sm, boot: Sm| {
        let a = 0.45 * swing(f, ph);
        let knee = [hip[0] + len * 0.5 * a.sin(), hip[1] + side, hip[2] - len * 0.5 * a.cos()];
        let b = (a - 0.35 * (1.0 + swing(f, ph + 1.2)).max(0.0)).min(a);
        let foot = [knee[0] + len * 0.5 * b.sin(), knee[1], (knee[2] - len * 0.5 * b.cos()).max(0.06)];
        s.add(cap(p, [hip[0], hip[1] + side, hip[2]], knee, r, r * 0.85), m);
        s.add(sph(p, knee, r * 1.05), Sm::Trim);
        s.add(cap(p, knee, foot, r * 0.85, r * 0.95), boot);
        s.add(rbox(p, [foot[0] + 0.05, foot[1], 0.05], [0.09, r * 0.8, 0.04], 0.02), boot);
    };
    match kind {
        Kind::Player => {
            // cyber-paladin: helm + visor, pauldrons, cuirass + core, belt, gauntlets, boots, cape, rifle
            let bob = 0.02 * swing(f, 0.0).abs();
            for (side, ph) in [(0.11, 0.0), (-0.11, std::f64::consts::PI)] {
                leg(&mut s, side, [0.0, 0.0, 0.95], 0.88, ph, 0.075, Sm::Under, Sm::Armor);
            }
            s.add(rbox(p, [0.0, 0.0, 0.98 + bob], [0.12, 0.17, 0.07], 0.04), Sm::Under);
            s.add(rbox(p, [0.0, 0.0, 1.03 + bob], [0.13, 0.19, 0.025], 0.01), Sm::Trim);
            s.add(rbox(p, [0.0, 0.0, 1.27 + bob], [0.12, 0.2, 0.2], 0.07), Sm::Armor);
            s.add(rbox(p, [0.11, 0.0, 1.3 + bob], [0.04, 0.12, 0.13], 0.04), Sm::Armor);
            s.add(sph(p, [0.17, 0.0, 1.33 + bob], 0.045), Sm::Glow);
            for side in [-1.0, 1.0] {
                s.add(ellip(p, [0.0, 0.25 * side, 1.47 + bob], [0.15, 0.12, 0.09]), Sm::Armor);
                s.add(ellip(p, [0.0, 0.27 * side, 1.5 + bob], [0.12, 0.09, 0.05]), Sm::Trim);
                let a = 0.4 * swing(f, if side > 0.0 { std::f64::consts::PI } else { 0.0 });
                let sh = [0.0, 0.27 * side, 1.42 + bob];
                let el = [sh[0] + 0.28 * a.sin(), sh[1] + 0.04 * side, sh[2] - 0.28 * a.cos()];
                s.add(cap(p, sh, el, 0.06, 0.055), Sm::Under);
                if side > 0.0 {
                    // gun arm: forearm forward, rifle
                    let hand = [el[0] + 0.25, el[1] - 0.05, el[2] + 0.05];
                    s.add(cap(p, el, hand, 0.065, 0.07), Sm::Armor);
                    s.add(cap(p, [hand[0] - 0.15, hand[1], hand[2] + 0.04], [hand[0] + 0.55, hand[1], hand[2] + 0.07], 0.035, 0.03), Sm::Under);
                    s.add(rbox(p, [hand[0] + 0.1, hand[1], hand[2] + 0.08], [0.18, 0.025, 0.035], 0.01), Sm::Armor);
                    s.add(sph(p, [hand[0] + 0.58, hand[1], hand[2] + 0.07], 0.028), Sm::Glow);
                } else {
                    let hand = [el[0] + 0.24 * a.sin(), el[1], el[2] - 0.24 * a.cos()];
                    s.add(cap(p, el, hand, 0.065, 0.07), Sm::Armor);
                }
            }
            s.add(sph(p, [0.02, 0.0, 1.68 + bob], 0.12), Sm::Armor);
            s.add(rbox(p, [0.03, 0.0, 1.79 + bob], [0.09, 0.02, 0.03], 0.01), Sm::Trim);
            s.add(rbox(p, [0.12, 0.0, 1.69 + bob], [0.015, 0.085, 0.018], 0.005), Sm::Glow);
            let sway = 0.05 * swing(f, 0.6);
            s.add(rbox(p, [-0.2 - sway, 0.0, 1.05], [0.02, 0.2 + 0.02 * (1.45 - p[2]).max(0.0), 0.42], 0.015), Sm::Cloth);
        }
        Kind::Stalker => {
            // cyber-ghoul: hunched spine, skull with twin eyes, long blade arms, digitigrade legs
            for (side, ph) in [(0.1, 0.0), (-0.1, std::f64::consts::PI)] {
                leg(&mut s, side, [-0.05, 0.0, 0.8], 0.78, ph, 0.055, Sm::Skin, Sm::Under);
            }
            for k in 0..6 {
                let t = k as f64 / 5.0;
                s.add(sph(p, [-0.05 + 0.3 * t, 0.0, 0.85 + 0.5 * t - 0.25 * t * t], 0.12 - 0.03 * t), Sm::Skin);
                s.add(rbox(p, [-0.1 + 0.3 * t, 0.0, 0.95 + 0.5 * t - 0.25 * t * t], [0.02, 0.02, 0.04], 0.01), Sm::Glow);
            }
            s.add(ellip(p, [0.38, 0.0, 1.18], [0.13, 0.09, 0.1]), Sm::Armor);
            for side in [-1.0, 1.0] {
                s.add(sph(p, [0.48, 0.035 * side, 1.2], 0.022), Sm::Glow2);
                let a = 0.6 * swing(f, if side > 0.0 { 0.0 } else { std::f64::consts::PI });
                let sh = [0.2, 0.15 * side, 1.2];
                let el = [0.35 + 0.15 * a, 0.25 * side, 0.95];
                s.add(cap(p, sh, el, 0.045, 0.035), Sm::Skin);
                s.add(cap(p, el, [0.75 + 0.15 * a, 0.28 * side, 0.6], 0.03, 0.005), Sm::Armor);
            }
        }
        Kind::Drone => {
            // sentinel: armored orb, rotating halo ring, eye, dangling cable tentacles
            let c = [0.0, 0.0, 1.2 + 0.05 * swing(f, 0.0)];
            s.add(sph(p, c, 0.22), Sm::Armor);
            s.add(rbox(p, c, [0.23, 0.23, 0.03], 0.0), Sm::Trim);
            let q = sub3(p, c);
            let ring = (((q[0] * q[0] + q[1] * q[1]).sqrt() - 0.36).powi(2) + q[2] * q[2]).sqrt() - 0.025;
            s.add(ring, Sm::Under);
            s.add(sph(p, [c[0] + 0.19, 0.0, c[2] + 0.02], 0.065), Sm::Glow);
            for k in 0..3 {
                let a = k as f64 / 3.0 * TAU;
                let mut prev = [c[0] + 0.12 * a.cos(), c[1] + 0.12 * a.sin(), c[2] - 0.15];
                for j in 1..5 {
                    let w = 0.06 * swing(f, j as f64 + a);
                    let nx = [prev[0] + w, prev[1] + w * 0.5, prev[2] - 0.17];
                    s.add(cap(p, prev, nx, 0.022, 0.018), Sm::Under);
                    prev = nx;
                }
                s.add(sph(p, prev, 0.025), Sm::Glow);
            }
        }
        Kind::Mech => {
            // brute: massive torso, armored legs, arm cannon, shoulder vents
            for (side, ph) in [(0.22, 0.0), (-0.22, std::f64::consts::PI)] {
                leg(&mut s, side, [0.0, 0.0, 1.1], 1.0, ph, 0.12, Sm::Under, Sm::Armor);
            }
            s.add(rbox(p, [0.0, 0.0, 1.55], [0.3, 0.38, 0.38], 0.12), Sm::Armor);
            s.add(rbox(p, [0.25, 0.0, 1.6], [0.08, 0.22, 0.22], 0.05), Sm::Trim);
            s.add(sph(p, [0.33, 0.0, 1.62], 0.07), Sm::Glow);
            s.add(ellip(p, [0.1, 0.0, 2.0], [0.15, 0.14, 0.12]), Sm::Armor);
            s.add(rbox(p, [0.22, 0.0, 2.0], [0.02, 0.1, 0.02], 0.01), Sm::Glow2);
            for side in [-1.0, 1.0] {
                s.add(ellip(p, [0.0, 0.48 * side, 1.85], [0.24, 0.2, 0.16]), Sm::Armor);
                for v in 0..3 {
                    s.add(rbox(p, [-0.12 + 0.1 * v as f64, 0.48 * side, 1.99], [0.025, 0.12, 0.015], 0.0), Sm::Glow);
                }
            }
            s.add(cap(p, [0.0, 0.5, 1.55], [0.75, 0.5, 1.45], 0.13, 0.11), Sm::Under);
            s.add(cap(p, [0.75, 0.5, 1.45], [0.85, 0.5, 1.44], 0.08, 0.08), Sm::Glow);
            s.add(cap(p, [0.0, -0.5, 1.6], [0.35, -0.55, 1.05], 0.11, 0.13), Sm::Armor);
        }
        Kind::Behemoth => {
            // siege-crawler: armored carapace, 4 jointed legs, sensor crown, eye cluster
            s.add(ellip(p, [0.0, 0.0, 1.6], [1.3, 1.0, 0.7]), Sm::Armor);
            s.add(ellip(p, [-0.1, 0.0, 2.05], [0.9, 0.6, 0.35]), Sm::Trim);
            s.add(ellip(p, [1.15, 0.0, 1.5], [0.45, 0.45, 0.4]), Sm::Under);
            for k in 0..5 {
                let y = -0.24 + 0.12 * k as f64;
                s.add(sph(p, [1.52, y, 1.6 + 0.06 * (k as f64 - 2.0).abs()], 0.06), Sm::Glow2);
            }
            for k in 0..4 {
                s.add(cap(p, [-0.5 + 0.3 * k as f64, 0.0, 2.3], [-0.5 + 0.3 * k as f64, 0.0, 2.7], 0.05, 0.01), Sm::Glow);
            }
            for (k, (fx, side)) in [(0.8, 1.0), (0.8, -1.0), (-0.8, 1.0), (-0.8, -1.0)].iter().enumerate() {
                let a = 0.35 * swing(f, k as f64 * 1.6);
                let hip = [*fx, 0.8 * side, 1.5];
                let knee = [fx + 0.4 * a, 1.6 * side, 2.0];
                let foot = [fx * 1.3 + 0.3 * a, 2.0 * side, 0.05];
                s.add(cap(p, hip, knee, 0.16, 0.12), Sm::Armor);
                s.add(sph(p, knee, 0.14), Sm::Trim);
                s.add(cap(p, knee, foot, 0.12, 0.05), Sm::Under);
            }
        }
    }
    s
}
fn hue_of(kind: Kind) -> Rgb {
    match kind {
        Kind::Player => Rgb::hex(0x19f3ff),
        Kind::Stalker => Rgb::hex(0xb6ff3b),
        Kind::Drone => Rgb::hex(0x19f3ff),
        Kind::Mech => Rgb::hex(0xff2bd6),
        Kind::Behemoth => Rgb::hex(0x7a2cff),
    }
}
fn extent(kind: Kind) -> (f64, f64) {
    match kind {
        Kind::Behemoth => (2.6, 2.9),
        Kind::Mech => (1.0, 2.2),
        _ => (0.9, 1.95),
    }
}
/// sprite id 6000 + kind*128 + frame*16 + yaw16; ray-march every subpixel, shade, store
fn bake_sdf(p: &Proj, id: u32) -> Sprite {
    let k = (id - 6000) as usize;
    let (kind, f, yaw) = (KINDS[k / 128], ((k % 128) / 16) as u32, (k % 16) as f64 / 16.0 * TAU);
    let th = yaw; // id carries facing RELATIVE to the camera (16 steps)
    let (s, c) = th.sin_cos();
    let (r, zmax) = extent(kind);
    let o = Proj {
        th,
        cx: 0.0,
        cy: 0.0,
        ox: 0.0,
        oy: 0.0,
        ..*p
    };
    // camera basis in local coords: to-camera V, key light L (upper screen-left, toward camera)
    let uv2xy = |u: f64, v: f64| [c * u + s * v, -s * u + c * v];
    let vv = {
        let a = uv2xy(CE * FRAC_1_SQRT_2, CE * FRAC_1_SQRT_2);
        [a[0], a[1], SE]
    };
    let lt = {
        let a = uv2xy(-0.55 + 0.35, 0.55 + 0.35);
        norm3([a[0], a[1], 0.85])
    };
    let hue = hue_of(kind);
    // screen bbox from the local AABB
    let (mut c0, mut c1, mut s0, mut s1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for x in [-r, r] {
        for y in [-r, r] {
            for z in [0.0, zmax] {
                let (a, b, _) = o.p(x, y, z);
                (c0, c1, s0, s1) = (c0.min(a), c1.max(a), s0.min(b), s1.max(b));
            }
        }
    }
    let mut px = vec![];
    for sy in s0.floor() as i32..=s1.ceil() as i32 {
        for sx in c0.floor() as i32..=c1.ceil() as i32 {
            let a = (sx as f64 + 0.5) / (p.kc * p.sx);
            let b = (sy as f64 + 0.5) * p.aspect / (p.kc * p.sy);
            // ray: u - v = a, (u+v)/2 - ZK z = b; march z from zmax down (away from camera), 2 m of ray per 1 m of z
            let at = |z: f64| {
                let su = 2.0 * (b + ZK * z);
                let (u, v) = ((su + a) / 2.0, (su - a) / 2.0);
                let xy = uv2xy(u, v);
                ([xy[0], xy[1], z], (u + v) * CE * FRAC_1_SQRT_2 + z * SE)
            };
            let mut z = zmax + 0.1;
            let mut hit = None;
            for _ in 0..96 {
                let (q, _) = at(z);
                if q[0].abs() > r + 0.3 && q[1].abs() > r + 0.3 && z < 0.0 {
                    break;
                }
                let d = body(kind, f, q).0;
                if d < 0.004 {
                    hit = Some(z);
                    break;
                }
                z -= (d * 0.5).max(0.004);
                if z < -0.02 {
                    break;
                }
            }
            let Some(z) = hit else { continue };
            let (q, near) = at(z);
            let sd = body(kind, f, q);
            let e = 0.004;
            let g = |dx: f64, dy: f64, dz: f64| body(kind, f, [q[0] + dx, q[1] + dy, q[2] + dz]).0;
            let n = norm3([g(e, 0.0, 0.0) - g(-e, 0.0, 0.0), g(0.0, e, 0.0) - g(0.0, -e, 0.0), g(0.0, 0.0, e) - g(0.0, 0.0, -e)]);
            let (base, metal, glow) = sm_col(sd.1, hue);
            let col = if glow {
                base
            } else {
                let diff = dot3(n, lt).max(0.0);
                let ndv = dot3(n, vv).max(0.0);
                let h = norm3([lt[0] + vv[0], lt[1] + vv[1], lt[2] + vv[2]]);
                let spec = dot3(n, h).max(0.0).powf(24.0) * metal;
                let rim = (1.0 - ndv).powf(3.0) * 0.8;
                let lit = 0.22 + 0.9 * diff;
                let mut out = base.scale(lit).mix(Rgb::hex(0xffffff), (spec * 0.8).min(1.0));
                out = out.mix(hue, (rim * 0.6).min(0.7));
                out
            };
            px.push((sx, sy, near as f32, col, if glow { 0x80 } else { 0 }, q[0] as f32, q[1] as f32));
        }
    }
    let rows = px.iter().fold((i32::MAX, i32::MIN), |(a, b), q| (a.min(q.1), b.max(q.1)));
    Sprite { px, core: None, rows }
}
fn yaw16(dx: f64, dy: f64) -> i32 {
    ((dy.atan2(dx) / TAU * 16.0).round() as i32).rem_euclid(16)
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
        o.push(if (dr < 0) != kn_flag("TOT_NEGSCROLL") { b'S' } else { b'T' });
        o.extend_from_slice(b"\x1b[r");
    }
    if dc != 0 {
        for r in 0..prow {
            o.extend_from_slice(b"\x1b[");
            num(o, r + 1);
            o.extend_from_slice(b";1H\x1b[");
            num(o, dc.unsigned_abs() as usize);
            o.push(if (dc < 0) != kn_flag("TOT_NEGHSCROLL") { b'P' } else { b'@' });
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
    room: (u32, u32),
}
fn opts(a: &[String]) -> Res<Opts> {
    let mut o = Opts {
        seed: 7,
        view_m: 40.0,
        cap: 300,
        room: (10, 20),
    };
    for k in (0..a.len()).step_by(2) {
        let v = a.get(k + 1).ok_or(format!("missing value for {}", a[k]))?;
        let n = v.parse::<f64>().map_err(|e| format!("{}: {e}", a[k]))?;
        match a[k].as_str() {
            "--seed" => o.seed = n as u64,
            "--view-m" => o.view_m = n,
            "--cap" => o.cap = n as usize,
            "--room-min" => o.room.0 = n as u32,
            "--room-max" => o.room.1 = n as u32,
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
    light: Light,
    last_org: (i64, i64, u32),
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
                org: (0, 0, u32::MAX),
            },
            cache: HashMap::new(),
            cur: vec![BLANK; cols * rows],
            prev: vec![BLANK; cols * rows],
            out: Vec::with_capacity(1 << 21),
            light: Light(vec![[0.0; 3]; W * W]),
            last_org: (0, 0, u32::MAX),
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
                org: (0, 0, u32::MAX),
            };
        }
        if !self.cache.contains_key(&(6000, 0, v.mode)) {
            prebake_bodies(&mut self.cache, &body_proj(v, cols), v.mode);
        }
        render(g, v, cols, prow, &mut self.cv, &mut self.cache, &mut self.light);
        let t1 = Instant::now();
        compose(&self.cv, v.mode, v.rim, cols, prow, &mut self.cur[..cols * prow]);
        let t2 = Instant::now();
        ui(g, v, &mut self.cur, cols, rows, leader, perf);
        let t3 = Instant::now();
        let us = t.elapsed().as_micros();
        self.out.clear();
        let (o0, o1) = (self.last_org, self.cv.org);
        self.last_org = o1;
        let pan = (o1.0 - o0.0, o1.1 - o0.1);
        let small = o0.2 == o1.2 && pan.0.unsigned_abs() < cols as u64 / 3 && pan.1.unsigned_abs() < prow as u64 / 3;
        if !force && small && pan != (0, 0) && !kn_flag("TOT_NOSCROLL") {
            if std::env::var("TOT_DBG").is_ok() {
                eprintln!("scroll pan {pan:?} neg {}", kn_flag("TOT_NEGSCROLL"));
            }
            scroll(&mut self.prev, cols, prow, pan, &mut self.out);
        }
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
    let mut g = Game::new(o.seed, o.cap, o.room);
    let tiny = cols >= 200; // font <= 4: SEXT world + pixel-font UI (scale 2 at font 2, 1 at font 4)
    let mut v = View {
        mode: if tiny { Mode::Sext } else { Mode::Half },
        view_m: o.view_m,
        aspect: 2.0,
        th: 0.0,
        rim: true,
        cut: true,
        map: false,
        ui_scale: if cols >= 400 {
            2
        } else if tiny {
            1
        } else {
            0
        },
    };
    let _ = std::fs::write("showcase-perf.log", "");
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
                    // arrows: ESC [ A up, B down, C right, D left (vic: "allow moving with arrows aditionally")
                    if let Some(k) = ["\x1b[A", "\x1b[D", "\x1b[B", "\x1b[C"].iter().position(|a| *a == s) {
                        held[k] = 0.15;
                    }
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
                b'f' => g.nova(),
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
                b'+' | b'=' | b'-' => {
                    v.view_m = if c == b'-' { (v.view_m / 0.85).min(100.0) } else { (v.view_m * 0.85).max(6.0) };
                    sc.cache.clear(); // sprites are baked at a scale (kc): zoom = rebake
                }
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
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("showcase-perf.log") {
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
            let mut g = Game::new(o.seed, o.cap, o.room);
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
                if f % 120 == 60 && !kn_flag("TOT_NONOVA") {
                    g.nova();
                }
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
    if a.get(1).map(String::as_str) == Some("sprite") {
        let n = |i: usize| a.get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
        spritedump(n(2), n(3) as u32, n(4) as u32);
        return;
    }
    if a.get(1).map(String::as_str) == Some("vtcheck") {
        if let Err(e) = opts(&a[2..]).and_then(vtcheck) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    let shot_mode = a.get(1).map(String::as_str) == Some("shot");
    let rest = &a[if shot_mode { 2 } else { 1 }..];
    let r = opts(rest).and_then(|o| if shot_mode { shot(o) } else { play(o).map(|perf| println!("last perf: {perf}")) });
    if let Err(e) = r {
        eprintln!("walk: {e}");
        std::process::exit(1);
    }
}

/// O-scroll oracle: minimal VT model (CUP, SGR truecolor, DECSTBM, SU/SD, ICH/DCH, UTF-8 chars) replays the
/// emitted bytes; after each frame the model grid must equal the intended cells (ch, bg; fg where ch != ' ')
struct Vt {
    w: usize,
    h: usize,
    g: Vec<Cell>,
    r: usize,
    c: usize,
    fg: Rgb,
    bg: Rgb,
    top: usize,
    bot: usize,
}
impl Vt {
    fn new(w: usize, h: usize) -> Vt {
        Vt {
            w,
            h,
            g: vec![BLANK; w * h],
            r: 0,
            c: 0,
            fg: VOID,
            bg: VOID,
            top: 0,
            bot: h - 1,
        }
    }
    fn blank(&self) -> Cell {
        Cell { ch: ' ', fg: self.fg, bg: self.bg }
    }
    fn feed(&mut self, b: &[u8]) {
        let s = String::from_utf8_lossy(b);
        let mut it = s.chars().peekable();
        while let Some(ch) = it.next() {
            if ch != '\x1b' {
                if self.c < self.w && self.r < self.h {
                    self.g[self.r * self.w + self.c] = Cell { ch, fg: self.fg, bg: self.bg };
                }
                self.c = (self.c + 1).min(self.w);
                continue;
            }
            if it.next() != Some('[') {
                continue;
            }
            let mut par = String::new();
            let fin = loop {
                match it.next() {
                    Some(x) if ('\x40'..='\x7e').contains(&x) => break x,
                    Some(x) => par.push(x),
                    None => return,
                }
            };
            if par.starts_with('?') {
                continue;
            }
            let n: Vec<usize> = par.split(';').map(|x| x.parse().unwrap_or(0)).collect();
            let n0 = n.first().copied().filter(|&v| v > 0).unwrap_or(1);
            match fin {
                'H' => {
                    self.r = n0 - 1;
                    self.c = n.get(1).copied().filter(|&v| v > 0).unwrap_or(1) - 1;
                }
                'm' => {
                    let mut i = 0;
                    while i < n.len() {
                        match n[i] {
                            0 => (self.fg, self.bg) = (VOID, VOID),
                            38 | 48 if i + 4 < n.len() => {
                                let c = Rgb(n[i + 2] as u8, n[i + 3] as u8, n[i + 4] as u8);
                                if n[i] == 38 {
                                    self.fg = c
                                } else {
                                    self.bg = c
                                }
                                i += 4;
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                'r' => {
                    (self.top, self.bot) = if par.is_empty() { (0, self.h - 1) } else { (n0 - 1, n.get(1).copied().unwrap_or(self.h) - 1) };
                    (self.r, self.c) = (0, 0);
                }
                'S' | 'T' => {
                    let bl = self.blank();
                    for _ in 0..n0 {
                        if fin == 'S' {
                            for y in self.top..self.bot {
                                for x in 0..self.w {
                                    self.g[y * self.w + x] = self.g[(y + 1) * self.w + x];
                                }
                            }
                            for x in 0..self.w {
                                self.g[self.bot * self.w + x] = bl;
                            }
                        } else {
                            for y in (self.top + 1..=self.bot).rev() {
                                for x in 0..self.w {
                                    self.g[y * self.w + x] = self.g[(y - 1) * self.w + x];
                                }
                            }
                            for x in 0..self.w {
                                self.g[self.top * self.w + x] = bl;
                            }
                        }
                    }
                }
                'P' | '@' => {
                    let (bl, row) = (self.blank(), self.r * self.w);
                    let line: Vec<Cell> = self.g[row + self.c..row + self.w].to_vec();
                    let k = n0.min(line.len());
                    for (x, cell) in self.g[row + self.c..row + self.w].iter_mut().enumerate() {
                        *cell = if fin == 'P' {
                            line.get(x + k).copied().unwrap_or(bl)
                        } else if x < k {
                            bl
                        } else {
                            line[x - k]
                        };
                    }
                }
                _ => {}
            }
        }
    }
    /// first mismatch vs intended cells (fg ignored on blanks)
    fn diff(&self, want: &[Cell]) -> Option<usize> {
        (0..want.len()).find(|&i| {
            let (a, b) = (self.g[i], want[i]);
            a.ch != b.ch || a.bg != b.bg || (b.ch != ' ' && a.fg != b.fg)
        })
    }
}
fn vtcheck(o: Opts) -> Res<()> {
    let (cols, rows) = (240usize, 90usize);
    let mut g = Game::new(o.seed, o.cap, o.room);
    let v = View {
        mode: Mode::Sext,
        view_m: o.view_m,
        aspect: 2.33,
        th: 0.0,
        rim: true,
        cut: true,
        map: false,
        ui_scale: 2,
    };
    let mut sc = Screen::new(cols, rows);
    let mut vt = Vt::new(cols, rows);
    let (mut scrolled, mut hz, mut vt_) = (0, 0, 0);
    for f in 0..600 {
        let a = f as f64 / 150.0;
        g.step(1.0 / 60.0, (a.cos(), a.sin()));
        let before = sc.last_org;
        sc.frame(&g, &v, f == 0, false, "");
        scrolled += usize::from(before != sc.last_org && f > 0);
        hz += usize::from(before.0 != sc.last_org.0 && f > 0);
        vt_ += usize::from(before.1 != sc.last_org.1 && f > 0);
        vt.feed(&sc.out);
        if std::env::var("TOT_DBG").is_ok() && before.1 != sc.last_org.1 && f > 0 {
            let s = String::from_utf8_lossy(&sc.out);
            eprintln!(
                "vpan f{f} bytes {} has S {} has T {} cells-differ {}",
                sc.out.len(),
                s.contains("S\x1b[r"),
                s.contains("T\x1b[r"),
                vt.diff(&sc.prev).map_or(-1, |i| i as i64)
            );
        }
        if let Some(i) = vt.diff(&sc.prev) {
            return Err(format!("O-scroll FAIL frame {f} cell {} (r {} c {}): vt {:?} want {:?}", i, i / cols, i % cols, vt.g[i], sc.prev[i]));
        }
    }
    println!("O-scroll PASS: 600 frames, {scrolled} pan frames ({hz} horizontal, {vt_} vertical), VT model == intended cells every frame");
    Ok(())
}
/// dev: print one baked body sprite as luma ASCII (2 chars per subpixel) + bake time
fn spritedump(kind: usize, yaw: u32, frame: u32) {
    let p = Proj {
        th: 0.0,
        kc: 249.0 / (SQRT_2 * 12.0),
        aspect: 2.5,
        cx: 0.0,
        cy: 0.0,
        ox: 0.0,
        oy: 0.0,
        sx: 2.0,
        sy: 3.0,
    };
    let t = Instant::now();
    let sp = bake_sdf(&p, 6000 + kind as u32 * 128 + frame * 16 + yaw);
    let us = t.elapsed().as_micros();
    let (c0, c1) = sp.px.iter().fold((i32::MAX, i32::MIN), |(a, b), q| (a.min(q.0), b.max(q.0)));
    let ramp: Vec<char> = " .:-=+*#%@".chars().collect();
    println!(
        "kind {kind} yaw {yaw} frame {frame}: {} px, {}x{} subpx, bake {us} us",
        sp.px.len(),
        c1 - c0 + 1,
        sp.rows.1 - sp.rows.0 + 1
    );
    for s in sp.rows.0..=sp.rows.1 {
        let line: String = (c0..=c1)
            .map(|c| {
                sp.px.iter().find(|q| q.0 == c && q.1 == s).map_or(' ', |q| {
                    if q.4 & 0x80 != 0 {
                        'O'
                    } else {
                        ramp[((0.2126 * q.3.0 as f64 + 0.7152 * q.3.1 as f64 + 0.0722 * q.3.2 as f64) / 255.0 * 9.0).round() as usize]
                    }
                })
            })
            .flat_map(|ch| [ch, ch])
            .collect();
        println!("{line}");
    }
}
/// projection params bodies are baked with (scale + cell mode + aspect; angle irrelevant)
fn body_proj(v: &View, cols: usize) -> Proj {
    let (sx, sy) = v.mode.sub();
    Proj {
        th: 0.0,
        kc: cols as f64 / (SQRT_2 * v.view_m),
        aspect: v.aspect,
        cx: 0.0,
        cy: 0.0,
        ox: 0.0,
        oy: 0.0,
        sx: sx as f64,
        sy: sy as f64,
    }
}
/// bake all 5 kinds x 8 frames x 16 relative facings in parallel (~20 ms each single-thread)
fn prebake_bodies(cache: &mut Cache, p: &Proj, mode: Mode) {
    let t = Instant::now();
    let ids: Vec<u32> = (0..KINDS.len() as u32 * 128).map(|k| 6000 + k).collect();
    let nt = std::thread::available_parallelism().map_or(1, |n| n.get());
    let out: Vec<(u32, Sprite)> = std::thread::scope(|s| {
        let hs: Vec<_> = ids
            .chunks(ids.len().div_ceil(nt))
            .map(|ch| s.spawn(move || ch.iter().map(|&id| (id, bake_sdf(p, id))).collect::<Vec<_>>()))
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    for (id, sp) in out {
        cache.insert((id, 0, mode), sp);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("showcase-perf.log") {
        let _ = writeln!(f, "prebake bodies {:?}: {} ms", mode, t.elapsed().as_millis());
    }
}
