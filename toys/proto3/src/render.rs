//! Pure render: sim state -> cell grid -> diff bytes (design sec 5). One buffer per frame, diff
//! against the front buffer, hand decimal writer (no core::fmt in the frame path, PF sec 1c).
use crate::data::{Content, Glyph, Hue, Rgb};
use crate::input::{Mode, Ui};
use crate::floorgen::{Arena, Template};
use crate::cam::Cam;
use crate::iso::Proj;
use crate::sim::{BARK_TICKS, BarkKind, CONF_PANIC, FxShape, Shape, Side, Sim, TeleKind, Tier, dist2};
use std::collections::HashSet;

/// Map box (top-left of the play area): cols x rows of the map.
const MAP_W: u16 = 26;
const MAP_H: u16 = 11;
/// Map cell = this many floor cells. shortcut: fixed zoom, upgrade when zoom is a player option.
const MAP_K: i32 = 4;
/// Stats panel (bottom-right of the play area).
const PANEL_W: u16 = 36;
const PANEL_H: u16 = 18;
/// Stats panel: power rows shown, first row of the foe list, XP bar width.
const PANEL_POWERS: usize = 5;
const MORALE_BAR_W: u16 = 10;
const PANEL_FOES_ROW: u16 = 12;
const XP_BAR_W: u16 = 18;
/// Chest sprite (3x2, centred on the chest cell) and its glow pulse period (ticks).
const CHEST: [&str; 2] = ["▛▀▜", "▙▄▟"];
const CHEST_PULSE: u32 = 8;
/// Bottom bar: size, orb width, face width, HP percent under which the face suffers, faces (3 rows x FACE_W).
const BAR_W: u16 = 76;
const BAR_H: u16 = 5;
const ORB_W: u16 = 9;
const FACE_W: u16 = 7;
const FACE_HURT_PCT: i32 = 35;
const FACE_CALM: [&str; 3] = [" .---. ", " (o o) ", "  \\_/  "];
const FACE_RAGE: [&str; 3] = [" \\---/ ", " (>.<) ", "  /W\\  "];
const FACE_HURT: [&str; 3] = [" .~~~. ", " (x x) ", "  /~\\  "];
/// Fed-thief gold glow strength (permille toward gold): base, per loot point, max.
const FED_GLOW_MIN: u32 = 250;
/// Fear tint strength on panicked foes (permille toward the fear colour).
const FEAR_TINT: u32 = 450;
const FED_GLOW_PER: u32 = 60;
const FED_GLOW_MAX: u32 = 800;
/// Thief bite lines. shortcut: fixed English set, upgrade to profile data when presets carry narration.
const STOLE_LINES: [&str; 4] = ["MINE NOW!", "yoink!", "tasty loot", "I grow STRONGER"];
/// Top-centre boss health bar width (cells).
const BOSS_BAR_W: u16 = 30;
/// Upgrade popup size (cells).
const POPUP_W: u16 = 64;
const POPUP_H: u16 = 16;
/// Arrow placement: retries along the ray toward the centre, and the factor per retry.
const POINTER_STEPS: usize = 24;
const POINTER_PULL: f64 = 0.9;
const ARROW_E: char = '▶';
const ARROW_W: char = '◀';
const ARROW_N: char = '▲';
const ARROW_S: char = '▼';

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    ch: Glyph,
    fg: Rgb,
    bg: Rgb,
}

/// Actor ink: `bg: None` keeps the cell's background (telegraph tint stays visible under actors).
#[derive(Clone, Copy)]
struct Ink {
    ch: Glyph,
    fg: Rgb,
    bg: Option<Rgb>,
}

/// Draw priority, typed total order (design sec 5.5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Prio {
    Floor,
    PlayerFx,
    Trash,
    Pickup,
    EliteBoss,
    IncomingHit,
    PlayerCore,
}

/// Bottom strip rows, always reserved (design sec 5.9).
pub const STRIP: u16 = 2;
const LUT_STEP: i32 = 250;

pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    cells: Vec<Cell>,
    z: Vec<Prio>,
    /// view depth of the actor drawn in each cell (ties inside one Prio: nearer wins)
    zd: Vec<i32>,
    /// view depth of the block face drawn in each cell; actors behind it are hidden
    bd: Vec<i32>,
    /// reused per frame: visible solid cells (depth, fx, fy), sorted back to front
    blocks: Vec<(i32, i32, i32)>,
    dens: Vec<u8>,
    front: Vec<Option<Cell>>,
    fog: [u16; 256],
    pub out: Vec<u8>,
    pub cam: Cam,
    eye: Eye,
    /// sim tick of `cur`; positions by id at the previous / current tick seen (interpolation)
    tick: Option<u32>,
    prev: Vec<(u32, i32, i32)>,
    cur: Vec<(u32, i32, i32)>,
    pprev: (i32, i32),
    cur_p: (i32, i32),
    /// interpolated actor positions this frame, by enemy index
    pos: Vec<(f64, f64)>,
    /// map cells the player has stood in (view state, not sim state)
    seen: HashSet<(i32, i32)>,
    /// wall ms (View::now_ms) when the current sim tick was first drawn
    tick_ms: u64,
}

/// This frame's projection: angle, camera centre (world mm), absolute screen-grid origin.
/// The floor grid is world-fixed: screen cell (c, r) shows grid cell (c - cc + o.0, r - rc + o.1).
#[derive(Clone, Copy)]
struct Eye {
    p: Proj,
    cx: f64,
    cy: f64,
    o: (i32, i32),
}

/// What the strip shows besides sim state; `now_ms` is the frame clock (fake in tests),
/// `alpha` in 0..=1 the frame's place between the last two sim ticks.
pub struct View<'a> {
    pub ui: &'a Ui,
    pub now_ms: u64,
    pub alpha: f64,
    pub phase: &'static str,
    pub last_bytes: usize,
}

