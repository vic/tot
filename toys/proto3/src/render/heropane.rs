//! The hero pane: bottom right, the stats pane's width. Tabs ARMOR (the hero itself, its worn gear coloured by
//! rarity) and SKILLS (the hero's skill tree as a grid of nodes with their links). `i` and `t` focus a tab;
//! out of those modes the pane keeps showing the last tab.
use super::*;
use crate::data::{Slot, Tree};

/// Rows the pane takes, its tab order, and the skill grid's cell size (cols, rows).
pub(super) const PANE_H: u16 = 15;
const NODE_W: i32 = 11;
const NODE_H: i32 = 3;
/// Text room inside the borders and margins.
const INNER: usize = PANEL_W as usize - 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum Tab {
    #[default]
    Armor,
    Skills,
}

impl Tab {
    const ALL: [Tab; 2] = [Tab::Armor, Tab::Skills];
    fn name(self) -> &'static str {
        match self {
            Tab::Armor => "ARMOR",
            Tab::Skills => "SKILLS",
        }
    }
}

/// The hero, 7 rows: each part names the slot whose item colours it (None = the body itself).
const HERO: [(&str, [Option<Slot>; 3]); 7] = [
    ("    ▄███▄    ", [Some(Slot::Armor), None, None]),
    ("    (◉ ◉)    ", [None, None, None]),
    ("  ╔═▓▓▓▓▓═╗  ", [Some(Slot::Armor), None, None]),
    ("  ║ ▓▓▓▓▓ ║─┼", [Some(Slot::Armor), Some(Slot::Weapon), None]),
    ("  ╚═╤═◆═╤═╝  ", [Some(Slot::Charm), Some(Slot::Jewel), None]),
    ("    ║   ║  ○ ", [None, Some(Slot::Ring), None]),
    ("   ▀▀▀ ▀▀▀   ", [None, None, None]),
];

impl Frame {
    /// Where the pane goes (left col, top row), if the screen has room under the stats pane. Focused, it shows
    /// even on a small screen (over the arena).
    pub(super) fn pane_origin(&self, focused: bool) -> Option<(u16, u16)> {
        let left = self.cols.checked_sub(PANEL_W)?;
        let bottom = self.play_rows();
        // keep clear of the bottom bar when it reaches under the pane
        let bar_under = self.bar_origin().is_some_and(|(bx, _)| bx + BAR_W > left);
        let top = bottom.checked_sub(PANE_H + if bar_under { BAR_H } else { 0 })?;
        if top >= PANEL_H {
            Some((left, top))
        } else if focused {
            Some((left, PANEL_H.min(bottom.saturating_sub(PANE_H))))
        } else {
            None
        }
    }

    pub(super) fn hero_pane(&mut self, sim: &Sim, ui: &Ui) {
        let focus = match ui.mode {
            Mode::Gear => Some(Tab::Armor),
            Mode::Tree => Some(Tab::Skills),
            _ => None,
        };
        if let Some(t) = focus {
            self.pane_tab = t;
        }
        self.pane_at = self.pane_origin(focus.is_some());
        let Some((left, top)) = self.pane_at else { return };
        let pal = &sim.c.pal;
        let (l, t, w) = (i32::from(left), i32::from(top), i32::from(PANEL_W));
        self.plate(l, t, w, i32::from(PANE_H), pal.hud_bg);
        // tabs on the top edge, the shown one lit
        let edge = if focus.is_some() { pal.alarm } else { pal.of(Hue::Kin) };
        self.text(top, left, &format!("╭{}╮", "─".repeat(usize::from(PANEL_W) - 2)), edge);
        self.text(top + PANE_H - 1, left, &format!("╰{}╯", "─".repeat(usize::from(PANEL_W) - 2)), edge);
        for r in top + 1..top + PANE_H - 1 {
            self.text(r, left, "│", edge);
            self.text(r, left + PANEL_W - 1, "│", edge);
        }
        let mut x = left + 2;
        for tab in Tab::ALL {
            let on = tab == self.pane_tab;
            let key = match tab {
                Tab::Armor => "i",
                Tab::Skills => "t",
            };
            x = self.text(top, x, key, key_fg(pal));
            x = self.text(top, x, &format!(" {} ", tab.name()), if on { pal.alarm } else { pal.dim });
            x += 1;
        }
        match self.pane_tab {
            Tab::Armor => self.pane_armor(sim, ui, (l + 2, t + 2)),
            Tab::Skills => self.pane_skills(sim, ui, (l + 2, t + 1)),
        }
    }

