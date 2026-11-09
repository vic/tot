//! Shown texts and line pools: every word the game puts on screen comes from the adventure's texts.jsonl. Each
//! entry here is the schema: its id, its `{slot}`s (filled by the game, in this order), the font it is drawn in,
//! and where it shows. `Txt<N>` and `Pool` handles are built only by `named`, at compile time: an id that is not
//! in the table, or a slot count that is not N, fails the build.

/// How a text is drawn: one line of cell glyphs, or the big block font (letters ! ? . and space only).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Font {
    Line,
    Big,
}

pub struct Spec {
    pub id: &'static str,
    pub slots: &'static [&'static str],
    pub font: Font,
    pub doc: &'static str,
}

const fn line(id: &'static str, slots: &'static [&'static str], doc: &'static str) -> Spec {
    Spec { id, slots, font: Font::Line, doc }
}

const fn big(id: &'static str, doc: &'static str) -> Spec {
    Spec { id, slots: &[], font: Font::Big, doc }
}

pub const TXTS: [Spec; 86] = [
    line("pick_adventure", &[], "Start screen, step 1: one header line centred under the title art, above the list of adventures."),
    line("pick_class", &[], "Start screen, step 2: one header line centred under the title art, above the hero classes of the picked adventure."),
    // upgrade picker (centred popup over the arena)
    line("pick_title", &[], "Upgrade picker popup, top line: the title before the seconds left to choose (keep a trailing space)."),
    line("pick_none", &[], "Upgrade picker popup: a card slot with no card."),
    line("pick_cursor", &[], "Upgrade picker popup: mark before the highlighted card (2 chars; a blank of the same width marks the others)."),
    line("pick_new", &[], "Upgrade picker card: tag of a skill the hero does not have yet."),
    line("pick_rank", &[], "Upgrade picker card: tag before the rank a known skill goes to."),
    line("rarity_common", &[], "Upgrade picker card: rarity word of a common card."),
    line("rarity_rare", &[], "Upgrade picker card: rarity word of a rare card."),
    line("rarity_epic", &[], "Upgrade picker card: rarity word of an epic card."),
    line("rarity_legend", &[], "Upgrade picker card: rarity word of a legendary card."),
    // shop popup
    line("shop_title", &["gold", "forge"], "Shop popup, top line: the hero's gold and the buy keys; {forge} is the forge part (shop_forge) at a blacksmith, else empty."),
    line("shop_forge", &["ore", "need"], "Shop popup title, blacksmith only: the forge key with the ore carried and the ore one forge takes."),
    line("shop_row", &["key", "glyph", "name", "price"], "Shop popup: one offer line: its buy key, the item glyph, its name, its price in gold."),
    // specials bar (bottom bar, one box per hotkey)
    line("special_passive", &[], "Specials bar: under a passive skill's name (it has no cast)."),
    line("special_empty", &[], "Specials bar: a hotkey slot with no skill."),
    // death screen
    line("death_keys", &[], "Death screen, last line: the keys to start a new run and to quit."),
    line("slain_by", &["killer"], "Death screen: the line naming what killed the hero."),
    line("hero_slayer", &["foe"], "Death screen: title over the foe that killed the hero, with its family name."),
    // shouts and barks over foes
    big("pal_help", "Over a pal in danger: the big blinking call for help."),
    line("roar_quote", &["word"], "Under an uber boss's ROAR: its taunt (a roar_uber line) in quotes."),
    line("thief_stole", &["line", "power"], "Over a thief that ate loot: a thief_stole line and the loot power it has now."),
    line("thief_learned", &["ability"], "Over a thief that just gained an ability: the ability's name (ability_*)."),
    line("ability_fast", &[], "Name of the thief ability that makes it faster (thief_learned, look tooltip)."),
    line("ability_rage", &[], "Name of the thief ability that makes it rage."),
    line("ability_shot", &[], "Name of the thief ability that makes it shoot."),
    // left edge: waiting power-ups
    line("power_up", &[], "Power-up pane (left edge, under the map): its one line; keep it within 13 chars."),
    line("power_more", &["n"], "Under the power-up panes: how many more wait."),
    // stats panel (top right, 40 cols)
    line("clock_day", &[], "Stats panel, first row: the sign before the clock by day (one wide emoji and a space is fine)."),
    line("clock_night", &[], "Stats panel, first row: the sign before the clock by night."),
    line("panel_next", &[], "Stats panel, first row: label before the next boss's name and time."),
    line("panel_no_boss", &[], "Stats panel, first row: shown when no boss is left."),
    line("panel_flux", &[], "Stats panel: label of the flux (xp) bar."),
    line("panel_kills", &[], "Stats panel: label of the kill count."),
    line("panel_hijack", &[], "Stats panel: label of the hijacked foe count."),
    line("panel_nerve", &[], "Stats panel: label of the foes' nerve (morale) bar."),
    line("panel_panic", &[], "Stats panel: label of the panicked foe count."),
    line("panel_gold", &[], "Stats panel: label of the gold count."),
    line("panel_pals", &[], "Stats panel: label of the pals saved count."),
    line("panel_food", &[], "Stats panel: label of the food (hunger) bar."),
    line("panel_sky", &[], "Stats panel: label of the weather row."),
    line("panel_powers", &[], "Stats panel: label of the owned skills row."),
    line("panel_foes", &[], "Stats panel: label of the foe tier counts."),
    line("panel_net", &[], "Stats panel: label of the bytes sent per frame."),
    line("panel_cast", &[], "Stats panel: label of the cast rows (who is on screen)."),
    line("cast_hero", &[], "Stats panel cast row: the hero's HP."),
    line("cast_ally", &[], "Stats panel cast row: hijacked foes fighting for the hero."),
    line("cast_panic", &[], "Stats panel cast row: panicked foes."),
    line("cast_thief", &[], "Stats panel cast row: thieves that ate loot."),
    // foe tiers and damage types (panel, look tooltip)
    line("tier_trash", &[], "Name of the weakest foe tier (look tooltip, stats panel)."),
    line("tier_elite", &[], "Name of the elite foe tier."),
    line("tier_champion", &[], "Name of the champion foe tier."),
    line("tier_boss", &[], "Name of the boss tier."),
    line("hue_kin", &[], "Name of the kin damage type (look tooltip), drawn in its colour."),
    line("hue_therm", &[], "Name of the therm damage type."),
    line("hue_emp", &[], "Name of the emp damage type."),
    line("hue_code", &[], "Name of the code damage type."),
    // story card (top centre)
    line("story", &[], "Story card, top border: its title."),
    // look mode tooltip (bottom rows)
    line("look_help", &[], "Look mode, nothing under the cursor: how to use look mode."),
    line("look_hp", &["hp", "max"], "Look tooltip: the foe's HP now and its max HP."),
    line("look_glitch", &[], "Look tooltip: label before the foe's glitch value."),
    line("look_hijacked", &[], "Look tooltip: a foe fighting for the hero."),
    line("look_hostile", &[], "Look tooltip: a foe fighting the hero."),
    line("look_purger", &[], "Look tooltip: warning on a foe that kills hijacked foes."),
    line("look_ate", &["n"], "Look tooltip: how much loot the foe ate (then a foe_quip line)."),
    // sponsor button (bottom right) and death screen plea
    line("support_label", &[], "Sponsor button, bottom right of the status line (clickable)."),
    line("support_plea", &[], "Death screen: the line asking players to sponsor the game."),
    line("support_url", &[], "Link the sponsor button opens (a URL, not shown as text)."),
    // level up line (under the hero)
    line("level_line", &["level", "taunt"], "Under the hero on level up: the level reached and a level_taunt line."),
    // status line mode tags (bottom left)
    line("mode_end", &[], "Status line, left: tag of a finished run."),
    line("mode_play", &[], "Status line, left: tag of play mode."),
    line("mode_pick", &[], "Status line, left: tag of the upgrade picker."),
    line("mode_look", &[], "Status line, left: tag of look mode."),
    line("mode_cmd", &[], "Status line, left: tag of command mode."),
    line("mode_tree", &[], "Status line, left: tag of the skill tree pane."),
    line("mode_gear", &[], "Status line, left: tag of the worn gear pane."),
    line("mode_shop", &[], "Status line, left: tag of the shop popup."),
    line("mode_bags", &[], "Status line, left: tag of the bags pane."),
    line("mode_start", &[], "Status line, left: tag of the start screen."),
    // key hints (status line rows, after each key)
    line("hint_move", &[], "Key hints: what the move keys do."),
    line("hint_turn", &[], "Key hints: what the turn keys do."),
    line("hint_special", &[], "Key hints: what the number keys (specials) do."),
    line("hint_take", &[], "Key hints, upgrade picker: what the card keys do."),
    line("hint_buy", &[], "Key hints, shop: what the offer keys do."),
    line("hint_new_run", &[], "Key hints, finished run: the new run key."),
    line("hint_quit", &[], "Key hints, finished run: the quit key."),
];

pub const POOLS: [Spec; 9] = [
    line("thief_stole", &[], "Over a thief that ate loot: one picked per bite (then its power)."),
    line("level_taunt", &[], "Under the hero on level up: one picked per level."),
    big("roar_champion", "Over a champion that appears: one picked, in the big font."),
    big("roar_mini", "Over a mini boss that appears: one picked, in the big font."),
    big("roar_super", "Over a super boss that appears: one picked, in the big font."),
    line("roar_uber", &[], "Under an uber boss's double-size ROAR: one taunt picked, in quotes."),
    line("foe_quip", &[], "Look tooltip, after what a foe ate: one picked per foe."),
    line("cheer", &[], "Death screen: a short cheer hopping over every third foe of the crowd (keep it within 6 chars)."),
    line("death_taunt", &[], "Death screen: one taunt line picked per run, under slain_by."),
];

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn find(table: &[Spec], id: &str) -> usize {
    let mut i = 0;
    while i < table.len() {
        if same(table[i].id, id) {
            return i;
        }
        i += 1;
    }
    panic!("id not in the table")
}

/// A shown text with N slots (see `TXTS`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Txt<const N: usize>(pub(crate) usize);

impl<const N: usize> Txt<N> {
    pub const fn named(id: &str) -> Txt<N> {
        let i = find(&TXTS, id);
        assert!(TXTS[i].slots.len() == N, "slot count differs from the table");
        Txt(i)
    }
}

/// A pool of lines, one picked per use (see `POOLS`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pool(pub(crate) usize);

impl Pool {
    pub const fn named(id: &str) -> Pool {
        Pool(find(&POOLS, id))
    }
}

/// A parsed text: literal runs and slot numbers, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seg {
    Lit(String),
    Slot(usize),
}