fn mix(a: Rgb, b: Rgb, f: u32) -> Rgb {
    let m = |x: u8, y: u8| ((u32::from(x) * (1000 - f) + u32::from(y) * f) / 1000) as u8;
    Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn hash(a: u64) -> u64 {
    let mut z = a.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Frame {
    pub fn new(c: &Content, cols: u16, rows: u16) -> Frame {
        let n = usize::from(cols) * usize::from(rows);
        let blank = Cell { ch: Glyph::SPACE, fg: c.pal.text, bg: c.pal.bg };
        let v = &c.view;
        let mut fog = [0u16; 256];
        for (i, f) in fog.iter_mut().enumerate() {
            let d = i as i32 * LUT_STEP;
            let t = ((d - v.fog_start).max(0) as i64 * 1000 / i64::from((v.fog_end - v.fog_start).max(1))).min(1000);
            let band = 1000 / i64::from(v.fog_bands);
            *f = ((t / band * band) * i64::from(v.fog_max) / 1000) as u16;
        }
        Frame {
            cols,
            rows,
            cells: vec![blank; n],
            z: vec![Prio::Floor; n],
            zd: vec![i32::MIN; n],
            bd: vec![i32::MIN; n],
            blocks: Vec::with_capacity(4096),
            dens: vec![0; n],
            front: vec![None; n],
            fog,
            out: Vec::with_capacity(1 << 20),
            cam: Cam::default(),
            eye: Eye { p: Proj::at(0.0), cx: 0.0, cy: 0.0, o: (0, 0) },
            tick: None,
            prev: Vec::new(),
            cur: Vec::new(),
            pprev: (0, 0),
            cur_p: (0, 0),
            pos: Vec::new(),
            seen: HashSet::new(),
            tick_ms: 0,
        }
    }

    /// New sim tick seen: the old current positions become the previous ones. A tick going back
    /// (new run) drops history and snaps the camera.
    fn shift(&mut self, sim: &Sim) {
        match self.tick {
            Some(t) if t == sim.tick => return,
            Some(t) if t < sim.tick => {
                std::mem::swap(&mut self.prev, &mut self.cur);
                self.pprev = self.cur_p;
            }
            _ => {
                self.cam.reset();
                self.prev.clear();
                self.cur.clear();
                self.cur_p = (sim.p.x, sim.p.y);
                self.pprev = self.cur_p;
            }
        }
        self.tick = Some(sim.tick);
        self.cur.clear();
        self.cur.extend((0..sim.e.len()).map(|k| (sim.e.id[k], sim.e.x[k], sim.e.y[k])));
        self.cur.sort_unstable();
        self.cur_p = (sim.p.x, sim.p.y);
        if self.prev.is_empty() {
            self.prev.clone_from(&self.cur);
        }
    }

    /// Position at `alpha` between the previous tick and now (unknown before: now).
    fn lerp(alpha: f64, from: Option<(i32, i32)>, x: i32, y: i32) -> (f64, f64) {
        let (x, y) = (f64::from(x), f64::from(y));
        from.map_or((x, y), |(px, py)| (f64::from(px) + alpha * (x - f64::from(px)), f64::from(py) + alpha * (y - f64::from(py))))
    }

    /// Screen cell (col, row) of a world point.
    fn scr(&self, x: f64, y: f64) -> (i32, i32) {
        let (c, r) = self.eye.p.project(x, y);
        let (cc, rc) = self.centre();
        (c - self.eye.o.0 + cc, r - self.eye.o.1 + rc)
    }

    /// World point (mm) at the centre of screen cell (col, row).
    fn world(&self, col: i32, row: i32) -> (f64, f64) {
        let (cc, rc) = self.centre();
        self.eye.p.unproject(col - cc + self.eye.o.0, row - rc + self.eye.o.1)
    }

    /// View depth (mm) relative to the camera.
    fn vd(&self, x: f64, y: f64) -> i32 {
        self.eye.p.depth(x - self.eye.cx, y - self.eye.cy) as i32
    }

    fn play_rows(&self) -> u16 {
        self.rows.saturating_sub(STRIP)
    }

    fn centre(&self) -> (i32, i32) {
        (i32::from(self.cols) / 2, i32::from(self.play_rows()) / 2)
    }

    /// Depth fog by distance from the camera.
    fn fogged(&self, c: &Content, col: Rgb, x: f64, y: f64) -> Rgb {
        let d = (x - self.eye.cx).hypot(y - self.eye.cy) as i32;
        mix(col, c.pal.fog, u32::from(self.fog[((d / LUT_STEP) as usize).min(255)]))
    }

    /// Screen cell of a world point, if inside the play area.
    fn cell_of(&self, x: f64, y: f64) -> Option<usize> {
        let (c, r) = self.scr(x, y);
        (c >= 0 && r >= 0 && c < i32::from(self.cols) && r < i32::from(self.play_rows())).then(|| r as usize * usize::from(self.cols) + c as usize)
    }

    /// Actor cell at view depth `d`: hidden behind a nearer block face, else higher (Prio, depth) wins.
    fn put(&mut self, i: usize, p: Prio, d: i32, ink: Ink) {
        if d >= self.bd[i] && (p, d) >= (self.z[i], self.zd[i]) {
            self.z[i] = p;
            self.zd[i] = d;
            self.cells[i] = Cell { ch: ink.ch, fg: ink.fg, bg: ink.bg.unwrap_or(self.cells[i].bg) };
        }
    }

    /// Standing actor on foot cell `i`: shadow on the foot, `n` rows rising above it, head on top.
    /// n = 0 is the flat form (head on the foot, no shadow).
    fn stand(&mut self, i: usize, d: i32, n: u8, p: Prio, [head, body, shadow]: [Ink; 3]) {
        let cols = usize::from(self.cols);
        if n == 0 {
            self.put(i, p, d, head);
            return;
        }
        if d >= self.bd[i] && self.z[i] <= Prio::PlayerFx {
            self.cells[i].ch = shadow.ch;
            self.cells[i].fg = shadow.fg;
        }
        for k in 1..=usize::from(n) {
            let Some(j) = i.checked_sub(k * cols) else { return };
            self.put(j, p, d, if k == usize::from(n) { head } else { body });
        }
    }

    fn tint(&mut self, i: usize, bg: Rgb) {
        if self.z[i] <= Prio::PlayerFx {
            self.z[i] = Prio::PlayerFx;
        }
        self.cells[i].bg = bg;
    }

    pub fn compose(&mut self, sim: &Sim, v: &View) {
        let c = sim.c;
        let ui = v.ui;
        let pal = &c.pal;
        let cols = usize::from(self.cols);
        let fc = c.view.floor_cell;
        // 0. interpolate actors between the last two ticks, move the camera, fix this frame's eye
        let before = self.tick;
        self.shift(sim);
        if self.tick != before {
            self.tick_ms = v.now_ms;
        }
        // interpolate only for one tick period after a new tick; a frozen sim (pick pause) settles on the current state
        let a = if v.now_ms.saturating_sub(self.tick_ms) >= 1000 / u64::from(c.hz().get()) { 1.0 } else { v.alpha.clamp(0.0, 1.0) };
        let (fpx, fpy) = Frame::lerp(a, Some(self.pprev), sim.p.x, sim.p.y);
        let mut pos = std::mem::take(&mut self.pos);
        pos.clear();
        pos.extend((0..sim.e.len()).map(|k| {
            let from = self.prev.binary_search_by_key(&sim.e.id[k], |p| p.0).ok().map(|j| (self.prev[j].1, self.prev[j].2));
            Frame::lerp(a, from, sim.e.x[k], sim.e.y[k])
        }));
        self.pos = pos;
        self.cam.update(&c.view, v.now_ms, ui.heading, fpx, fpy);
        let p = self.cam.proj(&c.view, v.now_ms);
        // camera shown at cell resolution: the grid origin is the camera's own cell
        self.eye = Eye { p, cx: self.cam.x, cy: self.cam.y, o: p.project(self.cam.x, self.cam.y) };
        // 1. floor: one glyph per world floor cell, at the cell its centre projects to
        for r in 0..i32::from(self.play_rows()) {
            for col in 0..i32::from(self.cols) {
                let i = r as usize * cols + col as usize;
                let (ax, ay) = self.world(col, r);
                let (fx, fy) = ((ax / f64::from(fc)).floor() as i32, (ay / f64::from(fc)).floor() as i32);
                let ar = &sim.arena;
                let h = hash(ar.salt ^ ((fx as u64) << 32) ^ (fy as u32 as u64));
                let anchor = self.scr(f64::from(fx * fc + fc / 2), f64::from(fy * fc + fc / 2)) == (col, r);
                let ch = if anchor && (h % 1000) < u64::from(ar.density) { ar.glyphs[(h >> 20) as usize % ar.glyphs.len()] } else { Glyph::SPACE };
                // blank cells keep one fg so camera motion only re-sends cells whose glyph moved
                let fg = if ch == Glyph::SPACE { pal.floor } else { self.fogged(c, pal.floor, ax, ay) };
                self.cells[i] = Cell { ch, fg, bg: pal.bg };
                self.z[i] = Prio::Floor;
                self.zd[i] = i32::MIN;
                self.bd[i] = i32::MIN;
                self.dens[i] = 0;
            }
        }
        for r in i32::from(self.play_rows())..i32::from(self.rows) {
            for col in 0..cols {
                let i = r as usize * cols + col;
                self.cells[i] = Cell { ch: Glyph::SPACE, fg: pal.text, bg: pal.hud_bg };
                self.z[i] = Prio::Floor;
            }
        }
        self.blocks(sim, (fpx, fpy));
        self.fx(sim);
        self.teles(sim);
        // 4. drops (no id: drawn at the current tick)
        for k in 0..sim.drops.x.len() {
            let (x, y) = (f64::from(sim.drops.x[k]), f64::from(sim.drops.y[k]));
            if let crate::sim::Drop::Chest = sim.drops.kind[k] {
                self.chest(sim, x, y);
                continue;
            }
            if let Some(i) = self.cell_of(x, y) {
                let ch = Glyph::of('◇').unwrap_or(Glyph::SPACE);
                self.put(i, Prio::Pickup, self.vd(x, y), Ink { ch, fg: pal.xp, bg: None });
            }
        }
        for k in 0..sim.treasures.len() {
            let (x, y) = sim.treasures[k];
            self.chest(sim, f64::from(x), f64::from(y));
        }
        self.enemies(sim);
        self.fed(sim);
        // 6. enemy shots (no id: drawn at the current tick)
        for k in 0..sim.shots.x.len() {
            let (x, y) = (f64::from(sim.shots.x[k]), f64::from(sim.shots.y[k]));
            if let Some(i) = self.cell_of(x, y) {
                self.put(i, Prio::IncomingHit, self.vd(x, y), Ink { ch: Glyph::of('o').unwrap_or(Glyph::SPACE), fg: pal.of(c.fam(sim.shots.fam[k]).hue), bg: None });
            }
        }
        // 7. player + crowd ring
        let Some(pi) = self.cell_of(fpx, fpy) else {
            self.strip(sim, v);
            return;
        };
        let (pcol, prow) = self.scr(fpx, fpy);
        let crowd = (0..sim.e.len()).filter(|&k| sim.e.side[k] == Side::Hostile && dist2(sim.e.x[k] - sim.p.x, sim.e.y[k] - sim.p.y) <= i64::from(c.view.crowd_r).pow(2)).count();
        if crowd >= usize::from(c.view.crowd_n) {
            for (dc, dr) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (x, y) = (pcol + dc, prow + dr);
                if x >= 0 && y >= 0 && x < i32::from(self.cols) && y < i32::from(self.play_rows()) {
                    self.cells[y as usize * cols + x as usize].bg = pal.ring;
                }
            }
        }
        let pfg = if sim.p.iframes > 0 { pal.xp } else { pal.player };
        // cutaway (blocks) keeps every block face out of the player's stack, so the player is never hidden
        let head = Ink { ch: Glyph::of('@').unwrap_or(Glyph::SPACE), fg: pfg, bg: Some(pal.ring) };
        let body = Ink { ch: pal.body, fg: pfg, bg: None };
        let shadow = Ink { ch: pal.shadow, fg: pal.dim, bg: None };
        self.stand(pi, i32::MAX, c.view.actor_rows[0], Prio::PlayerCore, [head, body, shadow]);
        if ui.mode == Mode::Look {
            let (cc, rc) = self.centre();
            let (x, y) = (cc + ui.cursor.0, rc + ui.cursor.1);
            if x >= 0 && y >= 0 && x < i32::from(self.cols) && y < i32::from(self.play_rows()) {
                let i = y as usize * cols + x as usize;
                self.cells[i].bg = pal.text;
                self.cells[i].fg = pal.bg;
            }
        }
        self.strip(sim, v);
    }

    /// Solid floor cells (pillar/rubble template) as extruded blocks, painter order back to front.
    /// Faces: top, left, right; x-facing world faces are lit, y-facing dark, so q/e swaps the sides.
    /// Cutaway: a block nearer than the player whose box meets the player window is cut to a dim
    /// ground silhouette.
    fn blocks(&mut self, sim: &Sim, (fpx, fpy): (f64, f64)) {
        let c = sim.c;
        let (pal, v, ar) = (&c.pal, &c.view, &sim.arena);
        let rows = match ar.template {
            Template::Open => return,
            Template::Pillars => v.block_rows[0],
            Template::Rubble => v.block_rows[1],
        };
        let (fc, hr, p) = (v.floor_cell, i32::from(rows), self.eye.p);
        let (pc, pr) = (i32::from(self.cols), i32::from(self.play_rows()));
        // footprint half extents in cells at any angle (design sec 5.1: 3 cols, 1.5 rows per metre)
        let (hw, hh) = ((3 * fc + 999) / 1000 + 1, (3 * fc / 2 + 999) / 1000 + 1);
        // world box of the screen, grown by one footprint up and by the block height down
        let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
        for (col, r) in [(-hw, -hh), (pc + hw, -hh), (-hw, pr + hr + hh), (pc + hw, pr + hr + hh)] {
            let (x, y) = self.world(col, r);
            (lo, hi) = ((lo.0.min(x), lo.1.min(y)), (hi.0.max(x), hi.1.max(y)));
        }
        let cell = |w: f64| (w / f64::from(fc)).floor() as i32;
        let centre = |f: i32| f64::from(f * fc + fc / 2);
        let mut blocks = std::mem::take(&mut self.blocks);
        blocks.clear();
        for fy in cell(lo.1)..=cell(hi.1) {
            for fx in cell(lo.0)..=cell(hi.0) {
                if solid(ar, fc, fx, fy) {
                    blocks.push((self.vd(centre(fx), centre(fy)), fx, fy));
                }
            }
        }
        blocks.sort_unstable();
        let (cut, n0) = (i32::from(v.cut_cols), i32::from(v.actor_rows[0]));
        // the 4 side faces: outward world normal; a face shows when its normal points at the camera
        let faces = [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)].map(|(nx, ny): (f64, f64)| (nx, ny, p.depth(nx, ny) > 1e-9));
        let (pd, (pcol, prow)) = (self.vd(fpx, fpy), self.scr(fpx, fpy));
        for &(d, fx, fy) in &blocks {
            let (bx, by) = (centre(fx), centre(fy));
            let (bc, br) = self.scr(bx, by);
            let cutaway = cut > 0 && d > pd && bc - hw <= pcol + cut && bc + hw >= pcol - cut && br - hh - hr <= prow + 1 && br + hh >= prow - n0 - 1;
            let hz = if cutaway { 0 } else { hr };
            let base = self.fogged(c, pal.floor, bx, by);
            let ink = |k: usize| if cutaway { (pal.face[2], mix(base, pal.bg, 400)) } else { (pal.face[k], mix(base, pal.text, u32::from(v.face_pct[k]) * 10)) };
            for r in (br - hh - hz).max(0)..=(br + hh).min(pr - 1) {
                for col in (bc - hw).max(0)..=(bc + hw).min(pc - 1) {
                    let ground = |z: i32| self.world(col, r + z);
                    let inside = |z: i32| {
                        let (x, y) = ground(z);
                        (cell(x), cell(y)) == (fx, fy)
                    };
                    let k = if inside(hz) {
                        0
                    } else if let Some(z) = (0..hz).rev().find(|&z| inside(z)) {
                        // nearest edge of the footprint under this wall cell picks the face; x faces lit
                        let (x, y) = ground(z);
                        let (lx, ly) = (x - f64::from(fx * fc), y - f64::from(fy * fc));
                        let gap = |(nx, ny): (f64, f64)| if nx > 0.0 { f64::from(fc) - lx } else if nx < 0.0 { lx } else if ny > 0.0 { f64::from(fc) - ly } else { ly };
                        let best = faces.iter().filter(|f| f.2).min_by(|a, b| gap((a.0, a.1)).total_cmp(&gap((b.0, b.1))));
                        if best.is_some_and(|f| f.0 != 0.0) { 1 } else { 2 }
                    } else {
                        continue;
                    };
                    let (ch, fg) = ink(k);
                    let i = r as usize * usize::from(self.cols) + col as usize;
                    self.cells[i] = Cell { ch, fg, bg: self.cells[i].bg };
                    self.bd[i] = d;
                }
            }
        }
        self.blocks = blocks;
    }

    fn fx(&mut self, sim: &Sim) {
        let c = sim.c;
        let dot = |f: &mut Frame, x: i32, y: i32, hue: Hue| {
            if let Some(i) = f.cell_of(f64::from(x), f64::from(y)) {
                let bg = mix(c.pal.bg, c.pal.of(hue), 350);
                f.tint(i, bg);
            }
        };
        for fx in &sim.fx {
            let hue = c.skill(fx.skill).hue;
            match fx.shape {
                FxShape::Ring { x, y, r } => {
                    let n = (r / 150).clamp(8, 96);
                    for k in 0..n {
                        let a = f64::from(k) * std::f64::consts::TAU / f64::from(n);
                        dot(self, x + (f64::from(r) * a.cos()) as i32, y + (f64::from(r) * a.sin()) as i32, hue);
                    }
                }
                FxShape::Cone { x, y, dx, dy, r, cos } => {
                    let half = (f64::from(cos) / 1000.0).clamp(-1.0, 1.0).acos();
                    let base = f64::from(dy).atan2(f64::from(dx));
                    for step in 1..=(r / 400) {
                        for k in -4..=4 {
                            let a = base + half * f64::from(k) / 4.0;
                            let d = f64::from(step * 400);
                            dot(self, x + (d * a.cos()) as i32, y + (d * a.sin()) as i32, hue);
                        }
                    }
                }
                FxShape::Line { x0, y0, x1, y1 } => {
                    for k in 0..=12 {
                        dot(self, x0 + (x1 - x0) * k / 12, y0 + (y1 - y0) * k / 12, hue);
                    }
                }
            }
        }
        let code = c.player.skills.first().map_or(Hue::Code, |&s| c.skill(s).hue);
        for (x, y) in sim.bolts() {
            dot(self, x, y, code);
        }
        for &(x, y) in &sim.blades {
            dot(self, x, y, Hue::Emp);
        }
        for (x, y) in sim.mines() {
            dot(self, x, y, Hue::Therm);
        }
    }

    fn teles(&mut self, sim: &Sim) {
        if sim.tele.is_empty() {
            return;
        }
        let c = sim.c;
        let blink_on = (sim.tick / c.view.blink).is_multiple_of(2);
        let cols = usize::from(self.cols);
        for r in 0..i32::from(self.play_rows()) {
            for col in 0..i32::from(self.cols) {
                let (wx, wy) = self.world(col, r);
                let (ax, ay) = (wx.round() as i32, wy.round() as i32);
                let mut hit = None;
                for t in &sim.tele {
                    if t.shape.contains(ax, ay) {
                        // arena-wide = steady tint (no full-screen blink, design sec 5.8)
                        let steady = matches!(t.shape, Shape::Arena);
                        let k = match t.kind {
                            TeleKind::HitPlayer { .. } => if blink_on || steady { 450 } else { 250 },
                            TeleKind::PurgeHijacked | TeleKind::Spawn { .. } => 180,
                        };
                        hit = Some(hit.map_or(k, |h: u32| h.max(k)));
                    }
                }
                if let Some(k) = hit {
                    let i = r as usize * cols + col as usize;
                    self.cells[i].bg = mix(c.pal.bg, c.pal.alarm, k);
                    if self.cells[i].ch == Glyph::SPACE {
                        self.cells[i].ch = Glyph::of('░').unwrap_or(Glyph::SPACE);
                        self.cells[i].fg = mix(c.pal.bg, c.pal.alarm, 600);
                    }
                }
            }
        }
    }

    fn enemies(&mut self, sim: &Sim) {
        let c = sim.c;
        let pal = &c.pal;
        let e = &sim.e;
        let cols = usize::from(self.cols);
        let view2 = i64::from(c.view.view_r * 2).pow(2);
        for k in 0..e.len() {
            if e.tier[k] == Tier::Trash
                && e.side[k] == Side::Hostile
                && let Some(i) = self.cell_of(self.pos[k].0, self.pos[k].1)
            {
                self.dens[i] = self.dens[i].saturating_add(1);
            }
        }
        for k in 0..e.len() {
            let (dx, dy) = (e.x[k] - sim.p.x, e.y[k] - sim.p.y);
            if dist2(dx, dy) > view2 {
                continue;
            }
            let (x, y) = self.pos[k];
            let Some(i) = self.cell_of(x, y) else { continue };
            let fam = c.fam(e.fam[k]);
            let hue = self.fogged(c, pal.of(fam.hue), x, y);
            let d = self.vd(x, y);
            let mk = |ch: Glyph, fg: Rgb| match e.side[k] {
                Side::Hostile => Ink { ch, fg, bg: None },
                Side::Hijacked => Ink { ch, fg: pal.bg, bg: Some(fg) }, // inverse video (design sec 5.4)
            };
            let shadow = Ink { ch: pal.shadow, fg: mix(pal.floor, pal.bg, 300), bg: None };
            let body = mk(pal.body, mix(hue, pal.bg, 300));
            match e.tier[k] {
                Tier::Trash => {
                    let ch = match (e.side[k], self.dens[i]) {
                        (Side::Hostile, 3..=4) => pal.density[0],
                        (Side::Hostile, 5..=7) => pal.density[1],
                        (Side::Hostile, 8..) => pal.density[2],
                        _ => fam.glyph,
                    };
                    self.stand(i, d, c.view.actor_rows[1], Prio::Trash, [mk(ch, hue), body, shadow]);
                }
                Tier::Elite | Tier::Champion => {
                    let n = c.view.actor_rows[2];
                    let rim = mk(Glyph::of(if e.tier[k] == Tier::Champion { '▓' } else { '·' }).unwrap_or(Glyph::SPACE), mix(hue, pal.bg, 450));
                    let (cx, cy) = ((i % cols) as i32, (i / cols) as i32);
                    // rim flanks the body rows (the foot row too when flat)
                    for oy in (if n == 0 { 0 } else { 1 })..=i32::from(n) {
                        for ox in [-1, 1] {
                            let (x, y) = (cx + ox, cy - oy);
                            if x >= 0 && y >= 0 && x < i32::from(self.cols) {
                                self.put(y as usize * cols + x as usize, Prio::EliteBoss, d, rim);
                            }
                        }
                    }
                    self.stand(i, d, n, Prio::EliteBoss, [mk(fam.glyph, hue), body, shadow]);
                }
                Tier::Boss => self.boss(sim, k, i, d, hue),
            }
        }
    }

    /// Boss = glyph cloud standing on its foot cell: dense bright core, sparse dim rim, glyphs
    /// shimmer every 4 ticks; a shadow row under it. Flat form (elite rows 0) centres it on the foot.
    fn boss(&mut self, sim: &Sim, k: usize, i: usize, d: i32, hue: Rgb) {
        let c = sim.c;
        let Some(def) = c.boss_of(sim.e.fam[k]) else { return };
        let cols = usize::from(self.cols);
        let (w, h) = (i32::from(def.w), i32::from(def.h));
        let lift = if c.view.actor_rows[2] == 0 { h / 2 } else { h };
        let (x0, y0) = ((i % cols) as i32 - w / 2, (i / cols) as i32 - lift);
        if lift == h {
            let shadow = Ink { ch: c.pal.shadow, fg: mix(c.pal.floor, c.pal.bg, 300), bg: None };
            for ox in w / 4..w - w / 4 {
                let x = x0 + ox;
                if x >= 0 && x < i32::from(self.cols) {
                    self.put(i - i % cols + x as usize, Prio::PlayerFx, d, shadow);
                }
            }
        }
        let frame = u64::from(sim.tick / 4);
        for oy in 0..h {
            for ox in 0..w {
                let (cx, cy) = (x0 + ox, y0 + oy);
                if cx < 0 || cy < 0 || cx >= i32::from(self.cols) || cy >= i32::from(self.play_rows()) {
                    continue;
                }
                // ellipse: core inside 0.5, rim to 1.0
                let (nx, ny) = (i64::from(2 * ox - w + 1) * 1000 / i64::from(w), i64::from(2 * oy - h + 1) * 1000 / i64::from(h));
                let rr = nx * nx + ny * ny;
                let hh = hash(frame << 16 ^ (ox as u64) << 8 ^ oy as u64);
                let (ch, fg) = if rr <= 250_000 {
                    (def.cloud[(hh % def.cloud.len() as u64) as usize], hue)
                } else if rr <= 1_000_000 && !hh.is_multiple_of(3) {
                    (def.cloud[(hh >> 8) as usize % def.cloud.len()], mix(hue, c.pal.bg, 500))
                } else {
                    continue;
                };
                let j = cy as usize * cols + cx as usize;
                self.put(j, Prio::EliteBoss, d, Ink { ch, fg, bg: None });
            }
        }
    }

    // ---------- strip ----------

    fn text(&mut self, row: u16, col: u16, s: &str, fg: Rgb) -> u16 {
        let cols = usize::from(self.cols);
        let mut x = usize::from(col);
        for g in Glyph::text(s) {
            if x >= cols {
                break;
            }
            let i = usize::from(row) * cols + x;
            self.cells[i].ch = g;
            self.cells[i].fg = fg;
            x += 1;
        }
        x as u16
    }

    fn num(&mut self, row: u16, col: u16, v: i64, fg: Rgb) -> u16 {
        let mut buf = [0u8; 24];
        let mut n = v.unsigned_abs();
        let mut k = buf.len();
        loop {
            k -= 1;
            buf[k] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        if v < 0 {
            k -= 1;
            buf[k] = b'-';
        }
        self.text(row, col, std::str::from_utf8(&buf[k..]).unwrap_or("?"), fg)
    }

    fn bar(&mut self, row: u16, col: u16, val: i64, max: i64, w: u16, fg: Rgb) -> u16 {
        let fill = (val.max(0) * i64::from(w) / max.max(1)) as u16;
        let mut x = col;
        for k in 0..w {
            x = self.text(row, x, if k < fill { "█" } else { "░" }, if k < fill { fg } else { self.cells[0].fg });
        }
        x
    }

    /// Map (top-left), stats panel (bottom-right), off-screen pointers to bosses.
    /// Upgrade picker: centred popup with each card's details and the time left to choose.
    fn popup(&mut self, sim: &Sim, ui: &Ui, now_ms: u64) {
        let (Mode::Pick, Some(offer)) = (ui.mode, sim.offer) else { return };
        let c = sim.c;
        let pal = &c.pal;
        let w = POPUP_W.min(self.cols.saturating_sub(4));
        let h = POPUP_H;
        if self.play_rows() < h {
            return;
        }
        let (x0, y0) = ((self.cols - w) / 2, (self.play_rows() - h) / 2);
        let cols = usize::from(self.cols);
        for r in 0..h {
            for col in 0..w {
                let i = usize::from(y0 + r) * cols + usize::from(x0 + col);
                self.cells[i] = Cell { ch: Glyph::SPACE, fg: pal.text, bg: pal.hud_bg };
            }
        }
        let edge = |f: &mut Self, row: u16, ch: char, fg: Rgb| {
            for col in 0..w {
                let i = usize::from(row) * cols + usize::from(x0 + col);
                f.cells[i] = Cell { ch: Glyph::of(ch).unwrap_or(Glyph::SPACE), fg, bg: pal.hud_bg };
            }
        };
        edge(self, y0, '─', pal.of(Hue::Kin));
        edge(self, y0 + h - 1, '─', pal.of(Hue::Kin));
        let limit = sim.pick_timeout_ms();
        let left_ms = limit.saturating_sub(now_ms.saturating_sub(ui.pick_open_ms));
        let x = self.text(y0 + 1, x0 + 2, "UPGRADE  choose before ", pal.alarm);
        self.num(y0 + 1, x, (left_ms / 1000) as i64, pal.alarm);
        for (k, card) in offer.cards.iter().enumerate() {
            let row = y0 + 3 + 4 * k as u16;
            let sel = k as u8 == ui.sel;
            let Some(card) = card else {
                self.text(row, x0 + 2, "(empty)", pal.dim);
                continue;
            };
            let s = c.skill(card.skill);
            let rar = [("common", pal.text), ("RARE", pal.of(Hue::Kin)), ("EPIC", pal.of(Hue::Therm)), ("LEGEND", pal.chest)][usize::from(card.rarity.min(3))];
            let mut x = self.text(row, x0 + 2, if sel { "> " } else { "  " }, pal.alarm);
            x = self.num(row, x, k as i64 + 1, pal.dim);
            // power type colour: same as the stats pane
            x = self.text(row, x + 1, &s.name, pal.of(s.hue));
            x = self.text(row, x + 2, rar.0, rar.1);
            x = self.text(row, x + 2, if card.new_rank == 1 { "NEW" } else { "rank" }, pal.dim);
            if card.new_rank > 1 {
                self.num(row, x + 1, i64::from(card.new_rank), pal.dim);
            }
            let text: Vec<char> = s.desc.chars().collect();
            let line = usize::from(w) - 6;
            for (j, chunk) in text.chunks(line).take(2).enumerate() {
                let s: String = chunk.iter().collect();
                self.text(row + 1 + j as u16, x0 + 4, &s, pal.text);
            }
        }
        // timer bar: shrinks to empty at the timeout
        let bar_w = w.saturating_sub(4);
        let fill = (left_ms * u64::from(bar_w) / limit.max(1)) as u16;
        for k in 0..bar_w {
            let ch = if k < fill { '█' } else { '░' };
            self.text(y0 + h - 2, x0 + 2 + k, &ch.to_string(), if k < fill { pal.alarm } else { pal.dim });
        }
    }

    /// Bottom bar origin (col, row) in play-area cells; None when the screen is too small.
    fn bar_origin(&self) -> Option<(u16, u16)> {
        (self.cols >= BAR_W && self.play_rows() >= BAR_H + MAP_H).then(|| ((self.cols - BAR_W) / 2, self.play_rows() - BAR_H))
    }

    /// Diablo/Doom bottom bar: blood orb | hotkeys 1,2 / 5,6 | face | hotkeys 3,4 / 7,8 | magic orb.
    fn hud_bar(&mut self, sim: &Sim) {
        let Some((x0, y0)) = self.bar_origin() else { return };
        let c = sim.c;
        let pal = &c.pal;
        let cols = usize::from(self.cols);
        for r in 0..BAR_H {
            for col in 0..BAR_W {
                self.cells[usize::from(y0 + r) * cols + usize::from(x0 + col)] = Cell { ch: Glyph::SPACE, fg: pal.text, bg: pal.hud_bg };
            }
        }
        // orbs fill from the bottom; the top row shows the number
        let orb = |f: &mut Self, ox: u16, val: i64, max: i64, fg: Rgb| {
            let fill = (val.max(0) * i64::from(BAR_H - 1) * 2 / max.max(1)) as u16; // half-rows
            for r in 0..BAR_H - 1 {
                let from_bottom = BAR_H - 2 - r;
                let ch = if fill >= (from_bottom + 1) * 2 { '█' } else if fill == from_bottom * 2 + 1 { '▄' } else { '░' };
                for col in 0..ORB_W {
                    let i = usize::from(y0 + 1 + r) * cols + usize::from(ox + col);
                    f.cells[i] = Cell { ch: Glyph::of(ch).unwrap_or(Glyph::SPACE), fg: if ch == '░' { pal.dim } else { fg }, bg: pal.hud_bg };
                }
            }
            let n = (val.max(0) / 1000).to_string();
            f.text(y0, ox + (ORB_W - n.len() as u16) / 2, &n, fg);
        };
        orb(self, x0, i64::from(sim.p.hp), i64::from(sim.p.max_hp), pal.alarm);
        orb(self, x0 + BAR_W - ORB_W, i64::from(sim.p.flux), i64::from(c.player.flux.max(1)), pal.of(Hue::Code));
        // hotkeys: slot k shows the k-th owned power in its type colour
        let half = (BAR_W - 2 * ORB_W - FACE_W - 4) / 2;
        let left = x0 + ORB_W + 1;
        let right = left + half + 1 + FACE_W + 1;
        for k in 0..8u16 {
            let (hx, row) = match k {
                0 | 1 => (left + (k % 2) * (half / 2), y0 + 1),
                2 | 3 => (right + (k % 2) * (half / 2), y0 + 1),
                4 | 5 => (left + (k % 2) * (half / 2), y0 + 3),
                _ => (right + (k % 2) * (half / 2), y0 + 3),
            };
            let x = self.text(row, hx, &(k + 1).to_string(), pal.dim);
            match sim.owned.get(usize::from(k)) {
                Some(o) => {
                    let s = c.skill(o.id);
                    let name: String = s.name.chars().take(usize::from(half / 2 - 3)).collect();
                    self.text(row, x + 1, &name, pal.of(s.hue));
                    self.bar(row + 1, hx + 2, i64::from(o.rank), 5, half / 2 - 4, pal.of(s.hue));
                }
                None => {
                    self.text(row, x + 1, "-", pal.dim);
                }
            }
        }
        // face: suffering when low, rage in a crowd, calm otherwise
        let crowd = (0..sim.e.len()).filter(|&k| sim.e.side[k] == Side::Hostile && dist2(sim.e.x[k] - sim.p.x, sim.e.y[k] - sim.p.y) <= i64::from(c.view.crowd_r).pow(2)).count();
        let (face, fg) = if sim.p.hp * 100 < sim.p.max_hp * FACE_HURT_PCT {
            (FACE_HURT, pal.alarm)
        } else if crowd >= usize::from(c.view.crowd_n) {
            (FACE_RAGE, pal.of(Hue::Therm))
        } else {
            (FACE_CALM, pal.player)
        };
        let fx = left + half + 1;
        for (r, line) in face.iter().enumerate() {
            self.text(y0 + 1 + r as u16, fx, line, fg);
        }
    }

    /// Fed thieves glow gold (stronger the more loot they ate); recent bites float a line above them.
    fn fed(&mut self, sim: &Sim) {
        let pal = &sim.c.pal;
        for k in 0..sim.e.len() {
            let loot = sim.e.loot[k];
            if loot == 0 && sim.e.panic[k] {
                // fear: a cold tint on foes that turned tail
                if let Some(i) = self.cell_of(f64::from(sim.e.x[k]), f64::from(sim.e.y[k])) {
                    self.cells[i].bg = mix(pal.bg, pal.of(Hue::Emp), FEAR_TINT);
                }
                continue;
            }
            if loot == 0 {
                continue;
            }
            let Some(i) = self.cell_of(f64::from(sim.e.x[k]), f64::from(sim.e.y[k])) else { continue };
            let f = (FED_GLOW_MIN + u32::from(loot) * FED_GLOW_PER).min(FED_GLOW_MAX);
            self.cells[i].bg = mix(pal.bg, pal.chest, f);
        }
        for b in &sim.barks {
            if sim.tick.saturating_sub(b.tick) >= BARK_TICKS || self.cell_of(f64::from(b.x), f64::from(b.y)).is_none() {
                continue;
            }
            let BarkKind::Stole { power } = b.kind;
            let line = STOLE_LINES[b.id as usize % STOLE_LINES.len()];
            let text = format!("{line} +{power}");
            let (col, row) = self.scr(f64::from(b.x), f64::from(b.y));
            // rise one row per third of the lifetime, like a speech bubble floating off
            let up = 2 + (sim.tick - b.tick) * 3 / BARK_TICKS;
            let r = row - up as i32;
            let c0 = col - text.chars().count() as i32 / 2;
            if r < 0 || r >= i32::from(self.play_rows()) || c0 < 0 {
                continue;
            }
            self.text(r as u16, c0 as u16, &text, pal.chest);
        }
    }

    /// Chest: 3x2 gold sprite on a pulsing gold glow, centred on the chest's cell.
    fn chest(&mut self, sim: &Sim, x: f64, y: f64) {
        let pal = &sim.c.pal;
        if self.cell_of(x, y).is_none() {
            return;
        }
        let (col, row) = self.scr(x, y);
        let d = self.vd(x, y);
        let pulse = if (sim.tick / CHEST_PULSE).is_multiple_of(2) { 550 } else { 750 };
        let glow = mix(pal.chest, pal.bg, pulse);
        for (dr, line) in CHEST.iter().enumerate() {
            for (dc, ch) in line.chars().enumerate() {
                let (c2, r2) = (col + dc as i32 - 1, row + dr as i32 - 1);
                if c2 < 0 || r2 < 0 || c2 >= i32::from(self.cols) || r2 >= i32::from(self.play_rows()) {
                    continue;
                }
                let i = r2 as usize * usize::from(self.cols) + c2 as usize;
                self.put(i, Prio::Pickup, d, Ink { ch: Glyph::of(ch).unwrap_or(Glyph::SPACE), fg: pal.chest, bg: Some(glow) });
            }
        }
    }

    /// Live boss: name + health bar, top centre between the map and the stats pane.
    fn boss_bar(&mut self, sim: &Sim) {
        let Some(b) = sim.boss_ix() else { return };
        let c = sim.c;
        let pal = &c.pal;
        let (left, right) = (MAP_W + 1, self.cols.saturating_sub(PANEL_W + 1));
        if right <= left + BOSS_BAR_W {
            return;
        }
        let name = &c.fam(sim.e.fam[b]).name;
        let w = BOSS_BAR_W + name.chars().count() as u16 + 1;
        let x0 = left + (right - left).saturating_sub(w) / 2;
        let x = self.text(0, x0, name, pal.alarm);
        self.bar(0, x + 1, i64::from(sim.e.hp[b]), i64::from(sim.e.max[b]), BOSS_BAR_W, pal.alarm);
    }

    fn overlays(&mut self, sim: &Sim, ui: &Ui, now_ms: u64) {
        self.boss_bar(sim);
        self.popup(sim, ui, now_ms);
        let c = sim.c;
        let unit = c.view.floor_cell.max(1) * MAP_K;
        let (px, py) = (sim.p.x.div_euclid(unit), sim.p.y.div_euclid(unit));
        self.seen.insert((px, py));
        // targets: scheduled bosses not yet spawned (home), and the live boss
        let mut t: Vec<(i32, i32, usize)> = sim.boss_home.iter().filter(|h| !h.spawned).map(|h| (h.x, h.y, h.fam.ix())).collect();
        if let Some(b) = sim.boss_ix() {
            t.push((sim.e.x[b], sim.e.y[b], sim.e.fam[b].ix()));
        }
        self.map(sim, unit, (px, py), &t, &sim.treasures);
        self.panel(sim);
        self.hud_bar(sim);
        for &(x, y, _) in &t {
            self.pointer(sim, x, y, c.pal.alarm);
        }
        for &(x, y) in &sim.treasures {
            self.pointer(sim, x, y, c.pal.chest);
        }
    }

    fn map(&mut self, sim: &Sim, unit: i32, (px, py): (i32, i32), t: &[(i32, i32, usize)], treasures: &[(i32, i32)]) {
        let c = sim.c;
        let pal = &c.pal;
        let cols = usize::from(self.cols);
        let (x0, y0) = (0u16, 0u16);
        if self.cols < MAP_W || self.play_rows() < MAP_H {
            return;
        }
        for dr in 0..MAP_H {
            for dc in 0..MAP_W {
                let (cx, cy) = (px + i32::from(dc) - i32::from(MAP_W / 2), py + i32::from(dr) - i32::from(MAP_H / 2));
                let (ch, fg) = if (cx, cy) == (px, py) {
                    ('@', pal.player)
                } else if treasures.iter().any(|&(x, y)| (x.div_euclid(unit), y.div_euclid(unit)) == (cx, cy)) {
                    ('■', pal.chest)
                } else if t.iter().any(|&(x, y, _)| (x.div_euclid(unit), y.div_euclid(unit)) == (cx, cy)) {
                    ('B', pal.alarm)
                } else if self.seen.contains(&(cx, cy)) {
                    ('·', pal.dim)
                } else {
                    (' ', pal.dim)
                };
                let i = usize::from(y0 + dr) * cols + usize::from(x0 + dc);
                self.cells[i] = Cell { ch: Glyph::of(ch).unwrap_or(Glyph::SPACE), fg, bg: pal.hud_bg };
            }
        }
    }

    fn panel(&mut self, sim: &Sim) {
        let c = sim.c;
        let pal = &c.pal;
        if self.cols < PANEL_W || self.play_rows() < PANEL_H {
            return;
        }
        let (x0, y0) = (self.cols - PANEL_W, 0);
        let cols = usize::from(self.cols);
        for r in 0..PANEL_H {
            for col in 0..PANEL_W {
                let i = usize::from(y0 + r) * cols + usize::from(x0 + col);
                self.cells[i] = Cell { ch: Glyph::SPACE, fg: pal.text, bg: pal.hud_bg };
            }
        }
        let hz = u64::from(c.hz().get());
        let mut r = y0;
        let mut x = self.text(r, x0 + 1, "TIME ", pal.dim);
        x = self.clock(r, x, sim.secs(), pal.text);
        // next boss: first scheduled boss event after now
        let next = c.waves.iter().filter_map(|w| match w {
            crate::data::WaveEv::Boss { at, fam, .. } if *at > sim.tick => Some((*at, *fam)),
            _ => None,
        }).min_by_key(|&(at, _)| at);
        match next {
            Some((at, fam)) => {
                let left = (u64::from(at - sim.tick) / hz) as u32;
                x = self.text(r, x + 2, "NEXT ", pal.dim);
                x = self.text(r, x, &c.fam(fam).name, pal.alarm);
                self.clock(r, x + 1, left, pal.alarm);
            }
            None => {
                self.text(r, x + 2, "NO BOSS LEFT", pal.dim);
            }
        }
        r += 1;
        let mut x = self.text(r, x0 + 1, "HP ", pal.text);
        x = self.bar(r, x, i64::from(sim.p.hp), i64::from(sim.p.max_hp), 16, pal.of(Hue::Kin));
        x = self.num(r, x + 1, i64::from(sim.p.hp.max(0) / 1000), pal.text);
        x = self.text(r, x, "/", pal.dim);
        self.num(r, x, i64::from(sim.p.max_hp / 1000), pal.dim);
        r += 1;
        // XP: the bar that drives "one more level" (wide, with numbers to go)
        let mut x = self.text(r, x0 + 1, "LV ", pal.text);
        x = self.num(r, x, i64::from(sim.p.level), pal.xp);
        x = self.bar(r, x + 1, i64::from(sim.p.xp), i64::from(sim.xp_need()), XP_BAR_W, pal.xp);
        x = self.num(r, x + 1, i64::from(sim.xp_need().saturating_sub(sim.p.xp)), pal.xp);
        self.text(r, x, " to go", pal.dim);
        r += 1;
        let mut x = self.text(r, x0 + 1, "FLUX ", pal.text);
        x = self.num(r, x, i64::from(sim.p.flux / 1000), pal.of(Hue::Code));
        x = self.text(r, x + 2, "KILLS ", pal.dim);
        x = self.num(r, x, i64::from(sim.stats.kills), pal.text);
        x = self.text(r, x + 2, "HIJACK ", pal.dim);
        self.num(r, x, i64::from(sim.hijacked), pal.of(Hue::Code));
        r += 1;
        // morale: hero NERVE (grows while unhurt) vs foe PANIC (share of foes fleeing)
        let hostile = (0..sim.e.len()).filter(|&k| sim.e.side[k] == Side::Hostile).count();
        let panicked = (0..sim.e.len()).filter(|&k| sim.e.side[k] == Side::Hostile && sim.e.panic[k]).count();
        let nerve = sim.confidence();
        let mut x = self.text(r, x0 + 1, "NERVE ", pal.text);
        x = self.bar(r, x, i64::from(nerve), 1000, MORALE_BAR_W, if nerve >= CONF_PANIC { pal.player } else { pal.dim });
        x = self.text(r, x + 2, "PANIC ", pal.text);
        self.bar(r, x, panicked as i64, hostile.max(1) as i64, MORALE_BAR_W, pal.of(Hue::Emp));
        r += 2;
        // powers: name in the power's type colour (same colour as the picker popup)
        self.text(r, x0 + 1, "POWERS", pal.dim);
        r += 1;
        for o in sim.owned.iter().take(PANEL_POWERS) {
            let s = c.skill(o.id);
            let mut x = self.text(r, x0 + 2, &s.name, pal.of(s.hue));
            x = self.text(r, x + 1, "r", pal.dim);
            self.num(r, x, i64::from(o.rank), pal.dim);
            r += 1;
        }
        // foes: one row per (family, tier), glyph + name in the foe's own render colour
        r = y0 + PANEL_FOES_ROW;
        self.text(r, x0 + 1, "FOES", pal.dim);
        let tiers = [Tier::Trash, Tier::Elite, Tier::Champion, Tier::Boss];
        let mut cnt = vec![0u32; c.fams.len() * tiers.len()];
        for k in 0..sim.e.len() {
            if sim.e.side[k] == Side::Hostile {
                let t = tiers.iter().position(|&t| t == sim.e.tier[k]).unwrap_or(0);
                cnt[sim.e.fam[k].ix() * tiers.len() + t] += 1;
            }
        }
        let mut top: Vec<(u32, usize)> = cnt.iter().enumerate().filter(|&(_, &n)| n > 0).map(|(i, &n)| (n, i)).collect();
        top.sort_by(|a, b| b.cmp(a));
        for (row, &(n, i)) in top.iter().take(PANEL_H.saturating_sub(PANEL_FOES_ROW + 1).into()).enumerate() {
            let (f, t) = (&c.fams[i / tiers.len()], tiers[i % tiers.len()]);
            let fg = pal.of(f.hue);
            let rr = r + 1 + row as u16;
            let mut x = self.text(rr, x0 + 2, &f.glyph.ch().to_string(), fg);
            x = self.num(rr, x + 1, i64::from(n), pal.text);
            x = self.text(rr, x + 1, &f.name, fg);
            let (tag, tc) = match t {
                Tier::Trash => ("", pal.dim),
                Tier::Elite => (" ELITE", pal.of(Hue::Kin)),
                Tier::Champion => (" CHAMPION", pal.of(Hue::Therm)),
                Tier::Boss => (" BOSS", pal.alarm),
            };
            self.text(rr, x, tag, tc);
        }
    }

    /// mm:ss at (row, col); returns the column after it.
    fn clock(&mut self, row: u16, col: u16, secs: u32, fg: Rgb) -> u16 {
        let mut x = self.num(row, col, i64::from(secs / 60), fg);
        x = self.text(row, x, if secs % 60 < 10 { ":0" } else { ":" }, fg);
        self.num(row, x, i64::from(secs % 60), fg)
    }

    /// Cell inside the map box or the stats box (both in play-area coordinates).
    fn boxed(&self, col: u16, row: u16) -> bool {
        let in_map = col < MAP_W && row < MAP_H;
        let in_panel = col >= self.cols.saturating_sub(PANEL_W) && row < PANEL_H;
        let in_bar = self.bar_origin().is_some_and(|(bx, by)| col >= bx && col < bx + BAR_W && row >= by);
        in_map || in_panel || in_bar
    }

    /// Arrow on the screen edge toward an off-screen world point (mm). On-screen points get none.
    fn pointer(&mut self, sim: &Sim, wx: i32, wy: i32, fg: Rgb) {
        let pal = &sim.c.pal;
        let (tx, ty) = self.scr(f64::from(wx), f64::from(wy));
        let (cols, rows) = (i32::from(self.cols), i32::from(self.play_rows()));
        if (0..cols).contains(&tx) && (0..rows).contains(&ty) {
            return;
        }
        let (cx, cy) = self.centre();
        let (dx, dy) = (f64::from(tx - cx), f64::from(ty - cy));
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        // scale the direction onto the inner border of the play area
        let (hw, hh) = (f64::from(cols / 2 - 1), f64::from(rows / 2 - 1));
        let sx = if dx == 0.0 { f64::INFINITY } else { hw / dx.abs() };
        let sy = if dy == 0.0 { f64::INFINITY } else { hh / dy.abs() };
        // pull the arrow toward the centre until it leaves the map and stats boxes (never drawn over them)
        let mut s = sx.min(sy);
        let (mut col, mut row) = (0u16, 0u16);
        for _ in 0..POINTER_STEPS {
            col = (f64::from(cx) + dx * s).round().clamp(0.0, f64::from(cols - 1)) as u16;
            row = (f64::from(cy) + dy * s).round().clamp(0.0, f64::from(rows - 1)) as u16;
            if !self.boxed(col, row) {
                break;
            }
            s *= POINTER_PULL;
        }
        if self.boxed(col, row) {
            return;
        }
        let ch = if dx.abs() > dy.abs() {
            if dx > 0.0 { ARROW_E } else { ARROW_W }
        } else if dy > 0.0 {
            ARROW_S
        } else {
            ARROW_N
        };
        let i = usize::from(row) * usize::from(self.cols) + usize::from(col);
        // glow: the arrow's own colour lifted off the background (gold for treasure, red for bosses)
        let glow = mix(fg, pal.hud_bg, 700);
        self.cells[i] = Cell { ch: Glyph::of(ch).unwrap_or(Glyph::SPACE), fg, bg: glow };
        // distance in metres beside the arrow, clear of both boxes
        let m = (dist2(wx - sim.p.x, wy - sim.p.y) as f64).sqrt() / 1000.0;
        let label = format!("{}m", m as i64);
        let n = label.chars().count() as u16;
        let start = if col + 2 + n < self.cols { col + 2 } else { col.saturating_sub(n + 1) };
        for (k, lc) in label.chars().enumerate() {
            let pos = start + k as u16;
            if pos < self.cols && !self.boxed(pos, row) {
                let j = usize::from(row) * usize::from(self.cols) + usize::from(pos);
                self.cells[j] = Cell { ch: Glyph::of(lc).unwrap_or(Glyph::SPACE), fg, bg: pal.hud_bg };
            }
        }
    }

    fn strip(&mut self, sim: &Sim, v: &View) {
        self.overlays(sim, v.ui, v.now_ms);
        let c = sim.c;
        let pal = &c.pal;
        let (rb, rcm) = (self.rows - 2, self.rows - 1);
        let ui = v.ui;
        // row B: pick cards | which-key | look tooltip | narration
        if let (Mode::Pick, Some(_)) = (ui.mode, sim.offer) {
            self.text(rb, 1, "PICK  wasd/hjkl/arrows move  1-3 or enter take  backspace skip  (popup above)", pal.alarm);
        } else if let Some((prefix, items)) = ui.whichkey(c, v.now_ms) {
            let mut x = self.text(rb, 1, &prefix, pal.alarm);
            x = self.text(rb, x, ":", pal.dim);
            for (k, label) in &items {
                x = self.text(rb, x + 2, k, pal.player);
                x = self.text(rb, x + 1, label, pal.text);
            }
        } else if ui.mode == Mode::Look {
            self.tooltip(sim, ui, rb);
        } else if let Some((ev, at)) = sim.narration.filter(|(_, at)| sim.tick.saturating_sub(*at) < c.hz().get() * 6) {
            let _ = at;
            if let Some(crate::data::WaveEv::Narrate { text, .. }) = c.waves.get(ev) {
                self.text(rb, 1, text, pal.of(Hue::Therm));
            }
        }
        // row C: mode line
        let mode = match ui.mode {
            Mode::Play => "-- PLAY --",
            Mode::Pick => "-- PICK --",
            Mode::Look => "-- LOOK --",
            Mode::Cmd => ":",
            Mode::Tree => "-- TREE --",
            Mode::Gear => "-- GEAR --",
        };
        let mut x = self.text(rcm, 1, mode, pal.player);
        match ui.mode {
            Mode::Cmd => {
                x = self.text(rcm, x, &ui.cmdline, pal.player);
                self.text(rcm, x, "_", pal.alarm);
            }
            Mode::Pick => {
                let desc = sim.offer.and_then(|o| o.cards.get(usize::from(ui.sel)).copied().flatten()).map(|cd| c.skill(cd.skill).desc.as_str()).unwrap_or("");
                x = self.text(rcm, x + 2, "h/l select  enter take  1-3  s skip | ", pal.dim);
                self.text(rcm, x, desc, pal.text);
            }
            _ => {
                x = self.text(rcm, x + 2, "AUTO ", pal.dim);
                x = self.text(rcm, x, if ui.autofire { "on" } else { "off" }, pal.text);
                x = self.text(rcm, x + 2, "CAM ", pal.dim);
                x = self.text(rcm, x, ui.heading.name(), pal.text);
                x = self.text(rcm, x + 2, v.phase, pal.dim);
                x = self.text(rcm, x + 2, "wasd/hjkl move  shift dodge  x hack  q/e rotate  spc menu  : cmd", pal.dim);
                x = self.text(rcm, x + 2, &ui.msg, pal.of(Hue::Kin));
                x = self.text(rcm, x + 2, "B ", pal.dim);
                self.num(rcm, x, (v.last_bytes / 1024) as i64, pal.dim);
            }
        }
    }

    fn tooltip(&mut self, sim: &Sim, ui: &Ui, row: u16) {
        let c = sim.c;
        let (cc, rc) = self.centre();
        let (wx, wy) = self.world(cc + ui.cursor.0, rc + ui.cursor.1);
        let (ax, ay) = (wx.round() as i32, wy.round() as i32);
        let best = (0..sim.e.len()).map(|k| (dist2(sim.e.x[k] - ax, sim.e.y[k] - ay), k)).min().filter(|(d2, _)| *d2 <= 2_250_000);
        let pal = &c.pal;
        let Some((_, k)) = best else {
            self.text(row, 1, "LOOK: move cursor with h j k l, esc back. nothing under cursor", pal.dim);
            return;
        };
        let fam = c.fam(sim.e.fam[k]);
        let mut x = self.text(row, 1, &fam.name, pal.of(fam.hue));
        let tier = match sim.e.tier[k] {
            Tier::Trash => " trash",
            Tier::Elite => " ELITE",
            Tier::Champion => " CHAMPION",
            Tier::Boss => " BOSS",
        };
        x = self.text(row, x, tier, pal.text);
        x = self.text(row, x + 2, ["KIN", "THERM", "EMP", "CODE"][fam.hue as usize], pal.of(fam.hue));
        x = self.text(row, x + 2, "hp ", pal.dim);
        x = self.num(row, x, i64::from(sim.e.hp[k] / 1000), pal.text);
        x = self.text(row, x, "/", pal.dim);
        x = self.num(row, x, i64::from(sim.e.max[k] / 1000), pal.dim);
        x = self.text(row, x + 2, "glitch ", pal.dim);
        x = self.num(row, x, i64::from(sim.e.glitch[k]), pal.of(Hue::Code));
        x = self.text(row, x + 2, if sim.e.side[k] == Side::Hijacked { "HIJACKED (yours)" } else { "hostile" }, pal.text);
        if sim.e.purger[k] {
            self.text(row, x + 2, "PURGER: kills your hijacked", pal.alarm);
        }
    }

    // ---------- emit ----------

    /// Diff vs front buffer into `out` (cleared, never freed). Returns changed cells.
    pub fn emit(&mut self) -> usize {
        self.out.clear();
        let cols = usize::from(self.cols);
        let mut cursor = None;
        let mut pen: Option<(Rgb, Rgb)> = None;
        let mut changed = 0;
        self.out.extend_from_slice(b"\x1b[?2026h");
        for (i, (f, b)) in self.front.iter_mut().zip(&self.cells).enumerate() {
            if *f == Some(*b) {
                continue;
            }
            changed += 1;
            if cursor != Some(i) {
                self.out.extend_from_slice(b"\x1b[");
                dec(&mut self.out, (i / cols + 1) as u64);
                self.out.push(b';');
                dec(&mut self.out, (i % cols + 1) as u64);
                self.out.push(b'H');
            }
            match pen {
                Some((fg, bg)) if fg == b.fg && bg == b.bg => {}
                Some((fg, _)) if fg == b.fg => sgr(&mut self.out, b"\x1b[48;2;", b.bg, None),
                Some((_, bg)) if bg == b.bg => sgr(&mut self.out, b"\x1b[38;2;", b.fg, None),
                _ => sgr(&mut self.out, b"\x1b[38;2;", b.fg, Some(b.bg)),
            }
            pen = Some((b.fg, b.bg));
            let mut u = [0u8; 4];
            self.out.extend_from_slice(b.ch.ch().encode_utf8(&mut u).as_bytes());
            *f = Some(*b);
            cursor = if (i + 1) % cols == 0 { None } else { Some(i + 1) };
        }
        if changed == 0 {
            self.out.clear();
            return 0;
        }
        self.out.extend_from_slice(b"\x1b[0m\x1b[?2026l");
        changed
    }

    /// Glyphs only, one line per row (for `proto2 snapshot`).
    pub fn plain(&self) -> String {
        let mut s = String::new();
        for row in self.cells.chunks(usize::from(self.cols)) {
            s.extend(row.iter().map(|c| c.ch.ch()));
            s.push('\n');
        }
        s
    }

    /// Forget what the terminal shows (after resize / clear).
    pub fn invalidate(&mut self) {
        self.front.fill(None);
    }
}

/// Floor cell is solid: its centre lies in a decor disc (one disc per decor super-cell, floorgen.rs).
/// Render only: the sim has no solids, actors walk through blocks.
fn solid(ar: &Arena, fc: i32, fx: i32, fy: i32) -> bool {
    let (ax, ay, dc) = (fx * fc + fc / 2, fy * fc + fc / 2, ar.decor_cell);
    let (sx, sy) = (ax.div_euclid(dc), ay.div_euclid(dc));
    let hd = hash(ar.salt.rotate_left(17) ^ ((sx as u64) << 32) ^ (sy as u32 as u64));
    let (cx, cy) = (sx * dc + ((hd >> 10) % dc as u64) as i32, sy * dc + ((hd >> 30) % dc as u64) as i32);
    (hd % 1000) < u64::from(ar.decor_chance) && dist2(ax - cx, ay - cy) <= i64::from(ar.decor_r).pow(2)
}

fn dec(out: &mut Vec<u8>, v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    let mut n = v;
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[i..]);
}

