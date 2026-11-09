//! Kitty keyboard events + modal UI (design sec 6): PLAY, PICK, LOOK, CMD; which-key on prefixes.
//! Held directions come only from press/release (no autorepeat path, design sec 6.1).
use crate::data::{Content, Owned, Slot, Tree};
use crate::iso::{Heading, screen_dir};
use crate::sim::{BAG_SLOTS, BagAct, Cmd, Input, PickChoice};
/// Bag grid columns (BAG_SLOTS / BAG_COLS rows).
pub const BAG_COLS: usize = 6;

/// Class the hero plays (class picking is a later screen).
const HERO_CLASS: usize = 0;

/// The tree the TREE screen shows: the hero class's first tree.
/// shortcut: one tree per class on screen, a tree switcher comes with class choice.
pub fn hero_tree(c: &Content) -> Option<&Tree> {
    c.trees.iter().find(|t| t.class.ix() == HERO_CLASS)
}

/// Cursor step on the node grid: sideways along the row, up/down to the nearest column of the next row.
fn tree_step(t: &Tree, from: usize, dc: i8, dr: i8) -> usize {
    let Some(n) = t.nodes.get(from) else { return from };
    let at = |c: i16, r: i16| Some((u8::try_from(c).ok()?, u8::try_from(r).ok()?)).and_then(|(c, r)| t.at(c, r));
    let (col, row) = (i16::from(n.col), i16::from(n.row));
    if dr == 0 {
        return (1..=i16::from(u8::MAX)).find_map(|k| at(col + i16::from(dc) * k, row)).unwrap_or(from);
    }
    let r = row + i16::from(dr);
    t.nodes.iter().enumerate().filter(|(_, m)| i16::from(m.row) == r).min_by_key(|(_, m)| (i16::from(m.col) - col).abs()).map_or(from, |(i, _)| i)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Shift,
    Up,
    Down,
    Left,
    Right,
}

