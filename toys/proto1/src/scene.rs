//! Typed scenes (plan §Scenes) and their deterministic frame generators.
use crate::grid::{Cell, Dims, Glyph, Grid, Rgb, Rng, hash};
use crate::kitty::Medium;
use crate::{Error, Result};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scene {
    S0,
    S1,
    S2,
    S3,
    S4,
    S5,
    S6,
    S7,
    S8,
    N1,
    N2,
    N3,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Text {
    Typical,
    Half,
    Full,
    Nothing,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gfx {
    None,
    Glow { medium: Medium, placed: bool },
    Boss,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sink {
    Null,
    Tty,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Plan {
    pub text: Text,
    pub sync: bool,
    /// frame bytes repeated this many times in one write (N1 = 10)
    pub repeat: u8,
    pub gfx: Gfx,
    pub sink: Sink,
}

impl Scene {
    pub const ALL: [Scene; 12] = [
        Scene::S0,
        Scene::S1,
        Scene::S2,
        Scene::S3,
        Scene::S4,
        Scene::S5,
        Scene::S6,
        Scene::S7,
        Scene::S8,
        Scene::N1,
        Scene::N2,
        Scene::N3,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scene::S0 => "S0",
            Scene::S1 => "S1",
            Scene::S2 => "S2",
            Scene::S3 => "S3",
            Scene::S4 => "S4",
            Scene::S5 => "S5",
            Scene::S6 => "S6",
            Scene::S7 => "S7",
            Scene::S8 => "S8",
            Scene::N1 => "N1",
            Scene::N2 => "N2",
            Scene::N3 => "N3",
        }
    }

    pub fn select(s: &str) -> Result<Vec<Scene>> {
        if s == "all" {
            return Ok(Scene::ALL.to_vec());
        }
        Scene::ALL.iter().copied().find(|c| c.name() == s).map(|c| vec![c]).ok_or_else(|| Error::Usage(format!("unknown scene {s}")))
    }

    pub fn plan(self) -> Plan {
        let p = Plan { text: Text::Typical, sync: false, repeat: 1, gfx: Gfx::None, sink: Sink::Tty };
        let glow = |medium, placed| Plan { gfx: Gfx::Glow { medium, placed }, ..p };
        match self {
            Scene::S0 => Plan { sink: Sink::Null, ..p },
            Scene::S1 => p,
            Scene::S2 => Plan { text: Text::Half, ..p },
            Scene::S3 => Plan { text: Text::Full, ..p },
            Scene::S4 => Plan { text: Text::Full, sync: true, ..p },
            Scene::S5 => glow(Medium::Shm, true),
            Scene::S6 => glow(Medium::File, true),
            Scene::S7 => glow(Medium::Direct, true),
            Scene::S8 => Plan { gfx: Gfx::Boss, ..p },
            Scene::N1 => Plan { text: Text::Full, repeat: 10, ..p },
            Scene::N2 => Plan { text: Text::Nothing, ..p },
            Scene::N3 => glow(Medium::Shm, false),
        }
    }
}

const ENTITIES: usize = 400;
pub const GLOW_W: u32 = 160;
pub const GLOW_H: u32 = 90;
pub const BOSS_W: u32 = 64;
pub const BOSS_H: u32 = 32;
pub const BOSS_FRAMES: u32 = 8;

pub struct World {
    pub grid: Grid,
    rng: Rng,
    seed: u64,
    pub frame: u64,
    ents: Vec<(u16, u16)>,
}

fn floor(seed: u64, i: usize) -> Cell {
    let h = hash(seed ^ (i as u64).wrapping_mul(0x1000_0001));
    let k = (h % 24) as u8;
    Cell { ch: Glyph::pick(h >> 8), fg: Rgb(60 + k, 70 + k, 90 + k), bg: Rgb(10 + k / 2, 12 + k / 2, 18 + k / 2) }
}

impl World {
    pub fn new(dims: Dims, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let (c, r) = (u64::from(dims.cols()), u64::from(dims.rows()));
        let ents = (0..ENTITIES).map(|_| ((rng.next() % c) as u16, (rng.next() % r) as u16)).collect();
        let mut grid = Grid::new(dims, floor(seed, 0));
        for (i, cell) in grid.cells.iter_mut().enumerate() {
            *cell = floor(seed, i);
        }
        World { grid, rng, seed, frame: 0, ents }
    }

    /// Advance one frame of `text` content into `grid`.
    pub fn step(&mut self, text: Text) {
        self.frame += 1;
        let dims = self.grid.dims;
        let (cols, rows) = (dims.cols(), dims.rows());
        match text {
            Text::Nothing => return,
            Text::Typical => {
                for (i, c) in self.grid.cells.iter_mut().enumerate() {
                    *c = floor(self.seed, i);
                }
                for e in &mut self.ents {
                    let r = self.rng.next();
                    let step = |v: u16, max: u16, bits: u64| match bits % 3 {
                        0 => v.saturating_sub(1),
                        1 => (v + 1).min(max - 1),
                        _ => v,
                    };
                    *e = (step(e.0, cols, r), step(e.1, rows, r >> 8));
                    let hot = Rgb(255, (r >> 16) as u8 | 128, 40);
                    self.grid.set(e.0, e.1, Cell { ch: Glyph::AT, fg: hot, bg: Rgb(40, 10, 10) });
                }
            }
            Text::Half => {
                for c in &mut self.grid.cells {
                    let r = self.rng.next();
                    if r & 1 == 1 {
                        let v = |s: u32| (r >> s) as u8;
                        *c = Cell { ch: Glyph::pick(r >> 40), fg: Rgb(v(8), v(16), v(24)), bg: Rgb(v(32) / 4, v(48) / 4, 30) };
                    }
                }
            }
            Text::Full => {
                // fog pulse: k shifts by 1 per frame mod 16, so every cell differs from last frame
                for (i, c) in self.grid.cells.iter_mut().enumerate() {
                    let (x, y) = ((i % cols as usize) as u64, (i / cols as usize) as u64);
                    let k = ((self.frame + x + y) % 16) as u8;
                    *c = Cell { ch: Glyph::pick(k.into()), fg: Rgb(110 + k * 8, 120 + k * 6, 140 + k * 4), bg: Rgb(20 + k * 6, 24 + k * 5, 40 + k * 8) };
                }
            }
        }
        self.grid.counter(self.frame);
    }
}

/// 1/4-res glow layer: soft orange blob that moves each frame.
pub fn glow_rgba(frame: u64, out: &mut Vec<u8>) {
    out.clear();
    let cx = (frame * 2 % u64::from(GLOW_W)) as i64;
    let cy = (GLOW_H / 2) as i64 + (frame % 20) as i64 - 10;
    for y in 0..i64::from(GLOW_H) {
        for x in 0..i64::from(GLOW_W) {
            let d2 = (x - cx).pow(2) + (y - cy).pow(2);
            let a = 255 - (d2 * 255 / 900).min(255);
            out.extend_from_slice(&[255, 140, 30, a as u8]);
        }
    }
}

pub fn boss_rgba(k: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((BOSS_W * BOSS_H * 4) as usize);
    for y in 0..BOSS_H {
        for x in 0..BOSS_W {
            let on = (x + y + k * 4) % 16 < 8;
            v.extend_from_slice(if on { &[200, 40, 220, 255] } else { &[20, 0, 30, 160] });
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::emit_diff;

    fn frames(text: Text, seed: u64, n: usize) -> (Vec<usize>, Vec<u8>) {
        let dims = Dims::new(200, 60).unwrap();
        let mut w = World::new(dims, seed);
        let mut front = vec![None; dims.cells()];
        let (mut changed, mut all) = (vec![], vec![]);
        for _ in 0..n {
            w.step(text);
            let mut out = Vec::new();
            changed.push(emit_diff(&mut front, &w.grid, &mut out));
            all.extend(out);
        }
        (changed, all)
    }

    #[test]
    fn full_changes_every_cell() {
        let (changed, _) = frames(Text::Full, 1, 5);
        assert!(changed[1..].iter().all(|&c| c == 12000), "{changed:?}");
    }

    #[test]
    fn half_changes_about_half() {
        let (changed, _) = frames(Text::Half, 1, 5);
        assert!(changed[1..].iter().all(|&c| (5600..6400).contains(&c)), "{changed:?}");
    }

    #[test]
    fn typical_is_sparse_and_nothing_is_empty() {
        let (changed, _) = frames(Text::Typical, 1, 5);
        assert!(changed[1..].iter().all(|&c| c < 1200), "{changed:?}");
        let (none, _) = frames(Text::Nothing, 1, 3);
        assert_eq!(&none[1..], &[0, 0]);
    }

    #[test]
    fn seeded_frames_are_deterministic() {
        assert_eq!(frames(Text::Half, 42, 3).1, frames(Text::Half, 42, 3).1);
        // negative control: other seed, other bytes
        assert_ne!(frames(Text::Half, 42, 3).1, frames(Text::Half, 43, 3).1);
    }

    #[test]
    fn scene_plans() {
        assert_eq!(Scene::N3.plan().gfx, Gfx::Glow { medium: Medium::Shm, placed: false });
        assert_eq!(Scene::N1.plan().repeat, 10);
        assert_eq!(Scene::S0.plan().sink, Sink::Null);
        assert!(Scene::select("S9").is_err());
        assert_eq!(Scene::select("all").unwrap().len(), 12);
    }

    #[test]
    fn glow_size() {
        let mut v = Vec::new();
        glow_rgba(3, &mut v);
        assert_eq!(v.len(), (GLOW_W * GLOW_H * 4) as usize);
        assert_eq!(boss_rgba(1).len(), (BOSS_W * BOSS_H * 4) as usize);
    }
}
