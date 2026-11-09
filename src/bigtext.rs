//! Block lettering, 3 rows tall, for shouts that must be noticed: weather changes, barks, roars. The letters
//! are adventure data (`letter` records in texts.jsonl), so each adventure has its own lettering.

/// Every character the block font has, in `Font` order (lower case draws as upper case).
pub const CHARS: [char; 30] = [
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '!', '?',
    '.', ' ',
];

/// Most columns of one letter.
pub const LETTER_MAX_W: usize = 6;

/// The block font draws `c`.
pub fn draws(c: char) -> bool {
    CHARS.contains(&c.to_ascii_uppercase())
}

/// One letter per `CHARS` entry: 3 rows of equal width. Built only by `Font::new`, which checks that.
pub struct Font([[String; 3]; CHARS.len()]);

impl Font {
    /// None unless every letter's 3 rows share one width in 1..=LETTER_MAX_W.
    pub fn new(letters: [[String; 3]; CHARS.len()]) -> Option<Font> {
        let even = |l: &[String; 3]| {
            let w = l[0].chars().count();
            (1..=LETTER_MAX_W).contains(&w) && l.iter().all(|r| r.chars().count() == w)
        };
        letters.iter().all(even).then_some(Font(letters))
    }

    fn letter(&self, c: char) -> Option<&[String; 3]> {
        CHARS.iter().position(|&k| k == c.to_ascii_uppercase()).map(|k| &self.0[k])
    }

    /// The 3 rows of `text` in block letters, one blank column between characters (a char the font lacks is
    /// skipped; the loader refuses shown big text with one).
    pub fn rows(&self, text: &str) -> [String; 3] {
        let mut out = [String::new(), String::new(), String::new()];
        for (k, l) in text.chars().filter_map(|c| self.letter(c)).enumerate() {
            for (r, part) in l.iter().enumerate() {
                if k > 0 {
                    out[r].push(' ');
                }
                out[r].push_str(part);
            }
        }
        out
    }

    /// Width in cells of `text` in block letters.
    pub fn width(&self, text: &str) -> usize {
        self.rows(text)[0].chars().count()
    }

    /// True when no two letters look the same (a font where they do cannot be read).
    #[cfg(test)]
    pub fn distinct(&self) -> bool {
        self.0.iter().enumerate().all(|(i, a)| self.0[i + 1..].iter().all(|b| b != a))
    }
}

/// Twice as big: every cell doubled across and every row doubled down (the uber boss's voice).
pub fn double(rows: &[String]) -> Vec<String> {
    rows.iter().flat_map(|r| {
        let wide: String = r.chars().flat_map(|c| [c, c]).collect();
        [wide.clone(), wide]
    }).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn rows_are_aligned_and_lower_case_is_upper_case() {
        let c = crate::data::tests::content();
        let f = c.texts.font();
        let r = f.rows("ACID RAIN!");
        assert!(r.iter().all(|l| l.chars().count() == f.width("ACID RAIN!")), "{r:?}");
        assert_eq!(f.rows("a"), f.rows("A"), "lower case is upper case");
        assert!(f.distinct(), "the shipped letters are distinct");
    }
}