impl Key {
    pub fn parse(s: &str) -> Option<Key> {
        Some(match s {
            "space" => Key::Char(' '),
            "enter" => Key::Enter,
            "esc" => Key::Esc,
            "tab" => Key::Tab,
            "backspace" => Key::Backspace,
            "shift" => Key::Shift,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            _ => {
                let mut it = s.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) if c.is_ascii_graphic() => Key::Char(c),
                    _ => return None,
                }
            }
        })
    }
    pub fn name(self) -> String {
        match self {
            Key::Char(' ') => "spc".into(),
            Key::Char(c) => c.to_string(),
            Key::Enter => "ret".into(),
            Key::Esc => "esc".into(),
            Key::Tab => "tab".into(),
            Key::Backspace => "bs".into(),
            Key::Shift => "shift".into(),
            Key::Up => "up".into(),
            Key::Down => "dn".into(),
            Key::Left => "lt".into(),
            Key::Right => "rt".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyEv {
    pub key: Key,
    pub kind: Kind,
    /// Text the key types (shifted when shift held), for CMD mode.
    pub text: Option<char>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Play,
    Pick,
    Look,
    Cmd,
    /// skill tree screen
    Tree,
    /// worn items (hero pane ARMOR tab)
    Gear,
    /// carried items (hero pane BAGS tab)
    Bags,
    /// merchant / blacksmith / witch offers
    Shop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// buy shop offer n (0-based)
    ShopTake(u8),
    /// at a blacksmith: forge an item from ore
    ShopForge,
    OpenGear,
    /// bags: open, move the cursor (cols, rows), wear the selected item, discard it
    OpenBags,
    BagMove(i8, i8),
    BagWear,
    BagDiscard,
    GearPrev,
    GearNext,
    GearDrop,
    OpenTree,
    TreeMove(i8, i8),
    TreeSpend,
    Up,
    Down,
    Left,
    Right,
    Dodge,
    Hack,
    RotLeft,
    RotRight,
    Autofire,
    Autowalk,
    Fire,
    Look,
    Cmd,
    Quit,
    Prev,
    Next,
    Confirm,
    Card(u8),
    /// number key in play: cast the special of bar slot n
    Special(u8),
    /// open the waiting power-up offer (it waits until the player opens it)
    OpenPick,
    Skip,
    CurUp,
    CurDown,
    CurLeft,
    CurRight,
    Back,
}

impl Action {
    pub fn parse(s: &str) -> Option<Action> {
        use Action::*;
        Some(match s {
            "up" => Up,
            "down" => Down,
            "left" => Left,
            "right" => Right,
            "dodge" => Dodge,
            "hack" => Hack,
            "rot_left" => RotLeft,
            "rot_right" => RotRight,
            "autofire" => Autofire,
            "shop_1" => ShopTake(0),
            "shop_2" => ShopTake(1),
            "shop_3" => ShopTake(2),
            "shop_4" => ShopTake(3),
            "forge" => ShopForge,
            "gear" => OpenGear,
            "bags" => OpenBags,
            "bag_left" => BagMove(-1, 0),
            "bag_right" => BagMove(1, 0),
            "bag_up" => BagMove(0, -1),
            "bag_down" => BagMove(0, 1),
            "bag_wear" => BagWear,
            "bag_discard" => BagDiscard,
            "gear_prev" => GearPrev,
            "gear_next" => GearNext,
            "gear_drop" => GearDrop,
            "tree" => OpenTree,
            "tree_left" => TreeMove(-1, 0),
            "tree_right" => TreeMove(1, 0),
            "tree_up" => TreeMove(0, -1),
            "tree_down" => TreeMove(0, 1),
            "tree_spend" => TreeSpend,
            "autowalk" => Autowalk,
            "fire" => Fire,
            "look" => Look,
            "cmd" => Cmd,
            "quit" => Quit,
            "prev" => Prev,
            "next" => Next,
            "confirm" => Confirm,
            "special1" => Special(0),
            "special2" => Special(1),
            "special3" => Special(2),
            "special4" => Special(3),
            "special5" => Special(4),
            "special6" => Special(5),
            "special7" => Special(6),
            "special8" => Special(7),
            "pick" => OpenPick,
            "card1" => Card(0),
            "card2" => Card(1),
            "card3" => Card(2),
            "skip" => Skip,
            "cur_up" => CurUp,
            "cur_down" => CurDown,
            "cur_left" => CurLeft,
            "cur_right" => CurRight,
            "back" => Back,
            _ => return None,
        })
    }
    /// Screen direction of a held-move action.
    fn dir(self) -> Option<usize> {
        match self {
            Action::Up => Some(0),
            Action::Down => Some(1),
            Action::Left => Some(2),
            Action::Right => Some(3),
            _ => None,
        }
    }
}

// ---------- kitty protocol parser ----------

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    Key(KeyEv),
    KbdFlags(u32),
    DeviceAttrs,
    /// left button pressed at screen cell (col, row), 0-based (SGR mouse)
    Click(u16, u16),
    Other,
}

#[derive(Default)]
pub struct Parser {
    buf: Vec<u8>,
}

/// SGR mouse report body after `<` (`b;x;y`, 1-based cells, final M = press): a left press is a click.
fn sgr_click(body: &str) -> Event {
    let mut f = body.split(';').map(|v| v.parse::<u16>().ok());
    match (f.next().flatten(), f.next().flatten(), f.next().flatten()) {
        (Some(0), Some(x), Some(y)) if x > 0 && y > 0 => Event::Click(x - 1, y - 1),
        _ => Event::Other,
    }
}

fn csi_key(params: &str, fin: u8) -> Event {
    let mut fields = params.split(';');
    let codes = fields.next().unwrap_or("");
    let mods = fields.next().unwrap_or("");
    let mut cs = codes.split(':').map(|c| c.parse::<u32>().ok());
    let base = cs.next().flatten();
    let shifted = cs.next().flatten();
    let mut ms = mods.split(':');
    let m = ms.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(1).saturating_sub(1);
    let kind = match ms.next() {
        Some("2") => Kind::Repeat,
        Some("3") => Kind::Release,
        _ => Kind::Press,
    };
    let shift = m & 1 != 0;
    let key = match (fin, base) {
        (b'A', _) => Key::Up,
        (b'B', _) => Key::Down,
        (b'C', _) => Key::Right,
        (b'D', _) => Key::Left,
        (b'u', Some(13)) => Key::Enter,
        (b'u', Some(27)) => Key::Esc,
        (b'u', Some(9)) => Key::Tab,
        (b'u', Some(127)) => Key::Backspace,
        (b'u', Some(57441 | 57447)) => Key::Shift,
        (b'u', Some(c)) => match char::from_u32(c) {
            // shifted symbol (`:` from `;`) is the key; letters stay lowercase so release matches press
            Some(ch) if ch.is_ascii_graphic() && shift && !ch.is_ascii_alphabetic() => {
                Key::Char(shifted.and_then(char::from_u32).filter(char::is_ascii_graphic).unwrap_or(ch))
            }
            Some(ch) if ch.is_ascii_graphic() || ch == ' ' => Key::Char(ch),
            _ => return Event::Other,
        },
        _ => return Event::Other,
    };
    let text = match key {
        Key::Char(c) if shift => shifted.and_then(char::from_u32).or(Some(c.to_ascii_uppercase())),
        Key::Char(c) => Some(c),
        _ => None,
    };
    Event::Key(KeyEv { key, kind, text })
}

impl Parser {
    /// Feed bytes, return complete events; keep an incomplete escape tail.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.buf.extend_from_slice(bytes);
        let mut evs = Vec::new();
        let b = &self.buf;
        let mut i = 0;
        while i < b.len() {
            if b[i] != 0x1b {
                // legacy byte (no protocol): printable = press
                let c = char::from(b[i]);
                evs.push(match b[i] {
                    b'\r' => Event::Key(KeyEv { key: Key::Enter, kind: Kind::Press, text: None }),
                    0x7f => Event::Key(KeyEv { key: Key::Backspace, kind: Kind::Press, text: None }),
                    0x20..0x7f => Event::Key(KeyEv { key: Key::Char(c.to_ascii_lowercase()), kind: Kind::Press, text: Some(c) }),
                    _ => Event::Other,
                });
                i += 1;
                continue;
            }
            match b.get(i + 1) {
                None => break,
                Some(b'[') => {
                    let Some(end) = b[i + 2..].iter().position(|c| (0x40..=0x7e).contains(c)) else { break };
                    let body = String::from_utf8_lossy(&b[i + 2..i + 2 + end]).into_owned();
                    let fin = b[i + 2 + end];
                    evs.push(match (fin, body.strip_prefix('?')) {
                        (b'u', Some(f)) => f.parse().map_or(Event::Other, Event::KbdFlags),
                        (b'c', Some(_)) => Event::DeviceAttrs,
                        (b'M', None) if body.starts_with('<') => sgr_click(&body[1..]),
                        (b'u' | b'A' | b'B' | b'C' | b'D', None) => csi_key(&body, fin),
                        _ => Event::Other,
                    });
                    i += 3 + end;
                }
                Some(_) => {
                    evs.push(Event::Other);
                    i += 2;
                }
            }
        }
        self.buf.drain(..i);
        evs
    }
}

// ---------- modal UI ----------