    /// The hero drawn in its gear: each part takes its slot item's rarity colour, bare parts are dim; the
    /// worn items listed beside it.
    fn pane_armor(&mut self, sim: &Sim, ui: &Ui, (l, t): (i32, i32)) {
        let c = sim.c;
        let pal = &c.pal;
        let ink = |slot: Option<Slot>| match slot.and_then(|s| sim.gear.worn(s)).and_then(|id| c.items.get(usize::from(id.0))) {
            Some(it) => pal.role(it.rarity.role()),
            None if slot.is_some() => pal.dim,
            None => pal.text,
        };
        for (r, (line, parts)) in HERO.iter().enumerate() {
            // the first slot colours the line, the later ones colour their own marks (weapon, jewel, ring)
            self.text((t + r as i32) as u16, l as u16, line, ink(parts[0]));
            for (mark, slot) in [('┼', Slot::Weapon), ('─', Slot::Weapon), ('◆', Slot::Jewel), ('○', Slot::Ring)] {
                if parts.contains(&Some(slot))
                    && let Some(k) = line.chars().position(|ch| ch == mark)
                {
                    self.text((t + r as i32) as u16, (l + k as i32) as u16, &mark.to_string(), ink(Some(slot)));
                }
            }
        }
        let x = (l + 15) as u16;
        for (k, slot) in Slot::ALL.iter().enumerate() {
            let row = (t + k as i32 * 2) as u16;
            self.text(row, x, &format!("{slot:?}").to_uppercase(), pal.dim);
            let (name, fg) = match sim.gear.worn(*slot).and_then(|id| c.items.get(usize::from(id.0))) {
                Some(it) => (it.name.chars().take(17).collect::<String>(), pal.role(it.rarity.role())),
                None => ("-".to_string(), pal.dim),
            };
            self.text(row + 1, x + 1, &name, fg);
        }
        // the selected slot's item, described on the last row
        if let Some(it) = Slot::ALL.get(ui.gear_sel).and_then(|s| sim.gear.worn(*s)).and_then(|id| c.items.get(usize::from(id.0))) {
            let d: String = it.desc.chars().take(usize::from(PANEL_W) - 4).collect();
            self.text((t + 11) as u16, l as u16, &d, pal.dim);
        }
    }

    /// The skill tree as a grid: a node per cell (learned in the xp colour, learnable in text colour, locked dim),
    /// lines to the nodes it requires, the cursor's node in a box, its details on the last row.
    fn pane_skills(&mut self, sim: &Sim, ui: &Ui, (l, t): (i32, i32)) {
        let c = sim.c;
        let pal = &c.pal;
        let Some(tree): Option<&Tree> = crate::input::hero_tree(c) else {
            self.text((t + 1) as u16, l as u16, "no skill tree", pal.dim);
            return;
        };
        // title: the tree, its class and power type, nodes learned
        let class = c.classes.get(tree.class.ix()).map_or("", |k| k.name.as_str());
        let title: String = format!("{} · {class} · {:?} · {} learned", tree.name, tree.tag, ui.tree_owned.count()).chars().take(INNER).collect();
        self.text(t as u16, l as u16, &title, pal.text);
        let at = |col: u8, row: u8| (l + i32::from(col) * NODE_W, t + 1 + i32::from(row) * NODE_H);
        // links first, under the nodes: down a column, or a diagonal step
        for n in &tree.nodes {
            for &q in &n.requires {
                let p = &tree.nodes[usize::from(q)];
                let ((px, py), (nx, ny)) = (at(p.col, p.row), at(n.col, n.row));
                let (mx, my) = ((px + nx) / 2 + NODE_W / 2 - 1, (py + ny) / 2 + 1);
                let ch = match nx.cmp(&px) {
                    std::cmp::Ordering::Less => "╱",
                    std::cmp::Ordering::Equal => "│",
                    std::cmp::Ordering::Greater => "╲",
                };
                self.text(my as u16, mx as u16, ch, pal.dim);
            }
        }
        for (k, n) in tree.nodes.iter().enumerate() {
            let (x, y) = at(n.col, n.row);
            let learned = ui.tree_owned.has(k);
            let open = n.requires.iter().all(|&q| ui.tree_owned.has(usize::from(q)));
            let fg = if learned { pal.xp } else if open { pal.text } else { pal.dim };
            let name: String = n.name.chars().take(usize::from(NODE_W as u16) - 2).collect();
            self.text(y as u16, x as u16, if learned { "◆" } else { "◇" }, fg);
            self.text(y as u16, x as u16 + 1, &name, fg);
            if k == ui.tree_sel {
                let i = y as usize * usize::from(self.cols) + x as usize;
                for cell in &mut self.cells[i..i + 1 + name.chars().count()] {
                    cell.bg = mix(pal.alarm, pal.hud_bg, 600);
                }
            }
        }
        let n = &tree.nodes[ui.tree_sel.min(tree.nodes.len() - 1)];
        let y = (t + 12) as u16;
        // points, cost and what it grants; clipped to the pane
        let detail = format!("pts {} cost {} {} {:?}", ui.tree_points(), n.cost, c.skill(n.effect).name, n.tag).to_uppercase();
        let detail: String = detail.chars().take(INNER).collect();
        self.text(y, l as u16, &detail, pal.of(c.skill(n.effect).hue));
    }
}
