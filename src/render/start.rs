//! The start screen: the title art, then the adventures (step 1, ending with the agentic entry) or the picked
//! adventure's hero classes (step 2), each entry a name line and a dim description line, the cursor's entry lit;
//! the agentic steps: a text field to describe the adventure, the live Claude Code sessions, the waiting screen.
//! The key hints sit on the last row.
use super::*;
use crate::data::ProfileInfo;
use crate::input::Start;
use crate::peer::Session;
use crate::text::{a, t};

/// Row of the title art's first row; how long one frame of the agent loading animation shows (ms).
const LOADING_FRAME_MS: u64 = 150;
const TITLE_TOP: u16 = 2;

/// What the start screen lists besides the content: the installed adventures, the live local Claude sessions,
/// and the agent request's last news (an error, a refusal) if any.
pub struct StartView<'a> {
    pub advs: &'a [ProfileInfo],
    pub agents: &'a [Session],
    pub news: Option<&'a str>,
    /// wall clock (ms) and, while an agent writes, when the request went out (drives the wait animation)
    pub now_ms: u64,
    pub asked_ms: Option<u64>,
}

impl Frame {
    /// The whole screen for `ui.start`. `c` is the base content on step 1 and the agentic steps, the picked
    /// adventure on the class step, so that step shows in that adventure's palette, texts and classes.
    pub fn start(&mut self, c: &Content, ui: &Ui, v: &StartView) {
        let pal = &c.pal;
        let tx = &c.texts;
        self.cells.fill(Cell { ch: Glyph::SPACE, fg: pal.text, bg: pal.bg });
        let rows = self.rows;
        let centre = |cols: u16, w: usize| cols.saturating_sub(w as u16) / 2;
        let art = tx.art(a::TITLE);
        let aw = art.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let mut row = TITLE_TOP;
        for line in art.iter().take(usize::from(rows.saturating_sub(row))) {
            self.text(row, centre(self.cols, aw), line, pal.role(Role::Legendary));
            row += 1;
        }
        let agent = |s: &Session| (s.name.clone(), format!("{}{}", s.cwd, if s.busy { tx.get(t::AGENT_BUSY) } else { "" }));
        let (head, items, sel): (String, Vec<(String, String)>, Option<usize>) = match ui.start {
            Start::Adventure { sel } => {
                let mut items: Vec<_> = v.advs.iter().map(|a| (a.name.clone(), a.blurb.clone())).collect();
                items.push((tx.get(t::AGENTIC_NAME).into(), tx.get(t::AGENTIC_BLURB).into()));
                (tx.get(t::PICK_ADVENTURE).into(), items, Some(sel))
            }
            Start::Class { sel, .. } => (tx.get(t::PICK_CLASS).into(), c.classes.iter().map(|k| (k.name.clone(), k.desc.clone())).collect(), Some(sel)),
            Start::Describe { .. } => (tx.get(t::DESCRIBE_HEADER).into(), Vec::new(), None),
            Start::Agent { .. } if v.agents.is_empty() => (tx.get(t::PICK_AGENT).into(), vec![(tx.get(t::NO_AGENTS).into(), String::new())], None),
            Start::Agent { sel, .. } => (tx.get(t::PICK_AGENT).into(), v.agents.iter().map(agent).collect(), Some(sel)),
            Start::Asking { agent: k, .. } => {
                let who = v.agents.get(k).map_or_else(String::new, |s| s.name.clone());
                (tx.fill(t::ASKING, [&who]), vec![(tx.get(t::AGENT_WAIT).into(), String::new())], None)
            }
        };
        row += 1;
        if row < rows {
            self.text(row, centre(self.cols, head.chars().count()), &head, pal.text);
        }
        row += 2;
        if let Start::Describe { .. } = ui.start {
            // the text field: what the player typed, else the dim example, in a box as wide as the screen allows
            let w = usize::from(self.cols.saturating_sub(8));
            let (shown, fg) = if ui.brief.is_empty() { (tx.get(t::DESCRIBE_PLACEHOLDER).to_string(), pal.dim) } else { (format!("{}_", ui.brief), pal.text) };
            let left = centre(self.cols, w);
            self.plate(i32::from(left), i32::from(row), w as i32, 3, pal.role(Role::Select));
            let fit: String = shown.chars().rev().take(w.saturating_sub(4)).collect::<Vec<_>>().into_iter().rev().collect();
            self.text(row + 1, left + 2, &fit, fg);
            row += 4;
        }
        let w = items.iter().map(|(n, d)| n.chars().count().max(d.chars().count())).max().unwrap_or(0) + 4;
        let left = centre(self.cols, w);
        // an entry takes its two lines and a gap, inside the play rows (the strip rows under them hold the hints)
        for (k, (name, desc)) in items.iter().enumerate() {
            if row + 2 > self.play_rows() {
                break;
            }
            let lit = sel == Some(k);
            if lit {
                self.plate(i32::from(left), i32::from(row), w as i32, 2, pal.role(Role::Select));
            }
            self.text(row, left + 2, name, if lit { pal.role(Role::Accent) } else { pal.text });
            self.text(row + 1, left + 2, desc, pal.dim);
            row += 3;
        }
        // while the agent writes: the adventure's loading animation (one frame per LOADING_FRAME_MS) and the time
        // it has been writing, so the wait never looks frozen
        if let (Start::Asking { .. }, Some(at)) = (ui.start, v.asked_ms) {
            let frames = tx.art(a::AGENT_LOADING);
            let ms = v.now_ms.saturating_sub(at);
            if let Some(frame) = frames.get((ms / LOADING_FRAME_MS) as usize % frames.len().max(1)).filter(|_| row + 2 < self.play_rows()) {
                self.text(row, centre(self.cols, frame.chars().count()), frame, pal.role(Role::Accent));
                let secs = ms / 1000;
                let took = tx.fill(t::AGENT_ELAPSED, [&format!("{}:{:02}", secs / 60, secs % 60)]);
                self.text(row + 1, centre(self.cols, took.chars().count()), &took, pal.dim);
                row += 3;
            }
        }
        if let Some(news) = v.news.filter(|_| row < self.play_rows()) {
            let fit: String = news.chars().take(usize::from(self.cols.saturating_sub(4))).collect();
            self.text(row, centre(self.cols, fit.chars().count()), &fit, pal.alarm);
        }
        if let Some(last) = rows.checked_sub(1) {
            // the sponsor button owns the right end of the last row; the key hints stop before it
            self.links.clear();
            let sx = self.sponsor(c, last);
            let mut x = 1;
            for h in hints(c, ui) {
                let w = (h.keys.chars().count() + h.label.chars().count() + 3) as u16;
                if x + w >= sx {
                    break;
                }
                x = self.text(last, x, &h.keys, key_fg(pal));
                x = self.text(last, x + 1, &h.label, pal.dim) + 2;
            }
        }
    }
}