pub struct Ui {
    pub mode: Mode,
    seq: Vec<Key>,
    seq_at: u64,
    held: [Option<(i32, i32)>; 4],
    pub heading: Heading,
    pub autofire: bool,
    /// hero walks by itself (bot::Autopilot movement); a held direction key overrides
    pub autowalk: bool,
    /// attack key held (manual fire while autofire is off)
    pub fire: bool,
    /// skill tree cursor (node index in the hero tree), nodes owned, points earned by levels / spent
    pub tree_sel: usize,
    pub tree_owned: Owned,
    pub tree_earned: u16,
    pub tree_spent: u16,
    /// worn-items cursor over `Slot::ALL`; `gear_drop` is the one-shot take-off request
    pub gear_sel: usize,
    pub gear_drop: Option<Slot>,
    /// bag cursor (slot index) and the one-shot bag request
    pub bag_sel: usize,
    pub bag_act: Option<BagAct>,
    /// one-shot: the skill a just-learned tree node grants (taken by `input`)
    pub learn: Option<crate::data::SkillId>,
    /// one-shot: forge at the blacksmith
    pub forge: bool,
    /// one-shot: special slot asked by a number key (taken by `input`)
    pub special: Option<u8>,
    /// auto-walk and auto-fire as they were before a popup opened (popups force both on so play goes on)
    popup_saved: Option<(bool, bool)>,
    /// shop offer count (set by `sync_shop`); `shop_buy` is the one-shot buy request
    pub shop_len: usize,
    pub shop_buy: Option<usize>,
    /// player left the shop with esc and still stands at the service: do not reopen
    shop_left: bool,
    pub cmdline: String,
    pub msg: String,
    pub quit: bool,
    pub sel: u8,
    pub cursor: (i32, i32),
    shift_clean: bool,
    dodge: bool,
    hack: bool,
    pick: Option<PickChoice>,
    pub pick_open_ms: u64,
    /// the sim holds a power-up offer the player has not opened yet (p opens it)
    pub offer_waiting: bool,
    /// clock of the last key event (ms), for actions that start a timer
    now_ms: u64,
    /// command typed by the human (`:spawn`), sent through the sim validator with src `human`
    pub human: Option<Cmd>,
}

impl Ui {
    pub fn new(autofire: bool) -> Ui {
        Ui {
            mode: Mode::Play,
            seq: Vec::with_capacity(8),
            seq_at: 0,
            held: [None; 4],
            heading: Heading::default(),
            autofire,
            autowalk: false,
            fire: false,
            tree_sel: 0,
            tree_owned: Owned::default(),
            tree_earned: 0,
            tree_spent: 0,
            gear_sel: 0,
            gear_drop: None,
            bag_sel: 0,
            bag_act: None,
            learn: None,
            forge: false,
            special: None,
            popup_saved: None,
            shop_len: 0,
            shop_buy: None,
            shop_left: false,
            cmdline: String::new(),
            msg: String::new(),
            quit: false,
            sel: 0,
            cursor: (0, 0),
            shift_clean: false,
            dodge: false,
            hack: false,
            pick: None,
            pick_open_ms: 0,
            offer_waiting: false,
            now_ms: 0,
            human: None,
        }
    }

    /// New run: clean state, same settings (the toggles).
    pub fn fresh(&self) -> Ui {
        // a popup open at restart forced the toggles on: the player's own choice is the saved one
        let (walk, fire) = self.choice();
        Ui { autowalk: walk, ..Ui::new(fire) }
    }

    /// Track the sim's power-up offer. It does not pop up by itself (that interrupts play): it waits until the
    /// player opens it with p; PICK closes when the offer is taken or gone.
    pub fn sync(&mut self, offer_open: bool) {
        self.offer_waiting = offer_open;
        if !offer_open && self.mode == Mode::Pick {
            self.mode = Mode::Play;
        }
    }

    /// Enter / leave SHOP: `Some(offers)` while the hero stands at a service, `None` away from it.
    pub fn sync_shop(&mut self, at_service: Option<usize>) {
        match at_service {
            None => {
                self.shop_left = false;
                if self.mode == Mode::Shop {
                    self.mode = Mode::Play;
                }
            }
            Some(n) => {
                self.shop_len = n;
                if self.mode == Mode::Play && !self.shop_left {
                    self.mode = Mode::Shop;
                    self.seq.clear();
                }
            }
        }
    }

    pub fn key(&mut self, ev: KeyEv, c: &Content, now_ms: u64) {
        self.now_ms = now_ms;
        if ev.kind == Kind::Release {
            if ev.key == Key::Shift && self.shift_clean && self.bound(c, &[Key::Shift]) == Some(Action::Dodge) {
                self.dodge = true;
            }
            for b in c.binds.iter().filter(|b| b.keys == [ev.key] && matches!(b.mode, Mode::Play | Mode::Pick)) {
                if let Some(d) = b.action.dir() {
                    self.held[d] = None;
                }
                if b.action == Action::Fire {
                    self.fire = false;
                }
            }
            return;
        }
        if ev.kind == Kind::Repeat && self.mode != Mode::Look {
            return;
        }
        self.shift_clean = ev.key == Key::Shift;
        if ev.key == Key::Shift {
            return;
        }
        if self.mode == Mode::Cmd && self.edit(ev, c) {
            return;
        }
        // a shifted letter arrives as its base key + uppercase text (so its release matches the press): look it up
        // as the uppercase key, which is how shift bindings (Q, W, F) are written
        let key = match (ev.key, ev.text) {
            (Key::Char(c), Some(t)) if c.is_ascii_lowercase() && t == c.to_ascii_uppercase() => Key::Char(t),
            (k, _) => k,
        };
        self.seq.push(key);
        let exact = self.bound(c, &self.seq.clone());
        let longer = c.binds.iter().any(|b| b.mode == self.mode && b.keys.len() > self.seq.len() && b.keys.starts_with(&self.seq));
        match (exact, longer) {
            (Some(a), _) => {
                self.seq.clear();
                self.act(a, c);
            }
            (None, true) => self.seq_at = now_ms,
            (None, false) => self.seq.clear(),
        }
    }