/// Split `s` at `{slot}` marks; each mark must name one of `slots`, and each slot must appear.
pub fn segs(s: &str, slots: &[&str]) -> Result<Vec<Seg>, String> {
    let mut out = Vec::new();
    let mut rest = s;
    let mut seen = vec![false; slots.len()];
    while let Some(open) = rest.find('{') {
        let close = rest[open..].find('}').ok_or("a '{' without '}'")? + open;
        let name = &rest[open + 1..close];
        let k = slots.iter().position(|s| *s == name).ok_or_else(|| format!("unknown slot {{{name}}}"))?;
        seen[k] = true;
        if open > 0 {
            out.push(Seg::Lit(rest[..open].to_string()));
        }
        out.push(Seg::Slot(k));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        out.push(Seg::Lit(rest.to_string()));
    }
    match slots.iter().zip(&seen).find(|(_, s)| !**s) {
        Some((name, _)) => Err(format!("missing slot {{{name}}}")),
        None => Ok(out),
    }
}

/// `segs` with the slots filled.
pub fn fill(segs: &[Seg], args: &[&str]) -> String {
    segs.iter().map(|s| match s {
        Seg::Lit(t) => t.as_str(),
        Seg::Slot(k) => args.get(*k).copied().unwrap_or_default(),
    }).collect()
}

