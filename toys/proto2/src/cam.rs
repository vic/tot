//! Render camera, never read by the sim: smooth follow (exponential, deadzone) and an eased q/e
//! turn. Pure fn of the (frame time, heading, target) sequence it is fed, so a fake clock makes it
//! deterministic in tests.
use crate::data::ViewDef;
use crate::iso::{Heading, Proj};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Turn {
    from: f64,
    to: f64,
    t0: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cam {
    /// camera centre, world mm
    pub x: f64,
    pub y: f64,
    /// last update (ms, heading); None = snap on the next update
    last: Option<(u64, Heading)>,
    turn: Option<Turn>,
    /// unwrapped target angle, quarter turns
    to: f64,
}

/// Smoothstep: eased 0..1, zero speed at both ends.
fn ease(s: f64) -> f64 {
    let s = s.clamp(0.0, 1.0);
    s * s * (3.0 - 2.0 * s)
}

impl Cam {
    /// Current angle in quarter turns (whole once a turn ends).
    pub fn angle(&self, v: &ViewDef, now: u64) -> f64 {
        match self.turn {
            Some(t) if now < t.t0 + u64::from(v.turn_ms) => t.from + (t.to - t.from) * ease((now - t.t0) as f64 / f64::from(v.turn_ms)),
            _ => self.to,
        }
    }

    pub fn proj(&self, v: &ViewDef, now: u64) -> Proj {
        Proj::at(self.angle(v, now))
    }

    pub fn reset(&mut self) {
        *self = Cam::default();
    }

    /// Advance to frame time `now`: follow (tx, ty), turn toward `h`. A q/e press during a turn
    /// retargets: the new turn starts from the current angle.
    pub fn update(&mut self, v: &ViewDef, now: u64, h: Heading, tx: f64, ty: f64) {
        let Some((t, h0)) = self.last else {
            *self = Cam { x: tx, y: ty, last: Some((now, h)), turn: None, to: f64::from(h.quarter()) };
            return;
        };
        let d = match (h.quarter() + 4 - h0.quarter()) % 4 {
            0 => 0.0,
            1 => 1.0,
            2 => 2.0,
            _ => -1.0,
        };
        if d != 0.0 {
            let from = self.angle(v, now);
            self.to += d;
            self.turn = Some(Turn { from, to: self.to, t0: now });
        }
        let dt = now.saturating_sub(t) as f64;
        let k = if v.follow_ms == 0 { 1.0 } else { 1.0 - (-dt / f64::from(v.follow_ms)).exp() };
        let (ex, ey) = (tx - self.x, ty - self.y);
        let len = ex.hypot(ey);
        let dz = f64::from(v.deadzone);
        if len > dz {
            let m = k * (len - dz) / len;
            self.x += ex * m;
            self.y += ey * m;
        }
        self.last = Some((now, h));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;

    #[test]
    fn turn_is_eased_monotone_and_snaps_to_the_grid() {
        let c = content();
        let v = &c.view;
        let mut cam = Cam::default();
        let h = Heading::default();
        cam.update(v, 0, h, 0.0, 0.0);
        cam.update(v, 100, h.right(), 0.0, 0.0);
        let a: Vec<f64> = (100..=100 + u64::from(v.turn_ms) + 50).step_by(16).map(|t| cam.angle(v, t)).collect();
        assert!(a.windows(2).all(|w| w[1] >= w[0]), "monotone {a:?}");
        assert!(a.iter().any(|x| *x > 0.2 && *x < 0.8), "armed: intermediate angles shown");
        assert_eq!(cam.angle(v, 100 + u64::from(v.turn_ms)), 1.0, "ends on the 90-degree grid");
        // retarget mid-turn: no jump, ends one quarter further
        cam.update(v, 150, h.right().right(), 0.0, 0.0);
        let mid = cam.angle(v, 150);
        assert!((mid - a[3]).abs() < 0.2, "retarget keeps the current angle: {mid}");
        assert_eq!(cam.angle(v, 150 + u64::from(v.turn_ms)), 2.0);
        // q right after e: turns back the short way
        cam.update(v, 1000, h.right(), 0.0, 0.0);
        assert_eq!(cam.angle(v, 2000), 1.0);
    }
}