    /// Binding of `keys` in the current mode. The shop is not a trap: keys it does not bind keep their play
    /// meaning (walk, dodge, fire), so the hero walks on and the shop closes behind.
    fn bound(&self, c: &Content, keys: &[Key]) -> Option<Action> {
        let find = |m: Mode| c.binds.iter().find(|b| b.mode == m && b.keys == keys).map(|b| b.action);
        find(self.mode).or_else(|| if self.mode == Mode::Shop { find(Mode::Play) } else { None })
    }

    /// CMD line editing; true if consumed.
    fn edit(&mut self, ev: KeyEv, c: &Content) -> bool {
        match (ev.key, ev.text) {
            (Key::Enter, _) => {
                let line = std::mem::take(&mut self.cmdline);
                self.mode = Mode::Play;
                self.run_cmd(&line, c);
                true
            }
            (Key::Backspace, _) => {
                if self.cmdline.pop().is_none() {
                    self.mode = Mode::Play;
                }
                true
            }
            (Key::Esc, _) => false,
            (_, Some(ch)) if self.cmdline.len() < 60 => {
                self.cmdline.push(ch);
                true
            }
            _ => true,
        }
    }

    fn run_cmd(&mut self, line: &str, c: &Content) {
        let words: Vec<&str> = line.split_whitespace().collect();
        self.msg = match words.as_slice() {
            ["autofire", "on"] => self.set_autofire(true),
            ["autofire", "off"] => self.set_autofire(false),
            ["autofire"] => self.set_autofire(!self.choice().1),
            ["autowalk", "on"] => self.set_autowalk(true),
            ["autowalk", "off"] => self.set_autowalk(false),
            ["autowalk"] => self.set_autowalk(!self.choice().0),
            ["cam", "left"] | ["cam"] => {
                self.heading = self.heading.left();
                format!("camera {}", self.heading.name())
            }
            ["cam", "right"] => {
                self.heading = self.heading.right();
                format!("camera {}", self.heading.name())
            }
            ["q" | "quit"] => {
                self.quit = true;
                "quit".into()
            }
            ["look"] => {
                self.mode = Mode::Look;
                "look".into()
            }
            ["spawn", rest @ ..] => match crate::director::parse_spawn(c, rest) {
                Ok(cmd) => {
                    self.human = Some(cmd);
                    "spawn queued (src human)".into()
                }
                Err(e) => format!("spawn: {e}"),
            },
            ["director"] => "director: builtin (no agent attached; MCP is FP2)".into(),
            [w @ ("prompt" | "edit" | "save" | "vcs" | "keep" | "feed"), ..] => format!(":{w} is FP2, not in FP1"),
            _ => format!("unknown :{line}  (autofire on|off, autowalk on|off, cam left|right, look, spawn, director, quit)"),
        };
    }

    fn set_autofire(&mut self, on: bool) -> String {
        // inside a popup both stay forced on; the toggle sets the choice restored when it closes
        match self.popup_saved.as_mut() {
            Some(s) => s.1 = on,
            None => self.autofire = on,
        }
        format!("autofire {}", if on { "on" } else { "off (aim = facing, x = hack)" })
    }

    /// Points still to spend (one per level after the first).
    pub fn tree_points(&self) -> u16 {
        self.tree_earned.saturating_sub(self.tree_spent)
    }

    fn spend(&mut self, c: &Content) -> String {
        let Some(t) = hero_tree(c) else { return "no skill tree".into() };
        match t.spend(self.tree_owned, self.tree_points(), self.tree_sel) {
            Ok((owned, left)) => {
                self.tree_spent = self.tree_earned.saturating_sub(left);
                self.tree_owned = owned;
                // the node grants its skill in the sim (a new hotkey with its special, or a rank up)
                self.learn = Some(t.nodes[self.tree_sel].effect);
                format!("learned {}", t.nodes[self.tree_sel].name)
            }
            Err(e) => format!("cannot spend: {e:?}"),
        }
    }

    /// The player's own (auto-walk, auto-fire): the saved pair while a popup forces both on.
    fn choice(&self) -> (bool, bool) {
        self.popup_saved.unwrap_or((self.autowalk, self.autofire))
    }

    fn set_autowalk(&mut self, on: bool) -> String {
        match self.popup_saved.as_mut() {
            Some(s) => s.0 = on,
            None => self.autowalk = on,
        }
        format!("autowalk {}", if on { "on" } else { "off" })
    }