/// Named handles, one per table entry the game uses.
pub mod t {
    use super::{Pool, Txt};
    pub const PICK_ADVENTURE: Txt<0> = Txt::named("pick_adventure");
    pub const PICK_CLASS: Txt<0> = Txt::named("pick_class");
    pub const PICK_TITLE: Txt<0> = Txt::named("pick_title");
    pub const PICK_NONE: Txt<0> = Txt::named("pick_none");
    pub const PICK_CURSOR: Txt<0> = Txt::named("pick_cursor");
    pub const PICK_NEW: Txt<0> = Txt::named("pick_new");
    pub const PICK_RANK: Txt<0> = Txt::named("pick_rank");
    pub const RARITY: [Txt<0>; 4] = [Txt::named("rarity_common"), Txt::named("rarity_rare"), Txt::named("rarity_epic"), Txt::named("rarity_legend")];
    pub const SHOP_TITLE: Txt<2> = Txt::named("shop_title");
    pub const SHOP_FORGE: Txt<2> = Txt::named("shop_forge");
    pub const SHOP_ROW: Txt<4> = Txt::named("shop_row");
    pub const SPECIAL_PASSIVE: Txt<0> = Txt::named("special_passive");
    pub const SPECIAL_EMPTY: Txt<0> = Txt::named("special_empty");
    pub const DEATH_KEYS: Txt<0> = Txt::named("death_keys");
    pub const SLAIN_BY: Txt<1> = Txt::named("slain_by");
    pub const HERO_SLAYER: Txt<1> = Txt::named("hero_slayer");
    pub const PAL_HELP: Txt<0> = Txt::named("pal_help");
    pub const ROAR_QUOTE: Txt<1> = Txt::named("roar_quote");
    pub const THIEF_STOLE: Txt<2> = Txt::named("thief_stole");
    pub const THIEF_LEARNED: Txt<1> = Txt::named("thief_learned");
    pub const ABILITY: [Txt<0>; 3] = [Txt::named("ability_fast"), Txt::named("ability_rage"), Txt::named("ability_shot")];
    pub const POWER_UP: Txt<0> = Txt::named("power_up");
    pub const POWER_MORE: Txt<1> = Txt::named("power_more");
    pub const CLOCK_DAY: Txt<0> = Txt::named("clock_day");
    pub const CLOCK_NIGHT: Txt<0> = Txt::named("clock_night");
    pub const PANEL_NEXT: Txt<0> = Txt::named("panel_next");
    pub const PANEL_NO_BOSS: Txt<0> = Txt::named("panel_no_boss");
    pub const PANEL_FLUX: Txt<0> = Txt::named("panel_flux");
    pub const PANEL_KILLS: Txt<0> = Txt::named("panel_kills");
    pub const PANEL_HIJACK: Txt<0> = Txt::named("panel_hijack");
    pub const PANEL_NERVE: Txt<0> = Txt::named("panel_nerve");
    pub const PANEL_PANIC: Txt<0> = Txt::named("panel_panic");
    pub const PANEL_GOLD: Txt<0> = Txt::named("panel_gold");
    pub const PANEL_PALS: Txt<0> = Txt::named("panel_pals");
    pub const PANEL_FOOD: Txt<0> = Txt::named("panel_food");
    pub const PANEL_SKY: Txt<0> = Txt::named("panel_sky");
    pub const PANEL_POWERS: Txt<0> = Txt::named("panel_powers");
    pub const PANEL_FOES: Txt<0> = Txt::named("panel_foes");
    pub const PANEL_NET: Txt<0> = Txt::named("panel_net");
    pub const PANEL_CAST: Txt<0> = Txt::named("panel_cast");
    /// cast rows: hero, ally, panic, thief
    pub const CAST: [Txt<0>; 4] = [Txt::named("cast_hero"), Txt::named("cast_ally"), Txt::named("cast_panic"), Txt::named("cast_thief")];
    /// foe tiers: trash, elite, champion, boss
    pub const TIER: [Txt<0>; 4] = [Txt::named("tier_trash"), Txt::named("tier_elite"), Txt::named("tier_champion"), Txt::named("tier_boss")];
    /// damage types in `Hue` order: kin, therm, emp, code
    pub const HUE: [Txt<0>; 4] = [Txt::named("hue_kin"), Txt::named("hue_therm"), Txt::named("hue_emp"), Txt::named("hue_code")];
    pub const STORY: Txt<0> = Txt::named("story");
    pub const LOOK_HELP: Txt<0> = Txt::named("look_help");
    pub const LOOK_HP: Txt<2> = Txt::named("look_hp");
    pub const LOOK_GLITCH: Txt<0> = Txt::named("look_glitch");
    pub const LOOK_HIJACKED: Txt<0> = Txt::named("look_hijacked");
    pub const LOOK_HOSTILE: Txt<0> = Txt::named("look_hostile");
    pub const LOOK_PURGER: Txt<0> = Txt::named("look_purger");
    pub const LOOK_ATE: Txt<1> = Txt::named("look_ate");
    pub const SUPPORT_LABEL: Txt<0> = Txt::named("support_label");
    pub const SUPPORT_PLEA: Txt<0> = Txt::named("support_plea");
    pub const SUPPORT_URL: Txt<0> = Txt::named("support_url");
    pub const LEVEL_LINE: Txt<2> = Txt::named("level_line");
    pub const MODE_END: Txt<0> = Txt::named("mode_end");
    pub const MODE_PLAY: Txt<0> = Txt::named("mode_play");
    pub const MODE_PICK: Txt<0> = Txt::named("mode_pick");
    pub const MODE_LOOK: Txt<0> = Txt::named("mode_look");
    pub const MODE_CMD: Txt<0> = Txt::named("mode_cmd");
    pub const MODE_TREE: Txt<0> = Txt::named("mode_tree");
    pub const MODE_GEAR: Txt<0> = Txt::named("mode_gear");
    pub const MODE_SHOP: Txt<0> = Txt::named("mode_shop");
    pub const MODE_BAGS: Txt<0> = Txt::named("mode_bags");
    pub const MODE_START: Txt<0> = Txt::named("mode_start");
    pub const HINT_MOVE: Txt<0> = Txt::named("hint_move");
    pub const HINT_TURN: Txt<0> = Txt::named("hint_turn");
    pub const HINT_SPECIAL: Txt<0> = Txt::named("hint_special");
    pub const HINT_TAKE: Txt<0> = Txt::named("hint_take");
    pub const HINT_BUY: Txt<0> = Txt::named("hint_buy");
    pub const HINT_NEW_RUN: Txt<0> = Txt::named("hint_new_run");
    pub const HINT_QUIT: Txt<0> = Txt::named("hint_quit");

    pub const STOLE_LINES: Pool = Pool::named("thief_stole");
    pub const LEVEL_TAUNTS: Pool = Pool::named("level_taunt");
    pub const ROAR_CHAMPION: Pool = Pool::named("roar_champion");
    pub const ROAR_MINI: Pool = Pool::named("roar_mini");
    pub const ROAR_SUPER: Pool = Pool::named("roar_super");
    pub const ROAR_UBER: Pool = Pool::named("roar_uber");
    pub const FOE_QUIPS: Pool = Pool::named("foe_quip");
    pub const CHEERS: Pool = Pool::named("cheer");
    pub const DEATH_TAUNTS: Pool = Pool::named("death_taunt");
}