fn sgr(out: &mut Vec<u8>, head: &[u8], c: Rgb, bg: Option<Rgb>) {
    out.extend_from_slice(head);
    for (k, v) in [c.0, c.1, c.2].into_iter().enumerate() {
        if k > 0 {
            out.push(b';');
        }
        dec(out, v.into());
    }
    if let Some(b) = bg {
        out.extend_from_slice(b";48;2;");
        for (k, v) in [b.0, b.1, b.2].into_iter().enumerate() {
            if k > 0 {
                out.push(b';');
            }
            dec(out, v.into());
        }
    }
    out.push(b'm');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Content;
    use crate::data::tests::texts;
    use crate::director::Director;
    use crate::iso::Heading;
    use crate::sim::{Input, PickChoice};

    // re-pinned 2026-11-08: arrows with distance + glow, chest sprite, boss bar, strip row A removed, panel XP/powers/foe tiers/morale bars, bottom bar (reviewed in snapshot)
    const GOLDEN: u64 = 0x24a8_f76b_50d4_6263;

    fn fnv(b: &[u8]) -> u64 {
        b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3))
    }

    /// Pillar arena on every decor super-cell (render-only override: the sim never reads it).
    fn pillars(sim: &mut Sim) {
        sim.arena.template = crate::floorgen::Template::Pillars;
        sim.arena.decor_chance = 1000;
        sim.arena.decor_r = 3000;
    }

    /// Fixed state: seed 7, 40 s of scripted play, 120x40 frame. Returns (hash, bytes, [changed,
    /// bytes] per frame, compose ns per frame).
    fn golden(c: &Content, solid: bool) -> (u64, usize, Vec<usize>, Vec<u128>) {
        let mut sim = Sim::new(c, 7, None);
        if solid {
            pillars(&mut sim);
        }
        let mut d = Director::new(7);
        let mut ui = Ui::new(true);
        let mut f = Frame::new(c, 120, 40);
        let (mut sizes, mut ns) = (Vec::new(), Vec::new());
        for t in 0..1200u32 {
            d.tick(&mut sim);
            let mv = [(1, 0), (0, 1), (-1, 0), (0, -1)][(t / 45 % 4) as usize];
            sim.step(&Input { mv, autofire: true, pick: Some(PickChoice::Card(0)), ..Input::default() });
            if t >= 1100 {
                let t0 = std::time::Instant::now();
                f.compose(&sim, &View { ui: &ui, now_ms: u64::from(t) * 33, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
                ns.push(t0.elapsed().as_nanos());
                sizes.push(f.emit());
                sizes.push(f.out.len());
            }
        }
        ui.heading = ui.heading.right();
        // the turn starts at 100 s and has ended 1 s later
        for now_ms in [100_000, 101_000] {
            f.invalidate();
            f.compose(&sim, &View { ui: &ui, now_ms, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.emit();
        }
        (fnv(&f.out), f.out.len(), sizes, ns)
    }

    #[test]
    fn golden_frame_hash_and_palette_control() {
        let c = Content::parse(&texts()).unwrap();
        let (h, len, _, _) = golden(&c, true);
        eprintln!("GOLDEN hash {h:#x} bytes {len}");
        assert_eq!(h, GOLDEN, "golden frame changed (bytes {len})");
        let mut t = texts();
        t[2] = t[2].replacen("\"kin\":\"#b6ff3b\"", "\"kin\":\"#b6ff3c\"", 1);
        let (h2, _, _, _) = golden(&Content::parse(&t).unwrap(), true);
        assert_ne!(h2, h, "negative control: 1 palette entry must change the hash");
    }

    #[test]
    fn typical_frames_fit_budget_and_idle_frame_is_empty() {
        let c = Content::parse(&texts()).unwrap();
        for solid in [false, true] {
            let (_, full, sizes, mut ns) = golden(&c, solid);
            let mut bytes: Vec<usize> = sizes.chunks(2).map(|p| p[1]).collect();
            bytes.sort_unstable();
            ns.sort_unstable();
            let (p50, p99) = (bytes[bytes.len() / 2], bytes[bytes.len() * 99 / 100]);
            let (t50, t99) = (ns[ns.len() / 2], ns[ns.len() * 99 / 100]);
            eprintln!("FRAME pillars={solid} bytes p50 {p50} p99 {p99} full {full} compose us p50 {} p99 {}", t50 / 1000, t99 / 1000);
            assert!(p50 <= 64 * 1024 && p99 <= 64 * 1024, "p50 {p50} p99 {p99}");
            assert!(t99 <= 2_000_000, "compose p99 {t99} ns");
        }
        let mut f = Frame::new(&c, 120, 40);
        let sim = Sim::new(&c, 1, None);
        let ui = Ui::new(true);
        let v = View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 };
        f.compose(&sim, &v);
        assert!(f.emit() > 0);
        f.compose(&sim, &v);
        assert_eq!((f.emit(), f.out.len()), (0, 0), "unchanged state => no bytes");
    }

    #[test]
    fn pointers_never_drawn_over_map_or_stats_box() {
        let c = Content::parse(&texts()).unwrap();
        let sim = Sim::new(&c, 7, None);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        // ring of far targets around the player: every arrow must land outside both boxes
        for k in 0..64 {
            let a = f64::from(k) * std::f64::consts::TAU / 64.0;
            let (x, y) = (sim.p.x + (a.cos() * 1.0e6) as i32, sim.p.y + (a.sin() * 1.0e6) as i32);
            f.pointer(&sim, x, y, c.pal.alarm);
        }
        let arrows = ['▶', '◀', '▲', '▼'];
        for r in 0..f.play_rows() {
            for col in 0..f.cols {
                let cell = f.cells[usize::from(r) * usize::from(f.cols) + usize::from(col)];
                if arrows.contains(&cell.ch.ch()) {
                    assert!(!f.boxed(col, r), "arrow inside a box at {col},{r}");
                }
            }
        }
    }

    #[test]
    fn pick_cards_drawn_in_strip_not_play_area() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 2, None);
        let mut d = Director::new(2);
        while sim.offer.is_none() {
            d.tick(&mut sim);
            sim.step(&Input { autofire: true, ..Input::default() });
        }
        let mut ui = Ui::new(true);
        ui.sync(true, 0);
        let mut f = Frame::new(&c, 120, 40);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let row = |r: usize| f.cells[r * 120..(r + 1) * 120].iter().map(|c| c.ch.ch()).collect::<String>();
        let name = sim.offer.and_then(|o| o.cards[0]).map(|k| c.skill(k.skill).name.clone()).unwrap();
        assert!(row(39).contains("-- PICK --"));
        // the picker is a popup in the play area: the card name shows there, the strip only points at it
        assert!((0..37).any(|r| row(r).contains(&format!("1 {name}"))), "popup shows card 1");
        assert!(!row(38).contains(&name), "{}", row(38));
        assert!(row(38).contains("popup above"));
    }

    #[test]
    fn player_never_covered_and_strip_reserved() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 1, None);
        let mut f = Frame::new(&c, 120, 40);
        let ui = Ui::new(true);
        let fam = c.fam_ids().next().unwrap();
        let _ = sim.command(crate::sim::Src::Builtin, crate::sim::Cmd::Spawn { fam, n: std::num::NonZeroU16::new(5).unwrap(), tier: Tier::Elite, form: crate::data::Formation::Ring });
        for k in 0..sim.e.len() {
            sim.e.x[k] = 0;
            sim.e.y[k] = 0;
        }
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let (cc, rc) = f.centre();
        let n = usize::from(c.view.actor_rows[0]);
        let pi = (rc as usize - n) * 120 + cc as usize;
        assert_eq!(f.cells[pi].ch.ch(), '@');
        assert!(f.cells[(40 - 2) * 120..].iter().all(|cell| cell.bg == c.pal.hud_bg || cell.bg == c.pal.bg));
    }

    fn headings() -> [Heading; 4] {
        let n = Heading::default();
        [n, n.right(), n.right().right(), n.left()]
    }

    /// Fixed iso scene: pillar arena, trash on a 1.6 m grid around the player (none inside a block).
    fn iso_scene(c: &Content) -> Sim<'_> {
        let mut sim = Sim::new(c, 3, None);
        pillars(&mut sim);
        let fam = c.fam_ids().next().unwrap();
        let _ = sim.command(crate::sim::Src::Builtin, crate::sim::Cmd::Spawn { fam, n: std::num::NonZeroU16::new(121).unwrap(), tier: Tier::Trash, form: crate::data::Formation::Ring });
        let fc = c.view.floor_cell;
        for k in 0..sim.e.len() {
            let (gx, gy) = ((k % 11) as i32 - 5, (k / 11) as i32 - 5);
            let (x, y) = (sim.p.x + gx * 1600 + 700, sim.p.y + gy * 1600 + 300);
            let away = solid(&sim.arena, fc, x.div_euclid(fc), y.div_euclid(fc)) || k >= 121;
            (sim.e.x[k], sim.e.y[k]) = if away { (sim.p.x + 200_000, sim.p.y) } else { (x, y) };
        }
        sim
    }

    /// O-iso: block faces in >= 3 shade classes, standing stacks (head over body over shadow),
    /// shadow cells, and >= 1 actor hidden by a nearer block. Ok = frame hash.
    fn o_iso(c: &Content, h: Heading) -> Result<u64, String> {
        let sim = iso_scene(c);
        let mut ui = Ui::new(true);
        ui.heading = h;
        let mut f = Frame::new(c, 120, 40);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let pal = &c.pal;
        let play = &f.cells[..120 * 37];
        let classes = pal.face.iter().filter(|g| play.iter().any(|cell| cell.ch == **g)).count();
        let shadows = play.iter().filter(|cell| cell.ch == pal.shadow).count();
        let n = usize::from(c.view.actor_rows[1]);
        let (mut stacks, mut hidden) = (0, 0);
        for k in 0..sim.e.len() {
            let (x, y) = (f64::from(sim.e.x[k]), f64::from(sim.e.y[k]));
            let Some(i) = f.cell_of(x, y) else { continue };
            let d = f.vd(x, y);
            if n >= 1 && i >= n * 120 && f.cells[i].ch == pal.shadow && (1..n).all(|r| f.cells[i - r * 120].ch == pal.body) && f.cells[i - n * 120].ch == c.fam(sim.e.fam[k]).glyph {
                stacks += 1;
            }
            if (0..=n).any(|r| i >= r * 120 && f.bd[i - r * 120] > d) {
                hidden += 1;
            }
        }
        eprintln!("O-iso {} classes {classes} shadows {shadows} stacks {stacks} hidden {hidden}", h.name());
        match () {
            () if classes < 3 => Err(format!("face classes {classes} < 3")),
            () if n < 2 || stacks == 0 => Err(format!("no standing stack of >= 2 rows (rows {n}, stacks {stacks})")),
            () if shadows == 0 => Err("no shadow cells".into()),
            () if hidden == 0 => Err("no actor occluded by a block".into()),
            () => Ok(fnv(f.plain().as_bytes()) ^ fnv(&f.cells.iter().flat_map(|cell| [cell.fg.0, cell.fg.1, cell.fg.2]).collect::<Vec<u8>>())),
        }
    }

    #[test]
    fn o_iso_four_headings_and_flat_control_fails() {
        let c = Content::parse(&texts()).unwrap();
        let mut hashes: Vec<u64> = headings().iter().map(|&h| o_iso(&c, h).unwrap()).collect();
        hashes.sort_unstable();
        hashes.dedup();
        assert_eq!(hashes.len(), 4, "heading change must change the frame");
        let mut t = texts();
        t[1] = t[1].replacen("\"block_rows\":[7,2],\"actor_rows\":[2,2,3]", "\"block_rows\":[0,0],\"actor_rows\":[0,0,0]", 1);
        let flat = Content::parse(&t).unwrap();
        assert_eq!(flat.view.block_rows, [0, 0], "armed: flat data loaded");
        for h in headings() {
            assert!(o_iso(&flat, h).is_err(), "negative control: flat render must fail O-iso at {}", h.name());
        }
    }

    /// (lit, dark) count of the leftmost cell of every block run per row (= the left side face).
    fn left_faces(c: &Content, h: Heading) -> (usize, usize) {
        let sim = iso_scene(c);
        let mut ui = Ui::new(true);
        ui.heading = h;
        let mut f = Frame::new(c, 120, 40);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let is_block = |i: usize| f.bd[i] != i32::MIN;
        let lefts: Vec<Glyph> = (0..37 * 120).filter(|&i| is_block(i) && (i % 120 == 0 || !is_block(i - 1))).map(|i| f.cells[i].ch).collect();
        (lefts.iter().filter(|g| **g == c.pal.face[1]).count(), lefts.iter().filter(|g| **g == c.pal.face[2]).count())
    }

    #[test]
    fn rotation_swaps_lit_side() {
        let c = Content::parse(&texts()).unwrap();
        let [n, e, s, _] = headings().map(|h| left_faces(&c, h));
        eprintln!("left faces (lit, dark) N {n:?} E {e:?} S {s:?}");
        assert!(n.1 > n.0 && e.0 > e.1 && s.1 > s.0, "N/S left side dark, E left side lit");
    }

    fn v(ui: &Ui, now_ms: u64, alpha: f64) -> View<'_> {
        View { ui, now_ms, alpha, phase: "BUILD", last_bytes: 0 }
    }

    /// Camera screen position (cols, rows x 2 = col widths: cells are 2:1) at heading N.
    fn cam_at(f: &Frame) -> (f64, f64) {
        let (col, row) = Proj::at(0.0).screen(f.cam.x, f.cam.y);
        (col, 2.0 * row)
    }

    /// Scripted walk at 60 fps on a fake clock: sim 30 Hz, render every 1000/60 ms with alpha
    /// (`interp` false = alpha pinned to 1: the camera target steps once per tick, the judder
    /// control). Returns per frame (camera screen position, bytes).
    fn walk(c: &Content, frames: u32, script: impl Fn(u32) -> Input, interp: bool) -> Vec<((f64, f64), usize)> {
        let mut sim = Sim::new(c, 7, None);
        pillars(&mut sim);
        let ui = Ui::new(true);
        let mut f = Frame::new(c, 120, 40);
        let mut out = Vec::new();
        for k in 0..frames {
            if k % 2 == 0 {
                sim.step(&script(k));
            }
            let alpha = if interp { f64::from(k % 2) / 2.0 } else { 1.0 };
            f.compose(&sim, &v(&ui, u64::from(k) * 1000 / 60, alpha));
            f.emit();
            out.push((cam_at(&f), f.out.len()));
        }
        out
    }

    fn steps(cams: &[(f64, f64)]) -> Vec<(f64, f64)> {
        cams.windows(2).map(|w| (w[1].0 - w[0].0, w[1].1 - w[0].1)).collect()
    }

    /// O-smooth: the camera moves at most 1.5 col widths per frame (a dodge included) and no screen
    /// axis moves both ways during a straight walk (jitter). X = 1.5: the 200 ms follow keeps a 3 m
    /// dodge under it; a snap camera does not.
    fn o_smooth(cams: &[(f64, f64)]) -> Result<f64, String> {
        let s = steps(cams);
        let max = s.iter().fold(0.0f64, |m, d| m.max(d.0.hypot(d.1)));
        let both_ways = |a: fn(&(f64, f64)) -> f64| s.iter().any(|d| a(d) > 1e-9) && s.iter().any(|d| a(d) < -1e-9);
        match () {
            () if max > 1.5 => Err(format!("camera step {max:.2} > 1.5 col widths")),
            () if both_ways(|d| d.0) => Err("camera col reversed (jitter)".into()),
            () if both_ways(|d| d.1) => Err("camera row reversed (jitter)".into()),
            () => Ok(max),
        }
    }

    /// Most a camera step may change from one frame to the next, col widths per frame.
    /// Why this metric: judder is an uneven cadence (move, stop, move: at 60 fps over a 30 Hz sim the
    /// un-interpolated step alternates 0 / 2v), and a snap camera starts and stops at the full walk
    /// step v (~0.35 col widths per frame at 5 m/s) in one frame. Both are a large change of step. A
    /// smooth follow changes its step by about (1 - exp(-dt / follow_ms)) v = 0.08 v per frame, so
    /// JERK = v / 4 passes it with margin and fails both controls.
    const JERK: f64 = 0.09;

    fn o_jerk(cams: &[(f64, f64)]) -> Result<f64, String> {
        let s = steps(cams);
        let max = s.windows(2).fold(0.0f64, |m, w| m.max((w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)));
        if max > JERK { Err(format!("camera step changed by {max:.3} > {JERK} col widths in one frame")) } else { Ok(max) }
    }

    fn snap_content() -> Content {
        let mut t = texts();
        t[1] = t[1].replacen("\"follow_ms\":200", "\"follow_ms\":0", 1);
        Content::parse(&t).unwrap()
    }

    #[test]
    fn o_smooth_follow_and_snap_camera_control_fails() {
        let c = Content::parse(&texts()).unwrap();
        // screen-right in world for heading N: (1, -1), one dodge at frame 40
        let dodge = |k| Input { mv: (1, -1), dodge: k == 40, ..Input::default() };
        let cams: Vec<(f64, f64)> = walk(&c, 120, dodge, true).iter().map(|r| r.0).collect();
        let max = o_smooth(&cams).unwrap();
        eprintln!("O-smooth max step {max:.3} cols, moved {:.1} cols", cams[119].0 - cams[0].0);
        assert!(cams[119].0 - cams[0].0 > 20.0, "armed: the camera really followed the walk");
        let cams: Vec<(f64, f64)> = walk(&snap_content(), 120, dodge, true).iter().map(|r| r.0).collect();
        eprintln!("snap control: {:?}", o_smooth(&cams));
        assert!(o_smooth(&cams).is_err(), "negative control: snap camera must fail O-smooth");
    }

    #[test]
    fn o_jerk_walk_and_stop_without_dodge_and_snap_and_judder_controls_fail() {
        let c = Content::parse(&texts()).unwrap();
        // world +x: screen right AND down, so both screen axes move; walk 1 s, stop 1 s
        let go_stop = |k| Input { mv: if k < 60 { (1, 0) } else { (0, 0) }, ..Input::default() };
        let cams = |c: &Content, interp: bool| walk(c, 120, go_stop, interp).iter().map(|r| r.0).collect::<Vec<_>>();
        let smooth = cams(&c, true);
        let (sm, jm) = (o_smooth(&smooth).unwrap(), o_jerk(&smooth).unwrap());
        let moved = (smooth[119].0 - smooth[0].0, smooth[119].1 - smooth[0].1);
        eprintln!("O-jerk smooth: max step {sm:.3} max jerk {jm:.4}, moved {moved:.1?}");
        assert!(moved.0 > 10.0 && moved.1 > 10.0, "armed: the camera moved on both axes");
        let snap = cams(&snap_content(), true);
        eprintln!("snap control (no dodge): O-smooth {:?} O-jerk {:?}", o_smooth(&snap), o_jerk(&snap));
        assert!(o_smooth(&snap).is_ok(), "the snap camera passes the step bound without a dodge (audit finding)");
        assert!(o_jerk(&snap).is_err(), "negative control: snap camera must fail O-jerk without a dodge");
        let judder = cams(&snap_content(), false);
        eprintln!("judder control (alpha 1, snap): O-jerk {:?}", o_jerk(&judder));
        assert!(o_jerk(&judder).is_err(), "negative control: un-interpolated camera must fail O-jerk");
    }

    #[test]
    fn interpolation_moves_actors_between_ticks() {
        assert_eq!(Frame::lerp(0.5, Some((0, 0)), 1000, -2000), (500.0, -1000.0));
        assert_eq!(Frame::lerp(0.5, None, 1000, -2000), (1000.0, -2000.0), "new actor: no history, drawn where it is");
    }

    /// O-interp: alpha 0 draws every moved actor exactly at the previous tick, alpha 1 exactly at the
    /// current one. NEG: alpha 0.5 matches neither.
    #[test]
    fn o_interp_alpha_0_and_1_exact_and_wrong_alpha_fails() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let mut d = Director::new(7);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let inp = Input { mv: (1, 0), autofire: true, pick: Some(PickChoice::Card(0)), ..Input::default() };
        for _ in 0..300 {
            d.tick(&mut sim);
            sim.step(&inp);
        }
        f.compose(&sim, &v(&ui, 0, 1.0));
        let prev: std::collections::HashMap<u32, (i32, i32)> = (0..sim.e.len()).map(|k| (sim.e.id[k], (sim.e.x[k], sim.e.y[k]))).collect();
        d.tick(&mut sim);
        sim.step(&inp);
        let moved: Vec<usize> = (0..sim.e.len()).filter(|&k| prev.get(&sim.e.id[k]).is_some_and(|p| *p != (sim.e.x[k], sim.e.y[k]))).collect();
        let at = |f: &Frame, a: f64| {
            moved.iter().all(|&k| {
                let (px, py) = prev[&sim.e.id[k]];
                f.pos[k] == if a == 0.0 { (f64::from(px), f64::from(py)) } else { (f64::from(sim.e.x[k]), f64::from(sim.e.y[k])) }
            })
        };
        f.compose(&sim, &v(&ui, 33, 0.0));
        let a0 = at(&f, 0.0);
        f.compose(&sim, &v(&ui, 34, 1.0));
        let a1 = at(&f, 1.0);
        f.compose(&sim, &v(&ui, 35, 0.5));
        let (neg0, neg1) = (at(&f, 0.0), at(&f, 1.0));
        eprintln!("O-interp moved {} alpha0 exact {a0} alpha1 exact {a1} | NEG alpha 0.5 as 0 {neg0} as 1 {neg1}", moved.len());
        assert!(moved.len() > 5, "armed: actors moved this tick");
        assert!(a0 && a1, "alpha 0 / 1 must be the previous / current tick exactly");
        assert!(!neg0 && !neg1, "negative control: a wrong alpha must not match");
    }

    /// Pick pause at 60 fps with play.rs's alpha (it keeps cycling 0..1 while the sim is frozen):
    /// walk into the first pick, hold it 2 s, pick, walk on. `forget` = NEG: the frame forgets each
    /// frame when it saw the tick (pre-P9 behaviour). Returns (pause frame range, per frame
    /// (bytes, camera, drawn player screen position)).
    fn pause(c: &Content, forget: bool) -> (std::ops::Range<usize>, Vec<PauseFrame>) {
        let walk_in = Input { mv: (1, 0), autofire: true, ..Input::default() };
        let run = |sim: &mut Sim, d: &mut Director, inp: &Input| {
            d.tick(sim);
            sim.step(inp);
        };
        // find the pick tick without rendering, then replay (same seed: same run) and render the last 1 s
        let (mut sim, mut d) = (Sim::new(c, 2, None), Director::new(2));
        while sim.offer.is_none() {
            run(&mut sim, &mut d, &walk_in);
        }
        let t0 = sim.tick;
        let (mut sim, mut d) = (Sim::new(c, 2, None), Director::new(2));
        while sim.tick + 30 < t0 {
            run(&mut sim, &mut d, &walk_in);
        }
        let ui = Ui::new(true);
        let mut f = Frame::new(c, 120, 40);
        let (mut out, mut start, mut end) = (Vec::new(), None, None);
        for k in 0..300u32 {
            if k % 2 == 0 {
                let held = sim.offer.is_some().then(|| *start.get_or_insert(out.len()));
                if end.is_none() && held.is_some_and(|s| out.len() >= s + 120) {
                    end = Some(out.len());
                    run(&mut sim, &mut d, &Input { pick: Some(PickChoice::Card(0)), ..walk_in });
                } else {
                    run(&mut sim, &mut d, &walk_in);
                }
            }
            let now_ms = u64::from(k) * 1000 / 60;
            if forget {
                f = Frame::new(c, 120, 40);
            }
            let alpha = f64::from(k % 2) / 2.0;
            f.compose(&sim, &v(&ui, now_ms, alpha));
            f.emit();
            let (px, py) = Frame::lerp(alpha, Some(f.pprev), sim.p.x, sim.p.y);
            let (pc, pr) = Proj::at(0.0).screen(px, py);
            out.push((f.out.len(), cam_at(&f), (pc, 2.0 * pr)));
        }
        let s = start.unwrap_or(0);
        (s..end.unwrap_or(s), out)
    }

    /// (bytes, camera, drawn player) of one frame, screen col widths.
    type PauseFrame = (usize, (f64, f64), (f64, f64));

    #[test]
    fn pick_pause_frame_is_still_and_resume_is_smooth_and_forget_control_flickers() {
        let c = Content::parse(&texts()).unwrap();
        assert!(c.opt.pick_pauses, "armed: picks pause the sim");
        let (r, out) = pause(&c, false);
        assert!(r.len() == 120, "armed: the sim was frozen for 120 frames ({r:?})");
        let held = &out[r.clone()];
        // the camera finishes its follow (< follow_ms x 4 = 48 frames), then the frame is still
        let tail: Vec<usize> = held[60..].iter().map(|o| o.0).collect();
        let cams: Vec<(f64, f64)> = held.iter().map(|o| o.1).collect();
        eprintln!("PAUSE bytes first 6 {:?} last 60 max {} sum {}", held[..6].iter().map(|o| o.0).collect::<Vec<_>>(), tail.iter().max().unwrap_or(&0), tail.iter().sum::<usize>());
        assert!(tail.iter().all(|b| *b == 0), "frozen sim: still frame after the camera settles {tail:?}");
        assert!(o_smooth(&cams).is_ok(), "frozen sim: camera converges without reversing {:?}", o_smooth(&cams));
        // resume: no snap or pop of the camera or the drawn player across the pick
        let around = &out[r.end - 10..r.end + 20];
        let (cams, players): (Vec<_>, Vec<_>) = around.iter().map(|o| (o.1, o.2)).unzip();
        let pmax = steps(&players).iter().fold(0.0f64, |m, d| m.max(d.0.hypot(d.1)));
        eprintln!("RESUME camera {:?} player max step {pmax:.3}", o_smooth(&cams));
        assert!(o_smooth(&cams).is_ok() && pmax <= 1.5, "resume pops: player step {pmax}");
        assert!(players[29] != players[0], "armed: the player walks on after the pick");
        let (r, out) = pause(&c, true);
        let tail: Vec<usize> = out[r][60..].iter().map(|o| o.0).collect();
        eprintln!("NEG forget: last 6 bytes {:?}", &tail[54..]);
        assert!(tail.iter().filter(|b| **b > 0).count() >= 30, "negative control: cycling alpha on a held tick must flicker");
    }

    /// Bytes per frame at 60 fps: walk, idle, and a q/e turn (turn_ms of whole-screen change).
    #[test]
    fn scroll_and_turn_bytes_measured_against_budget() {
        let c = Content::parse(&texts()).unwrap();
        let pct = |v: &[usize], p: usize| {
            let mut s = v.to_vec();
            s.sort_unstable();
            s[(s.len() - 1) * p / 100]
        };
        let walk_b: Vec<usize> = walk(&c, 240, |_| Input { mv: (1, -1), ..Input::default() }, false).iter().skip(30).map(|r| r.1).collect();
        let idle: Vec<usize> = walk(&c, 120, |_| Input::default(), false).iter().skip(30).map(|r| r.1).collect();
        // turn: idle scene, press e at frame 10, render 30 frames
        let mut sim = Sim::new(&c, 7, None);
        pillars(&mut sim);
        let mut ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let mut turn = Vec::new();
        for k in 0..40u64 {
            if k == 10 {
                ui.heading = ui.heading.right();
            }
            f.compose(&sim, &View { ui: &ui, now_ms: k * 1000 / 60, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.emit();
            turn.push(f.out.len());
        }
        let turn = &turn[10..30];
        eprintln!(
            "BYTES 60fps walk p50 {} p99 {} max {} | 3 consecutive {:?} | idle p99 {} | turn frames {} sum {} max {}",
            pct(&walk_b, 50),
            pct(&walk_b, 99),
            pct(&walk_b, 100),
            &walk_b[100..103],
            pct(&idle, 99),
            turn.iter().filter(|b| **b > 0).count(),
            turn.iter().sum::<usize>(),
            pct(turn, 100)
        );
        assert!(pct(&walk_b, 50) <= 64 * 1024 && pct(&walk_b, 99) <= 160 * 1024, "walk over budget");
        assert!(pct(&idle, 99) <= 2048, "idle: only the HUD clock changes");
        assert!(pct(turn, 100) <= 416 * 1024, "turn frame over full-screen size");
    }

    /// Player standing just behind a pillar (block nearer the camera, below on screen).
    fn cut_scene(c: &Content) -> Sim<'_> {
        let mut sim = Sim::new(c, 3, None);
        pillars(&mut sim);
        let fc = c.view.floor_cell;
        let h = Proj::at(0.0);
        for k in 0..40_000i32 {
            let (px, py) = ((k % 200 - 100) * 500, (k / 200 - 100) * 500);
            let front = (-2..=2).any(|fy| {
                (-2..=2).any(|fx| {
                    let (bx, by) = (px.div_euclid(fc) + fx, py.div_euclid(fc) + fy);
                    let (dc, dr) = h.project(f64::from(bx * fc + fc / 2 - px), f64::from(by * fc + fc / 2 - py));
                    solid(&sim.arena, fc, bx, by) && dc.abs() <= 1 && (2..=4).contains(&dr)
                })
            });
            if front && !solid(&sim.arena, fc, px.div_euclid(fc), py.div_euclid(fc)) {
                (sim.p.x, sim.p.y) = (px, py);
                return sim;
            }
        }
        sim
    }

    /// Faces in the player window: (tall faces, silhouette cells).
    fn window_faces(c: &Content) -> (usize, usize) {
        let sim = cut_scene(c);
        let ui = Ui::new(true);
        let mut f = Frame::new(c, 120, 40);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let (cc, rc) = f.centre();
        let n = i32::from(c.view.actor_rows[0]);
        let cells: Vec<Glyph> = ((rc - n - 1)..=(rc + 1)).flat_map(|r| ((cc - 3)..=(cc + 3)).map(move |col| (r * 120 + col) as usize)).map(|i| f.cells[i].ch).collect();
        let tall = cells.iter().filter(|g| **g == c.pal.face[0] || **g == c.pal.face[1]).count();
        (tall, cells.iter().filter(|g| **g == c.pal.face[2]).count())
    }

    #[test]
    fn cutaway_keeps_player_window_clear_and_control_without_cut_covers_it() {
        let c = Content::parse(&texts()).unwrap();
        let (tall, sil) = window_faces(&c);
        assert_eq!(tall, 0, "rule: no tall block face in the player window");
        assert!(sil > 0, "armed: the pillar in front is drawn as a silhouette ({sil})");
        let mut t = texts();
        t[1] = t[1].replacen("\"cut_cols\":10", "\"cut_cols\":0", 1);
        let (tall, _) = window_faces(&Content::parse(&t).unwrap());
        assert!(tall > 0, "negative control: without cutaway the pillar covers the player window");
    }
}