    fn act(&mut self, a: Action, c: &Content) {
        if let Some(d) = a.dir() {
            let (sx, sy) = [(0, -1), (0, 1), (-1, 0), (1, 0)][d];
            self.held[d] = Some(screen_dir(self.heading, sx, sy));
            return;
        }
        match a {
            Action::Dodge => self.dodge = true,
            Action::Hack => self.hack = true,
            Action::RotLeft => self.heading = self.heading.left(),
            Action::RotRight => self.heading = self.heading.right(),
            Action::Autofire => self.msg = self.set_autofire(!self.choice().1),
            Action::OpenGear => self.mode = Mode::Gear,
            Action::OpenBags => self.mode = Mode::Bags,
            Action::BagMove(dc, dr) => {
                let (col, row) = ((self.bag_sel % BAG_COLS) as i32 + i32::from(dc), (self.bag_sel / BAG_COLS) as i32 + i32::from(dr));
                let (col, row) = (col.clamp(0, BAG_COLS as i32 - 1), row.clamp(0, (BAG_SLOTS / BAG_COLS) as i32 - 1));
                self.bag_sel = row as usize * BAG_COLS + col as usize;
            }
            Action::BagWear => self.bag_act = Some(BagAct::Wear(self.bag_sel as u8)),
            Action::BagDiscard => self.bag_act = Some(BagAct::Discard(self.bag_sel as u8)),
            Action::ShopTake(n) => self.shop_buy = (usize::from(n) < self.shop_len).then_some(usize::from(n)),
            Action::ShopForge => self.forge = true,
            Action::GearPrev => self.gear_sel = self.gear_sel.saturating_sub(1),
            Action::GearNext => self.gear_sel = (self.gear_sel + 1).min(Slot::ALL.len() - 1),
            Action::GearDrop => self.gear_drop = Slot::ALL.get(self.gear_sel).copied(),
            Action::OpenTree => self.mode = Mode::Tree,
            Action::TreeMove(dc, dr) => {
                if let Some(t) = hero_tree(c) {
                    self.tree_sel = tree_step(t, self.tree_sel, dc, dr);
                }
            }
            Action::TreeSpend => self.msg = self.spend(c),
            Action::Autowalk => self.msg = self.set_autowalk(!self.choice().0),
            Action::Fire => self.fire = true,
            Action::Special(n) => self.special = Some(n),
            Action::OpenPick if self.offer_waiting => {
                self.mode = Mode::Pick;
                self.sel = 0;
                self.pick_open_ms = self.now_ms;
            }
            Action::OpenPick => self.msg = "no power-up waiting".into(),
            Action::Look => {
                self.mode = Mode::Look;
                self.cursor = (0, 0);
            }
            Action::Cmd => {
                self.mode = Mode::Cmd;
                self.cmdline.clear();
            }
            Action::Quit => self.quit = true,
            Action::Prev => self.sel = (self.sel + c.picks.cards as u8 - 1) % c.picks.cards as u8,
            Action::Next => self.sel = (self.sel + 1) % c.picks.cards as u8,
            Action::Confirm => self.pick = Some(PickChoice::Card(self.sel)),
            Action::Card(i) => self.pick = Some(PickChoice::Card(i)),
            Action::Skip => self.pick = Some(PickChoice::Skip),
            Action::CurUp => self.cursor.1 -= 1,
            Action::CurDown => self.cursor.1 += 1,
            Action::CurLeft => self.cursor.0 -= 1,
            Action::CurRight => self.cursor.0 += 1,
            Action::Back => {
                self.shop_left |= self.mode == Mode::Shop;
                self.mode = Mode::Play;
                self.cmdline.clear();
            }
            Action::Up | Action::Down | Action::Left | Action::Right => {}
        }
    }

    /// which-key panel: (prefix, [(next key, label)]) once a prefix is pending >= whichkey_ms.
    pub fn whichkey(&self, c: &Content, now_ms: u64) -> Option<(String, Vec<(String, String)>)> {
        if self.seq.is_empty() || now_ms.saturating_sub(self.seq_at) < c.opt.whichkey_ms {
            return None;
        }
        let n = self.seq.len();
        let mut items: Vec<(String, String)> = Vec::new();
        for b in c.binds.iter().filter(|b| b.mode == self.mode && b.keys.len() > n && b.keys.starts_with(&self.seq)) {
            let k = b.keys[n];
            let name = k.name();
            if items.iter().any(|(x, _)| *x == name) {
                continue;
            }
            let label = if b.keys.len() == n + 1 {
                b.label.clone()
            } else {
                let g = c.groups.iter().find(|g| g.mode == self.mode && g.keys.len() == n + 1 && g.keys.starts_with(&self.seq) && g.keys[n] == k);
                format!("+{}", g.map_or("more", |g| g.label.as_str()))
            };
            items.push((name, label));
        }
        let prefix = self.seq.iter().map(|k| k.name()).collect::<Vec<_>>().join(" ");
        Some((prefix, items))
    }

