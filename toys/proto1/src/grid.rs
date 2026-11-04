//! Cell grid, seeded RNG, SGR diff writer. Frame path uses hand decimal, never core::fmt
//! (research-perf §1c: fmt = 2/3 of emit cost).
use crate::{Error, Result};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Printable single-byte ASCII glyph. Only built from the fixed tables below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Glyph(u8);

const GLYPHS: &[u8] = b".,:;-=+*#%@";

impl Glyph {
    pub fn pick(n: u64) -> Self {
        Glyph(GLYPHS[(n % GLYPHS.len() as u64) as usize])
    }
    pub fn digit(d: u64) -> Self {
        Glyph(b'0' + (d % 10) as u8)
    }
    pub const AT: Glyph = Glyph(b'@');
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: Glyph,
    pub fg: Rgb,
    pub bg: Rgb,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Dims {
    cols: u16,
    rows: u16,
}

impl Dims {
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        if (1..=1000).contains(&cols) && (1..=1000).contains(&rows) {
            Ok(Dims { cols, rows })
        } else {
            Err(Error::Dims { cols, rows })
        }
    }
    pub fn cols(self) -> u16 {
        self.cols
    }
    pub fn rows(self) -> u16 {
        self.rows
    }
    pub fn cells(self) -> usize {
        self.cols as usize * self.rows as usize
    }
}

pub struct Grid {
    pub dims: Dims,
    pub cells: Vec<Cell>,
}

impl Grid {
    pub fn new(dims: Dims, fill: Cell) -> Self {
        Grid { dims, cells: vec![fill; dims.cells()] }
    }
    pub fn set(&mut self, x: u16, y: u16, c: Cell) {
        let i = y as usize * self.dims.cols as usize + x as usize;
        if let Some(slot) = self.cells.get_mut(i) {
            *slot = c;
        }
    }
    /// Frame counter in the top-left corner: glyph only, keeps the cell colours.
    pub fn counter(&mut self, frame: u64) {
        let mut n = frame;
        for i in (0..6.min(self.cells.len())).rev() {
            self.cells[i].ch = Glyph::digit(n);
            n /= 10;
        }
    }
}

