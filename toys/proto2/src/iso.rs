//! Iso projection (design sec 5.1): col = 3(x-y), row = 1.5(x+y), x/y in metres, after a camera
//! rotation. Sim is world-space and integer; only render and input use this. The render angle is
//! continuous (animated q/e turn), so the projection is f64; input keeps the discrete Heading.

/// Camera heading, 4 steps of 90 degrees.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Heading(u8);

impl Heading {
    pub fn left(self) -> Heading {
        Heading((self.0 + 3) % 4)
    }
    pub fn right(self) -> Heading {
        Heading((self.0 + 1) % 4)
    }
    pub fn name(self) -> &'static str {
        ["N", "E", "S", "W"][usize::from(self.0)]
    }
    /// Quarter turns from N, 0..4.
    pub fn quarter(self) -> u8 {
        self.0
    }
    /// view -> world
    fn unrot(self, x: i32, y: i32) -> (i32, i32) {
        match self.0 {
            0 => (x, y),
            1 => (y, -x),
            2 => (-x, -y),
            _ => (-y, x),
        }
    }
}

/// Screen-relative direction (sx right, sy down; each -1..=1) -> world direction signs.
pub fn screen_dir(h: Heading, sx: i32, sy: i32) -> (i32, i32) {
    let (x, y) = h.unrot(sx + sy, sy - sx);
    (x.signum(), y.signum())
}

/// Projection at a rotation of `q` quarter turns (any real q; whole q are exact).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Proj {
    c: f64,
    s: f64,
}

impl Proj {
    pub fn at(q: f64) -> Proj {
        let (s, c) = match q.rem_euclid(4.0) {
            0.0 => (0.0, 1.0),
            1.0 => (1.0, 0.0),
            2.0 => (0.0, -1.0),
            3.0 => (-1.0, 0.0),
            r => (r * std::f64::consts::FRAC_PI_2).sin_cos(),
        };
        Proj { c, s }
    }

    /// world -> view
    fn rot(self, x: f64, y: f64) -> (f64, f64) {
        (x * self.c - y * self.s, x * self.s + y * self.c)
    }

    /// World point (mm) -> continuous screen position (cols, rows).
    pub fn screen(self, x: f64, y: f64) -> (f64, f64) {
        let (x, y) = self.rot(x, y);
        (3.0 * (x - y) / 1000.0, 3.0 * (x + y) / 2000.0)
    }

    /// World point (mm) -> screen cell (round half up).
    pub fn project(self, x: f64, y: f64) -> (i32, i32) {
        let (c, r) = self.screen(x, y);
        ((c + 0.5).floor() as i32, (r + 0.5).floor() as i32)
    }

    /// Screen cell -> world point (mm) of the cell centre.
    pub fn unproject(self, col: i32, row: i32) -> (f64, f64) {
        let (u, v) = (f64::from(col) * 1000.0 / 3.0, f64::from(row) * 2000.0 / 3.0); // x - y, x + y
        let (x, y) = ((u + v) / 2.0, (v - u) / 2.0);
        (x * self.c + y * self.s, -x * self.s + y * self.c)
    }

    /// View depth (mm): larger = nearer the camera = drawn later (painter order).
    pub fn depth(self, x: f64, y: f64) -> f64 {
        let (x, y) = self.rot(x, y);
        x + y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn angles() -> impl Iterator<Item = f64> {
        (0..32).map(|k| f64::from(k) / 8.0)
    }

    #[test]
    fn unproject_then_project_is_identity_on_cells_at_every_angle() {
        // every screen cell maps to one world point and back: no holes, no double cells, mid-turn too
        for q in angles() {
            let p = Proj::at(q);
            for dc in -70..70 {
                for dr in -25..25 {
                    let (x, y) = p.unproject(dc, dr);
                    assert_eq!(p.project(x, y), (dc, dr), "q={q} {dc},{dr}");
                }
            }
        }
    }

    #[test]
    fn world_line_projects_monotone_during_a_turn() {
        // a straight world line stays a monotone screen run at every intermediate angle (no seams)
        for q in angles() {
            let p = Proj::at(q);
            let cols: Vec<(i32, i32)> = (0..200).map(|k| p.project(f64::from(k) * 50.0, 0.0)).collect();
            let sx = (cols[199].0 - cols[0].0).signum();
            let sy = (cols[199].1 - cols[0].1).signum();
            for w in cols.windows(2) {
                let (dc, dr) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
                assert!(dc * sx >= 0 && dr * sy >= 0 && dc.abs() <= 1 && dr.abs() <= 1, "q={q} step {dc},{dr}");
            }
        }
    }

    #[test]
    fn screen_right_moves_right_on_screen_for_every_heading() {
        let mut h = Heading::default();
        for _ in 0..4 {
            let p = Proj::at(f64::from(h.quarter()));
            let (x, y) = screen_dir(h, 1, 0);
            let (c, r) = p.project(f64::from(x) * 1000.0, f64::from(y) * 1000.0);
            assert!(c > 0 && r == 0, "h={h:?} -> {c},{r}");
            let (x, y) = screen_dir(h, 0, -1);
            let (c, r) = p.project(f64::from(x) * 1000.0, f64::from(y) * 1000.0);
            assert!(r < 0 && c == 0, "h={h:?} up -> {c},{r}");
            h = h.right();
        }
    }

    #[test]
    fn world_circle_is_round_on_screen() {
        // 1 m right on screen = 4.24 cols; 1 m up = 2.12 rows = 4.24 col widths (2:1 cells)
        let p = Proj::at(0.0);
        let (c, _) = p.project(707.0, -707.0);
        let (_, r) = p.project(-707.0, -707.0);
        assert_eq!((c, r), (4, -2));
    }
}
