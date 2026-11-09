//! The start screen: the title art, then the adventures (step 1) or the picked adventure's hero classes (step 2),
//! each entry a name line and a dim description line, the cursor's entry lit; the key hints on the last row.
use super::*;
use crate::data::{Art, Msg, ProfileInfo};
use crate::input::Start;

/// Row of the title art's first row.
const TITLE_TOP: u16 = 2;

impl Frame {
    /// The whole screen for `ui.start`. `c` is the base content on step 1 and the picked adventure on step 2, so
    /// step 2 shows in that adventure's palette, texts and classes.
    pub fn start(&mut self, c: &Content, ui: &Ui, advs: &[ProfileInfo]) {
        let pal = &c.pal;
        self.cells.fill(Cell { ch: Glyph::SPACE, fg: pal.text, bg: pal.bg });
        let rows = self.rows;
        let centre = |cols: u16, w: usize| cols.saturating_sub(w as u16) / 2;
        let art = c.texts.art(Art::Title);
        let aw = art.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let mut row = TITLE_TOP;
        for line in art.iter().take(usize::from(rows.saturating_sub(row))) {
            self.text(row, centre(self.cols, aw), line, pal.role(Role::Legendary));
            row += 1;
        }
        let (head, items, sel): (&str, Vec<(&str, &str)>, usize) = match ui.start {
            Start::Adventure { sel } => (c.texts.msg(Msg::PickAdventure), advs.iter().map(|a| (a.name.as_str(), a.blurb.as_str())).collect(), sel),
            Start::Class { sel, .. } => (c.texts.msg(Msg::PickClass), c.classes.iter().map(|k| (k.name.as_str(), k.desc.as_str())).collect(), sel),
        };
        row += 1;
        if row < rows {
            self.text(row, centre(self.cols, head.chars().count()), head, pal.text);
        }
        row += 2;
        let w = items.iter().map(|(n, d)| n.chars().count().max(d.chars().count())).max().unwrap_or(0) + 4;
        let left = centre(self.cols, w);
        // an entry takes its two lines and a gap, inside the play rows (the strip rows under them hold the hints)
        for (k, (name, desc)) in items.iter().enumerate() {
            if row + 2 > self.play_rows() {
                break;
            }
            let lit = k == sel;
            if lit {
                self.plate(i32::from(left), i32::from(row), w as i32, 2, pal.role(Role::Select));
            }
            self.text(row, left + 2, name, if lit { pal.role(Role::Accent) } else { pal.text });
            self.text(row + 1, left + 2, desc, pal.dim);
            row += 3;
        }
        if let Some(last) = rows.checked_sub(1) {
            let mut x = 1;
            for h in hints(c, ui) {
                x = self.text(last, x, &h.keys, key_fg(pal));
                x = self.text(last, x + 1, &h.label, pal.dim) + 2;
            }
        }
    }
}
