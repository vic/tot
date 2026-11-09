//! Pure render: sim state -> cell grid -> diff bytes (design sec 5). One buffer per frame, diff
//! against the front buffer, hand decimal writer (no core::fmt in the frame path, PF sec 1c).
use crate::data::{Content, Glyph, Hue, Rgb, Role, Slot};
use crate::face::{self, Face, Mood};
use crate::input::{Mode, Ui};
use crate::floorgen::{Arena, Template};
use crate::cam::Cam;
use crate::iso::{Heading, Proj};
use crate::sim::{Ability, BARK_TICKS, BarkKind, BossClass, CONF_PANIC, Cause, FxShape, Outcome, Shape, Side, Sim, TeleKind, Tier, Weather, dist2};
#[cfg(test)]
use crate::sim::ItemId;
use crate::text::{a, g, t};
use std::collections::HashSet;

mod heropane;
mod start;

/// Hero levels where the shoulders grow, and the quadrant glyphs (left, right) of each tier (tot-rcn.6).
/// shortcut: fixed in code, move to tuning data when a profile wants its own.
const HERO_TIERS: [u16; 3] = [5, 10, 20];
const SHOULDERS: [[char; 2]; 3] = [['▗', '▖'], ['▟', '▙'], ['█', '█']];

/// Shoulder glyphs (left, right) of `tier` (0 = none, past the last = the last).
fn shoulders(tier: usize, fg: Rgb) -> [Option<(Glyph, Rgb)>; 2] {
    let pair = tier.checked_sub(1).map(|t| SHOULDERS[t.min(SHOULDERS.len() - 1)]);
    [0, 1].map(|s| pair.and_then(|p| Glyph::of(p[s])).map(|g| (g, fg)))
}
/// Map box (top-left of the play area): cols x rows of the map.
const MAP_W: u16 = 26;
const MAP_H: u16 = 11;
/// Map cell = this many floor cells. shortcut: fixed zoom, upgrade when zoom is a player option.
const MAP_K: i32 = 4;
/// Stats panel (bottom-right of the play area).
const PANEL_W: u16 = 36;
const PANEL_H: u16 = PANEL_CAST_ROW + 1 + Cast::ALL.len() as u16;
/// Stats panel: power rows shown, first row of the foe list, XP bar width.
const PANEL_POWERS: usize = 4;
const MORALE_BAR_W: u16 = 10;
const FOOD_BAR_W: u16 = 8;
const PANEL_FOES_ROW: u16 = 12;
/// Hostile (family, tier) rows under FOES, then the CAST block (hero, allies, panicked, thieves).
const PANEL_FOE_ROWS: u16 = 5;
const PANEL_CAST_ROW: u16 = PANEL_FOES_ROW + 1 + PANEL_FOE_ROWS;
/// Width of the share bar on each foe / cast row.
const MINI_BAR_W: u16 = 6;
const XP_BAR_W: u16 = 18;
/// Chest glow pulse period (ticks); the chest sprite is the adventure's `chest` art, centred on the chest cell.
const CHEST_PULSE: u32 = 8;
/// Bottom bar: size, orb width, face width (face rows come from `face::rows`).
const BAR_W: u16 = 76;
const BAR_H: u16 = 5;
const ORB_W: u16 = 9;
const FACE_W: u16 = 7;
/// Fed-thief gold glow strength (permille toward gold): base, per loot point, max.
const FED_GLOW_MIN: u32 = 250;
/// Fear tint strength on panicked foes (permille toward the fear colour).
const FEAR_TINT: u32 = 450;
const FED_GLOW_PER: u32 = 60;
const FED_GLOW_MAX: u32 = 800;
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
/// Arrows to pals, keepers and goblins: at most this many, within this distance (mm).
const PAL_CALLS: usize = 4;
const PAL_CALL_MM: i64 = 150_000;
/// Pal HELP!: blink period and sound-wave step (sim ticks).
const PAL_BLINK: u32 = 8;
const PAL_WAVE: u32 = 6;

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
/// Key rows at most (fewer when every key fits on one row, see `Frame::strip`).
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
    /// hero level last frame, and the running level-up flash (wall ms it began, level reached)
    level: Option<u16>,
    level_flash: Option<(u64, u16)>,
    /// the running hero shout: sim tick it began, screen cell (col, row) of the hero then
    shout_at: Option<(u32, (i32, i32))>,
    /// kill bursts (world x, y, sim tick of the death) and the boss ROAR (boss id, sim tick it appeared)
    pops: Vec<(i32, i32, u32)>,
    roars: Vec<(u32, u32)>,
    /// Doom face memory (moods, barks) for the bottom bar
    face: Face,
    /// key rows at the bottom: 1 when every play key, the mode tag and the sponsor button fit on one row, else 2
    strip: u16,
    /// frame clock (ms) when the death show began; None while alive
    died_ms: Option<u64>,
    /// weather last frame, and the change banner (weather, frame clock ms it turned)
    sky: Option<Weather>,
    sky_banner: Option<(Weather, u64)>,
    /// where the hero pane is drawn this frame (left col, top row); pointers keep off it
    pane_at: Option<(u16, u16)>,
    /// clickable cell spans this frame: [start, end) cell index and the URL (OSC 8 in `emit`)
    links: Vec<(usize, usize, String)>,
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
/// Colour of a key on the key rows and the hotkey bar: prominent, on the plain background.
fn key_fg(pal: &crate::data::Palette) -> Rgb {
    pal.role(Role::Accent)
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
            level: None,
            level_flash: None,
            shout_at: None,
            face: Face::default(),
            strip: strip_rows(c, cols),
            died_ms: None,
            sky: None,
            sky_banner: None,
            pane_at: None,
            links: Vec::new(),
            pops: Vec::with_capacity(POPS_MAX),
            roars: Vec::new(),
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
                self.pops.clear();
                self.roars.clear();
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
        let now = sim.tick;
        self.pops.retain(|p| now.saturating_sub(p.2) < POP_TICKS);
        let room = POPS_MAX.saturating_sub(self.pops.len());
        self.pops.extend(vanished(&self.prev, &self.cur).take(room).map(|(x, y)| (x, y, now)));
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
        self.rows.saturating_sub(self.strip)
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

    /// Left and right neighbours of the cell `up` rows above foot cell `i`.
    fn flank(&mut self, i: usize, up: u8, sides: [Option<(Glyph, Rgb)>; 2], p: Prio, d: i32) {
        let cols = usize::from(self.cols);
        let Some(row) = (i / cols).checked_sub(usize::from(up)) else { return };
        for ((ch, fg), x) in sides.into_iter().zip([(i % cols).checked_sub(1), Some(i % cols + 1).filter(|&x| x < cols)]).filter_map(|(s, x)| s.zip(x)) {
            self.put(row * cols + x, p, d, Ink { ch, fg, bg: None });
        }
    }

    fn tint(&mut self, i: usize, bg: Rgb) {
        if self.z[i] <= Prio::PlayerFx {
            self.z[i] = Prio::PlayerFx;
        }
        self.cells[i].bg = bg;
    }

    pub fn compose(&mut self, sim: &Sim, v: &View) {
        self.links.clear();
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
                let ar = sim.arena();
                let h = hash(ar.salt ^ ((fx as u64) << 32) ^ (fy as u32 as u64));
                let anchor = self.scr(f64::from(fx * fc + fc / 2), f64::from(fy * fc + fc / 2)) == (col, r);
                let ch = if anchor && (h % 1000) < u64::from(ar.density) { ar.glyphs[(h >> 20) as usize % ar.glyphs.len()] } else { Glyph::SPACE };
                // blank cells keep one fg so camera motion only re-sends cells whose glyph moved
                // a share of the floor glyphs wears the adventure's floor motif colour (blood, moss, neon)
                let tint = if (h >> 40) % 1000 < u64::from(pal.floor_motif.1) { pal.floor_motif.0 } else { pal.floor };
                let fg = if ch == Glyph::SPACE { pal.floor } else { self.fogged(c, tint, ax, ay) };
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
            if let crate::sim::Drop::Ore = sim.drops.kind[k]
                && let Some(i) = self.cell_of(x, y)
            {
                self.put(i, Prio::Pickup, self.vd(x, y), Ink { ch: sim.c.texts.glyph(g::ORE), fg: pal.text, bg: None });
                continue;
            }
            if let crate::sim::Drop::Food = sim.drops.kind[k]
                && let Some(i) = self.cell_of(x, y)
            {
                self.put(i, Prio::Pickup, self.vd(x, y), Ink { ch: sim.c.texts.glyph(g::FOOD), fg: pal.role(Role::Heal), bg: None });
                continue;
            }
            if let crate::sim::Drop::Magnet = sim.drops.kind[k]
                && let Some(i) = self.cell_of(x, y)
            {
                let glow = if (sim.tick / LOOT_PULSE).is_multiple_of(2) { mix(pal.xp, pal.bg, 400) } else { mix(pal.xp, pal.bg, 700) };
                self.put(i, Prio::Pickup, self.vd(x, y), Ink { ch: sim.c.texts.glyph(g::MAGNET), fg: pal.xp, bg: Some(glow) });
                continue;
            }
            if let crate::sim::Drop::Item(id) = sim.drops.kind[k] {
                self.loot(sim, x, y, id);
                continue;
            }
            if let Some(i) = self.cell_of(x, y) {
                let ch = sim.c.texts.glyph(g::XP_GEM);
                self.put(i, Prio::Pickup, self.vd(x, y), Ink { ch, fg: pal.xp, bg: None });
            }
        }
        for k in 0..sim.treasures.len() {
            let (x, y) = sim.treasures[k];
            self.chest(sim, f64::from(x), f64::from(y));
        }
        self.world_layer(sim);
        self.enemies(sim);
        self.fed(sim);
        self.sound_fx(sim);
        // 6. enemy shots (no id: drawn at the current tick)
        for k in 0..sim.shots.x.len() {
            let (x, y) = (f64::from(sim.shots.x[k]), f64::from(sim.shots.y[k]));
            if let Some(i) = self.cell_of(x, y) {
                self.put(i, Prio::IncomingHit, self.vd(x, y), Ink { ch: sim.c.texts.glyph(g::FOE_SHOT), fg: pal.of(c.fam(sim.shots.fam[k]).hue), bg: None });
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
        let head = Ink { ch: c.texts.glyph(g::HERO_HEAD), fg: pfg, bg: Some(pal.ring) };
        let worn = |s: Slot| sim.gear.worn(s).and_then(|id| c.items.get(usize::from(id.0))).map(|it| (it.glyph, pal.role(it.rarity.role())));
        // worn armour plates the torso in its rarity colour
        let body = match worn(Slot::Armor) {
            Some((_, fg)) => Ink { ch: c.texts.glyph(g::HERO_ARMOR), fg, bg: None },
            None => Ink { ch: pal.body, fg: pfg, bg: None },
        };
        let shadow = Ink { ch: pal.shadow, fg: pal.dim, bg: None };
        let n = c.view.actor_rows[0];
        self.stand(pi, i32::MAX, n, Prio::PlayerCore, [head, body, shadow]);
        // beside the stack: weapon in the right hand, charm in the left, shoulders that grow per level tier
        if n > 0 {
            self.flank(pi, 1, [worn(Slot::Charm), worn(Slot::Weapon)], Prio::PlayerCore, i32::MAX);
            self.flank(pi, n, shoulders(HERO_TIERS.iter().filter(|&&l| sim.p.level >= l).count(), pal.player), Prio::PlayerCore, i32::MAX);
        }
        if self.level.is_some_and(|l| sim.p.level > l) {
            self.level_flash = Some((v.now_ms, sim.p.level));
        }
        self.level = Some(sim.p.level);
        self.level_fx(sim, v.now_ms, (pcol, prow));
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
        let (pal, v, ar) = (&c.pal, &c.view, sim.arena());
        let rows = match ar.template {
            Template::Open | Template::Pillars => v.block_rows[0],
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
            let base = self.fogged(c, pal.wall, bx, by);
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
                    let (ch, mut fg) = ink(k);
                    // the lit faces carry the adventure's wall lore as shadow only: where a lore line has a mark, the
                    // face glyph stays and darkens (an engraving seen as shade, never as letters on the wall)
                    if k == 1 && !cutaway {
                        let line = c.texts.pick(t::WALL_LORE, hash((fx as u64) << 32 ^ fy as u32 as u64) as usize);
                        let at = (r - (br - hh - hz)) as usize * (2 * hw + 1) as usize + (col - (bc - hw)) as usize;
                        if line.chars().nth(at % line.chars().count().max(1)).is_some_and(|m| m != ' ') {
                            fg = mix(fg, pal.wall_motif.0, pal.wall_motif.1);
                        }
                    }
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
        // sight ends at the fog end: foes beyond it are not drawn (the spawn ring lies there, so none pops in)
        let view2 = i64::from(c.view.fog_end).pow(2);
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
            // boss class reads like loot rarity: mini keeps the family hue, the others take colours no family uses (tot-751.7)
            let tint = match BossClass::of_kind(e.tier[k], &fam.kind) {
                Some(BossClass::Super) => pal.role(Role::Legendary),
                Some(BossClass::Uber) => pal.role(Role::Danger),
                Some(BossClass::Mini) | None => pal.of(fam.hue),
            };
            let hue = self.fogged(c, tint, x, y);
            let d = self.vd(x, y);
            let mk = |ch: Glyph, fg: Rgb| match e.side[k] {
                Side::Hostile => Ink { ch, fg, bg: None },
                Side::Hijacked => Ink { ch, fg: pal.bg, bg: Some(fg) }, // inverse video (design sec 5.4)
            };
            let shadow = Ink { ch: pal.shadow, fg: mix(pal.floor, pal.bg, 300), bg: None };
            let body = mk(pal.body, mix(hue, pal.bg, 300));
            // a foe that ate loot wears it: shoulders per ability owned, as the hero's per level tier (tot-76k)
            let fed = Ability::ALL.iter().filter(|a| a.owned_at(e.loot[k])).count();
            match e.tier[k] {
                Tier::Trash => {
                    let ch = match (e.side[k], self.dens[i]) {
                        (Side::Hostile, 3..=4) => pal.density[0],
                        (Side::Hostile, 5..=7) => pal.density[1],
                        (Side::Hostile, 8..) => pal.density[2],
                        _ => fam.glyph,
                    };
                    self.stand(i, d, c.view.actor_rows[1], Prio::Trash, [mk(ch, hue), body, shadow]);
                    self.flank(i, c.view.actor_rows[1], shoulders(fed, hue), Prio::Trash, d);
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
                    self.flank(i, n, shoulders(fed, hue), Prio::EliteBoss, d);
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
        // the cloud grows with the class, as the sim body radius does
        let cls = BossClass::of_kind(Tier::Boss, &c.fam(sim.e.fam[k]).kind).unwrap_or(BossClass::Mini);
        let (w, h) = (cls.radius(i32::from(def.w)), cls.radius(i32::from(def.h)));
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
            // a wide glyph takes two cells: the glyph, then its hidden right half (it never splits at the edge)
            let wide = g.is_wide() && x + 1 < cols;
            self.cells[i].ch = if g.is_wide() && !wide { Glyph::SPACE } else { g };
            self.cells[i].fg = fg;
            if wide {
                self.cells[i + 1].ch = Glyph::CONT;
                self.cells[i + 1].fg = fg;
                x += 1;
            }
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
        // round up: any nonzero value shows at least one cell, so a minority is never an empty bar
        let fill = ((val.max(0) * i64::from(w) + max.max(1) - 1) / max.max(1)).min(i64::from(w)) as u16;
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
        let Some((x0, y0, w, h)) = self.popup_box(pal) else { return };
        let limit = sim.pick_timeout_ms();
        let left_ms = limit.saturating_sub(now_ms.saturating_sub(ui.pick_open_ms));
        let x = self.text(y0 + 1, x0 + 2, c.texts.get(t::PICK_TITLE), pal.alarm);
        self.num(y0 + 1, x, (left_ms / 1000) as i64, pal.alarm);
        for (k, card) in offer.cards.iter().enumerate() {
            let row = y0 + 3 + 4 * k as u16;
            let sel = k as u8 == ui.sel;
            let Some(card) = card else {
                self.text(row, x0 + 2, c.texts.get(t::PICK_NONE), pal.dim);
                continue;
            };
            let s = c.skill(card.skill);
            let rk = usize::from(card.rarity.min(3));
            let rar = (c.texts.get(t::RARITY[rk]), [pal.text, pal.of(Hue::Kin), pal.of(Hue::Therm), pal.chest][rk]);
            // the card reads in a glance: its rows tinted by rarity, a kind icon in the power colour, then the name
            if card.rarity > 0 {
                let i = usize::from(row) * usize::from(self.cols) + usize::from(x0 + 1);
                let tint = mix(rar.1, pal.hud_bg, CARD_TINT);
                for r in 0..3 {
                    for cell in &mut self.cells[i + r * usize::from(self.cols)..i + r * usize::from(self.cols) + usize::from(w - 2)] {
                        cell.bg = tint;
                    }
                }
            }
            let cursor = c.texts.get(t::PICK_CURSOR);
            let blank = " ".repeat(cursor.chars().count());
            let mut x = self.text(row, x0 + 2, if sel { cursor } else { &blank }, pal.alarm);
            x = self.num(row, x, k as i64 + 1, key_fg(pal));
            x = self.text(row, x + 1, kind_icon(s.kind), pal.of(s.hue));
            // power type colour: same as the stats pane
            x = self.text(row, x + 1, &s.name, pal.of(s.hue));
            x = self.text(row, x + 2, rar.0, rar.1);
            x = self.text(row, x + 2, c.texts.get(if card.new_rank == 1 { t::PICK_NEW } else { t::PICK_RANK }), pal.dim);
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

    /// Centred popup box, cleared, with top and bottom edges: (x0, y0, w, h). None if the screen is too small.
    fn popup_box(&mut self, pal: &crate::data::Palette) -> Option<(u16, u16, u16, u16)> {
        let (w, h) = (POPUP_W.min(self.cols.saturating_sub(4)), POPUP_H);
        if self.play_rows() < h {
            return None;
        }
        let (x0, y0) = ((self.cols - w) / 2, (self.play_rows() - h) / 2);
        let cols = usize::from(self.cols);
        for r in 0..h {
            let edge = r == 0 || r == h - 1;
            let cell = if edge { Cell { ch: Glyph::of('─').unwrap_or(Glyph::SPACE), fg: pal.of(Hue::Kin), bg: pal.hud_bg } } else { Cell { ch: Glyph::SPACE, fg: pal.text, bg: pal.hud_bg } };
            let i = usize::from(y0 + r) * cols + usize::from(x0);
            self.cells[i..i + usize::from(w)].fill(cell);
        }
        Some((x0, y0, w, h))
    }

    /// The shop: a plain-text list in the popup box.
    fn menu(&mut self, sim: &Sim, ui: &Ui) {
        let c = sim.c;
        let pal = &c.pal;
        // (title, rows of (text, colour)), cursor
        let (title, rows, sel): (String, Vec<(String, Rgb)>, Option<usize>) = match ui.mode {
            Mode::Shop => {
                let deals = sim.world.open_shop.and_then(|i| sim.world.shops.get(i)).map_or(&[][..], Vec::as_slice);
                let rows = deals.iter().zip(1..).filter_map(|(d, k)| c.items.get(usize::from(d.item.0)).map(|it| (c.texts.fill(t::SHOP_ROW, [&k.to_string(), &it.glyph.ch().to_string(), &it.name, &d.price.to_string()]), pal.role(it.rarity.role()))));
                let smith = sim.world.open_shop.and_then(|i| sim.world.npcs.get(i)).is_some_and(|p| matches!(p.npc, crate::sim::Npc::Blacksmith { .. }));
                let forge = if smith { c.texts.fill(t::SHOP_FORGE, [&sim.p.ore.to_string(), &crate::sim::FORGE_ORE.to_string()]) } else { String::new() };
                (c.texts.fill(t::SHOP_TITLE, [&sim.p.gold.to_string(), &forge]), rows.collect(), None)
            }
            // the skill tree and the gear live in the hero pane, not in a popup
            Mode::Play | Mode::Pick | Mode::Look | Mode::Cmd | Mode::Tree | Mode::Gear | Mode::Bags | Mode::Start => return,
        };
        let Some((x0, y0, w, h)) = self.popup_box(pal) else { return };
        self.text(y0 + 1, x0 + 2, &title, pal.alarm);
        let fit = usize::from(w) - 4;
        // scroll so the cursor row stays inside the box
        let shown = usize::from(h) - 4;
        let first = sel.unwrap_or(0).saturating_sub(shown - 1);
        for (k, (s, fg)) in rows.iter().enumerate().skip(first).take(shown) {
            let row = y0 + 3 + (k - first) as u16;
            let cursor = c.texts.get(t::PICK_CURSOR);
            let mark = if Some(k) == sel { cursor.to_string() } else { " ".repeat(cursor.chars().count()) };
            let line: String = format!("{mark}{s}").chars().take(fit).collect();
            self.text(row, x0 + 2, &line, *fg);
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
            // the digit is a key: highlighted like every key hint
            let x = self.text(row, hx, &(k + 1).to_string(), key_fg(pal));
            match sim.owned.get(usize::from(k)) {
                Some(o) => {
                    let s = c.skill(o.id);
                    let name: String = s.name.chars().take(usize::from(half / 2 - 3)).collect();
                    self.text(row, x + 1, &name, pal.of(s.hue));
                    // special reload gauge: fills as it reloads, full + bright = ready; passive powers say so
                    if s.kind.has_special() {
                        let full = i64::from(sim.special_reload(o.rank));
                        let ready = o.sp == 0;
                        self.bar(row + 1, hx + 2, full - i64::from(o.sp), full, half / 2 - 4, if ready { pal.of(s.hue) } else { pal.dim });
                    } else {
                        self.text(row + 1, hx + 2, c.texts.get(t::SPECIAL_PASSIVE), pal.dim);
                    }
                }
                None => {
                    self.text(row, x + 1, c.texts.get(t::SPECIAL_EMPTY), pal.dim);
                }
            }
        }
        // Doom face: mood from what just happened, eyes on the nearest foe on screen, barks above the bar
        let Some((fx, _)) = self.face_origin() else { return };
        self.face.observe(sim);
        let (cc, _) = self.centre();
        let near = (0..sim.e.len()).filter(|&k| sim.e.side[k] == Side::Hostile).map(|k| (dist2(sim.e.x[k] - sim.p.x, sim.e.y[k] - sim.p.y), k)).min();
        let toward = near.filter(|&(d2, _)| d2 <= i64::from(c.view.view_r).pow(2)).map(|(_, k)| self.scr(f64::from(sim.e.x[k]), f64::from(sim.e.y[k])).0 - cc);
        let fighting = near.is_some_and(|(d2, _)| d2 <= i64::from(c.view.crowd_r).pow(2)) && sim.bolts().next().is_some();
        let mood = self.face.mood(sim, fighting);
        let fg = match mood {
            Mood::Calm => pal.player,
            Mood::Fencing => pal.of(Hue::Therm),
            Mood::Pain | Mood::Grim => pal.alarm,
            Mood::Laugh => pal.xp,
            Mood::Frozen => pal.of(Hue::Code),
            Mood::Dead => pal.dim,
        };
        for (r, line) in face::rows(sim.c, mood, Face::eyes(sim, toward)).iter().enumerate() {
            self.text(y0 + 1 + r as u16, fx, line, fg);
        }
    }

    /// Level-up: an XP-coloured ring bursts out of the hero and a taunt floats above them for LEVEL_FLASH_MS.
    fn level_fx(&mut self, sim: &Sim, now_ms: u64, (pcol, prow): (i32, i32)) {
        let Some((start, level)) = self.level_flash else { return };
        let t = now_ms.saturating_sub(start);
        if t >= LEVEL_FLASH_MS {
            self.level_flash = None;
            return;
        }
        let pal = &sim.c.pal;
        let r = ring_radius(t);
        let star = sim.c.texts.glyph(g::STAR);
        let n = LEVEL_RING_POINTS;
        let mid = prow - i32::from(sim.c.view.actor_rows[0]) / 2;
        // bright colours chase round the ring, each star on a glow of its own colour, so it cannot be missed (tot-f6r)
        let hues = LEVEL_GLOW.map(|role| pal.role(role));
        let spin = (t / LEVEL_SPIN_MS) as usize;
        let (cols, rows) = (i32::from(self.cols), i32::from(self.play_rows()));
        for k in 0..n {
            let a = f64::from(k) * std::f64::consts::TAU / f64::from(n);
            // terminal cells are 2:1, so the horizontal reach is doubled to look round
            let (x, y) = (pcol + (r * 2.0 * a.cos()).round() as i32, mid + (r * a.sin()).round() as i32);
            if x >= 0 && y >= 0 && x < cols && y < rows {
                let fg = hues[(k as usize + spin) % hues.len()];
                self.cells[y as usize * cols as usize + x as usize] = Cell { ch: star, fg, bg: mix(fg, pal.bg, LEVEL_GLOW_BG) };
            }
        }
        let line = level_line(sim.c, level);
        // below the feet: the hero's own LEVEL UP! shout and bubble take the space above its head
        let top = prow + LEVEL_LINE_GAP;
        let c0 = (pcol - line.chars().count() as i32 / 2).max(0);
        if top >= 0 {
            self.text(top as u16, c0 as u16, &line, pal.xp);
        }
    }

    /// Sound as sight: a burst where a foe vanished, and a shaking ROAR over a boss that just appeared.
    fn sound_fx(&mut self, sim: &Sim) {
        let (c, now) = (sim.c, sim.tick);
        let pal = &c.pal;
        for k in 0..self.pops.len() {
            let (x, y, at) = self.pops[k];
            let (col, row) = self.scr(f64::from(x), f64::from(y));
            let frames = c.texts.art(a::KILL_BURST);
            let n = frames.len() as u32;
            let Some(frame) = frames.get(((now.saturating_sub(at)) * n / POP_TICKS).min(n.saturating_sub(1)) as usize) else { continue };
            for (dc, ch) in frame.chars().enumerate() {
                if ch != ' ' {
                    self.flash_cell(col + dc as i32 - 1, row - 1, Glyph::of(ch).unwrap_or(Glyph::SPACE), pal.xp);
                }
            }
        }
    }

    /// The sky over the play area: rain and acid streaks on open floor, a red (blood moon) or pale (full moon)
    /// cast with the moon itself, and a big banner when the weather turns. Run before the HUD boxes.
    fn sky_fx(&mut self, sim: &Sim, now_ms: u64) {
        let pal = &sim.c.pal;
        let w = sim.world.forecast(sim.tick).now;
        if self.sky != Some(w) {
            if self.sky.is_some() {
                self.sky_banner = Some((w, now_ms));
            }
            self.sky = Some(w);
        }
        let (cols, rows) = (usize::from(self.cols), usize::from(self.play_rows()));
        // streak glyph, colour and drift (cols, rows per step; snow drifts every third step), and the cast over all
        let (wx, wy) = sim.world.wind(sim.tick, sim.c.hz().get());
        let (streak, cast) = match w {
            Weather::Rain => (Some((sim.c.texts.glyph(g::RAIN), pal.of(Hue::Code), (0, 1))), None),
            Weather::AcidRain => (Some((sim.c.texts.glyph(g::ACID_RAIN), pal.xp, (0, 1))), Some((pal.xp, SKY_ACID_CAST))),
            Weather::Snow => (Some((sim.c.texts.glyph(g::SNOW), pal.text, (0, 1))), Some((pal.text, SKY_SNOW_CAST))),
            Weather::Heat => (None, Some((pal.of(Hue::Therm), SKY_HEAT_CAST))),
            Weather::Wind => (Some((sim.c.texts.glyph(g::WIND), pal.dim, (wx.signum(), wy.signum()))), None),
            Weather::BloodMoon => (None, Some((pal.alarm, SKY_BLOOD_CAST))),
            Weather::FullMoon => (None, Some((pal.text, SKY_FULL_CAST))),
            Weather::Clear => (None, None),
        };
        let step = i64::from(sim.tick / SKY_FALL_TICKS / if w == Weather::Snow { 3 } else { 1 });
        let night = sim.night();
        for r in 0..rows {
            for col in 0..cols {
                let i = r * cols + col;
                if night {
                    self.cells[i].bg = mix(self.cells[i].bg, Rgb(0, 0, 0), NIGHT_DARK);
                    self.cells[i].fg = mix(self.cells[i].fg, Rgb(0, 0, 0), NIGHT_DARK / 2);
                }
                if let Some((tint, f)) = cast {
                    self.cells[i].bg = mix(self.cells[i].bg, tint, f);
                }
                if let Some((ch, fg, (dc, dr))) = streak
                    && self.z[i] == Prio::Floor
                    && hash(((col as i64 - step * i64::from(dc)) as u64) << 32 ^ (r as i64 - step * i64::from(dr)) as u64).is_multiple_of(SKY_STREAK_1_IN)
                {
                    self.cells[i].ch = ch;
                    self.cells[i].fg = fg;
                }
            }
        }
        if let Weather::BloodMoon | Weather::FullMoon = w {
            let fg = if w == Weather::BloodMoon { pal.alarm } else { pal.text };
            let col = self.cols.saturating_sub(PANEL_W + 6);
            self.text(1, col, "●", fg);
        }
    }

    /// Champions and bosses roar when they first come into sight, sized by class. Drawn last, over the HUD.
    fn roars_fx(&mut self, sim: &Sim) {
        let (c, now) = (sim.c, sim.tick);
        let pal = &c.pal;
        // every champion and boss roars once, the first tick it is in sight; louder the stronger it is
        let sight2 = i64::from(c.view.fog_end).pow(2);
        for k in 0..sim.e.len() {
            let loud = Roar::of(sim, k);
            if loud.is_some() && dist2(sim.e.x[k] - sim.p.x, sim.e.y[k] - sim.p.y) <= sight2 && !self.roars.iter().any(|r| r.0 == sim.e.id[k]) {
                self.roars.push((sim.e.id[k], now));
            }
        }
        self.roars.retain(|&(id, at)| now.saturating_sub(at) < ROAR_TICKS && sim.e.id.contains(&id) || now.saturating_sub(at) < ROAR_FORGET);
        for r in 0..self.roars.len() {
            let (id, at) = self.roars[r];
            if now.saturating_sub(at) >= ROAR_TICKS {
                continue;
            }
            let Some(k) = sim.e.id.iter().position(|&x| x == id) else { continue };
            let Some(loud) = Roar::of(sim, k) else { continue };
            let (col, row) = self.scr(self.pos[k].0, self.pos[k].1);
            let h = c.boss_of(sim.e.fam[k]).map_or(i32::from(c.view.actor_rows[0]), |d| i32::from(d.h));
            let word = sim.c.texts.pick(loud.pool(), (id as usize).wrapping_mul(7));
            let lines: Vec<String> = match loud {
                Roar::Uber => crate::bigtext::double(c.texts.art(a::ROAR_SIGN)),
                _ => c.texts.font().rows(word).to_vec(),
            };
            let shake = if loud >= Roar::Mini { if now.is_multiple_of(2) { -1 } else { 1 } } else { 0 };
            let w = lines[0].chars().count() as i32;
            let (left, top) = (col - w / 2 + shake, row - h - ROAR_GAP - lines.len() as i32);
            let fg = match loud {
                Roar::Champion => pal.of(Hue::Therm),
                Roar::Mini | Roar::Super | Roar::Uber => pal.alarm,
            };
            if loud >= Roar::Super {
                self.plate(left - 1, top - 1, w + 2, lines.len() as i32 + 2, mix(fg, pal.hud_bg, 850));
            }
            for (r, line) in lines.iter().enumerate() {
                for (i, ch) in line.chars().enumerate() {
                    if ch != ' ' {
                        self.flash_cell(left + i as i32, top + r as i32, Glyph::of(ch).unwrap_or(Glyph::SPACE), fg);
                    }
                }
            }
            // the uber's taunt, under its ROAR
            if loud == Roar::Uber {
                let quote = sim.c.texts.fill(t::ROAR_QUOTE, [word]);
                let tl = col - quote.chars().count() as i32 / 2;
                for (i, ch) in quote.chars().enumerate() {
                    self.flash_cell(tl + i as i32, top + lines.len() as i32, Glyph::of(ch).unwrap_or(Glyph::SPACE), pal.text);
                }
            }
        }
    }

    /// Pals still standing call for help: a big HELP! over them blinking between two colours, and sound waves
    /// rippling out from the pal. Drawn last, like every shout.
    fn pal_calls(&mut self, sim: &Sim) {
        let pal = &sim.c.pal;
        let big = sim.c.texts.font().rows(sim.c.texts.get(t::PAL_HELP));
        let bw = big[0].chars().count() as i32;
        let (blink, wave) = ((sim.tick / PAL_BLINK).is_multiple_of(2), (sim.tick / PAL_WAVE % 3) as i32 + 1);
        let fg = if blink { pal.player } else { pal.xp };
        for p in sim.world.npcs.iter().filter(|p| matches!(p.npc, crate::sim::Npc::Pal { .. })) {
            if self.cell_of(f64::from(p.x), f64::from(p.y)).is_none() {
                continue;
            }
            let (col, row) = self.scr(f64::from(p.x), f64::from(p.y));
            for (r, l) in big.iter().enumerate() {
                for (i, ch) in l.chars().enumerate() {
                    if ch != ' ' {
                        self.flash_cell(col - bw / 2 + i as i32, row - 5 + r as i32, Glyph::of(ch).unwrap_or(Glyph::SPACE), fg);
                    }
                }
            }
            for k in 1..=wave {
                self.flash_cell(col - 1 - k, row - 1, sim.c.texts.glyph(g::CALL_LEFT), pal.player);
                self.flash_cell(col + 1 + k, row - 1, sim.c.texts.glyph(g::CALL_RIGHT), pal.player);
            }
        }
    }

    /// The hero talks big: the shout word in block letters where its head was when it spoke, held still on screen
    /// so it reads while the hero moves (tot-7op), and the line in a rounded speech bubble on the Doom face in the bar.
    fn hero_shout(&mut self, sim: &Sim) {
        let Some((word, line, at)) = self.face.shout(sim) else { return };
        let pal = &sim.c.pal;
        let here = self.scr(f64::from(self.cur_p.0), f64::from(self.cur_p.1));
        let (col, row) = match self.shout_at {
            Some((t, spot)) if t == at => spot,
            _ => self.shout_at.insert((at, here)).1,
        };
        let big = sim.c.texts.font().rows(word);
        let (bw, top) = (big[0].chars().count() as i32, row - i32::from(sim.c.view.actor_rows[0]) - 1 - big.len() as i32);
        for (r, l) in big.iter().enumerate() {
            for (i, ch) in l.chars().enumerate() {
                if ch != ' ' {
                    self.flash_cell(col - bw / 2 + i as i32, top + r as i32, Glyph::of(ch).unwrap_or(Glyph::SPACE), pal.player);
                }
            }
        }
        let Some((fx, y0)) = self.face_origin() else { return };
        let say = format!(" {line} ");
        let sw = say.chars().count() as i32;
        let fc = i32::from(fx + FACE_W / 2);
        let (bl, bt) = ((fc - sw / 2 - 1).clamp(0, (i32::from(self.cols) - sw - 2).max(0)), i32::from(y0) - 3);
        self.plate(bl, bt, sw + 2, 3, pal.hud_bg);
        for (r, s) in [(bt, format!("╭{}╮", "─".repeat(sw as usize))), (bt + 1, format!("│{say}│")), (bt + 2, format!("╰{}╯", "─".repeat(sw as usize)))] {
            for (i, ch) in s.chars().enumerate() {
                self.flash_cell(bl + i as i32, r, Glyph::of(ch).unwrap_or(Glyph::SPACE), if r == bt + 1 && i > 0 && (i as i32) < sw + 1 { pal.text } else { pal.player });
            }
        }
        // the tail points down at the face
        self.flash_cell(fc, bt + 2, Glyph::of('┬').unwrap_or(Glyph::SPACE), pal.player);
    }

    /// Top-left (col, row) of the Doom face in the bottom bar.
    fn face_origin(&self) -> Option<(u16, u16)> {
        let (x0, y0) = self.bar_origin()?;
        let half = (BAR_W - 2 * ORB_W - FACE_W - 4) / 2;
        Some((x0 + ORB_W + 1 + half + 1, y0))
    }

    /// The weather-change banner: big letters on a dark plate, drawn last so nothing covers it.
    fn sky_banner_fx(&mut self, sim: &Sim, now_ms: u64) {
        let pal = &sim.c.pal;
        if let Some((bw, at)) = self.sky_banner {
            if now_ms.saturating_sub(at) >= SKY_BANNER_MS {
                self.sky_banner = None;
                return;
            }
            let fg = sky_fg(pal, bw);
            let text = sim.c.texts.get(t::BANNER[bw as usize]).to_string();
            let left = (self.cols as usize / 2).saturating_sub(sim.c.texts.font().width(&text) / 2) as i32;
            self.plate(left - 2, 1, sim.c.texts.font().width(&text) as i32 + 4, 5, mix(fg, pal.hud_bg, 850));
            for (r, line) in sim.c.texts.font().rows(&text).iter().enumerate() {
                for (k, ch) in line.chars().enumerate() {
                    if ch != ' ' {
                        self.flash_cell(left + k as i32, 2 + r as i32, Glyph::of(ch).unwrap_or(Glyph::SPACE), fg);
                    }
                }
            }
        }
    }

    /// Death is loud: a shaking YOU DIED banner, who did it, the killer crowned HERO-SLAYER, and every foe in
    /// sight cheering and hopping. Sim time is frozen at death, so this runs on the frame clock (`now_ms`).
    fn death_fx(&mut self, sim: &Sim, now_ms: u64) {
        let Some(o @ Outcome::Dead { cause, .. }) = sim.outcome else {
            self.died_ms = None;
            return;
        };
        let pal = &sim.c.pal;
        let t = now_ms.saturating_sub(*self.died_ms.get_or_insert(now_ms));
        let sight2 = i64::from(sim.c.view.fog_end).pow(2);
        let fam = match cause {
            Cause::Contact { fam, .. } | Cause::Shot { fam } | Cause::BossAttack { fam } => Some(fam),
            Cause::Blast => None,
        };
        // the killer: the nearest foe in sight of the family that dealt the blow
        let killer = fam.and_then(|f| {
            (0..sim.e.len()).filter(|&k| sim.e.side[k] == Side::Hostile && sim.e.fam[k] == f).map(|k| (dist2(sim.e.x[k] - sim.p.x, sim.e.y[k] - sim.p.y), k)).filter(|&(d2, _)| d2 <= sight2).min().map(|(_, k)| k)
        });
        // the crowd cheers: every foe in sight hops, some shout
        for k in 0..sim.e.len() {
            if sim.e.side[k] != Side::Hostile || Some(k) == killer || dist2(sim.e.x[k] - sim.p.x, sim.e.y[k] - sim.p.y) > sight2 {
                continue;
            }
            let (col, row) = self.scr(self.pos[k].0, self.pos[k].1);
            let id = u64::from(sim.e.id[k]);
            let hop = (now_ms / CHEER_HOP_MS + id).is_multiple_of(2);
            self.flash_cell(col, row - 1 - i32::from(hop), sim.c.texts.glyph(g::CHEER_HOP), pal.alarm);
            if id.is_multiple_of(3) && hop {
                let word = sim.c.texts.pick(t::CHEERS, (id / 3) as usize);
                for (i, ch) in word.chars().enumerate() {
                    self.flash_cell(col - 1 + i as i32, row - 3, Glyph::of(ch).unwrap_or(Glyph::SPACE), pal.chest);
                }
            }
        }
        if let Some(k) = killer {
            let (col, row) = self.scr(self.pos[k].0, self.pos[k].1);
            let gold = if (now_ms / CHEER_HOP_MS).is_multiple_of(2) { pal.chest } else { pal.xp };
            let title = sim.c.texts.fill(t::HERO_SLAYER, [&sim.c.fam(sim.e.fam[k]).name]);
            for (i, ch) in "\\^/".chars().enumerate() {
                self.flash_cell(col - 1 + i as i32, row - 2, Glyph::of(ch).unwrap_or(Glyph::SPACE), gold);
            }
            let start = col - title.chars().count() as i32 / 2;
            for (i, ch) in title.chars().enumerate() {
                self.flash_cell(start + i as i32, row - 4, Glyph::of(ch).unwrap_or(Glyph::SPACE), gold);
            }
            for (i, ch) in "✦ ✦".chars().enumerate() {
                self.flash_cell(col - 1 + i as i32, row - 3, Glyph::of(ch).unwrap_or(Glyph::SPACE), gold);
            }
        }
        // banner: shakes for the first DIED_SHAKE_MS, then stands
        let shake = if t < DIED_SHAKE_MS { if (now_ms / 50).is_multiple_of(2) { -1 } else { 1 } } else { 0 };
        let banner = sim.c.texts.art(a::DEATH_BANNER);
        let bw = banner.iter().map(|l| l.chars().count()).max().unwrap_or(0) as i32;
        let left = i32::from(self.cols) / 2 - bw / 2 + shake;
        let top = i32::from(self.play_rows()) / 4;
        // a dark red plate behind banner and text, so it reads over anything
        let plate = mix(pal.alarm, pal.hud_bg, 820);
        let plea = sim.c.texts.get(t::SUPPORT_PLEA);
        let (pw, ph) = (plea.chars().count() as i32 + 8, banner.len() as i32 + 9);
        let pl = i32::from(self.cols) / 2 - pw / 2;
        self.plate(pl, top - 1, pw, ph, plate);
        for (r, line) in banner.iter().enumerate() {
            for (i, ch) in line.chars().enumerate() {
                if ch != ' ' {
                    self.flash_cell(left + i as i32, top + r as i32, Glyph::of(ch).unwrap_or(Glyph::SPACE), pal.alarm);
                }
            }
        }
        let tx = &sim.c.texts;
        let by = tx.fill(t::SLAIN_BY, [&cause_text(sim.c, &o)]);
        let taunt = tx.pick(t::DEATH_TAUNTS, (sim.tick as usize).wrapping_mul(31));
        for (dr, (line, fg)) in [(by.as_str(), pal.text), (taunt, pal.dim), (tx.get(t::DEATH_KEYS), pal.player)].into_iter().enumerate() {
            let w = line.chars().count() as i32;
            let col = i32::from(self.cols) / 2 - w / 2;
            for (i, ch) in line.chars().enumerate() {
                self.flash_cell(col + i as i32, top + banner.len() as i32 + 1 + dr as i32, Glyph::of(ch).unwrap_or(Glyph::SPACE), fg);
            }
        }
        // the plea: below the plate, a clickable sponsor link
        let w = plea.chars().count() as i32 + 4;
        let (col, row) = (i32::from(self.cols) / 2 - w / 2, top + banner.len() as i32 + 6);
        if col >= 0 && row >= 1 && row + 1 < i32::from(self.play_rows()) {
            self.button((row as u16, col as u16), plea, (pal.chest, pal.bg), true, sim.c.texts.get(t::SUPPORT_URL));
        }
    }

    /// A clickable button: ` label ` filled with `bg`, rounded by half blocks (`▐ … ▌`); `tall` adds rims above and
    /// below in quadrant blocks so it reads as a raised key. Returns its width. Off-screen parts are clipped.
    fn button(&mut self, (row, col): (u16, u16), label: &str, (bg, fg): (Rgb, Rgb), tall: bool, url: &str) -> u16 {
        let w = label.chars().count() as u16 + 4;
        let rows = if tall { 3 } else { 1 };
        let r0 = if tall { row.saturating_sub(1) } else { row };
        let back = self.cells.get(usize::from(row) * usize::from(self.cols) + usize::from(col)).map_or(bg, |c| c.bg);
        for r in 0..rows {
            let line: String = match (tall, r) {
                (true, 0) => format!("▗{}▖", "▄".repeat(usize::from(w) - 2)),
                (true, 2) => format!("▝{}▘", "▀".repeat(usize::from(w) - 2)),
                _ => format!("▐ {label} ▌"),
            };
            let y = r0 + r;
            if y >= self.rows {
                continue;
            }
            for (k, ch) in line.chars().enumerate() {
                let x = col + k as u16;
                if x >= self.cols {
                    break;
                }
                let i = usize::from(y) * usize::from(self.cols) + usize::from(x);
                // rims and edges: button colour on the backdrop; inside: label on the button colour
                let inside = (!tall || r == 1) && k > 0 && k + 1 < usize::from(w);
                self.cells[i] = if inside { Cell { ch: Glyph::of(ch).unwrap_or(Glyph::SPACE), fg, bg } } else { Cell { ch: Glyph::of(ch).unwrap_or(Glyph::SPACE), fg: bg, bg: back } };
            }
        }
        let i = usize::from(row) * usize::from(self.cols) + usize::from(col);
        self.links.push((i, i + usize::from(w), url.to_string()));
        w
    }

    /// Clear a w x h rectangle at (left, top) of the play area to blank cells on `bg` (clipped).
    fn plate(&mut self, left: i32, top: i32, w: i32, h: i32, bg: Rgb) {
        for r in top.max(0)..(top + h).min(i32::from(self.play_rows())) {
            for col in left.max(0)..(left + w).min(i32::from(self.cols)) {
                let i = r as usize * usize::from(self.cols) + col as usize;
                self.cells[i] = Cell { ch: Glyph::SPACE, fg: bg, bg };
            }
        }
    }

    fn flash_cell(&mut self, col: i32, row: i32, ch: Glyph, fg: Rgb) {
        if col >= 0 && row >= 0 && col < i32::from(self.cols) && row < i32::from(self.play_rows()) {
            let i = row as usize * usize::from(self.cols) + col as usize;
            self.cells[i].ch = ch;
            self.cells[i].fg = fg;
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
            let text = match b.kind {
                BarkKind::Stole { power } => sim.c.texts.fill(t::THIEF_STOLE, [sim.c.texts.pick(t::STOLE_LINES, b.id as usize), &power.to_string()]),
                BarkKind::Learned { ability } => sim.c.texts.fill(t::THIEF_LEARNED, [sim.c.texts.get(t::ABILITY[ability as usize])]),
            };
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

    /// World layer: NPCs (friends in the hero colour, keepers in gold, neutrals dim), goblins and their gold,
    /// world chests (a revealed fake gets a red `!`), boss blast fuses and traps (dim while arming).
    fn world_layer(&mut self, sim: &Sim) {
        let pal = &sim.c.pal;
        let w = &sim.world;
        let at = |f: &mut Self, x: i32, y: i32, p: Prio, ch: Glyph, fg: Rgb| {
            let (x, y) = (f64::from(x), f64::from(y));
            if let Some(i) = f.cell_of(x, y) {
                f.put(i, p, f.vd(x, y), Ink { ch, fg, bg: None });
            }
        };
        for g in &w.gold_piles {
            // gold is golden and glows (the chest pulse)
            if let Some(i) = self.cell_of(f64::from(g.x), f64::from(g.y)) {
                self.put(i, Prio::Pickup, self.vd(f64::from(g.x), f64::from(g.y)), Ink { ch: sim.c.texts.glyph(g::GOLD_PILE), fg: pal.chest, bg: Some(chest_glow(pal, sim.tick)) });
            }
        }
        for c in &w.chests {
            self.chest(sim, f64::from(c.x), f64::from(c.y));
            if c.tell {
                at(self, c.x, c.y - sim.c.view.floor_cell, Prio::IncomingHit, sim.c.texts.glyph(g::CHEST_TELL), pal.alarm);
            }
        }
        for t in &w.traps {
            let fg = match t.state(sim.c.hz().get()) {
                crate::sim::TrapState::Arming => pal.dim,
                crate::sim::TrapState::Armed | crate::sim::TrapState::Expired => pal.alarm,
            };
            at(self, t.x, t.y, Prio::IncomingHit, sim.c.texts.glyph(g::TRAP), fg);
        }
        for b in &w.blasts {
            at(self, b.x, b.y, Prio::IncomingHit, sim.c.texts.glyph(g::BLAST), pal.alarm);
        }
        for g in &w.goblins {
            at(self, g.x, g.y, Prio::EliteBoss, sim.c.texts.glyph(g::GOBLIN), pal.chest);
        }
        // a persona stands as tall as an elite foe, its head on a glow of its own colour and its body glowing
        // fainter, so a pal or a keeper reads at a glance in a crowd
        let n = sim.c.view.actor_rows[2].max(1);
        for p in &w.npcs {
            let (ch, fg) = npc_ink(sim.c, p.npc);
            let (x, y) = (f64::from(p.x), f64::from(p.y));
            let Some(i) = self.cell_of(x, y) else { continue };
            let head = Ink { ch, fg, bg: Some(mix(fg, pal.bg, NPC_GLOW)) };
            let body = Ink { ch: pal.body, fg, bg: Some(mix(fg, pal.bg, NPC_BODY_GLOW)) };
            let shadow = Ink { ch: pal.shadow, fg: mix(pal.floor, pal.bg, 300), bg: None };
            self.stand(i, self.vd(x, y), n, Prio::EliteBoss, [head, body, shadow]);
        }
    }

    /// An item on the floor, Diablo style: its glyph in its rarity colour on a glow of that colour, and a light
    /// beam rising above it, taller the rarer it is (common none, unique LOOT_BEAM_MAX rows, pulsing).
    fn loot(&mut self, sim: &Sim, x: f64, y: f64, id: crate::sim::ItemId) {
        let Some(it) = sim.c.items.get(usize::from(id.0)) else { return };
        let Some(i) = self.cell_of(x, y) else { return };
        let pal = &sim.c.pal;
        let fg = pal.role(it.rarity.role());
        let glow = mix(fg, pal.bg, LOOT_GLOW);
        self.put(i, Prio::Pickup, self.vd(x, y), Ink { ch: it.glyph, fg, bg: Some(glow) });
        let rank = it.rarity as i32;
        let pulse = if (sim.tick / LOOT_PULSE).is_multiple_of(2) { 0 } else { 1 };
        let (col, row) = self.scr(x, y);
        for h in 1..=(rank * LOOT_BEAM_MAX / 4 + if rank == 4 { pulse } else { 0 }) {
            self.flash_cell(col, row - h, Glyph::of(if h == 1 { '│' } else { '╎' }).unwrap_or(Glyph::SPACE), fg);
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
        let glow = chest_glow(pal, sim.tick);
        for (dr, line) in sim.c.texts.art(a::CHEST).iter().enumerate() {
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

    fn overlays(&mut self, sim: &Sim, v: &View) {
        let (ui, now_ms) = (v.ui, v.now_ms);
        self.sky_fx(sim, now_ms);
        self.boss_bar(sim);
        let c = sim.c;
        let unit = c.view.floor_cell.max(1) * MAP_K;
        let (px, py) = (sim.p.x.div_euclid(unit), sim.p.y.div_euclid(unit));
        // view_m can be smaller than one map cell (unit); still reveal the hero's neighbours
        explore(&mut self.seen, (px, py), (c.view.view_r / unit).max(1));
        // targets: scheduled bosses not yet spawned (home), and the live boss
        let mut t: Vec<(i32, i32, usize)> = sim.boss_home.iter().filter(|h| !h.spawned).map(|h| (h.x, h.y, h.fam.ix())).collect();
        if let Some(b) = sim.boss_ix() {
            t.push((sim.e.x[b], sim.e.y[b], sim.e.fam[b].ix()));
        }
        self.map(sim, ui.heading, unit, (px, py), &t, &sim.treasures);
        self.loot_panes(sim, ui);
        self.panel(sim, v.phase, v.last_bytes);
        self.hero_pane(sim, ui);
        self.hud_bar(sim);
        for &(x, y, _) in &t {
            self.pointer(sim, x, y, c.pal.alarm);
        }
        for &(x, y) in &sim.treasures {
            self.pointer(sim, x, y, c.pal.chest);
        }
        // the nearest few pals, keepers and goblins in earshot get an arrow in their own colour; every one on the
        // floor would bury the screen edge in arrows
        let goblins = sim.world.goblins.iter().map(|g| (g.x, g.y, c.pal.chest));
        let npcs = sim.world.npcs.iter().filter(|p| p.npc != crate::sim::Npc::Neutral).map(|p| (p.x, p.y, npc_ink(c, p.npc).1));
        let mut calls: Vec<(i64, i32, i32, Rgb)> = npcs.chain(goblins).map(|(x, y, fg)| (dist2(x - sim.p.x, y - sim.p.y), x, y, fg)).filter(|&(d2, ..)| d2 <= PAL_CALL_MM.pow(2)).collect();
        calls.sort_unstable_by_key(|&(d2, x, y, _)| (d2, x, y));
        for &(_, x, y, fg) in calls.iter().take(PAL_CALLS) {
            self.pointer(sim, x, y, fg);
        }
        // shouts last: only the popups may cover them
        self.pal_calls(sim);
        self.roars_fx(sim);
        self.hero_shout(sim);
        // popups (picker, shop) over everything in the arena, shouts included (tot-ms8)
        self.popup(sim, ui, now_ms);
        self.menu(sim, ui);
        self.death_fx(sim, now_ms);
        self.sky_banner_fx(sim, now_ms);
    }

    fn map(&mut self, sim: &Sim, heading: Heading, unit: i32, (px, py): (i32, i32), t: &[(i32, i32, usize)], treasures: &[(i32, i32)]) {
        let c = sim.c;
        let pal = &c.pal;
        let cols = usize::from(self.cols);
        let (x0, y0) = (0u16, 0u16);
        if self.cols < MAP_W || self.play_rows() < MAP_H {
            return;
        }
        for dr in 0..MAP_H {
            for dc in 0..MAP_W {
                let (ox, oy) = heading.unrot(i32::from(dc) - i32::from(MAP_W / 2), i32::from(dr) - i32::from(MAP_H / 2));
                let (cx, cy) = (px + ox, py + oy);
                let known = self.seen.contains(&(cx, cy));
                let treasure = treasures.iter().any(|&(x, y)| (x.div_euclid(unit), y.div_euclid(unit)) == (cx, cy));
                let (ch, fg) = if (cx, cy) == (px, py) {
                    (c.texts.glyph(g::MAP_HERO), pal.player)
                } else if treasure {
                    (c.texts.glyph(g::MAP_CHEST), pal.chest)
                } else if t.iter().any(|&(x, y, _)| (x.div_euclid(unit), y.div_euclid(unit)) == (cx, cy)) {
                    (c.texts.glyph(g::MAP_BOSS), pal.alarm)
                } else if sim.world.goblins.iter().any(|gb| (gb.x.div_euclid(unit), gb.y.div_euclid(unit)) == (cx, cy)) {
                    (c.texts.glyph(g::MAP_GOBLIN), pal.chest)
                } else if let Some(p) = sim.world.npcs.iter().find(|p| p.npc != crate::sim::Npc::Neutral && (p.x.div_euclid(unit), p.y.div_euclid(unit)) == (cx, cy)) {
                    npc_ink(c, p.npc)
                } else if known && rock(sim.arena(), cx, cy) {
                    (c.texts.glyph(g::MAP_ROCK), pal.text)
                } else if known {
                    (c.texts.glyph(g::MAP_FLOOR), pal.floor)
                } else {
                    (Glyph::SPACE, pal.fog)
                };
                let i = usize::from(y0 + dr) * cols + usize::from(x0 + dc);
                self.cells[i] = Cell { ch, fg, bg: if treasure && (cx, cy) != (px, py) { chest_glow(pal, sim.tick) } else if known { pal.hud_bg } else { pal.fog } };
            }
        }
        self.rose(sim.c);
    }
    /// Waiting power-ups stack as small loot panes down the left edge under the map; `p` opens the top one.
    fn loot_panes(&mut self, sim: &Sim, ui: &Ui) {
        let n = sim.offers_waiting();
        if n == 0 || ui.mode == Mode::Pick {
            return;
        }
        let pal = &sim.c.pal;
        // a vivid role colour, never the plain chest white: each pane filled with its glow, the top one pulsing (tot-8zt)
        let fg = pal.role(POWER_ROLE);
        let pulse = (sim.tick / CHEST_PULSE).is_multiple_of(2);
        for k in 0..n.min(LOOT_PANES) {
            let top = i32::from(MAP_H) + 1 + i32::from(k) * 3;
            if top + 3 > i32::from(self.play_rows()) {
                break;
            }
            let glow = mix(fg, pal.hud_bg, if k == 0 && pulse { POWER_GLOW_HI } else { POWER_GLOW });
            self.plate(0, top, LOOT_PANE_W, 3, glow);
            let w = LOOT_PANE_W as usize - 2;
            self.text(top as u16, 0, &format!("╭{}╮", "─".repeat(w)), fg);
            self.text(top as u16 + 1, 0, "│", fg);
            self.text(top as u16 + 1, 1, &format!("{:<w$}", sim.c.texts.get(t::POWER_UP)), pal.text);
            self.text(top as u16 + 1, w as u16 + 1, "│", fg);
            self.text(top as u16 + 2, 0, &format!("╰{}╯", "─".repeat(w)), fg);
            if k == 0 {
                // the top pane carries the key
                self.text(top as u16 + 2, 2, " p ", key_fg(pal));
            }
        }
        if n > LOOT_PANES {
            self.text((i32::from(MAP_H) + 1 + i32::from(LOOT_PANES) * 3) as u16, 1, &sim.c.texts.fill(t::POWER_MORE, [&(n - LOOT_PANES).to_string()]), pal.dim);
        }
    }

    /// Tiny compass in the map's top-left corner: a 3x3 plate of its own (so the map never hides it), a star in
    /// the middle and N E S W round it. It turns with the eased camera (the same rotation the map makes at rest),
    /// so a q/e turn swings it smoothly. N in the alarm colour (tot-5ah).
    fn rose(&mut self, c: &Content) {
        let (p, pal) = (self.eye.p, &c.pal);
        self.plate(0, 0, 3, 3, pal.hud_bg);
        let at = |(x, y): (f64, f64)| ((ROSE_C + x).round() as i32, (ROSE_C + y).round() as i32);
        self.flash_cell(ROSE_C as i32, ROSE_C as i32, c.texts.glyph(g::STAR), pal.dim);
        // each letter takes one cell: the first glyph of the adventure's compass word
        for (k, (wx, wy)) in COMPASS.into_iter().enumerate() {
            let (col, row) = at(p.rot(f64::from(wx), f64::from(wy)));
            let letter = Glyph::text(c.texts.get(t::DIR[k])).next().unwrap_or(Glyph::SPACE);
            self.flash_cell(col, row, letter, if k == 0 { pal.alarm } else { pal.text });
        }
    }


    fn panel(&mut self, sim: &Sim, phase: &str, bytes: usize) {
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
        // the clock wears the time of day: sun by day, moon by night
        let mut x = self.text(r, x0 + 1, c.texts.get(if sim.night() { t::CLOCK_NIGHT } else { t::CLOCK_DAY }), pal.text);
        x = self.clock(r, x, sim.secs(), pal.text);
        // next boss: first scheduled boss event after now
        let next = c.waves.iter().filter_map(|w| match w {
            crate::data::WaveEv::Boss { at, fam, .. } if *at > sim.tick => Some((*at, *fam)),
            _ => None,
        }).min_by_key(|&(at, _)| at);
        match next {
            Some((at, fam)) => {
                let left = (u64::from(at - sim.tick) / hz) as u32;
                x = self.text(r, x + 2, c.texts.get(t::PANEL_NEXT), pal.dim);
                x = self.text(r, x, &c.fam(fam).name, pal.alarm);
                self.clock(r, x + 1, left, pal.alarm);
            }
            None => {
                self.text(r, x + 2, c.texts.get(t::PANEL_NO_BOSS), pal.dim);
            }
        }
        r += 1;
        let mut x = self.text(r, x0 + 1, c.texts.get(t::PANEL_HP), pal.text);
        x = self.bar(r, x, i64::from(sim.p.hp), i64::from(sim.p.max_hp), 16, pal.of(Hue::Kin));
        x = self.num(r, x + 1, i64::from(sim.p.hp.max(0) / 1000), pal.text);
        x = self.text(r, x, "/", pal.dim);
        self.num(r, x, i64::from(sim.p.max_hp / 1000), pal.dim);
        r += 1;
        // XP: the bar that drives "one more level" (wide, with numbers to go)
        let mut x = self.text(r, x0 + 1, c.texts.get(t::PANEL_LV), pal.text);
        x = self.num(r, x, i64::from(sim.p.level), pal.xp);
        x = self.bar(r, x + 1, i64::from(sim.p.xp), i64::from(sim.xp_need()), XP_BAR_W, pal.xp);
        x = self.num(r, x + 1, i64::from(sim.xp_need().saturating_sub(sim.p.xp)), pal.xp);
        self.text(r, x, c.texts.get(t::PANEL_TO_GO), pal.dim);
        r += 1;
        let mut x = self.text(r, x0 + 1, c.texts.get(t::PANEL_FLUX), pal.text);
        x = self.num(r, x, i64::from(sim.p.flux / 1000), pal.of(Hue::Code));
        x = self.text(r, x + 2, c.texts.get(t::PANEL_KILLS), pal.dim);
        x = self.num(r, x, i64::from(sim.stats.kills), pal.text);
        x = self.text(r, x + 2, c.texts.get(t::PANEL_HIJACK), pal.dim);
        self.num(r, x, i64::from(sim.hijacked), pal.of(Hue::Code));
        r += 1;
        // morale: hero NERVE (grows while unhurt) vs foe PANIC (share of foes fleeing)
        let hostile = (0..sim.e.len()).filter(|&k| sim.e.side[k] == Side::Hostile).count();
        let panicked = (0..sim.e.len()).filter(|&k| sim.e.side[k] == Side::Hostile && sim.e.panic[k]).count();
        let nerve = sim.confidence();
        let mut x = self.text(r, x0 + 1, c.texts.get(t::PANEL_NERVE), pal.text);
        x = self.bar(r, x, i64::from(nerve), 1000, MORALE_BAR_W, if nerve >= CONF_PANIC { pal.player } else { pal.dim });
        x = self.text(r, x + 2, c.texts.get(t::PANEL_PANIC), pal.text);
        self.bar(r, x, panicked as i64, hostile.max(1) as i64, MORALE_BAR_W, pal.of(Hue::Emp));
        r += 1;
        // purse (shops take it) and pals won (rescued or befriended)
        let mut x = self.text(r, x0 + 1, c.texts.get(t::PANEL_GOLD), pal.text);
        x = self.num(r, x, i64::from(sim.p.gold), pal.chest);
        x = self.text(r, x + 2, c.texts.get(t::PANEL_PALS), pal.text);
        x = self.num(r, x, i64::from(sim.world.pals), pal.player);
        // the belly: green when fed, red when hungry
        x = self.text(r, x + 2, c.texts.get(t::PANEL_FOOD), pal.text);
        let fed = sim.p.food >= crate::sim::FOOD_LOW;
        self.bar(r, x, i64::from(sim.p.food), i64::from(crate::sim::FOOD_FULL), FOOD_BAR_W, pal.role(if fed { Role::Heal } else { Role::Danger }));
        r += 1;
        // sky: weather now, what comes next and when (a full or blood moon is the threat)
        let fc = sim.world.forecast(sim.tick);
        let sky_fg = |w: Weather| sky_fg(pal, w);
        let mut x = self.text(r, x0 + 1, c.texts.get(t::PANEL_SKY), pal.text);
        x = self.text(r, x, &format!("{} {}", c.texts.get(t::ICON[fc.now as usize]), c.texts.get(t::WEATHER[fc.now as usize])), sky_fg(fc.now));
        x = self.text(r, x + 1, ">", pal.dim);
        x = self.text(r, x + 1, &format!("{} {}", c.texts.get(t::ICON[fc.next as usize]), c.texts.get(t::WEATHER[fc.next as usize])), sky_fg(fc.next));
        self.clock(r, x + 1, fc.ticks_to_next / c.hz().get(), pal.dim);
        r += 1;
        // powers: name in the power's type colour (same colour as the picker popup)
        self.text(r, x0 + 1, c.texts.get(t::PANEL_POWERS), pal.dim);
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
        let x = self.text(r, x0 + 1, c.texts.get(t::PANEL_FOES), pal.dim);
        // director phase and the last frame size (kB): status numbers live here, not on the key rows
        let x = self.text(r, x, phase, pal.text);
        let x = self.text(r, x + 2, c.texts.get(t::PANEL_NET), pal.dim);
        let x = self.num(r, x, (bytes / 1024) as i64, pal.dim);
        self.text(r, x, "k", pal.dim);
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
        for (row, &(n, i)) in top.iter().take(PANEL_FOE_ROWS.into()).enumerate() {
            let (f, t) = (&c.fams[i / tiers.len()], tiers[i % tiers.len()]);
            let fg = pal.of(f.hue);
            let rr = r + 1 + row as u16;
            let mut x = self.text(rr, x0 + 2, &f.glyph.ch().to_string(), fg);
            x = self.num(rr, x + 1, i64::from(n), pal.text);
            x = self.bar(rr, x + 1, i64::from(n), hostile.max(1) as i64, MINI_BAR_W, fg);
            x = self.text(rr, x + 1, &f.name, fg);
            let (tag, tc) = match t {
                Tier::Trash => ("", pal.dim),
                Tier::Elite => (c.texts.get(t::TIER[1]), pal.of(Hue::Kin)),
                Tier::Champion => (c.texts.get(t::TIER[2]), pal.of(Hue::Therm)),
                Tier::Boss => (c.texts.get(t::TIER[3]), pal.alarm),
            };
            self.text(rr, x, tag, tc);
        }
        // cast: hero, hijacked allies, panicked foes, fed thieves; glyph drawn as on the map screen
        r = y0 + PANEL_CAST_ROW;
        self.text(r, x0 + 1, c.texts.get(t::PANEL_CAST), pal.dim);
        let tally = cast_tally(sim);
        for (row, kind) in Cast::ALL.into_iter().enumerate() {
            let (n, rep) = tally[kind as usize];
            let rr = r + 1 + row as u16;
            let fam = rep.map(|k| c.fam(sim.e.fam[k]));
            let (hue, glyph) = fam.map_or((pal.dim, '·'), |f| (pal.of(f.hue), f.glyph.ch()));
            let (fg, bg, glyph) = match kind {
                Cast::Hero => (pal.player, pal.hud_bg, '@'),
                Cast::Ally => (pal.bg, hue, glyph),
                Cast::Panic => (hue, mix(pal.bg, pal.of(Hue::Emp), FEAR_TINT), glyph),
                Cast::Thief => (hue, mix(pal.bg, pal.chest, FED_GLOW_MAX), glyph),
            };
            let mut x = self.text(rr, x0 + 2, &glyph.to_string(), fg);
            self.cells[usize::from(rr) * usize::from(self.cols) + usize::from(x0) + 2].bg = bg;
            x = self.num(rr, x + 1, i64::from(n), pal.text);
            let (val, max) = match kind {
                Cast::Hero => (i64::from(sim.p.hp), i64::from(sim.p.max_hp)),
                Cast::Ally => (i64::from(n), sim.e.len() as i64),
                Cast::Panic | Cast::Thief => (i64::from(n), hostile.max(1) as i64),
            };
            x = self.bar(rr, x + 1, val, max, MINI_BAR_W, if n == 0 { pal.dim } else if kind == Cast::Hero { pal.player } else { hue });
            self.text(rr, x + 1, c.texts.get(t::CAST[kind as usize]), pal.dim);
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
        let in_pane = self.pane_at.is_some_and(|(px, py)| col >= px && row >= py && row < py + heropane::PANE_H);
        in_map || in_panel || in_bar || in_pane
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
        // whole label or none (a cut "50" for "502m" lies): right side, else left, on cells no box or label holds
        let free = |f: &Self, s: u16| s + n <= f.cols && (s..s + n).all(|p| !f.boxed(p, row) && f.cells[usize::from(row) * usize::from(f.cols) + usize::from(p)].bg != pal.hud_bg);
        let Some(start) = [col + 2, col.saturating_sub(n + 1)].into_iter().find(|&s| free(self, s)) else { return };
        for (k, lc) in label.chars().enumerate() {
            let j = usize::from(row) * usize::from(self.cols) + usize::from(start + k as u16);
            self.cells[j] = Cell { ch: Glyph::of(lc).unwrap_or(Glyph::SPACE), fg, bg: pal.hud_bg };
        }
    }

    /// The last two rows: every key of the current mode (or of the pending which-key group), key highlighted,
    /// in a fixed layout with no numbers or live text, so walking never makes it shift or flicker. Toggles show
    /// on/off by colour only. What just happened goes on the message line at the top of the play area.
    fn strip(&mut self, sim: &Sim, v: &View) {
        self.overlays(sim, v);
        let c = sim.c;
        let pal = &c.pal;
        let ui = v.ui;
        let (rb, rcm) = (self.rows - self.strip, self.rows - 1);
        let ended = sim.outcome.is_some();
        let cols = usize::from(self.cols);
        for r in [rb, rcm] {
            let i = usize::from(r) * cols;
            self.cells[i..i + cols].fill(Cell { ch: Glyph::SPACE, fg: pal.text, bg: pal.bg });
        }
        self.message(sim, v);
        let (tag, ink) = mode_tag(ui.mode, ended);
        let tag_fg = match ink {
            ModeInk::Player => pal.player,
            ModeInk::Alarm => pal.alarm,
            ModeInk::Xp => pal.xp,
            ModeInk::Code => pal.of(Hue::Code),
            ModeInk::Dim => pal.dim,
            ModeInk::Role(r) => pal.role(r),
        };
        let tag_end = self.text(rcm, 1, c.texts.get(tag), tag_fg);
        if ui.mode == Mode::Cmd {
            let x = self.text(rcm, tag_end + 1, ":", pal.player);
            let x = self.text(rcm, x, &ui.cmdline, pal.player);
            self.text(rcm, x, "_", pal.alarm);
        }
        if let Some((prefix, items)) = ui.whichkey(c, v.now_ms) {
            self.whichkey_popup(pal, &prefix, &items);
        }
        let entries = if ended { vec![Hint { keys: "r".into(), label: c.texts.get(t::HINT_NEW_RUN).into(), on: None }, Hint { keys: "q".into(), label: c.texts.get(t::HINT_QUIT).into(), on: None }] } else { hints(c, ui) };
        // first fit: each entry goes on row B if it fits there, else on row C after the mode tag; never split
        let key = key_fg(pal);
        // the sponsor button owns the right end of row C; hints never cover it
        let sx = self.cols.saturating_sub(c.texts.get(t::SUPPORT_LABEL).chars().count() as u16 + 5);
        self.button((rcm, sx), c.texts.get(t::SUPPORT_LABEL), (pal.alarm, pal.bg), false, c.texts.get(t::SUPPORT_URL));
        let limit = [self.cols, sx.saturating_sub(1)];
        let mut ends = [1u16, if ui.mode == Mode::Cmd { self.cols } else { tag_end + 2 }];
        for h in &entries {
            let w = hint_w(h) as u16;
            // one key row: everything after the mode tag; two: row B from the left, then row C
            let slots: &[usize] = if self.strip == 1 { &[1] } else { &[0, 1] };
            let Some(&slot) = slots.iter().find(|&&s| ends[s] + w <= limit[s]) else { continue };
            let (row, x) = ([rb, rcm][slot], ends[slot]);
            ends[slot] += w;
            for (k, ch) in h.keys.chars().enumerate() {
                let i = usize::from(row) * cols + usize::from(x) + k;
                self.cells[i] = Cell { ch: Glyph::of(ch).unwrap_or(Glyph::SPACE), fg: key, bg: pal.bg };
            }
            // a toggle shows its state as the label's background: green on, red off (no extra text)
            let lx = x + h.keys.chars().count() as u16 + 1;
            let end = self.text(row, lx, &h.label, pal.text);
            if let Some(on) = h.on {
                let bg = mix(pal.role(if on { Role::Heal } else { Role::Danger }), pal.bg, TOGGLE_BG);
                let i = usize::from(row) * cols;
                for cell in &mut self.cells[i + usize::from(lx)..i + usize::from(end)] {
                    cell.bg = bg;
                }
            }
        }
    }

    /// Vim-style which-key: a rounded box above the key rows, titled with the pending keys, entries in columns
    /// (key in the accent colour, label plain).
    fn whichkey_popup(&mut self, pal: &crate::data::Palette, prefix: &str, items: &[(String, String)]) {
        let cell_w = items.iter().map(|(k, l)| k.chars().count() + 3 + l.chars().count()).max().unwrap_or(0) + 2;
        let inner = (usize::from(self.cols) - 4).min(WK_MAX_W);
        let per_row = (inner / cell_w.max(1)).max(1);
        let rows = items.len().div_ceil(per_row);
        let (w, h) = ((per_row * cell_w + 3).min(inner + 2) as i32, rows as i32 + 2);
        let (left, top) = (1, i32::from(self.play_rows()) - h);
        let bg = pal.hud_bg;
        self.plate(left, top, w, h, bg);
        let edge = pal.of(Hue::Kin);
        let title = format!("─ {prefix} ");
        let top_line = format!("╭{title}{}╮", "─".repeat((w as usize).saturating_sub(2 + title.chars().count())));
        let bot_line = format!("╰{}╯", "─".repeat(w as usize - 2));
        for (r, line) in [(top, top_line), (top + h - 1, bot_line)] {
            self.text(r as u16, left as u16, &line, edge);
        }
        for r in top + 1..top + h - 1 {
            self.text(r as u16, left as u16, "│", edge);
            self.text(r as u16, (left + w - 1) as u16, "│", edge);
        }
        self.text(top as u16, left as u16 + 2, &format!(" {prefix} "), pal.alarm);
        for (n, (k, l)) in items.iter().enumerate() {
            let (r, col) = (top + 1 + (n / per_row) as i32, left + 2 + (n % per_row * cell_w) as i32);
            let x = self.text(r as u16, col as u16, k, key_fg(pal));
            let x = self.text(r as u16, x + 1, "▸", pal.dim);
            self.text(r as u16, x + 1, l, pal.text);
        }
    }

    /// Message line at the top of the play area (between map and panel): look tooltip, the picked card, narration,
    /// The story speaks in a framed card at the top centre, typed out letter by letter so the eye catches it.
    fn story_card(&mut self, sim: &Sim, text: &str, at: u32) {
        let pal = &sim.c.pal;
        let (lo, hi) = (i32::from(MAP_W) + 1, i32::from(self.cols.saturating_sub(PANEL_W + 1)));
        let room = (hi - lo - 4).max(0) as usize;
        let line: String = text.chars().take(room).collect();
        let n = line.chars().count();
        let typed = (sim.tick.saturating_sub(at) as usize * STORY_CPS / sim.c.hz().get() as usize + 1).min(n);
        let w = n as i32 + 4;
        let left = lo + (hi - lo - w) / 2;
        self.plate(left, 0, w, 3, pal.hud_bg);
        let edge = pal.of(Hue::Therm);
        let story = sim.c.texts.get(t::STORY);
        self.text(0, left as u16, &format!("╭─ {story} {}╮", "─".repeat((w as usize).saturating_sub(story.chars().count() + 6))), edge);
        self.text(1, left as u16, "│", edge);
        self.text(1, (left + w - 1) as u16, "│", edge);
        self.text(2, left as u16, &format!("╰{}╯", "─".repeat(w as usize - 2)), edge);
        let shown: String = line.chars().take(typed).collect();
        self.text(1, left as u16 + 2, &shown, pal.text);
    }

    /// or the last action's result.
    fn message(&mut self, sim: &Sim, v: &View) {
        let (c, ui) = (sim.c, v.ui);
        let pal = &c.pal;
        if ui.mode == Mode::Look {
            self.tooltip(sim, ui, 0, MAP_W + 1);
            return;
        }
        let narration = sim.narration.filter(|(_, at)| sim.tick.saturating_sub(*at) < c.hz().get() * STORY_S).and_then(|(ev, at)| match c.waves.get(ev) {
            Some(crate::data::WaveEv::Narrate { text, .. }) => Some((text.as_str(), at)),
            _ => None,
        });
        let card = (ui.mode == Mode::Pick).then(|| sim.offer.and_then(|o| o.cards.get(usize::from(ui.sel)).copied().flatten()).map(|cd| c.skill(cd.skill).desc.as_str())).flatten();
        let (text, fg) = match (card, narration) {
            (Some(d), _) => (d, pal.text),
            (None, Some((n, at))) => {
                self.story_card(sim, n, at);
                return;
            }
            (None, None) => (ui.msg.as_str(), pal.of(Hue::Kin)),
        };
        if text.is_empty() {
            return;
        }
        let (lo, hi) = (usize::from(MAP_W) + 1, usize::from(self.cols.saturating_sub(PANEL_W + 1)));
        let room = hi.saturating_sub(lo);
        let line: String = text.chars().take(room).collect();
        let x = lo + (room - line.chars().count()) / 2;
        self.text(0, x as u16, &line, fg);
    }

    fn tooltip(&mut self, sim: &Sim, ui: &Ui, row: u16, x0: u16) {
        let c = sim.c;
        let (cc, rc) = self.centre();
        let (wx, wy) = self.world(cc + ui.cursor.0, rc + ui.cursor.1);
        let (ax, ay) = (wx.round() as i32, wy.round() as i32);
        let best = (0..sim.e.len()).map(|k| (dist2(sim.e.x[k] - ax, sim.e.y[k] - ay), k)).min().filter(|(d2, _)| *d2 <= 2_250_000);
        let pal = &c.pal;
        let tx = &c.texts;
        let Some((_, k)) = best else {
            self.text(row, x0, tx.get(t::LOOK_HELP), pal.dim);
            return;
        };
        let fam = c.fam(sim.e.fam[k]);
        let mut x = self.text(row, x0, &fam.name, pal.of(fam.hue));
        x = self.text(row, x, tx.get(t::TIER[sim.e.tier[k] as usize]), pal.text);
        x = self.text(row, x + 2, tx.get(t::HUE[fam.hue as usize]), pal.of(fam.hue));
        x = self.text(row, x + 2, &tx.fill(t::LOOK_HP, [&(sim.e.hp[k] / 1000).to_string(), &(sim.e.max[k] / 1000).to_string()]), pal.dim);
        x = self.text(row, x + 2, tx.get(t::LOOK_GLITCH), pal.dim);
        x = self.num(row, x, i64::from(sim.e.glitch[k]), pal.of(Hue::Code));
        x = self.text(row, x + 2, tx.get(if sim.e.side[k] == Side::Hijacked { t::LOOK_HIJACKED } else { t::LOOK_HOSTILE }), pal.text);
        if sim.e.purger[k] {
            self.text(row, x + 2, tx.get(t::LOOK_PURGER), pal.alarm);
        }
        // what it has gained, with a line to match: fed thieves grow and learn tricks
        let loot = sim.e.loot[k];
        if loot > 0 && usize::from(row) + 1 < usize::from(self.play_rows()) {
            let tricks: Vec<&str> = crate::sim::Ability::ALL.iter().filter(|a| a.owned_at(loot)).map(|&a| tx.get(t::ABILITY[a as usize])).collect();
            let x = self.text(row + 1, x0, &tx.fill(t::LOOK_ATE, [&loot.to_string()]), pal.alarm);
            let x = if tricks.is_empty() { x } else { self.text(row + 1, x + 2, &tricks.join(" "), pal.of(Hue::Therm)) };
            self.text(row + 1, x + 2, tx.pick(t::FOE_QUIPS, usize::from(loot)), pal.dim);
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
            // the right half of a wide glyph: the terminal painted it with the glyph, nothing to send
            if b.ch == Glyph::CONT && i % cols > 0 && self.cells[i - 1].ch.is_wide() {
                *f = Some(*b);
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
            let link = self.links.iter().find(|(lo, hi, _)| (*lo..*hi).contains(&i)).map(|l| l.2.as_str());
            if let Some(url) = link {
                self.out.extend_from_slice(b"\x1b]8;id=tot;");
                self.out.extend_from_slice(url.as_bytes());
                self.out.extend_from_slice(b"\x1b\\");
            }
            // an orphaned right half (its wide glyph was overwritten) shows as a space
            let ch = if b.ch == Glyph::CONT { ' ' } else { b.ch.ch() };
            let mut u = [0u8; 4];
            self.out.extend_from_slice(ch.encode_utf8(&mut u).as_bytes());
            if link.is_some() {
                self.out.extend_from_slice(b"\x1b]8;;\x1b\\");
            }
            *f = Some(*b);
            let next = i + if b.ch.is_wide() { 2 } else { 1 };
            cursor = if next % cols == 0 || next / cols != i / cols { None } else { Some(next) };
        }
        if changed == 0 {
            self.out.clear();
            return 0;
        }
        self.out.extend_from_slice(b"\x1b[0m\x1b[?2026l");
        changed
    }

    /// URL of the clickable span under screen cell (col, row), if any.
    pub fn link_at(&self, col: u16, row: u16) -> Option<&str> {
        let i = usize::from(row) * usize::from(self.cols) + usize::from(col);
        self.links.iter().find(|(lo, hi, _)| (*lo..*hi).contains(&i)).map(|l| l.2.as_str())
    }

    /// Glyphs only, one line per row (for `proto2 snapshot`).
    pub fn plain(&self) -> String {
        let mut s = String::new();
        for row in self.cells.chunks(usize::from(self.cols)) {
            s.extend(row.iter().filter(|c| c.ch != Glyph::CONT).map(|c| c.ch.ch()));
            s.push('\n');
        }
        s
    }

    /// Forget what the terminal shows (after resize / clear).
    pub fn invalidate(&mut self) {
        self.front.fill(None);
    }

    /// New run: forget the explored map (the arena is a new one).
    pub fn reset_run(&mut self) {
        self.seen.clear();
        self.face = Face::default();
        self.died_ms = None;
        (self.sky, self.sky_banner, self.shout_at) = (None, None, None);
    }
}

/// One icon per power kind for the picker: attacks are arrows and blasts, traits are solid marks.
fn kind_icon(k: crate::data::SkillKind) -> &'static str {
    use crate::data::SkillKind as K;
    match k {
        K::Bolt => "▶",
        K::Hack => "▣",
        K::Orbit => "◐",
        K::Pulse => "◎",
        K::Chain => "◆",
        K::Mine => "●",
        K::Might => "▲",
        K::Haste => "►",
        K::Plating => "■",
        K::Magnet => "◇",
        K::Regen => "◉",
    }
}

/// Letter and colour of an NPC, on the arena and on the map: pals in the hero colour, keepers in gold.
fn npc_ink(c: &Content, npc: crate::sim::Npc) -> (Glyph, Rgb) {
    use crate::sim::Npc;
    let (pal, gl) = (&c.pal, |k| c.texts.glyph(k));
    match npc {
        Npc::Pal { .. } => (gl(g::NPC_PAL), pal.player),
        Npc::Merchant { .. } => (gl(g::NPC_MERCHANT), pal.chest),
        Npc::Blacksmith { .. } => (gl(g::NPC_BLACKSMITH), pal.chest),
        Npc::Witch { .. } => (gl(g::NPC_WITCH), pal.chest),
        Npc::Neutral => (gl(g::NPC_NEUTRAL), pal.dim),
    }
}

/// Colour a sky speaks in (panel, banner): danger skies in alarm red, wet ones cool, warm ones warm.
fn sky_fg(pal: &crate::data::Palette, w: Weather) -> Rgb {
    match w {
        Weather::BloodMoon | Weather::AcidRain => pal.alarm,
        Weather::Rain | Weather::Snow | Weather::Wind => pal.of(Hue::Code),
        Weather::Heat => pal.of(Hue::Therm),
        Weather::FullMoon | Weather::Clear => pal.text,
    }
}

/// Map cell (MAP_K x MAP_K tiles) is mostly rock: at least a quarter of its tiles are solid.
fn rock(ar: &Arena, cx: i32, cy: i32) -> bool {
    let (t0x, t0y) = (cx * MAP_K, cy * MAP_K);
    (0..MAP_K).flat_map(|dy| (0..MAP_K).map(move |dx| (dx, dy))).filter(|&(dx, dy)| ar.tile(t0x + dx, t0y + dy).solid()).count() as i32 * 4 >= MAP_K * MAP_K
}

fn solid(ar: &Arena, fc: i32, fx: i32, fy: i32) -> bool {
    ar.tile_mm(fx * fc + fc / 2, fy * fc + fc / 2).solid()
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
/// Mark every map cell within `r` cells (round) of `at` as explored: what the hero could see, not just where they stood.
/// shortcut: no line of sight, walls do not stop the reveal; upgrade with a ray march over `Arena::tile_mm` once walls matter here.
/// Mark every map cell within `r` cells (round) of `at` as explored: what the hero could see, not just where they stood.
fn explore(seen: &mut HashSet<(i32, i32)>, (px, py): (i32, i32), r: i32) {
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                seen.insert((px + dx, py + dy));
            }
        }
    }
}

/// Level-up flash length (wall ms): long enough to read the line, short enough not to hide the fight.
const LEVEL_FLASH_MS: u64 = 1000;
/// Points on the level-up ring; enough that it reads as a circle at radius 5.
const LEVEL_RING_POINTS: u32 = 24;
/// Ring radius in rows at the start and end of the flash.
const RING_R0: f64 = 1.0;
const RING_R1: f64 = 5.0;
/// Level-up ring colours (roles, so every scheme keeps them bright), how fast they chase round, and how far the
/// glow under each star is mixed toward the background (permille).
const LEVEL_GLOW: [Role; 5] = [Role::Legendary, Role::Unique, Role::Rare, Role::Magic, Role::Heal];
const LEVEL_SPIN_MS: u64 = 60;
const LEVEL_GLOW_BG: u32 = 450;
/// Rows between the hero's head and the level-up line.
const LEVEL_LINE_GAP: i32 = 2;

/// Kill burst life in sim ticks, split evenly over the `kill_burst` art frames.
const POP_TICKS: u32 = 9;
/// Most bursts alive at once: a wipe of the screen must not cost a frame.
const POPS_MAX: usize = 64;
/// Roar life in sim ticks (about 1.5 s), gap above the head, the uber ROAR sign, and how long a roared id is
/// remembered after it left (so it does not roar again when it steps back into sight).
const ROAR_TICKS: u32 = 45;
const ROAR_FORGET: u32 = 30 * 60 * 5;
const ROAR_GAP: i32 = 1;

/// How loud a foe roars when it first comes into sight: champions, then bosses by class. Order = loudness.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Roar {
    Champion,
    Mini,
    Super,
    Uber,
}

impl Roar {
    fn of(sim: &Sim, k: usize) -> Option<Roar> {
        match sim.e.tier[k] {
            Tier::Champion => Some(Roar::Champion),
            Tier::Boss => Some(match BossClass::of_kind(Tier::Boss, &sim.c.fam(sim.e.fam[k]).kind) {
                Some(BossClass::Uber) => Roar::Uber,
                Some(BossClass::Super) => Roar::Super,
                _ => Roar::Mini,
            }),
            _ => None,
        }
    }

    /// What it shouts (the uber's is the taunt under its ROAR).
    fn pool(self) -> crate::text::Pool {
        match self {
            Roar::Champion => t::ROAR_CHAMPION,
            Roar::Mini => t::ROAR_MINI,
            Roar::Super => t::ROAR_SUPER,
            Roar::Uber => t::ROAR_UBER,
        }
    }
}

/// Death: how long the banner shakes, the crowd hop period.
/// Which-key popup: widest inner width.
const WK_MAX_W: usize = 72;
/// Toggle label background: state colour mixed this far (permille) toward the background.
const TOGGLE_BG: u32 = 450;
/// Picker card rows: rarity colour mixed this far (permille) toward the panel background.
const CARD_TINT: u32 = 780;
/// Story card: seconds it stays, letters typed per second.
const STORY_S: u32 = 8;
const STORY_CPS: usize = 40;
/// Floor loot: glow mix toward the background (permille), tallest beam (rows), pulse period (ticks).
const LOOT_GLOW: u32 = 650;
const LOOT_BEAM_MAX: i32 = 4;
const LOOT_PULSE: u32 = 10;
/// Sponsor link: always at the bottom right, and pleaded on the death screen.
/// Sky: tint strength (permille) per weather, one streak cell in SKY_STREAK_1_IN, fall speed, banner life.
const SKY_ACID_CAST: u32 = 90;
const SKY_BLOOD_CAST: u32 = 160;
const SKY_FULL_CAST: u32 = 70;
const SKY_SNOW_CAST: u32 = 60;
const SKY_HEAT_CAST: u32 = 110;
/// Night darkens the arena this far toward black (permille; text half as much).
const NIGHT_DARK: u32 = 450;
const SKY_STREAK_1_IN: u64 = 45;
const SKY_FALL_TICKS: u32 = 2;
const SKY_BANNER_MS: u64 = 2500;
const DIED_SHAKE_MS: u64 = 1200;
const CHEER_HOP_MS: u64 = 250;

/// Positions of ids in `prev` that are gone from `cur` (both sorted by id): the foes that died this tick.
fn vanished<'a>(prev: &'a [(u32, i32, i32)], cur: &'a [(u32, i32, i32)]) -> impl Iterator<Item = (i32, i32)> + 'a {
    prev.iter().filter(|p| cur.binary_search_by_key(&p.0, |q| q.0).is_err()).map(|p| (p.1, p.2))
}

fn ring_radius(t_ms: u64) -> f64 {
    RING_R0 + (RING_R1 - RING_R0) * (t_ms.min(LEVEL_FLASH_MS) as f64) / LEVEL_FLASH_MS as f64
}

/// What ended the run, in the adventure's words (death screen, status message). A win has no cause.
pub fn cause_text(c: &Content, o: &Outcome) -> String {
    use crate::sim::Cause;
    let tx = &c.texts;
    match o {
        Outcome::Win => String::new(),
        Outcome::Timeout => tx.get(t::CAUSE_TIMEOUT).to_string(),
        Outcome::Dead { cause, .. } => match cause {
            Cause::Contact { fam, tier } => tx.fill(t::CAUSE_CONTACT, [&c.fam(*fam).name, tx.get(t::TIER[*tier as usize]).trim()]),
            Cause::Shot { fam } => tx.fill(t::CAUSE_SHOT, [&c.fam(*fam).name]),
            Cause::BossAttack { fam } => tx.fill(t::CAUSE_BOSS, [&c.fam(*fam).name]),
            Cause::Blast => tx.get(t::CAUSE_BLAST).to_string(),
        },
    }
}

fn level_line(c: &Content, level: u16) -> String {
    c.texts.fill(t::LEVEL_LINE, [&level.to_string(), c.texts.pick(t::LEVEL_TAUNTS, usize::from(level))])
}

/// Gold glow behind a chest, pulsing every CHEST_PULSE ticks; screen sprite and map marker share it.
fn chest_glow(pal: &crate::data::Palette, tick: u32) -> Rgb {
    mix(pal.chest, pal.bg, if (tick / CHEST_PULSE).is_multiple_of(2) { 550 } else { 750 })
}

/// Compass centre cell (col = row) in the 3x3 plate at the map's top-left.
const ROSE_C: f64 = 1.0;
/// Waiting power-up panes: most shown, and their width.
const LOOT_PANES: u16 = 4;
const LOOT_PANE_W: i32 = 16;
/// Power-up panes: their colour role and fill (permille toward the panel background; the top pane pulses to HI).
const POWER_ROLE: Role = Role::Unique;
const POWER_GLOW: u32 = 700;
const POWER_GLOW_HI: u32 = 450;
/// Personas (NPCs): the glow under the head and under the body, the own colour mixed this far (permille) toward
/// the background.
const NPC_GLOW: u32 = 550;
const NPC_BODY_GLOW: u32 = 780;
/// Compass world directions (y grows south), in `t::DIR` order: N, E, S, W.
const COMPASS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

/// Character kinds besides hostile foes that the stats pane counts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cast {
    Hero,
    Ally,
    Panic,
    Thief,
}

impl Cast {
    const ALL: [Cast; 4] = [Cast::Hero, Cast::Ally, Cast::Panic, Cast::Thief];
}

/// Per kind (indexed by `Cast as usize`): how many live, and one example entity to borrow its glyph and hue from.
fn cast_tally(sim: &Sim) -> [(u32, Option<usize>); 4] {
    let mut t = [(0, None); 4];
    t[Cast::Hero as usize] = (1, None);
    for k in 0..sim.e.len() {
        let kind = match (sim.e.side[k], sim.e.panic[k], sim.e.loot[k] > 0) {
            (Side::Hijacked, _, _) => Cast::Ally,
            (_, _, true) => Cast::Thief,
            (_, true, _) => Cast::Panic,
            _ => continue,
        };
        let slot = &mut t[kind as usize];
        slot.0 += 1;
        slot.1.get_or_insert(k);
    }
    t
}

/// Which palette colour paints a mode tag; one colour per mode so the tag is learnt at a glance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ModeInk {
    Player,
    Alarm,
    Xp,
    Code,
    Dim,
    /// palette role colour (new modes read the shared roles, tot-z6a.1)
    Role(Role),
}

/// Vim-style mode tag for the status line. A finished run shows END whatever the mode was.
fn mode_tag(mode: Mode, ended: bool) -> (crate::text::Txt<0>, ModeInk) {
    match (ended, mode) {
        (true, _) => (t::MODE_END, ModeInk::Dim),
        (_, Mode::Play) => (t::MODE_PLAY, ModeInk::Player),
        (_, Mode::Pick) => (t::MODE_PICK, ModeInk::Alarm),
        (_, Mode::Look) => (t::MODE_LOOK, ModeInk::Xp),
        (_, Mode::Cmd) => (t::MODE_CMD, ModeInk::Code),
        (_, Mode::Tree) => (t::MODE_TREE, ModeInk::Role(Role::Magic)),
        (_, Mode::Gear) => (t::MODE_GEAR, ModeInk::Role(Role::Rare)),
        (_, Mode::Shop) => (t::MODE_SHOP, ModeInk::Role(Role::Legendary)),
        (_, Mode::Bags) => (t::MODE_BAGS, ModeInk::Role(Role::Rare)),
        (_, Mode::Start) => (t::MODE_START, ModeInk::Player),
    }
}

/// One key hint on the bottom rows: keys (highlighted), what they do, and for toggles whether they are on.
struct Hint {
    keys: String,
    label: String,
    on: Option<bool>,
}

/// Every binding of the current mode as hints, read from the binding data (nothing to keep in sync by hand).
/// Movement, specials, cards and shop offers fold into one hint each; digit runs read "1-8". In a shop the play
/// One key row when the widest mode (play) fits whole with the mode tag and the sponsor button, else two.
fn strip_rows(c: &Content, cols: u16) -> u16 {
    let need: usize = hints(c, &Ui::new(true)).iter().map(hint_w).sum::<usize>() + 14 + c.texts.get(t::SUPPORT_LABEL).chars().count() + 6;
    if need <= usize::from(cols) { 1 } else { STRIP }
}

/// Cells one hint takes on the key rows: keys, a space, the label, two spaces.
fn hint_w(h: &Hint) -> usize {
    h.keys.chars().count() + 1 + h.label.chars().count() + 2
}

/// keys the shop does not bind still work, so they are listed too.
fn hints(c: &Content, ui: &Ui) -> Vec<Hint> {
    use crate::input::Action;
    let shadowed = |b: &crate::data::Bind| ui.mode == Mode::Shop && c.binds.iter().any(|s| s.mode == Mode::Shop && s.keys == b.keys);
    let binds = c.binds.iter().filter(|b| b.mode == ui.mode || (ui.mode == Mode::Shop && b.mode == Mode::Play && !shadowed(b)));
    let mut groups: Vec<(String, Vec<String>, Option<bool>)> = Vec::new();
    for b in binds {
        let (label, on) = match b.action {
            // a key sequence lives in a which-key group: one entry for the group, its keys show when it opens
            _ if b.keys.len() > 1 => match c.groups.iter().find(|g| g.mode == b.mode && b.keys.starts_with(&g.keys)) {
                Some(g) => (format!("+{}", g.label), None),
                None => (b.label.clone(), None),
            },
            Action::Up | Action::Down | Action::Left | Action::Right => (c.texts.get(t::HINT_MOVE).to_string(), None),
            Action::RotLeft | Action::RotRight => (c.texts.get(t::HINT_TURN).to_string(), None),
            Action::Special(_) => (c.texts.get(t::HINT_SPECIAL).to_string(), None),
            Action::Card(_) => (c.texts.get(t::HINT_TAKE).to_string(), None),
            Action::ShopTake(_) => (c.texts.get(t::HINT_BUY).to_string(), None),
            Action::Autofire => (b.label.clone(), Some(ui.autofire)),
            Action::Autowalk => (b.label.clone(), Some(ui.autowalk)),
            _ => (b.label.clone(), None),
        };
        // a group entry shows only its prefix key
        let key = if label.starts_with('+') { b.keys[0].name() } else { b.keys.iter().map(|k| k.name()).collect::<Vec<_>>().join(" ") };
        match groups.iter_mut().find(|g| g.0 == label) {
            Some(g) if !g.1.contains(&key) => g.1.push(key),
            Some(_) => {}
            None => groups.push((label, vec![key], on)),
        }
    }
    groups
        .into_iter()
        .map(|(label, keys, on)| {
            let digits = keys.iter().all(|k| k.len() == 1 && k.as_bytes()[0].is_ascii_digit());
            let keys = if label == c.texts.get(t::HINT_MOVE) {
                "wasd/hjkl".to_string()
            } else if digits && keys.len() > 2 {
                format!("{}-{}", keys[0], keys[keys.len() - 1])
            } else {
                keys.join("/")
            };
            Hint { keys, label, on }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_turns_with_heading_and_shows_explored_vs_fog() {
        let c = Content::parse(&texts()).unwrap();
        let sim = Sim::new(&c, 7, None);
        let unit = c.view.floor_cell.max(1) * MAP_K;
        let (px, py) = (sim.p.x.div_euclid(unit), sim.p.y.div_euclid(unit));
        // a treasure 2 map cells east of the hero, in world axes
        let chest = [((px + 2) * unit + unit / 2, py * unit + unit / 2)];
        let at = |f: &Frame, (dc, dr): (i32, i32)| f.cells[(MAP_H as i32 / 2 + dr) as usize * 120 + (MAP_W as i32 / 2 + dc) as usize];
        let mut north = Frame::new(&c, 120, 40);
        north.map(&sim, Heading::default(), unit, (px, py), &[], &chest);
        for h in [Heading::default().right(), Heading::default().right().right()] {
            let mut f = Frame::new(&c, 120, 40);
            f.map(&sim, h, unit, (px, py), &[], &chest);
            let want = h.rot(2, 0);
            assert_eq!(at(&f, want).ch.ch(), '■', "chest lands at the turned offset {want:?} for {h:?}");
            assert_ne!(at(&north, want).ch.ch(), '■', "control: heading N puts it elsewhere");
        }
        // explored vs fog: radius 1 around the hero
        let mut f = Frame::new(&c, 120, 40);
        explore(&mut f.seen, (px, py), 1);
        f.map(&sim, Heading::default(), unit, (px, py), &[], &[]);
        let (known, unknown) = (at(&f, (1, 0)), at(&f, (3, 0)));
        assert_eq!((known.ch.ch(), known.bg), ('░', c.pal.hud_bg));
        assert_eq!((unknown.ch.ch(), unknown.bg), (' ', c.pal.fog));
    }

    #[test]
    fn bar_rounds_up_so_a_minority_is_visible() {
        let c = Content::parse(&texts()).unwrap();
        let mut f = Frame::new(&c, 120, 40);
        let filled = |f: &mut Frame, val, max| {
            f.bar(0, 0, val, max, 6, c.pal.text);
            (0..6).filter(|&x| f.cells[x].ch.ch() == '█').count()
        };
        assert_eq!(filled(&mut f, 4, 27), 1, "4 of 27 shows one cell");
        assert_eq!(filled(&mut f, 27, 27), 6);
        // negative control: zero stays empty
        assert_eq!(filled(&mut f, 0, 27), 0);
    }

    #[test]
    fn rose_turns_with_the_map_and_swings_smoothly() {
        let c = Content::parse(&texts()).unwrap();
        let mut f = Frame::new(&c, 120, 40);
        let n_at = |f: &mut Frame, q: f64| {
            f.eye.p = Proj::at(q);
            f.rose(&c);
            let i = (0..usize::from(MAP_H) * 120).find(|&i| i % 120 < usize::from(MAP_W) && f.cells[i].ch.ch() == 'N' && f.cells[i].fg == c.pal.alarm).unwrap();
            // tiny: the whole compass on its own plate in the 3x3 top-left corner
            assert!((i % 120) < 3 && i / 120 < 3, "N in the top-left 3x3");
            assert!([0, 1, 2].iter().all(|&r| f.cells[r * 120..r * 120 + 3].iter().all(|cl| cl.bg == c.pal.hud_bg)), "on its own plate");
            for row in 0..usize::from(MAP_H) {
                f.cells[row * 120..row * 120 + usize::from(MAP_W)].fill(Cell { ch: Glyph::SPACE, fg: c.pal.text, bg: c.pal.bg });
            }
            ((i % 120) as f64 - ROSE_C, (i / 120) as f64 - ROSE_C)
        };
        let mut h = Heading::default();
        for k in 0..4 {
            let (dx, dy) = n_at(&mut f, f64::from(k));
            let (wx, wy) = h.rot(0, -1);
            assert_eq!((dx.signum() as i32 * i32::from(dx.abs() > 0.5), dy.signum() as i32 * i32::from(dy.abs() > 0.5)), (wx, wy), "at rest N points where the map's north is (heading {k})");
            h = h.right();
        }
        // half a turn: N sits between where it was and where it goes (smooth, not a jump)
        let (a, mid, b) = (n_at(&mut f, 0.0), n_at(&mut f, 0.5), n_at(&mut f, 1.0));
        assert!(mid != a && mid != b && mid.0 > a.0 && mid.0 < b.0 + 0.5, "swings through: {a:?} {mid:?} {b:?}");
    }

    #[test]
    fn cast_tally_counts_each_kind_once_and_panel_fits_its_rows() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let mut d = Director::new(7);
        for _ in 0..300 {
            d.tick(&mut sim);
            sim.step(&Input { autofire: true, ..Input::default() });
        }
        assert!(sim.e.len() >= 4, "need foes");
        // negative control: a plain crowd counts only the hero
        for k in 0..sim.e.len() {
            sim.e.side[k] = Side::Hostile;
            sim.e.panic[k] = false;
            sim.e.loot[k] = 0;
        }
        let t = cast_tally(&sim);
        assert_eq!(t.map(|x| x.0), [1, 0, 0, 0]);
        sim.e.side[0] = Side::Hijacked;
        sim.e.panic[1] = true;
        sim.e.loot[2] = 3;
        sim.e.panic[2] = true; // fed AND panicked: counted once, as a thief
        let t = cast_tally(&sim);
        assert_eq!(t.map(|x| x.0), [1, 1, 1, 1]);
        assert_eq!(t[Cast::Ally as usize].1, Some(0));
        assert_eq!(t[Cast::Panic as usize].1, Some(1));
        assert!(PANEL_H > PANEL_CAST_ROW + Cast::ALL.len() as u16);
    }

    #[test]
    fn kills_burst_and_roar_sign_is_well_formed() {
        let prev = [(1, 10, 20), (2, 30, 40), (5, 50, 60)];
        let cur = [(2, 31, 41), (6, 0, 0)];
        assert_eq!(vanished(&prev, &cur).collect::<Vec<_>>(), vec![(10, 20), (50, 60)]);
        // negative control: nobody died when the id sets match
        assert_eq!(vanished(&cur, &cur).count(), 0);
    }

    #[test]
    fn a_kill_between_ticks_draws_a_burst_that_fades() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let mut d = Director::new(7);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let view = |now_ms| View { ui: &ui, now_ms, alpha: 1.0, phase: "BUILD", last_bytes: 0 };
        for _ in 0..400 {
            d.tick(&mut sim);
            sim.step(&Input { autofire: true, ..Input::default() });
            f.compose(&sim, &view(0));
        }
        // fabricate a death: the frame saw an id last tick that the sim no longer has
        f.cur.push((u32::MAX, sim.p.x, sim.p.y));
        f.cur.sort_unstable();
        sim.step(&Input::default());
        f.compose(&sim, &view(0));
        let at = (sim.p.x, sim.p.y);
        let mine = |f: &Frame| f.pops.iter().filter(|p| (p.0, p.1) == at).count();
        assert_eq!(mine(&f), 1, "the vanished id leaves one burst where it stood");
        for _ in 0..POP_TICKS + 1 {
            sim.step(&Input::default());
            f.compose(&sim, &view(0));
        }
        assert_eq!(mine(&f), 0, "burst ages out");
    }

    #[test]
    fn level_up_flash_shows_ring_and_line_for_one_second_only() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let view = |now_ms| View { ui: &ui, now_ms, alpha: 1.0, phase: "BUILD", last_bytes: 0 };
        let text = |f: &Frame| f.cells.iter().map(|c| c.ch.ch()).collect::<String>();
        f.compose(&sim, &view(0));
        assert!(!text(&f).contains("LEVEL"), "no flash before a level up");
        sim.p.level += 1;
        f.compose(&sim, &view(100));
        let t = text(&f);
        assert!(t.contains(&level_line(&c, sim.p.level)), "line shown on level up");
        // ring: stars in several bright colours, each on a glow of its own colour
        let ring: Vec<Rgb> = f.cells.iter().filter(|cl| cl.ch.ch() == '✦' && cl.bg == mix(cl.fg, c.pal.bg, LEVEL_GLOW_BG)).map(|cl| cl.fg).collect();
        assert!(ring.len() >= 8, "ring drawn with glow");
        assert!(LEVEL_GLOW.iter().filter(|&&r| ring.contains(&c.pal.role(r))).count() >= 3, "several bright colours");
        assert!(ring.iter().all(|&fg| LEVEL_GLOW.iter().any(|&r| c.pal.role(r) == fg)), "only the bright roles");
        f.compose(&sim, &view(100 + LEVEL_FLASH_MS + 1));
        assert!(!text(&f).contains("LEVEL"), "flash ends");
        // negative control: same level, no new flash
        f.compose(&sim, &view(5000));
        assert!(!text(&f).contains("LEVEL"));
        assert!(ring_radius(LEVEL_FLASH_MS) > ring_radius(0));
    }

    #[test]
    fn chest_glow_pulses_and_is_gold_tinted() {
        let c = Content::parse(&texts()).unwrap();
        let (a, b) = (chest_glow(&c.pal, 0), chest_glow(&c.pal, CHEST_PULSE));
        assert_ne!(a, b, "glow must pulse");
        assert_eq!(a, chest_glow(&c.pal, 2 * CHEST_PULSE), "period is two phases");
        // negative control: the glow is the chest colour lifted off the bg, not the bg itself
        assert_ne!(a, c.pal.bg);
    }

    #[test]
    fn pal_on_screen_calls_help_and_off_screen_pal_in_earshot_gets_an_arrow() {
        use crate::sim::{Npc, Placed};
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let view = View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 };
        let play = |f: &Frame| (0..usize::from(f.play_rows())).map(|r| f.cells[r * 120..(r + 1) * 120].iter().map(|c| c.ch.ch()).collect::<String>()).collect::<Vec<_>>().join("\n");
        let arrows = |f: &Frame| [ARROW_E, ARROW_W, ARROW_S, ARROW_N].iter().map(|&a| play(f).matches(a).count()).sum::<usize>();
        sim.world.npcs.clear();
        sim.world.goblins.clear();
        f.compose(&sim, &view);
        let base = arrows(&f);
        let help = c.texts.font().rows(c.texts.get(t::PAL_HELP));
        assert!(!painted(&f, &help), "control: no pal, no call");
        sim.world.npcs = vec![Placed { npc: Npc::Pal { traitor: false }, x: sim.p.x + 3000, y: sim.p.y }];
        f.compose(&sim, &view);
        assert!(painted(&f, &help) && play(&f).contains('p') && play(&f).contains('('), "big HELP! with waves\n{}", play(&f));
        // it blinks: another colour one blink later
        let fg_now = f.cells.iter().find(|cl| cl.ch.ch() == '▄' && (cl.fg == c.pal.player || cl.fg == c.pal.xp)).map(|cl| cl.fg);
        sim.tick += PAL_BLINK;
        f.compose(&sim, &view);
        let fg_next = f.cells.iter().find(|cl| cl.ch.ch() == '▄' && (cl.fg == c.pal.player || cl.fg == c.pal.xp)).map(|cl| cl.fg);
        assert!(fg_now.is_some() && fg_now != fg_next, "blinks");
        assert_eq!(arrows(&f), base, "on screen: no arrow");
        sim.world.npcs[0].x = sim.p.x + 100_000;
        f.compose(&sim, &view);
        assert!(!painted(&f, &help));
        assert_eq!(arrows(&f), base + 1, "pal off screen in earshot: one more arrow");
        sim.world.npcs[0].x = sim.p.x + 400_000;
        f.compose(&sim, &view);
        assert_eq!(arrows(&f), base, "NEG: pal out of earshot: no arrow");
    }

    #[test]
    fn night_darkens_and_shows_the_moon() {
        use crate::sim::DAY_S;
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let hz = c.hz().get();
        let at = |f: &mut Frame, sim: &Sim| {
            f.compose(sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            let bright: u32 = f.cells[..120 * 30].iter().map(|c| u32::from(c.bg.0) + u32::from(c.bg.1) + u32::from(c.bg.2)).sum();
            (f.plain(), bright)
        };
        sim.tick = 10 * hz;
        assert!(!sim.night());
        let (day, day_light) = at(&mut f, &sim);
        sim.tick = (DAY_S * 2 / 3 + 10) * hz;
        assert!(sim.night(), "night is the last third of the cycle");
        let (night, night_light) = at(&mut f, &sim);
        assert!(day.contains("🌞 ") && night.contains("🌙 "), "sun by day, moon by night");
        assert!(night_light < day_light, "the arena darkens at night");
    }

    #[test]
    fn weather_shows_in_sky_panel_and_banner() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let tick_of = |s: &Sim, w: Weather| (0..20_000u32).map(|k| k * 30).find(|&t| s.world.forecast(t).now == w).expect("weather occurs");
        let at = |f: &mut Frame, sim: &Sim, ms: u64| {
            f.compose(sim, &View { ui: &ui, now_ms: ms, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.plain()
        };
        sim.tick = tick_of(&sim, Weather::Clear);
        let clear = at(&mut f, &sim, 0);
        let clear_bg: Vec<Rgb> = f.cells.iter().map(|c| c.bg).collect();
        assert!(clear.contains("SKY 🌞 CLEAR"), "panel names the sky with its icon\n{clear}");
        sim.tick = tick_of(&sim, Weather::BloodMoon);
        let blood = at(&mut f, &sim, 100);
        assert!(blood.contains("SKY 🩸 BLOOD MOON"), "{blood}");
        // the emoji goes out once, its hidden half never
        f.emit();
        let out = String::from_utf8_lossy(&f.out).to_string();
        assert!(!out.contains('\u{1}'), "no continuation byte on the wire");
        let banner = |f: &Frame| {
            let text = "BLOOD MOON!";
            let left = 60 - c.texts.font().width(text) / 2;
            c.texts.font().rows(text).iter().enumerate().all(|(r, line)| line.chars().enumerate().all(|(k, ch)| ch == ' ' || f.cells[(2 + r) * 120 + left + k].ch.ch() == ch))
        };
        assert!(banner(&f), "change banner\n{blood}");
        let tinted = f.cells.iter().zip(&clear_bg).filter(|(a, b)| a.bg != **b).count();
        assert!(tinted > 1000, "the blood moon casts red ({tinted} cells)");
        // banner fades
        let _ = at(&mut f, &sim, 100 + SKY_BANNER_MS);
        assert!(!banner(&f), "banner fades");
        sim.tick = tick_of(&sim, Weather::Rain);
        let rain = at(&mut f, &sim, 10_000);
        let streak = c.texts.glyph(g::RAIN).ch();
        assert!(rain.matches(streak).count() > clear.matches(streak).count() + 20, "rain streaks");
    }

    #[test]
    fn death_is_loud_banner_killer_crowned_crowd_cheers() {
        use crate::sim::{Cause, Outcome};
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        sim.world.npcs.clear();
        for k in 0..6 {
            sim.push_enemy(fam, Tier::Trash, (sim.p.x + 4_000 + k * 1_500, sim.p.y + 2_000), 1_000_000, (false, false));
        }
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let at = |f: &mut Frame, sim: &Sim, ms: u64| {
            f.compose(sim, &View { ui: &ui, now_ms: ms, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.plain()
        };
        let alive = at(&mut f, &sim, 0);
        assert!(!alive.contains("HERO-SLAYER") && !alive.contains("slain by"), "control: alive, no death show");
        sim.outcome = Some(Outcome::Dead { cause: Cause::Contact { fam, tier: Tier::Trash }, foreseen: true });
        let s0 = at(&mut f, &sim, 1000);
        let banner = c.texts.art(a::DEATH_BANNER);
        assert!(s0.contains(banner[0].trim_end()), "banner\n{s0}");
        assert!(s0.contains("slain by") && s0.contains("HERO-SLAYER"), "{s0}");
        // the crowd hops: two frames a hop apart differ
        let s1 = at(&mut f, &sim, 1000 + CHEER_HOP_MS);
        assert_ne!(s0, s1, "the crowd moves while the hero lies dead");
    }

    #[test]
    fn map_shows_rock_only_where_explored() {
        let c = Content::parse(&texts()).unwrap();
        let sim = Sim::new(&c, 7, None);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let map = |f: &Frame| (0..usize::from(MAP_H)).flat_map(|r| f.cells[r * 120..r * 120 + usize::from(MAP_W)].iter().map(|c| c.ch.ch())).filter(|&ch| ch == '█').count();
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let near = map(&f);
        // explore the whole map window: the room's cave walls appear
        explore(&mut f.seen, (0, 0), i32::from(MAP_W));
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        assert!(map(&f) > near + 10, "explored rock shows on the map ({} vs {near})", map(&f));
    }

    #[test]
    fn panel_shows_gold_and_pals() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let view = View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 };
        f.compose(&sim, &view);
        assert!(f.plain().contains("GOLD 0  PALS 0"), "control: empty purse");
        (sim.p.gold, sim.world.pals) = (437, 2);
        f.compose(&sim, &view);
        assert!(f.plain().contains("GOLD 437  PALS 2"), "{}", f.plain());
    }

    #[test]
    fn reset_run_forgets_explored_cells() {
        let c = Content::parse(&texts()).unwrap();
        let mut f = Frame::new(&c, 120, 40);
        explore(&mut f.seen, (0, 0), 3);
        assert!(!f.seen.is_empty());
        f.reset_run();
        assert!(f.seen.is_empty());
    }

    #[test]
    fn explore_marks_view_radius_not_just_the_standing_cell() {
        let mut s = HashSet::new();
        explore(&mut s, (10, 10), 3);
        assert!(s.contains(&(10, 10)) && s.contains(&(13, 10)) && s.contains(&(10, 7)));
        // negative controls: outside the circle stays fog
        assert!(!s.contains(&(14, 10)) && !s.contains(&(13, 13)));
        assert_eq!(s.len(), 29);
    }

    /// Some placement on the frame shows every non-space cell of `lines` (painted words leave gaps to the floor).
    fn painted(f: &Frame, lines: &[String]) -> bool {
        let (w, h) = (usize::from(f.cols), usize::from(f.rows));
        let rows: Vec<Vec<char>> = lines.iter().map(|l| l.chars().collect()).collect();
        let lw = rows.iter().map(Vec::len).max().unwrap_or(0);
        (0..h.saturating_sub(rows.len())).any(|r0| (0..w.saturating_sub(lw)).any(|c0| rows.iter().enumerate().all(|(r, l)| l.iter().enumerate().all(|(k, &ch)| ch == ' ' || f.cells[(r0 + r) * w + c0 + k].ch.ch() == ch))))
    }

    #[test]
    fn hero_shows_worn_gear_and_level_tier() {
        let c = Content::parse(&texts()).unwrap();
        let ui = Ui::new(true);
        let n = i32::from(c.view.actor_rows[0]);
        let item = |s: Slot| c.items.iter().position(|it| it.slot == s).map(|k| ItemId(k as u16)).unwrap();
        // (torso, right hand, left hand, left shoulder, right shoulder): glyph + colour, None where nothing of the hero's
        let look = |sim: &Sim| {
            let mut f = Frame::new(&c, 160, 50);
            f.compose(sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            let w = i32::from(f.cols);
            let head = (0..f.cells.len()).find(|&j| f.z[j] == Prio::PlayerCore && f.cells[j].ch.ch() == '@').unwrap() as i32;
            let at = |dx: i32, dy: i32| {
                let j = (head + dy * w + dx) as usize;
                (f.z[j] == Prio::PlayerCore).then(|| (f.cells[j].ch.ch(), f.cells[j].fg))
            };
            [at(0, n - 1), at(1, n - 1), at(-1, n - 1), at(-1, 0), at(1, 0)]
        };
        let mut sim = Sim::new(&c, 7, None);
        sim.world.npcs.clear();
        let bare = look(&sim);
        assert_eq!(bare[0].map(|t| t.0), Some(c.pal.body.ch()), "control: bare torso");
        assert_eq!(bare[1..], [None; 4], "control: nothing beside a bare level-1 hero");
        let ink = |id: ItemId| (c.items[usize::from(id.0)].glyph.ch(), c.pal.role(c.items[usize::from(id.0)].rarity.role()));
        let [armor, weapon, charm] = [Slot::Armor, Slot::Weapon, Slot::Charm].map(item);
        for id in [armor, weapon, charm] {
            sim.gear.wear(&c, id);
        }
        sim.p.level = HERO_TIERS[1];
        let geared = look(&sim);
        assert_eq!(geared[0], Some((c.texts.glyph(g::HERO_ARMOR).ch(), ink(armor).1)), "armour plates the torso");
        assert_eq!((geared[1], geared[2]), (Some(ink(weapon)), Some(ink(charm))), "weapon right, charm left");
        assert_eq!((geared[3].map(|t| t.0), geared[4].map(|t| t.0)), (Some('▟'), Some('▙')), "tier-2 shoulders");
    }

    #[test]
    fn a_foe_that_ate_loot_grows_shoulders_per_ability() {
        let c = Content::parse(&texts()).unwrap();
        let ui = Ui::new(true);
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        let mut sim = Sim::new(&c, 7, None);
        sim.world.npcs.clear();
        sim.push_enemy(fam, Tier::Trash, (sim.p.x + 3_000, sim.p.y + 2_000), 1_000_000, (false, false));
        // glyphs beside the foe's head
        let flanks = |sim: &Sim| {
            let mut f = Frame::new(&c, 160, 50);
            f.compose(sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            let head = (0..f.cells.len()).find(|&j| f.z[j] == Prio::Trash && f.cells[j].ch == c.fam(fam).glyph).unwrap();
            [head - 1, head + 1].map(|j| f.cells[j].ch.ch())
        };
        assert!(!flanks(&sim).contains(&'▟'), "control: a foe that ate nothing");
        sim.e.loot[0] = Ability::Rage.loot_needed();
        assert_eq!(flanks(&sim), ['▟', '▙'], "two abilities owned = tier-2 shoulders");
    }

    #[test]
    fn boss_cloud_grows_and_colours_by_class() {
        let ui = Ui::new(true);
        // one boss def (sweeper, 40k HP = Super); the control copy drops its HP into the Mini range
        let mini_hp = texts().into_iter().map(|t| t.replace("\"hp_abs\":40000", "\"hp_abs\":4000")).collect::<Vec<_>>();
        assert_ne!(mini_hp, texts(), "control edit applied");
        // (boss cells, cells in the family hue, cells in the Legendary role colour), both fogged as drawn
        let drawn = |texts: &[String]| {
            let c = Content::parse(texts).unwrap();
            let mut sim = Sim::new(&c, 7, None);
            sim.world.npcs.clear();
            let f_id = c.fam_ids().find(|&f| c.fam(f).key == "sweeper").unwrap();
            sim.push_enemy(f_id, Tier::Boss, (sim.p.x + 3_000, sim.p.y + 2_000), 1_000_000, (false, false));
            let mut f = Frame::new(&c, 160, 50);
            f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            let (x, y) = f.pos[0];
            let (fam_hue, gold) = (f.fogged(&c, c.pal.of(c.fam(f_id).hue), x, y), f.fogged(&c, c.pal.role(Role::Legendary), x, y));
            let boss = |col: Option<Rgb>| (0..f.cells.len()).filter(|&j| f.z[j] == Prio::EliteBoss && col.is_none_or(|col| f.cells[j].fg == col)).count();
            (boss(None), boss(Some(fam_hue)), boss(Some(gold)))
        };
        let (mini, super_) = (drawn(&mini_hp), drawn(&texts()));
        assert!(super_.0 > mini.0, "super cloud is bigger: {super_:?} vs {mini:?}");
        assert!(mini.1 > 0 && mini.2 == 0, "control: mini keeps the family hue {mini:?}");
        assert!(super_.2 > 0, "super core in the Legendary colour {super_:?}");
    }

    #[test]
    fn champions_and_bosses_roar_by_class_and_hero_shouts() {
        let c = Content::parse(&texts()).unwrap();
        let ui = Ui::new(true);
        let roared = |fam: &str, tier: Tier| {
            let mut sim = Sim::new(&c, 7, None);
            sim.world.npcs.clear();
            let f_id = c.fam_ids().find(|&f| c.fam(f).key == fam).unwrap();
            sim.push_enemy(f_id, tier, (sim.p.x + 9_000, sim.p.y + 6_000), 1_000_000, (false, false));
            let mut f = Frame::new(&c, 160, 50);
            f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            let loud = Roar::of(&sim, 0).unwrap();
            let word = c.texts.pick(loud.pool(), (sim.e.id[0] as usize).wrapping_mul(7));

            (loud, painted(&f, &c.texts.font().rows(word)))
        };
        assert_eq!(roared("scrapper", Tier::Champion), (Roar::Champion, true));
        assert_eq!(roared("kiosk", Tier::Boss), (Roar::Mini, true));
        assert_eq!(roared("sweeper", Tier::Boss), (Roar::Super, true));
        assert!(Roar::Champion < Roar::Mini && Roar::Super < Roar::Uber, "loudness order");
        let sign = c.texts.art(a::ROAR_SIGN);
        assert_eq!(crate::bigtext::double(sign).len(), 2 * sign.len(), "uber ROAR is double size");
        // hero: gold makes it shout SHINY! big, with the line in a bubble
        let mut sim = Sim::new(&c, 7, None);
        sim.p.unhurt = 1000;
        let mut f = Frame::new(&c, 120, 40);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        sim.p.gold += 5;
        sim.tick += 1;
        f.compose(&sim, &View { ui: &ui, now_ms: 40, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let s = f.plain();

        assert!(painted(&f, &c.texts.font().rows(c.texts.shout(crate::data::Bark::Gold))) && s.contains('╭') && s.contains('┬'), "big shout + bubble\n{s}");
        // the bubble stands on the Doom face: its tail is right above the face centre
        let (fx, y0) = f.face_origin().unwrap();
        assert_eq!(f.cells[(usize::from(y0) - 1) * 120 + usize::from(fx + FACE_W / 2)].ch.ch(), '┬', "bubble on the face");
        // the shout holds its screen spot while the hero walks on (tot-7op)
        let word = c.texts.font().rows(c.texts.shout(crate::data::Bark::Gold));
        // first cell where the word's top row is painted
        let spot = |f: &Frame| {
            let w = usize::from(f.cols);
            (0..f.cells.len()).find(|&j| j % w + word[0].chars().count() <= w && word[0].chars().enumerate().all(|(k, ch)| ch == ' ' || f.cells[j + k].ch.ch() == ch))
        };
        let hero = |f: &Frame| (0..f.cells.len()).find(|&j| f.z[j] == Prio::PlayerCore && f.cells[j].ch.ch() == '@');
        let (spot0, hero0) = (spot(&f), hero(&f));
        sim.p.x += 4_000;
        sim.tick += 1;
        f.compose(&sim, &View { ui: &ui, now_ms: 80, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        assert_ne!(hero(&f), hero0, "control: the hero moved on screen");
        assert_eq!(spot(&f), spot0, "the shout did not follow");
    }

    #[test]
    fn floor_loot_glows_by_rarity_and_gold_glows() {
        use crate::data::Rarity;
        use crate::sim::{Drop, ItemId};
        let c = Content::parse(&texts()).unwrap();
        let ui = Ui::new(true);
        let by = |r: Rarity| ItemId(c.items.iter().position(|it| it.rarity == r).unwrap() as u16);
        let beam = |id: ItemId| {
            let mut sim = Sim::new(&c, 7, None);
            sim.world.npcs.clear();
            let (x, y) = (sim.p.x + 3_000, sim.p.y + 3_000);
            sim.drops.x.push(x);
            sim.drops.y.push(y);
            sim.drops.kind.push(Drop::Item(id));
            sim.drops.pulled.push(false);
            let mut f = Frame::new(&c, 120, 40);
            f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            let (col, row) = f.scr(f64::from(x), f64::from(y));
            let fg = c.pal.role(c.items[usize::from(id.0)].rarity.role());
            let cell = f.cells[row as usize * 120 + col as usize];
            assert!(cell.fg == fg && cell.bg != c.pal.bg, "item in its rarity colour on a glow");
            (1..=6).take_while(|h| f.cells[(row - h) as usize * 120 + col as usize].fg == fg && "│╎".contains(f.cells[(row - h) as usize * 120 + col as usize].ch.ch())).count()
        };
        let (common, unique) = (beam(by(Rarity::Common)), beam(by(Rarity::Unique)));
        assert!(common == 0 && unique >= 4, "beam grows with rarity: common {common}, unique {unique}");
        assert!(beam(by(Rarity::Rare)) < unique);
        // gold piles: golden on the pulsing gold glow
        let mut sim = Sim::new(&c, 7, None);
        sim.world.npcs.clear();
        let (x, y) = (sim.p.x + 3_000, sim.p.y - 3_000);
        sim.world.gold_piles.push(crate::sim::GoldPile { x, y, gold: 5 });
        let mut f = Frame::new(&c, 120, 40);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let (col, row) = f.scr(f64::from(x), f64::from(y));
        let cell = f.cells[row as usize * 120 + col as usize];
        assert_eq!((cell.ch.ch(), cell.fg, cell.bg), ('$', c.pal.chest, chest_glow(&c.pal, sim.tick)), "gold glows");
    }

    #[test]
    fn floor_motif_recolours_its_share_of_floor_glyphs_and_walls_take_the_wall_colour() {
        let at = texts().iter().position(|t| t.contains("\"t\":\"palette\"")).unwrap();
        let with = |from: &str, to: &str| {
            let mut ts = texts();
            assert!(ts[at].contains(from), "control edit applies: {from}");
            ts[at] = ts[at].replace(from, to);
            Content::parse(&ts).unwrap()
        };
        let shot = |c: &Content| {
            let mut sim = Sim::new(c, 7, None);
            pillars(&mut sim);
            let ui = Ui::new(true);
            let mut f = Frame::new(c, 120, 40);
            f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.cells.iter().map(|cl| (cl.ch.ch(), cl.fg)).collect::<Vec<_>>()
        };
        let base = shot(&Content::parse(&texts()).unwrap());
        let diff = |a: &[(char, Rgb)], b: &[(char, Rgb)]| a.iter().zip(b).filter(|(x, y)| x != y).count();
        let motif = shot(&with("\"floor_motif_share\":0,", "\"floor_motif_share\":0.5,"));
        assert_eq!(base.iter().map(|x| x.0).collect::<Vec<_>>(), motif.iter().map(|x| x.0).collect::<Vec<_>>(), "same glyphs");
        assert!(diff(&base, &shot(&with("\"floor_motif\":\"#4a2ad8\"", "\"floor_motif\":\"#ff0000\""))) == 0, "control: share 0 draws no motif");
        let red = shot(&with("\"floor_motif\":\"#4a2ad8\",\"floor_motif_share\":0,", "\"floor_motif\":\"#ff0000\",\"floor_motif_share\":0.5,"));
        let n = diff(&base, &red);
        let all = diff(&base, &shot(&with("\"floor_motif\":\"#4a2ad8\",\"floor_motif_share\":0,", "\"floor_motif\":\"#ff0000\",\"floor_motif_share\":1,")));
        assert!(0 < n && n < all, "the motif share sets how many floor glyphs wear it: half {n} of {all}");
        assert!(diff(&base, &shot(&with("\"wall\":\"#4a2ad8\"", "\"wall\":\"#101010\""))) > 20, "walls take the wall colour");
    }

    #[test]
    fn personas_stand_as_tall_as_elites_and_glow() {
        use crate::sim::{Npc, Placed};
        let c = Content::parse(&texts()).unwrap();
        let ui = Ui::new(true);
        let glowing = |npcs: Vec<Placed>| {
            let mut sim = Sim::new(&c, 7, None);
            sim.world.npcs = npcs.into_iter().map(|p| Placed { x: sim.p.x + 4_000, y: sim.p.y + 3_000, ..p }).collect();
            let mut f = Frame::new(&c, 120, 40);
            f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.cells.iter().map(|cl| cl.bg).collect::<Vec<_>>()
        };
        let fg = npc_ink(&c, Npc::Merchant { honest: true }).1;
        let (bare, with) = (glowing(vec![]), glowing(vec![Placed { npc: Npc::Merchant { honest: true }, x: 0, y: 0 }]));
        let new = |col: Rgb| bare.iter().zip(&with).filter(|(a, b)| *a != *b && **b == col).count();
        let (head, body) = (new(mix(fg, c.pal.bg, NPC_GLOW)), new(mix(fg, c.pal.bg, NPC_BODY_GLOW)));
        assert!(head == 1 && body >= usize::from(c.view.actor_rows[2]) - 1, "a glowing head over a glowing body as tall as an elite: {head} {body}");
        assert_eq!(bare, glowing(vec![]), "control: the same frame without the persona");
    }

    #[test]
    fn keepers_and_goblins_on_the_map_and_pointed_at() {
        use crate::sim::{Npc, Placed};
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let view = View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 };
        let map = |f: &Frame| (0..usize::from(MAP_H)).flat_map(|r| f.cells[r * 120..r * 120 + usize::from(MAP_W)].iter().map(|c| c.ch.ch())).collect::<String>();
        sim.world.npcs.clear();
        sim.world.goblins.clear();
        f.compose(&sim, &view);
        let (merchant, goblin) = (c.texts.glyph(g::NPC_MERCHANT).ch(), c.texts.glyph(g::MAP_GOBLIN).ch());
        assert!(!map(&f).contains(merchant) && !map(&f).contains(goblin), "control: none on the map");
        let gold_arrows = |f: &Frame| f.cells.iter().filter(|cl| [ARROW_E, ARROW_W, ARROW_N, ARROW_S].contains(&cl.ch.ch()) && cl.fg == c.pal.chest).count();
        let before = gold_arrows(&f);
        sim.world.npcs = vec![Placed { npc: Npc::Merchant { honest: true }, x: sim.p.x + 40_000, y: sim.p.y }];
        f.compose(&sim, &view);
        assert!(map(&f).contains(merchant), "merchant on the map");
        assert_eq!(gold_arrows(&f), before + 1, "and a gold arrow points at it");
        sim.world.goblins.push(crate::sim::Goblin { x: sim.p.x, y: sim.p.y - 30_000 });
        f.compose(&sim, &view);
        assert!(map(&f).contains(goblin), "goblin on the map");
    }

    #[test]
    fn hero_shout_never_covers_the_picker() {
        let c = Content::parse(&texts()).unwrap();
        let mut ui = Ui::new(true);
        ui.sync(true);
        ui.key(crate::input::KeyEv { key: crate::input::Key::Char('p'), kind: crate::input::Kind::Press, text: Some('p') }, &c, 0);
        assert_eq!(ui.mode, Mode::Pick);
        // the same run up to its first offer, then the hero gets gold (a shout) or not
        let shot = |gold: u32| {
            let mut s = Sim::new(&c, 2, None);
            let mut d = Director::new(2);
            while s.offer.is_none() {
                d.tick(&mut s);
                s.step(&Input { autofire: true, ..Input::default() });
            }
            s.p.unhurt = 1000;
            let mut f = Frame::new(&c, 120, 40);
            f.compose(&s, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            s.p.gold += gold;
            s.tick += 1;
            f.compose(&s, &View { ui: &ui, now_ms: 40, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f
        };
        let (quiet, loud) = (shot(0), shot(5));
        let (w, h) = (POPUP_W.min(120 - 4), POPUP_H);
        let (x0, y0) = ((120 - w) / 2, (quiet.play_rows() - h) / 2);
        let in_box = |j: usize| (x0..x0 + w).contains(&((j % 120) as u16)) && (y0..y0 + h).contains(&((j / 120) as u16));
        let diff: Vec<usize> = (0..quiet.cells.len()).filter(|&j| quiet.cells[j] != loud.cells[j]).collect();
        assert!(diff.iter().any(|&j| !in_box(j)), "control: the shout shows outside the picker");
        assert!(diff.iter().all(|&j| !in_box(j)), "the picker is untouched by the shout");
    }

    #[test]
    fn power_ups_wait_as_loot_panes_until_p() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 2, None);
        let mut d = Director::new(2);
        let mut ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        assert!(!f.plain().contains("POWER-UP"), "control: nothing waiting");
        while sim.offer.is_none() {
            d.tick(&mut sim);
            sim.step(&Input { autofire: true, ..Input::default() });
        }
        ui.sync(sim.offer.is_some());
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        assert!(f.plain().contains("POWER-UP") && !f.plain().contains("UPGRADE"), "waits as a pane, no popup");
        // coloured to be spotted: the frame in the power colour (not the plain chest white), the inside filled
        let corner = f.cells[usize::from(MAP_H + 1) * 120];
        assert_eq!((corner.ch.ch(), corner.fg), ('╭', c.pal.role(POWER_ROLE)), "pane frame in the power colour");
        assert_ne!(c.pal.role(POWER_ROLE), c.pal.chest, "control: not the chest white");
        assert_ne!(f.cells[usize::from(MAP_H + 2) * 120 + 3].bg, c.pal.hud_bg, "pane filled with its glow");
        assert_eq!(ui.mode, Mode::Play);
        ui.key(crate::input::KeyEv { key: crate::input::Key::Char('p'), kind: crate::input::Kind::Press, text: Some('p') }, &c, 7_000);
        f.compose(&sim, &View { ui: &ui, now_ms: 7_000, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        assert!(f.plain().contains("UPGRADE") && !f.plain().contains("POWER-UP"), "p opens the picker");
        assert_eq!(ui.pick_open_ms, 7_000, "its timer starts on opening");
    }

    #[test]
    fn look_shows_what_a_foe_has_gained() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        sim.world.npcs.clear();
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        sim.push_enemy(fam, Tier::Trash, (sim.p.x + 300, sim.p.y), 1_000_000, (false, false));
        let mut ui = Ui::new(true);
        ui.mode = Mode::Look;
        let mut f = Frame::new(&c, 120, 40);
        let mut look = |sim: &Sim| {
            f.compose(sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.plain()
        };
        assert!(look(&sim).contains("SCRAPPER") && !look(&sim).contains("UPGRADED"), "control: a foe that ate nothing");
        sim.e.loot[0] = 30;
        let s = look(&sim);
        assert!(s.contains("UPGRADED: ate 30 loot") && c.texts.pool(t::FOE_QUIPS).iter().any(|q| s.contains(q.as_str())), "what it gained, with a line\n{s}");
    }

    #[test]
    fn story_types_out_in_a_framed_card() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let mut d = Director::new(7);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 160, 45);
        while sim.narration.is_none() {
            d.tick(&mut sim);
            sim.step(&Input::default());
        }
        let (ev, at) = sim.narration.unwrap();
        let crate::data::WaveEv::Narrate { text, .. } = &c.waves[ev] else { panic!("not a narration") };
        let top = |f: &mut Frame, sim: &Sim| {
            f.compose(sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.plain().lines().take(3).collect::<Vec<_>>().join("\n")
        };
        sim.tick = at + 3;
        let early = top(&mut f, &sim);
        assert!(early.contains("◆ STORY") && !early.contains(text.as_str()), "framed, still typing\n{early}");
        sim.tick = at + c.hz().get() * 4;
        assert!(top(&mut f, &sim).contains(text.as_str()), "typed out whole");
        sim.tick = at + c.hz().get() * STORY_S;
        assert!(!top(&mut f, &sim).contains("◆ STORY"), "gone after its time");
    }

    #[test]
    fn start_screen_shows_title_adventures_then_the_picked_adventures_classes_in_its_colours() {
        let d = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/data"));
        let base = Content::load_profile(d, None).unwrap();
        let advs = crate::data::profiles(d).unwrap();
        let mut ui = Ui::new(true);
        ui.mode = Mode::Start;
        let mut f = Frame::new(&base, 120, 40);
        f.start(&base, &ui, &advs);
        let s = f.plain();
        let art = base.texts.art(crate::text::a::TITLE);
        assert!(art.iter().all(|r| s.contains(r.trim())), "title art\n{s}");
        assert!(s.contains(base.texts.get(crate::text::t::PICK_ADVENTURE)) && advs.iter().all(|a| s.contains(&a.name) && s.contains(&a.blurb)));
        assert!(f.cells.iter().any(|cl| cl.bg == base.pal.role(Role::Select)), "the cursor's entry is lit");
        // step 2: the dark adventure, loaded: its classes, in its colours
        let dark_ix = advs.iter().position(|a| a.id == "hellish").unwrap();
        let dark = Content::load_profile(d, Some("hellish")).unwrap();
        ui.start = crate::input::Start::Class { adv: dark_ix, sel: 0 };
        let mut f = Frame::new(&dark, 120, 40);
        f.start(&dark, &ui, &advs);
        let s = f.plain();
        assert!(s.contains(dark.texts.get(crate::text::t::PICK_CLASS)) && dark.classes.iter().all(|k| s.contains(&k.name) && s.contains(&k.desc)), "{s}");
        assert!(!base.classes.iter().any(|k| s.contains(&k.name)), "control: not the base classes");
        assert_ne!(dark.pal.bg, base.pal.bg, "control: the adventures differ in colour");
        assert_eq!(f.cells[0].bg, dark.pal.bg, "painted in the adventure's colours");
    }

    #[test]
    fn shown_words_come_from_the_adventure_texts() {
        let at = texts().iter().position(|t| t.contains("\"pick_adventure\"")).unwrap();
        let mut ts = texts();
        ts[at] = ts[at].replace("\"text\":\"FLUX \"", "\"text\":\"MANA \"").replace("\"text\":\"-- PLAY --\"", "\"text\":\"~ ROAM ~\"").replace("\"id\":\"map_hero\",\"glyph\":\"@\"", "\"id\":\"map_hero\",\"glyph\":\"Y\"");
        let (base, mine) = (Content::parse(&texts()).unwrap(), Content::parse(&ts).unwrap());
        let ui = Ui::new(true);
        let shot = |c: &Content| {
            let sim = Sim::new(c, 7, None);
            let mut f = Frame::new(c, 120, 40);
            f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.plain()
        };
        let (a, b) = (shot(&base), shot(&mine));
        assert!(a.contains("FLUX ") && a.contains("-- PLAY --") && !a.contains("MANA"), "control: the base words");
        assert!(b.contains("MANA ") && b.contains("~ ROAM ~") && !b.contains("FLUX ") && !b.contains("-- PLAY --"), "the adventure's words\n{b}");
        let map = |s: &str| s.lines().take(usize::from(MAP_H)).map(|l| l.chars().take(usize::from(MAP_W)).collect::<String>()).collect::<String>();
        assert!(map(&a).contains('@') && !map(&a).contains('Y') && map(&b).contains('Y'), "the adventure's map hero glyph");
    }

    #[test]
    fn hero_pane_draws_the_picked_class_art() {
        let at = texts().iter().position(|t| t.contains("\"t\":\"class\"")).unwrap();
        let mut ts = texts();
        // the second class gets its own head row
        let second = ts[at].lines().filter(|l| l.contains("\"t\":\"class\"")).nth(1).unwrap().to_string();
        ts[at] = ts[at].replace(&second, &second.replacen("\"    (◉ ◉)    \"", "\"    {o_o}    \"", 1));
        let c = Content::parse(&ts).unwrap();
        let sim = Sim::new(&c, 7, None);
        let mut ui = Ui::new(true);
        ui.mode = Mode::Gear;
        let mut f = Frame::new(&c, 200, 60);
        let mut shot = |ui: &Ui| {
            f.compose(&sim, &View { ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.plain()
        };
        let first = shot(&ui);
        assert!(first.contains("[o_o]") && !first.contains("{o_o}"), "control: the first class's art");
        ui.class = c.class_id(1).unwrap();
        let s = shot(&ui);
        assert!(s.contains("{o_o}") && !s.contains("[o_o]"), "the second class's art\n{s}");
    }

    #[test]
    fn hero_pane_highlights_the_selection_in_every_tab() {
        let c = Content::parse(&texts()).unwrap();
        let sim = Sim::new(&c, 7, None);
        let mut f = Frame::new(&c, 200, 60);
        let sel = mix(c.pal.alarm, c.pal.hud_bg, heropane::SEL_BG);
        // cells on the selection background
        let mut lit = |ui: &Ui| {
            f.compose(&sim, &View { ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            (0..f.cells.len()).filter(|&j| f.cells[j].bg == sel).collect::<Vec<_>>()
        };
        let mut ui = Ui::new(true);
        assert!(lit(&ui).is_empty(), "control: no pane, no selection");
        for mode in [Mode::Gear, Mode::Tree, Mode::Bags] {
            ui.mode = mode;
            (ui.gear_sel, ui.tree_sel, ui.bag_sel) = (0, 0, 0);
            let a = lit(&ui);
            (ui.gear_sel, ui.tree_sel, ui.bag_sel) = (1, 1, 1);
            let b = lit(&ui);
            assert!(!a.is_empty() && !b.is_empty() && a != b, "{mode:?}: selection lit and it moves");
        }
    }

    #[test]
    fn hero_pane_shows_the_hero_and_the_tree() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let mut ui = Ui::new(true);
        let mut f = Frame::new(&c, 200, 60);
        let mut show = |ui: &Ui, sim: &Sim| {
            f.compose(sim, &View { ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.plain()
        };
        ui.mode = Mode::Tree;
        let tree = show(&ui, &sim);
        let root = &crate::input::hero_tree(&c, crate::data::ClassId::FIRST).unwrap().nodes[0].name;
        assert!(tree.contains("SKILLS") && tree.contains(&format!("◇{root}")), "tree grid in the pane");
        let worn = ItemId(0);
        sim.gear.wear(&c, worn);
        ui.mode = Mode::Gear;
        let armor = show(&ui, &sim);

        assert!(armor.contains("[o_o]") && armor.contains(&c.items[0].name), "the hero and its gear");
        ui.mode = Mode::Play;
        let play = show(&ui, &sim);
        assert!(!play.contains("(◉ ◉)") && !play.contains(" SKILLS "), "out of the tab modes there is no pane");
        sim.bag.push(ItemId(1));
        ui.mode = Mode::Bags;
        ui.bag_sel = 0;
        let bags = show(&ui, &sim);
        assert!(bags.contains(" BAGS ") && bags.contains(&c.items[1].name) && bags.contains("┌─┐"), "the bag grid with the selected item named");
    }

    #[test]
    fn whichkey_opens_a_vim_style_popup() {
        use crate::input::{Key, KeyEv, Kind};
        let c = Content::parse(&texts()).unwrap();
        let sim = Sim::new(&c, 7, None);
        let mut ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let show = |f: &mut Frame, ui: &Ui, ms: u64| {
            f.compose(&sim, &View { ui, now_ms: ms, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.plain()
        };
        assert!(!show(&mut f, &ui, 0).contains('╭'), "control: no popup");
        ui.key(KeyEv { key: Key::Char('c'), kind: Kind::Press, text: Some('c') }, &c, 1000);
        let s = show(&mut f, &ui, 1000 + c.opt.whichkey_ms + 50);
        assert!(s.contains("╭─ c ") && s.contains('╰'), "bordered, titled with the prefix\n{s}");
        assert!(s.contains("q ▸ turn left") && s.contains("e ▸ turn right"), "{s}");
        assert!(s.lines().nth(39).unwrap().contains("-- PLAY --"), "key rows stay");
    }

    #[test]
    fn support_link_bottom_right_and_on_death() {
        use crate::sim::{Cause, Outcome};
        let c = Content::parse(&texts()).unwrap();
        let (label, url) = (c.texts.get(t::SUPPORT_LABEL), c.texts.get(t::SUPPORT_URL));
        let mut sim = Sim::new(&c, 7, None);
        let ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let last = f.plain().lines().nth(39).unwrap().to_string();
        assert!(last.trim_end().ends_with(&format!("▐ {label} ▌")), "a pill button: {last}");
        let lx = last.chars().count() - last.chars().rev().position(|ch| ch == '▌').unwrap() - 1;
        assert_eq!(f.cells[39 * 120 + lx - 2].bg, c.pal.alarm, "filled");
        f.emit();
        let out = String::from_utf8_lossy(&f.out).to_string();
        assert!(out.contains(&format!("\x1b]8;id=tot;{url}")), "OSC 8 link emitted");
        assert_eq!(out.matches("\x1b]8;;").count(), c.texts.get(t::SUPPORT_LABEL).chars().count() + 4, "each link cell closed");
        let fam = c.fam_ids().next().unwrap();
        sim.outcome = Some(Outcome::Dead { cause: Cause::Contact { fam, tier: Tier::Trash }, foreseen: true });
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        assert!(f.plain().contains(c.texts.get(t::SUPPORT_PLEA)), "death plea");
        assert_eq!(f.links.len(), 2, "button + plea are both links");
        let (fx, _) = (0..120u16).map(|x| (x, f.link_at(x, 39))).find(|p| p.1.is_some()).unwrap();
        assert_eq!(f.link_at(fx, 39), Some(c.texts.get(t::SUPPORT_URL)), "click on the bottom button hits the link");
        assert_eq!(f.link_at(0, 0), None, "NEG: elsewhere no link");
    }

    #[test]
    fn one_key_row_when_everything_fits() {
        let c = Content::parse(&texts()).unwrap();
        let (narrow, wide) = (Frame::new(&c, 120, 40), Frame::new(&c, 260, 40));
        assert_eq!((narrow.strip, wide.strip), (2, 1), "two rows only when the keys do not fit on one");
        assert_eq!(wide.play_rows(), narrow.play_rows() + 1, "the play area gains the row");
        let mut f = wide;
        let sim = Sim::new(&c, 7, None);
        let ui = Ui::new(true);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let last = f.plain().lines().nth(39).unwrap().to_string();
        for h in hints(&c, &ui) {
            assert!(last.contains(&format!("{} {}", h.keys, h.label)), "{} missing: {last}", h.label);
        }
        assert!(last.contains("-- PLAY --") && last.contains(c.texts.get(t::SUPPORT_LABEL)));
    }

    #[test]
    fn bottom_rows_list_every_key_fixed_and_highlighted() {
        let c = Content::parse(&texts()).unwrap();
        let mut sim = Sim::new(&c, 7, None);
        let mut ui = Ui::new(true);
        let mut f = Frame::new(&c, 120, 40);
        let rows = |f: &Frame| f.plain().lines().skip(38).collect::<Vec<_>>().join("\n");
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let a = rows(&f);
        for b in c.binds.iter().filter(|b| b.mode == Mode::Play) {
            let shown = hints(&c, &ui).into_iter().any(|h| a.contains(&format!("{} {}", h.keys, h.label)) && (h.label == b.label || ["move", "special", "turn", "+camera"].contains(&h.label.as_str())));
            assert!(shown, "binding {:?} ({}) not on the bottom rows:\n{a}", b.keys, b.label);
        }
        assert!(a.contains("spc fire") && a.contains("Q quit") && a.contains("1-8 special"), "{a}");
        // walking, time and bytes change: the rows do not
        for k in 0..40 {
            sim.step(&Input { mv: (1, 0), autofire: true, ..Input::default() });
            f.compose(&sim, &View { ui: &ui, now_ms: k * 33, alpha: 1.0, phase: "SWARM", last_bytes: 99_999 });
        }
        assert_eq!(rows(&f), a, "fixed layout");
        // keys stand out by colour (accent, plain background); the label is ordinary text
        let at = f.plain().lines().skip(38).enumerate().find_map(|(r, l)| l.find("spc fire").map(|x| (38 + r, l[..x].chars().count()))).unwrap();
        let cell = |f: &Frame, dx: usize| f.cells[at.0 * 120 + at.1 + dx];
        assert!(cell(&f, 0).fg == key_fg(&c.pal) && cell(&f, 0).bg == cell(&f, 5).bg && cell(&f, 5).fg != key_fg(&c.pal), "key in accent, no block");
        // a toggle shows its state as the label background only: text never changes
        let lab = f.plain().lines().skip(38).enumerate().find_map(|(r, l)| l.find("F autofire").map(|x| (38 + r, l[..x].chars().count() + 2))).unwrap();
        let bg_at = |f: &Frame| f.cells[lab.0 * 120 + lab.1].bg;
        let on_bg = bg_at(&f);
        assert!(on_bg != c.pal.bg, "ON has a coloured background");
        ui.autofire = false;
        f.compose(&sim, &View { ui: &ui, now_ms: 9_999, alpha: 1.0, phase: "SWARM", last_bytes: 0 });
        assert_eq!(rows(&f), a, "same text with auto-fire off");
        assert!(bg_at(&f) != on_bg && bg_at(&f) != c.pal.bg, "OFF: another colour");
        assert_eq!(rows(&f).replace("auto-fire [OFF]", "auto-fire [ON] "), a, "only the tag changed");
    }

    #[test]
    fn status_line_tags_and_keys_follow_mode_and_end() {
        let c = Content::parse(&texts()).unwrap();
        for (m, tag) in [(Mode::Play, "PLAY"), (Mode::Pick, "PICK"), (Mode::Look, "LOOK"), (Mode::Cmd, "CMD")] {
            assert!(c.texts.get(mode_tag(m, false).0).contains(tag));
        }
        assert!(c.texts.get(mode_tag(Mode::Play, true).0).contains("END"));
        let inks = [Mode::Play, Mode::Pick, Mode::Look, Mode::Cmd].map(|m| mode_tag(m, false).1);
        assert!(inks.iter().enumerate().all(|(i, a)| inks.iter().skip(i + 1).all(|b| a != b)));
    }

    use crate::data::Content;
    use crate::data::tests::texts;
    use crate::director::Director;
    use crate::iso::Heading;
    use crate::sim::{Input, PickChoice};

    // re-pinned 2026-11-08 (tot-ecr.11 status line: coloured mode tag + contextual keys; tot-sul.2 map compass + camera-turned map; tot-sul.1 explored map texture; tot-2f4.3 map chest glow; tot-z6a.2 kill bursts + ROAR; tot-ecr.6 stats pane bars + CAST rows; fix batch: bar() rounds up, status row hint last; solid() via tile_mm): arrows with distance + glow, chest sprite, boss bar, strip row A removed, panel XP/powers/foe tiers/morale bars, bottom bar (reviewed in snapshot)
    // re-pinned 2026-11-08: key rows, message line, phase+NET in panel, hotkey digits, sponsor button bottom right, keys in accent (no block) and [ON]/[OFF] toggle tags, sponsor pill button, toggles by label background, short labels, NPCs+goblins on the map
    // re-pinned 2026-11-09: hero shout holds the screen spot where it began (tot-7op)
    // re-pinned 2026-11-09: tiny compass on a 3x3 plate at the map top-left (tot-5ah)
    // re-pinned 2026-11-09: cyberpunk faces, walls and art from the adventure data (tot-62z.6)
    // re-pinned 2026-11-09: cyberpunk sprite glyphs themed (tot-62z.17)
    // re-pinned 2026-11-09: cyberpunk neon base24 and wall lore (neon signs on lit wall faces)
    // re-pinned 2026-11-09: cyberpunk cyan-purple neon, wall lore as shadow only
    const GOLDEN: u64 = 0x1cd8_6c2f_133d_97d7;

    fn fnv(b: &[u8]) -> u64 {
        b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3))
    }

    /// Pillar arena on every decor super-cell (sim walls follow it too).
    fn pillars(sim: &mut Sim) {
        sim.with_arena(|a| {
            a.template = crate::floorgen::Template::Pillars;
            a.decor_chance = 1000;
            a.decor_r = 3000;
        });
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
        ui.sync(true);
        ui.key(crate::input::KeyEv { key: crate::input::Key::Char('p'), kind: crate::input::Kind::Press, text: Some('p') }, &c, 0);
        let mut f = Frame::new(&c, 120, 40);
        f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
        let row = |r: usize| f.cells[r * 120..(r + 1) * 120].iter().map(|c| c.ch.ch()).collect::<String>();
        let name = sim.offer.and_then(|o| o.cards[0]).map(|k| c.skill(k.skill).name.clone()).unwrap();
        assert!(row(39).contains("-- PICK --"));
        // the picker is a popup in the play area: the card name shows there, the strip only points at it
        let kind = sim.offer.and_then(|o| o.cards[0]).map(|k| kind_icon(c.skill(k.skill).kind)).unwrap();
        assert!((0..37).any(|r| row(r).contains(&format!("1 {kind} {name}"))), "popup shows card 1 with its kind icon");
        // every kind has its own icon, drawable
        use crate::data::SkillKind as K;
        let all = [K::Bolt, K::Hack, K::Orbit, K::Pulse, K::Chain, K::Mine, K::Might, K::Haste, K::Plating, K::Magnet, K::Regen].map(kind_icon);
        assert!(all.iter().enumerate().all(|(i, a)| a.chars().all(|ch| Glyph::of(ch).is_some()) && !all[i + 1..].contains(a)), "distinct drawable icons");
        assert!(!row(38).contains(&name), "{}", row(38));
        assert!((38..40).any(|r| row(r).contains("take")), "pick keys on the key rows");
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
        // the strip is plain except the sponsor button (a link span) and toggle labels (state colours)
        let in_link = |i: usize| f.links.iter().any(|(lo, hi, _)| (*lo..*hi).contains(&i));
        let toggles = [Role::Heal, Role::Danger].map(|r| mix(c.pal.role(r), c.pal.bg, TOGGLE_BG));
        assert!(f.cells.iter().enumerate().skip((40 - 2) * 120).all(|(i, cell)| cell.bg == c.pal.hud_bg || cell.bg == c.pal.bg || in_link(i) || toggles.contains(&cell.bg)));
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
            let away = solid(sim.arena(), fc, x.div_euclid(fc), y.div_euclid(fc)) || k >= 121;
            (sim.e.x[k], sim.e.y[k]) = if away { (sim.p.x + 200_000, sim.p.y) } else { (x, y) };
        }
        sim
    }

    /// O-iso: block faces in >= 3 shade classes, standing stacks (head over body over shadow),
    /// shadow cells, and >= 1 actor hidden by a nearer block. Ok = frame hash.
    #[test]
    fn lit_walls_carry_the_adventure_lore_as_shadow_only() {
        let at = texts().iter().position(|t| t.contains("\"id\":\"wall_lore\"")).unwrap();
        let lore = |line: &str| {
            let mut ts = texts();
            let old = ts[at].lines().find(|l| l.contains("\"id\":\"wall_lore\"")).unwrap().to_string();
            ts[at] = ts[at].replace(&old, &format!("{{\"t\":\"pool\",\"id\":\"wall_lore\",\"lines\":[\"{line}\"]}}"));
            Content::parse(&ts).unwrap()
        };
        // the frame's cells (glyph, colour) on a pillar map
        let shot = |c: &Content| {
            let mut sim = Sim::new(c, 7, None);
            pillars(&mut sim);
            let ui = Ui::new(true);
            let mut f = Frame::new(c, 120, 40);
            f.compose(&sim, &View { ui: &ui, now_ms: 0, alpha: 1.0, phase: "BUILD", last_bytes: 0 });
            f.cells.iter().map(|cl| (cl.ch.ch(), cl.fg)).collect::<Vec<_>>()
        };
        let (marked, bare) = (shot(&lore("ZZZZ")), shot(&lore("    ")));
        assert!(!marked.iter().any(|(ch, _)| *ch == 'Z'), "lore is never drawn as letters");
        assert_eq!(marked.iter().map(|x| x.0).collect::<Vec<_>>(), bare.iter().map(|x| x.0).collect::<Vec<_>>(), "same glyphs: lore only shades");
        let shaded = marked.iter().zip(&bare).filter(|(a, b)| a.1 != b.1).count();
        assert!(shaded > 10, "lore darkens the lit wall faces ({shaded} cells)");
    }

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
        // clear sky: rain streaks animate on purpose (weather_shows_in_sky_panel_and_banner); these budgets are for a still scene
        sim.tick = (0..20_000u32).map(|k| k * 30).find(|&t| sim.world.forecast(t).now == Weather::Clear).unwrap_or(0);
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

    /// Pick pause test sizes in 60 fps frames: frames rendered, frames the pick is held, and frames
    /// the camera needs to settle (follow_ms x 4 = 48).
    const PAUSE_FRAMES_RUN: u32 = 190;
    const PAUSE_HOLD: usize = 80;
    const PAUSE_SETTLE: usize = 50;
    /// Ticks walked before the xp grant (the camera has something to follow).
    const PAUSE_WARMUP: u32 = 30;

    /// Pick pause at 60 fps with play.rs's alpha (it keeps cycling 0..1 while the sim is frozen):
    /// walk into the first pick, hold it PAUSE_HOLD frames, pick, walk on. `forget` = NEG: the frame
    /// forgets each frame when it saw the tick (pre-P9 behaviour). The popup rect is blanked before
    /// the diff: its countdown ticks on purpose. Returns (pause frame range, per frame (bytes,
    /// camera, drawn player screen position), whether the hero moved after the pick: a wall may
    /// block the walk).
    fn pause(c: &Content) -> (std::ops::Range<usize>, [Vec<PauseFrame>; 2], bool) {
        let walk_in = Input { mv: (1, 0), autofire: true, ..Input::default() };
        let run = |sim: &mut Sim, d: &mut Director, inp: &Input| {
            d.tick(sim);
            sim.step(inp);
        };
        // warm up a little walk, then grant exactly one level of xp: the pick opens within a tick or two
        let (mut sim, mut d) = (Sim::new(c, 2, None), Director::new(2));
        for _ in 0..PAUSE_WARMUP {
            run(&mut sim, &mut d, &walk_in);
        }
        sim.p.xp = sim.xp_need();
        let ui = Ui::new(true);
        let mut fs = [Frame::new(c, 120, 40), Frame::new(c, 120, 40)];
        let (mut outs, mut start, mut end) = ([Vec::new(), Vec::new()], None, None);
        let mut at_pick = (sim.p.x, sim.p.y);
        for k in 0..PAUSE_FRAMES_RUN {
            if k % 2 == 0 {
                let held = sim.offer.is_some().then(|| *start.get_or_insert(outs[0].len()));
                if end.is_none() && held.is_some_and(|s| outs[0].len() >= s + PAUSE_HOLD) {
                    end = Some(outs[0].len());
                    run(&mut sim, &mut d, &Input { pick: Some(PickChoice::Card(0)), ..walk_in });
                } else {
                    run(&mut sim, &mut d, &walk_in);
                }
            }
            let now_ms = u64::from(k) * 1000 / 60;
            let alpha = f64::from(k % 2) / 2.0;
            for (n, (f, out)) in fs.iter_mut().zip(outs.iter_mut()).enumerate() {
                if n == 1 {
                    *f = Frame::new(c, 120, 40); // NEG: forgets the tick it saw
                }
                f.compose(&sim, &v(&ui, now_ms, alpha));
                let (w, h) = (POPUP_W.min(f.cols - 4), POPUP_H);
                let (x0, y0) = ((f.cols - w) / 2, (f.play_rows() - h) / 2);
                for r in y0..y0 + h {
                    for col in x0..x0 + w {
                        f.cells[usize::from(r) * usize::from(f.cols) + usize::from(col)] = Cell { ch: Glyph::SPACE, fg: c.pal.text, bg: c.pal.bg };
                    }
                }
                f.emit();
                if n == 0 && end == Some(out.len()) {
                    at_pick = (sim.p.x, sim.p.y);
                }
                let (px, py) = Frame::lerp(alpha, Some(f.pprev), sim.p.x, sim.p.y);
                let (pc, pr) = Proj::at(0.0).screen(px, py);
                out.push((f.out.len(), cam_at(f), (pc, 2.0 * pr)));
            }
        }
        let s = start.unwrap_or(0);
        (s..end.unwrap_or(s), outs, (sim.p.x, sim.p.y) != at_pick)
    }

    /// (bytes, camera, drawn player) of one frame, screen col widths.
    type PauseFrame = (usize, (f64, f64), (f64, f64));

    #[test]
    fn pick_pause_frame_is_still_and_resume_is_smooth_and_forget_control_flickers() {
        // the pause option stays available: this test arms it
        let mut c = Content::parse(&texts()).unwrap();
        c.opt.pick_pauses = true;
        let (r, [out, neg], moved) = pause(&c);
        assert!(r.len() == PAUSE_HOLD, "armed: the sim was frozen for {PAUSE_HOLD} frames ({r:?})");
        let held = &out[r.clone()];
        // the camera finishes its follow (< follow_ms x 4 = 48 frames), then the frame is still
        let tail: Vec<usize> = held[PAUSE_SETTLE..].iter().map(|o| o.0).collect();
        let cams: Vec<(f64, f64)> = held.iter().map(|o| o.1).collect();
        eprintln!("PAUSE bytes first 6 {:?} settled max {} sum {}", held[..6].iter().map(|o| o.0).collect::<Vec<_>>(), tail.iter().max().unwrap_or(&0), tail.iter().sum::<usize>());
        assert!(tail.iter().all(|b| *b == 0), "frozen sim: still frame after the camera settles {tail:?}");
        assert!(o_smooth(&cams).is_ok(), "frozen sim: camera converges without reversing {:?}", o_smooth(&cams));
        // resume: no snap or pop of the camera or the drawn player across the pick
        let around = &out[r.end - 10..r.end + 20];
        let (cams, players): (Vec<_>, Vec<_>) = around.iter().map(|o| (o.1, o.2)).unzip();
        let pmax = steps(&players).iter().fold(0.0f64, |m, d| m.max(d.0.hypot(d.1)));
        eprintln!("RESUME camera {:?} player max step {pmax:.3}", o_smooth(&cams));
        assert!(o_smooth(&cams).is_ok() && pmax <= 1.5, "resume pops: player step {pmax}");
        assert!(!moved || players[29] != players[0], "armed: the player walks on after the pick (unless a wall blocks)");
        let tail: Vec<usize> = neg[r][PAUSE_SETTLE..].iter().map(|o| o.0).collect();
        eprintln!("NEG forget: last 6 bytes {:?}", &tail[tail.len() - 6..]);
        assert!(tail.iter().filter(|b| **b > 0).count() >= tail.len() / 2, "negative control: cycling alpha on a held tick must flicker");
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
                    solid(sim.arena(), fc, bx, by) && dc.abs() <= 1 && (2..=4).contains(&dr)
                })
            });
            if front && !solid(sim.arena(), fc, px.div_euclid(fc), py.div_euclid(fc)) {
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