    /// Per-tick sim input; one-shot actions are consumed.
    pub fn input(&mut self, pick_pauses: bool) -> Input {
        // a popup (picker, tree, gear, shop) keeps the game going: auto-walk and auto-fire on while it is open,
        // restored to what they were when it closes
        let popup = matches!(self.mode, Mode::Pick | Mode::Tree | Mode::Gear | Mode::Bags | Mode::Shop);
        match (popup, self.popup_saved) {
            (true, None) => {
                self.popup_saved = Some((self.autowalk, self.autofire));
                (self.autowalk, self.autofire) = (true, true);
            }
            (false, Some((walk, fire))) => {
                (self.autowalk, self.autofire) = (walk, fire);
                self.popup_saved = None;
            }
            _ => {}
        }
        // fire is a PLAY hold: any mode change drops it. The kitty protocol is required (NoKitty
        // otherwise), so release events always arrive; without them fire would stick until a mode change.
        if self.mode != Mode::Play {
            self.fire = false;
        }
        let moving = matches!(self.mode, Mode::Play | Mode::Look | Mode::Cmd | Mode::Shop) || (self.mode == Mode::Pick && !pick_pauses);
        let (mut x, mut y) = (0, 0);
        for (dx, dy) in self.held.iter().flatten() {
            x += dx;
            y += dy;
        }
        Input {
            mv: if moving { (x.signum(), y.signum()) } else { (0, 0) },
            dodge: std::mem::take(&mut self.dodge),
            hack: std::mem::take(&mut self.hack),
            autofire: self.autofire,
            fire: self.fire,
            pick: self.pick.take(),
            buy: self.shop_buy.take(),
            drop: self.gear_drop.take(),
            bag: self.bag_act.take(),
            learn: self.learn.take(),
            forge: std::mem::take(&mut self.forge),
            special: self.special.take(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;

    fn press(c: char) -> KeyEv {
        KeyEv { key: Key::Char(c), kind: Kind::Press, text: Some(c) }
    }

    /// Real terminal bytes end to end: kitty shift+w and legacy W toggle auto-walk (and do not walk); space held
    /// keeps the hero firing volley after volley with auto-fire off, and the release stops it.
    #[test]
    fn shift_letters_and_space_hold_from_real_bytes() {
        use crate::sim::{Sim, Tier};
        let c = content();
        let feed = |ui: &mut Ui, bytes: &[u8]| {
            for e in Parser::default().feed(bytes) {
                if let Event::Key(k) = e {
                    ui.key(k, &c, 0);
                }
            }
        };
        let mut ui = Ui::new(false);
        feed(&mut ui, b"\x1b[119:87;2u");
        assert!(ui.autowalk, "kitty shift+w toggles auto-walk");
        assert_eq!(ui.input(false).mv, (0, 0), "and does not walk");
        feed(&mut ui, b"\x1b[119:87;2:3u");
        feed(&mut ui, b"W");
        assert!(!ui.autowalk, "legacy W toggles back");
        feed(&mut ui, b"\x1b[119u");
        assert_eq!(ui.input(false).mv.1, -1, "control: plain w walks");
        feed(&mut ui, b"\x1b[119;1:3u");
        // space hold: auto-fire off, a foe in range
        let mut sim = Sim::new(&c, 7, None);
        let fam = c.fam_ids().find(|&f| c.fam(f).key == "scrapper").unwrap();
        sim.push_enemy(fam, Tier::Trash, (sim.p.x + 5_000, sim.p.y), 100_000_000, (false, false));
        let volleys = |ui: &mut Ui, sim: &mut Sim, ticks: u32| {
            let mut n = 0;
            for _ in 0..ticks {
                let before = sim.bolts().count();
                sim.p.hp = sim.p.max_hp;
                sim.step(&ui.input(false));
                n += u32::from(sim.bolts().count() > before);
            }
            n
        };
        assert_eq!(volleys(&mut ui, &mut sim, 60), 0, "control: nothing pressed, no fire");
        feed(&mut ui, b"\x1b[32u");
        assert!(volleys(&mut ui, &mut sim, 120) >= 3, "held space fires again and again");
        feed(&mut ui, b"\x1b[32;1:3u");
        let _ = volleys(&mut ui, &mut sim, 30);
        assert_eq!(volleys(&mut ui, &mut sim, 60), 0, "released: stops");
    }

    #[test]
    fn sgr_left_press_is_a_click_others_are_not() {
        let mut p = Parser::default();
        let e = p.feed(b"\x1b[<0;10;40M\x1b[<0;10;40m\x1b[<2;5;5M");
        assert!(matches!(e[0], Event::Click(9, 39)), "{:?}", e.len());
        assert!(matches!(e[1], Event::Other) && matches!(e[2], Event::Other), "release and right button ignored");
    }

    #[test]
    fn decodes_kitty_events_across_chunks() {
        let mut p = Parser::default();
        assert!(p.feed(b"\x1b[119;1:").is_empty());
        let e = p.feed(b"3u\x1b[59:58;2u\x1b[57441u\x1b[1;1:2A\x1b[?15u\x1b[?62;22c");
        let k = |key, kind, text| Event::Key(KeyEv { key, kind, text });
        assert_eq!(
            e,
            vec![
                k(Key::Char('w'), Kind::Release, Some('w')),
                k(Key::Char(':'), Kind::Press, Some(':')),
                k(Key::Shift, Kind::Press, None),
                k(Key::Up, Kind::Repeat, None),
                Event::KbdFlags(15),
                Event::DeviceAttrs,
            ]
        );
    }

    #[test]
    fn held_key_keeps_world_direction_across_rotation() {
        let c = content();
        let mut ui = Ui::new(true);
        ui.key(press('d'), &c, 0);
        let before = ui.input(true).mv;
        ui.key(press('e'), &c, 0); // rotate while holding
        assert_eq!(ui.input(true).mv, before);
        ui.key(KeyEv { key: Key::Char('d'), kind: Kind::Release, text: None }, &c, 0);
        assert_eq!(ui.input(true).mv, (0, 0));
        ui.key(press('d'), &c, 0);
        assert_ne!(ui.input(true).mv, before, "new press uses new heading");
    }

    fn release(c: char) -> KeyEv {
        KeyEv { key: Key::Char(c), kind: Kind::Release, text: None }
    }

    #[test]
    fn autowalk_toggles_by_shift_w_and_cmd_space_fire_is_held() {
        let c = content();
        let mut ui = Ui::new(false);
        assert!(!ui.autowalk && !ui.fire);
        ui.key(press('w'), &c, 0);
        assert!(!ui.autowalk, "NEG: plain w walks, it does not toggle");
        ui.key(KeyEv { key: Key::Char('W'), kind: Kind::Press, text: Some('W') }, &c, 0);
        assert!(ui.autowalk);
        ui.run_cmd("autowalk off", &c);
        assert!(!ui.autowalk);
        ui.key(press(' '), &c, 0);
        assert!(ui.fire);
        ui.key(release(' '), &c, 0);
        assert!(!ui.fire);
        // negative controls: other key release keeps it, a direction key never sets it
        ui.key(press(' '), &c, 0);
        ui.key(release('d'), &c, 0);
        assert!(ui.fire);
        ui.key(release(' '), &c, 0);
        ui.key(press('d'), &c, 0);
        assert!(!ui.fire);
        // mode change drops a held fire even without its release
        ui.key(press(' '), &c, 0);
        ui.key(press('t'), &c, 0);
        let _ = ui.input(true);
        assert!(!ui.fire);
    }


    #[test]
    fn shop_opens_at_service_buys_and_esc_does_not_reopen_until_away() {
        let c = content();
        let mut ui = Ui::new(true);
        ui.sync_shop(None);
        assert_eq!(ui.mode, Mode::Play);
        ui.sync_shop(Some(3));
        assert_eq!(ui.mode, Mode::Shop);
        ui.key(press('4'), &c, 0);
        assert_eq!(ui.shop_buy, None, "NEG: offer 4 of 3 does not exist");
        ui.key(press('2'), &c, 0);
        assert_eq!(ui.shop_buy, Some(1), "2 buys the second offer");
        // walking keys keep walking in a shop (no trap)
        ui.key(press('d'), &c, 0);
        let inp = ui.input(true);
        assert!(inp.mv != (0, 0) && inp.buy == Some(1), "walks and buys: {inp:?}");
        ui.key(KeyEv { key: Key::Char('d'), kind: Kind::Release, text: None }, &c, 0);
        ui.key(KeyEv { key: Key::Esc, kind: Kind::Press, text: None }, &c, 0);
        ui.sync_shop(Some(3));
        assert_eq!(ui.mode, Mode::Play, "esc leaves while standing there");
        ui.sync_shop(None);
        ui.sync_shop(Some(3));
        assert_eq!(ui.mode, Mode::Shop, "walking away and back reopens");
        ui.sync_shop(None);
        assert_eq!(ui.mode, Mode::Play, "walked away");
    }

    #[test]
    fn tree_mode_moves_on_the_grid_and_spends_by_requirements_and_points() {
        let c = content();
        let mut ui = Ui::new(true);
        let enter = KeyEv { key: Key::Enter, kind: Kind::Press, text: None };
        ui.key(press('t'), &c, 0);
        assert_eq!(ui.mode, Mode::Tree);
        let t = hero_tree(&c).unwrap();
        let node = |i: usize| (t.nodes[i].col, t.nodes[i].row);
        assert_eq!(node(ui.tree_sel), (1, 0), "starts on the root");
        ui.key(press('h'), &c, 0);
        assert_eq!(node(ui.tree_sel), (1, 0), "nothing left of the root: stays");
        ui.key(press('j'), &c, 0);
        assert_eq!(node(ui.tree_sel).1, 1, "down to the next row");
        // NEG: no points yet
        ui.key(enter, &c, 0);
        assert!(ui.msg.contains("NeedPoints") || ui.msg.contains("Locked"), "{}", ui.msg);
        assert!(!ui.tree_owned.has(ui.tree_sel));
        ui.tree_earned = 1;
        ui.key(enter, &c, 0);
        assert!(ui.msg.contains("Locked"), "NEG: requirement unmet: {}", ui.msg);
        assert_eq!(ui.tree_points(), 1, "a refused spend keeps the point");
        ui.key(press('k'), &c, 0);
        ui.key(enter, &c, 0);
        assert!(ui.tree_owned.has(ui.tree_sel) && ui.tree_points() == 0, "{}", ui.msg);
        ui.key(KeyEv { key: Key::Esc, kind: Kind::Press, text: None }, &c, 0);
        assert_eq!(ui.mode, Mode::Play);
    }

    #[test]
    fn popups_keep_play_going_and_restore_toggles() {
        let c = content();
        let mut ui = Ui::new(false);
        ui.autowalk = false;
        let _ = ui.input(false);
        assert!(!ui.autowalk && !ui.autofire, "control: both off in play");
        ui.key(press('t'), &c, 0);
        assert_eq!(ui.mode, Mode::Tree);
        let _ = ui.input(false);
        assert!(ui.autowalk && ui.autofire, "tree open: the hero walks and fights on");
        ui.key(KeyEv { key: Key::Esc, kind: Kind::Press, text: None }, &c, 0);
        let _ = ui.input(false);
        assert!(!ui.autowalk && !ui.autofire, "closed: restored");
        // toggled inside a shop (play keys fall through): that choice sticks
        ui.sync_shop(Some(2));
        let _ = ui.input(false);
        ui.key(KeyEv { key: Key::Char('F'), kind: Kind::Press, text: Some('F') }, &c, 0);
        ui.sync_shop(None);
        let _ = ui.input(false);
        assert!(ui.autofire && !ui.autowalk, "auto-fire toggled on in the shop stays on, auto-walk restored off");
    }

    /// Learning a tree node reaches the sim: the root's skill ranks up; a new skill takes the next hotkey slot.
    #[test]
    fn tree_nodes_grant_skills_in_the_sim() {
        use crate::sim::Sim;
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let mut ui = Ui::new(true);
        let tree = hero_tree(&c).unwrap();
        ui.key(press('t'), &c, 0);
        ui.tree_earned = 2;
        let root = tree.nodes[0].effect;
        let rank0 = s.owned.iter().find(|o| o.id == root).map_or(0, |o| o.rank);
        ui.key(KeyEv { key: Key::Enter, kind: Kind::Press, text: None }, &c, 0);
        s.step(&ui.input(true));
        assert_eq!(s.owned.iter().find(|o| o.id == root).map_or(0, |o| o.rank), rank0 + 1, "root learned: its skill ranks up (or arrives)");
        // a node whose skill the hero lacks: it arrives in the next hotkey slot
        let k = tree.nodes.iter().position(|n| n.requires == [0] && !s.owned.iter().any(|o| o.id == n.effect)).expect("a child granting a new skill");
        ui.tree_sel = k;
        let slots = s.owned.len();
        ui.key(KeyEv { key: Key::Enter, kind: Kind::Press, text: None }, &c, 0);
        s.step(&ui.input(true));
        assert_eq!((s.owned.len(), s.owned[slots].id), (slots + 1, tree.nodes[k].effect), "new skill in hotkey slot {}", slots + 1);
        assert!(c.skill(tree.nodes[k].effect).kind.has_special(), "and it has a special to bind");
    }

    #[test]
    fn number_keys_ask_specials_once() {
        let c = content();
        let mut ui = Ui::new(true);
        ui.key(press('3'), &c, 0);
        assert_eq!(ui.input(true).special, Some(2), "key 3 = slot 3");
        assert_eq!(ui.input(true).special, None, "one cast per press");
    }

    #[test]
    fn pane_keys_switch_tabs_and_own_key_closes() {
        let c = content();
        let mut ui = Ui::new(true);
        let modes = [('i', Mode::Gear), ('t', Mode::Tree), ('b', Mode::Bags)];
        for (from_key, from) in modes {
            for (to_key, to) in modes {
                ui.mode = Mode::Play;
                ui.key(press(from_key), &c, 0);
                assert_eq!(ui.mode, from);
                ui.key(press(to_key), &c, 0);
                // the open tab's own key closes the pane (control); another key switches to its tab
                assert_eq!(ui.mode, if from == to { Mode::Play } else { to }, "{from_key} then {to_key}");
            }
        }
    }

    #[test]
    fn gear_mode_navigates_drops_and_esc_returns() {
        let c = content();
        let mut ui = Ui::new(true);
        ui.key(press('i'), &c, 0);
        assert_eq!(ui.mode, Mode::Gear);
        assert_eq!(ui.mode, Mode::Gear);
        ui.key(press('x'), &c, 0);
        assert_eq!(ui.gear_drop, Some(Slot::Armor), "cursor starts on the first slot");
        for _ in 0..9 {
            ui.key(press('j'), &c, 0);
        }
        ui.key(press('x'), &c, 0);
        assert_eq!(ui.gear_drop, Some(Slot::Charm), "cursor stops on the last slot");
        assert_eq!(ui.input(true).drop, Some(Slot::Charm), "the request reaches the sim once");
        assert_eq!(ui.input(true).drop, None);
        ui.key(KeyEv { key: Key::Esc, kind: Kind::Press, text: None }, &c, 0);
        assert_eq!(ui.mode, Mode::Play);
    }

    #[test]
    fn whichkey_waits_then_lists_group_and_never_fires_early() {
        let c = content();
        let mut ui = Ui::new(true);
        ui.key(press('c'), &c, 1000);
        assert!(ui.whichkey(&c, 1100).is_none(), "before 250 ms");
        let (_, items) = ui.whichkey(&c, 1300).unwrap();
        assert!(items.contains(&("q".into(), "turn left".into())), "{items:?}");
        assert!(items.contains(&("e".into(), "turn right".into())), "{items:?}");
        let before = ui.heading;
        ui.key(press('q'), &c, 1400);
        assert!(ui.heading != before && ui.whichkey(&c, 9999).is_none(), "c q turns the camera, menu closes");
    }

    #[test]
    fn shift_tap_dodges_but_shift_chord_does_not() {
        let c = content();
        let mut ui = Ui::new(true);
        let sh = |kind| KeyEv { key: Key::Shift, kind, text: None };
        ui.key(sh(Kind::Press), &c, 0);
        ui.key(sh(Kind::Release), &c, 0);
        assert!(ui.input(true).dodge);
        ui.key(sh(Kind::Press), &c, 0);
        ui.key(press(':'), &c, 0);
        ui.key(sh(Kind::Release), &c, 0);
        assert!(!ui.input(true).dodge);
        assert_eq!(ui.mode, Mode::Cmd);
    }

    #[test]
    fn cmd_autofire_off_and_unknown() {
        let c = content();
        let mut ui = Ui::new(true);
        ui.key(press(':'), &c, 0);
        for ch in "autofire off".chars() {
            ui.key(press(ch), &c, 0);
        }
        ui.key(KeyEv { key: Key::Enter, kind: Kind::Press, text: None }, &c, 0);
        assert!(!ui.autofire && ui.mode == Mode::Play);
        ui.run_cmd("prompt x", &c);
        assert!(ui.msg.contains("FP2"));
        ui.run_cmd("spawn brute 4 elite ring", &c);
        assert!(matches!(ui.human, Some(Cmd::Spawn { .. })));
        ui.run_cmd("spawn dragon", &c);
        assert!(ui.msg.contains("dragon"));
    }

    #[test]
    fn pick_mode_keys_choose_and_block_moves_when_paused() {
        let c = content();
        let mut ui = Ui::new(true);
        ui.sync(true);
        assert_eq!(ui.mode, Mode::Play, "the offer waits, it does not pop up");
        ui.key(press('p'), &c, 500);
        assert_eq!((ui.mode, ui.pick_open_ms), (Mode::Pick, 500), "p opens it, the timer starts now");
        ui.key(press('l'), &c, 0);
        ui.key(KeyEv { key: Key::Enter, kind: Kind::Press, text: None }, &c, 0);
        let i = ui.input(true);
        assert_eq!(i.pick, Some(PickChoice::Card(1)));
        // direction keys move the selection in PICK (a/w/h/k/left/up = prev, d/s/j/l/right/down = next), never the hero
        ui.sel = 1;
        ui.key(press('w'), &c, 0);
        assert_eq!(ui.sel, 0);
        ui.key(press('s'), &c, 0);
        ui.key(press('j'), &c, 0);
        assert_eq!(ui.sel, 2);
        assert_eq!(ui.input(true).mv, (0, 0));
        assert_eq!(ui.input(false).mv, (0, 0));
    }
}
