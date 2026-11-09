//! Block lettering, 3 rows tall (half-block glyphs), for shouts that must be noticed: weather changes, barks.
//! Same family as the boss ROAR, smaller. Unknown characters become a space.

/// Rows of one character; letters are 3 wide except M, N, W.
fn glyph(c: char) -> [&'static str; 3] {
    match c.to_ascii_uppercase() {
        'A' => ["▄▀▄", "█▀█", "▀ ▀"],
        'B' => ["█▀▄", "█▀▄", "▀▀ "],
        'C' => ["█▀▀", "█  ", "▀▀▀"],
        'D' => ["█▀▄", "█ █", "▀▀ "],
        'E' => ["█▀▀", "█▀ ", "▀▀▀"],
        'F' => ["█▀▀", "█▀ ", "▀  "],
        'G' => ["█▀▀", "█ ▄", "▀▀▀"],
        'H' => ["█ █", "█▀█", "▀ ▀"],
        'I' => ["▀█▀", " █ ", "▀▀▀"],
        'J' => ["  █", "  █", "▀▀ "],
        'K' => ["█ █", "█▀▄", "▀ ▀"],
        'L' => ["█  ", "█  ", "▀▀▀"],
        'M' => ["█▄ ▄█", "█ ▀ █", "▀   ▀"],
        'N' => ["█▄  █", "█ ▀▄█", "▀   ▀"],
        'O' => ["█▀█", "█ █", "▀▀▀"],
        'P' => ["█▀█", "█▀▀", "▀  "],
        'Q' => ["█▀█", "█ █", "▀▀█"],
        'R' => ["█▀█", "█▀▄", "▀ ▀"],
        'S' => ["█▀▀", "▀▀█", "▀▀▀"],
        'T' => ["▀█▀", " █ ", " ▀ "],
        'U' => ["█ █", "█ █", "▀▀▀"],
        'V' => ["█ █", "█ █", " ▀ "],
        'W' => ["█ █ █", "█ █ █", " ▀ ▀ "],
        'X' => ["█ █", "▄▀▄", "▀ ▀"],
        'Y' => ["█ █", "▀█▀", " ▀ "],
        'Z' => ["▀▀█", "▄▀ ", "▀▀▀"],
        '!' => ["█", "█", "▄"],
        '?' => ["▀▀█", " █▀", " ▄ "],
        '.' => [" ", " ", "▄"],
        _ => ["  ", "  ", "  "],
    }
}

/// The block font draws `c`: letters (either case), `!`, `?`, `.` and the space.
pub fn draws(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '!' | '?' | '.' | ' ')
}

/// The 3 rows of `text` in block letters, one blank column between characters.
pub fn rows(text: &str) -> [String; 3] {
    let mut out = [String::new(), String::new(), String::new()];
    for (k, c) in text.chars().enumerate() {
        for (r, part) in glyph(c).iter().enumerate() {
            if k > 0 {
                out[r].push(' ');
            }
            out[r].push_str(part);
        }
    }
    out
}

/// Twice as big: every cell doubled across and every row doubled down (the uber boss's voice).
pub fn double(rows: &[String]) -> Vec<String> {
    rows.iter().flat_map(|r| {
        let wide: String = r.chars().flat_map(|c| [c, c]).collect();
        [wide.clone(), wide]
    }).collect()
}

/// Width in cells of `text` in block letters.
pub fn width(text: &str) -> usize {
    rows(text)[0].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_aligned_and_letters_distinct() {
        let r = rows("ACID RAIN!");
        assert!(r.iter().all(|l| l.chars().count() == width("ACID RAIN!")), "{r:?}");
        let all: Vec<_> = ('A'..='Z').map(glyph).collect();
        for (i, a) in all.iter().enumerate() {
            assert!(a.iter().all(|l| l.chars().count() == a[0].chars().count()), "letter {i} ragged");
            assert!(all[i + 1..].iter().all(|b| b != a), "letter {} looks like a later one", (b'A' + i as u8) as char);
        }
        assert_eq!(rows("a"), rows("A"), "lower case is upper case");
    }
}