/// xorshift64* (seeded, deterministic).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng((seed ^ 0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// splitmix64 finalizer: stateless per-cell hash.
pub fn hash(a: u64) -> u64 {
    let mut z = a.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub fn dec(out: &mut Vec<u8>, v: u64) {
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

fn rgb(out: &mut Vec<u8>, c: Rgb) {
    dec(out, c.0.into());
    out.push(b';');
    dec(out, c.1.into());
    out.push(b';');
    dec(out, c.2.into());
}

/// Diff `back` against what the terminal shows (`front`), append escapes, update `front`.
/// CUP only on gaps, SGR only on pen change, SGR reset at frame end. Returns changed cells.
pub fn emit_diff(front: &mut [Option<Cell>], back: &Grid, out: &mut Vec<u8>) -> usize {
    let cols = back.dims.cols as usize;
    let mut cursor = None;
    let mut pen: Option<(Rgb, Rgb)> = None;
    let mut changed = 0;
    for (i, (f, b)) in front.iter_mut().zip(&back.cells).enumerate() {
        if *f == Some(*b) {
            continue;
        }
        changed += 1;
        if cursor != Some(i) {
            out.extend_from_slice(b"\x1b[");
            dec(out, (i / cols + 1) as u64);
            out.push(b';');
            dec(out, (i % cols + 1) as u64);
            out.push(b'H');
        }
        match pen {
            Some((fg, bg)) if fg == b.fg && bg == b.bg => {}
            Some((fg, _)) if fg == b.fg => {
                out.extend_from_slice(b"\x1b[48;2;");
                rgb(out, b.bg);
                out.push(b'm');
            }
            Some((_, bg)) if bg == b.bg => {
                out.extend_from_slice(b"\x1b[38;2;");
                rgb(out, b.fg);
                out.push(b'm');
            }
            _ => {
                out.extend_from_slice(b"\x1b[38;2;");
                rgb(out, b.fg);
                out.extend_from_slice(b";48;2;");
                rgb(out, b.bg);
                out.push(b'm');
            }
        }
        pen = Some((b.fg, b.bg));
        out.push(b.ch.0);
        *f = Some(*b);
        // no reliance on pending-wrap at the last column
        cursor = if (i + 1) % cols == 0 { None } else { Some(i + 1) };
    }
    if pen.is_some() {
        out.extend_from_slice(b"\x1b[0m");
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    /// Reference writer: same algorithm via core::fmt. Oracle for emit_diff bytes.
    fn reference(front: &mut [Option<Cell>], back: &Grid) -> String {
        let cols = back.dims.cols as usize;
        let (mut s, mut cursor, mut pen) = (String::new(), None, None::<(Rgb, Rgb)>);
        for (i, (f, b)) in front.iter_mut().zip(&back.cells).enumerate() {
            if *f == Some(*b) {
                continue;
            }
            if cursor != Some(i) {
                write!(s, "\x1b[{};{}H", i / cols + 1, i % cols + 1).unwrap();
            }
            let (Rgb(fr, fg_, fb), Rgb(br, bg_, bb)) = (b.fg, b.bg);
            match pen {
                Some(p) if p == (b.fg, b.bg) => {}
                Some((f0, _)) if f0 == b.fg => write!(s, "\x1b[48;2;{br};{bg_};{bb}m").unwrap(),
                Some((_, b0)) if b0 == b.bg => write!(s, "\x1b[38;2;{fr};{fg_};{fb}m").unwrap(),
                _ => write!(s, "\x1b[38;2;{fr};{fg_};{fb};48;2;{br};{bg_};{bb}m").unwrap(),
            }
            pen = Some((b.fg, b.bg));
            s.push(b.ch.0 as char);
            *f = Some(*b);
            cursor = if (i + 1) % cols == 0 { None } else { Some(i + 1) };
        }
        if pen.is_some() {
            s.push_str("\x1b[0m");
        }
        s
    }

    fn random_grid(rng: &mut Rng, dims: Dims, palette: u64) -> Grid {
        let mut g = Grid::new(dims, Cell { ch: Glyph::AT, fg: Rgb(0, 0, 0), bg: Rgb(0, 0, 0) });
        for c in &mut g.cells {
            let r = rng.next();
            let col = |s: u32| ((r >> s) % palette) as u8 * 60;
            *c = Cell { ch: Glyph::pick(r), fg: Rgb(col(8), col(16), 7), bg: Rgb(col(24), 255, col(32)) };
        }
        g
    }

    #[test]
    fn sgr_writer_byte_identical_to_reference() {
        let dims = Dims::new(37, 11).unwrap();
        let mut rng = Rng::new(7);
        let (mut fa, mut fb) = (vec![None; dims.cells()], vec![None; dims.cells()]);
        for palette in [1, 2, 3, 5] {
            for _ in 0..20 {
                let g = random_grid(&mut rng, dims, palette);
                let mut out = Vec::new();
                emit_diff(&mut fa, &g, &mut out);
                assert_eq!(String::from_utf8(out).unwrap(), reference(&mut fb, &g));
            }
        }
    }

    #[test]
    fn sgr_oracle_catches_mutant() {
        // negative control: a grid that differs in one byte must give different output
        let dims = Dims::new(4, 2).unwrap();
        let g = random_grid(&mut Rng::new(1), dims, 5);
        let mut h = random_grid(&mut Rng::new(1), dims, 5);
        h.cells[3].fg.0 ^= 1;
        let (mut out1, mut out2) = (Vec::new(), Vec::new());
        emit_diff(&mut [None; 8], &g, &mut out1);
        emit_diff(&mut [None; 8], &h, &mut out2);
        assert_ne!(out1, out2);
    }

    #[test]
    fn unchanged_grid_emits_nothing() {
        let dims = Dims::new(10, 3).unwrap();
        let g = random_grid(&mut Rng::new(3), dims, 4);
        let mut front = vec![None; dims.cells()];
        let mut out = Vec::new();
        assert_eq!(emit_diff(&mut front, &g, &mut out), 30);
        out.clear();
        assert_eq!(emit_diff(&mut front, &g, &mut out), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn dec_matches_fmt() {
        for v in [0u64, 9, 10, 99, 100, 255, 1000, 65535, u64::MAX] {
            let mut o = Vec::new();
            dec(&mut o, v);
            assert_eq!(o, v.to_string().into_bytes());
        }
    }

    #[test]
    fn dims_reject_zero_and_huge() {
        assert!(Dims::new(0, 5).is_err());
        assert!(Dims::new(5, 1001).is_err());
        assert!(Dims::new(200, 60).is_ok());
    }
}
