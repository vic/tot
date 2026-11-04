//! Integer trig (no float in sim or generation): angle in 1/1024 turn, result in permille.
//! Bhaskara I: sin(pi t / T) ~= 16 t (T - t) / (5 T^2 - 4 t (T - t)); max error ~0.0016.
pub const TURN: u32 = 1024;
const HALF: i64 = 512;

pub fn sin(a: u32) -> i32 {
    let a = i64::from(a % TURN);
    let (t, sign) = if a < HALF { (a, 1) } else { (a - HALF, -1) };
    let p = t * (HALF - t);
    let den = 5 * HALF * HALF - 4 * p;
    (sign * ((16 * p * 1000 + den / 2) / den)) as i32
}

pub fn cos(a: u32) -> i32 {
    sin(a + TURN / 4)
}

/// Degrees (data literal) -> table angle, rounded; basic IEEE ops only (exactly rounded).
pub fn deg(d: f64) -> u32 {
    (d * f64::from(TURN) / 360.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_libm_within_2_5_permille() {
        for a in 0..TURN {
            let t = f64::from(a) * std::f64::consts::TAU / f64::from(TURN);
            assert!((f64::from(sin(a)) - t.sin() * 1000.0).abs() <= 2.5, "sin {a}");
            assert!((f64::from(cos(a)) - t.cos() * 1000.0).abs() <= 2.5, "cos {a}");
        }
        assert_eq!((sin(0), sin(256), cos(0), cos(512)), (0, 1000, 1000, -1000));
    }
}
