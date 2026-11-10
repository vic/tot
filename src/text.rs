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

pub const TXTS: [Spec; 235] = [
    line("pick_adventure", &[], "Start screen, step 1: one header line centred under the title art, above the list of adventures."),
    line("pick_class", &[], "Start screen, step 2: one header line centred under the title art, above the hero classes of the picked adventure."),
    // agentic adventure steps of the start screen (drawn in the base adventure)
    line("agentic_name", &[], "Start screen, step 1: the last entry, the adventure a local Claude Code agent writes for you."),
    line("agentic_blurb", &[], "Start screen, step 1: dim line under the agentic entry."),
    line("describe_header", &[], "Agentic step 1: header over the text field where the player describes the adventure they want."),
    line("describe_placeholder", &[], "Agentic step 1: the dim example in the empty text field (also the request when the player types nothing)."),
    line("pick_agent", &[], "Agentic step 2: header over the list of live local Claude Code sessions."),
    line("no_agents", &[], "Agentic step 2: shown when no local Claude Code session runs."),
    line("agent_busy", &[], "Agentic step 2: tag after a session that is working right now."),
    line("asking", &["agent"], "Agentic step 3: the request went out to this session; tell the player to accept it there."),
    line("agent_wait", &[], "Agentic step 3: dim line under asking while the agent writes."),
    line("agent_elapsed", &["time"], "Agentic step 3: how long the agent has been writing (m:ss), under the animation."),
    line("agent_refused", &["error"], "Agentic step 3: the game refused the answer and sent the error back to the agent."),
    line("agent_failed", &["error"], "Agentic step 3: the request could not be sent (the session closed, the socket failed)."),
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
    line("panel_hp", &[], "Stats panel, second row: label of the hero HP bar."),
    line("panel_lv", &[], "Stats panel, third row: label of the hero level, before the xp bar."),
    line("panel_to_go", &[], "Stats panel, third row: after the xp still needed for the next level."),
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
    line("mode_prompt", &[], "Status line, left: tag of the prompt popup (P: speak to the agents)."),
    line("mode_legacy", &[], "Status line, left: tag of the legacy popup (L)."),
    line("mode_start", &[], "Status line, left: tag of the start screen."),
    // key hints (status line rows, after each key)
    line("hint_move", &[], "Key hints: what the move keys do."),
    line("hint_turn", &[], "Key hints: what the turn keys do."),
    line("hint_special", &[], "Key hints: what the number keys (specials) do."),
    line("hint_take", &[], "Key hints, upgrade picker: what the card keys do."),
    line("hint_buy", &[], "Key hints, shop: what the offer keys do."),
    line("hint_new_run", &[], "Key hints, finished run: the new run key."),
    line("hint_quit", &[], "Key hints, finished run: the quit key."),
    // hero pane (bottom right): tabs, slots, tree, bags
    line("tab_armor", &[], "Hero pane, top border: the worn gear tab's name (after its key)."),
    line("tab_skills", &[], "Hero pane, top border: the skill tree tab's name."),
    line("tab_bags", &[], "Hero pane, top border: the bags tab's name."),
    line("slot_armor", &[], "Hero pane, gear tab: label of the armor slot (beside the hero art)."),
    line("slot_ring", &[], "Hero pane, gear tab: label of the ring slot."),
    line("slot_jewel", &[], "Hero pane, gear tab: label of the jewel slot."),
    line("slot_weapon", &[], "Hero pane, gear tab: label of the weapon slot."),
    line("slot_charm", &[], "Hero pane, gear tab: label of the charm slot."),
    line("pane_no_tree", &[], "Hero pane, skills tab: shown when the hero class has no tree."),
    line("tree_title", &["tree", "class", "tag", "learned"], "Hero pane, skills tab, first row: the tree's name, the class, its power tag (tag_*), nodes learned; cut to the pane width (36 chars)."),
    line("tree_detail", &["points", "cost", "skill", "tag"], "Hero pane, skills tab, last row: points left, the cursor node's cost, the skill it grants and its tag; cut to 36 chars."),
    line("bags_used", &["n", "max"], "Hero pane, bags tab: shown when the cursor slot is empty: slots used of the total."),
    line("bags_ore", &[], "Hero pane, bags tab: label before the ore carried."),
    // power tags of skills and trees
    line("tag_magic", &[], "Name of the magic power tag (skill tree title and details)."),
    line("tag_melee", &[], "Name of the melee power tag."),
    line("tag_necro", &[], "Name of the necro power tag."),
    line("tag_thunder", &[], "Name of the thunder power tag."),
    line("tag_speed", &[], "Name of the speed power tag."),
    line("tag_dex", &[], "Name of the dex power tag."),
    // weather (stats panel sky row, weather banner)
    line("weather_clear", &[], "Name of clear weather (stats panel sky row, after its icon)."),
    line("weather_rain", &[], "Name of rain (stats panel sky row)."),
    line("weather_acid_rain", &[], "Name of acid rain (stats panel sky row)."),
    line("weather_snow", &[], "Name of snow (stats panel sky row)."),
    line("weather_heat", &[], "Name of the heat wave."),
    line("weather_wind", &[], "Name of wind (stats panel sky row)."),
    line("weather_full_moon", &[], "Name of the full moon night."),
    line("weather_blood_moon", &[], "Name of the blood moon night."),
    line("icon_clear", &[], "Icon of clear weather before its name (one wide emoji)."),
    line("icon_rain", &[], "Icon of rain (one wide emoji)."),
    line("icon_acid_rain", &[], "Icon of acid rain (one wide emoji)."),
    line("icon_snow", &[], "Icon of snow (one wide emoji)."),
    line("icon_heat", &[], "Icon of the heat wave."),
    line("icon_wind", &[], "Icon of wind (one wide emoji)."),
    line("icon_full_moon", &[], "Icon of the full moon."),
    line("icon_blood_moon", &[], "Icon of the blood moon."),
    big("banner_clear", "Big banner across the arena when the weather turns clear (block font)."),
    big("banner_rain", "Big banner when rain starts."),
    big("banner_acid_rain", "Big banner when acid rain starts."),
    big("banner_snow", "Big banner when snow starts."),
    big("banner_heat", "Big banner when the heat wave starts."),
    big("banner_wind", "Big banner when wind starts."),
    big("banner_full_moon", "Big banner when the full moon rises."),
    big("banner_blood_moon", "Big banner when the blood moon rises."),
    // what killed the hero (death screen slain_by, status message)
    line("cause_contact", &["foe", "tier"], "What killed the hero: a foe's touch, with its family name and tier (tier_*)."),
    line("cause_shot", &["foe"], "What killed the hero: a foe's shot."),
    line("cause_boss", &["foe"], "What killed the hero: a boss attack."),
    line("cause_blast", &[], "What killed the hero: a blast (trap, mine)."),
    line("cause_timeout", &[], "What ended the run: time ran out."),
    // compass and camera
    line("dir_n", &[], "Compass letter of north (map corner compass, camera message)."),
    line("dir_e", &[], "Compass letter of east."),
    line("dir_s", &[], "Compass letter of south."),
    line("dir_w", &[], "Compass letter of west."),
    // status message line (bottom rows, left of the key hints)
    line("msg_autofire_on", &[], "Status message: auto-fire turned on."),
    line("msg_autofire_off", &[], "Status message: auto-fire turned off (say how to aim and hack)."),
    line("msg_autowalk_on", &[], "Status message: auto-walk turned on."),
    line("msg_autowalk_off", &[], "Status message: auto-walk turned off."),
    line("msg_camera", &["dir"], "Status message: the camera turned to face a compass direction (dir_*)."),
    line("msg_quit", &[], "Status message: quitting."),
    line("msg_look", &[], "Status message: look mode on."),
    line("msg_spawn_queued", &[], "Status message: a :spawn command was queued."),
    line("msg_spawn_error", &["error"], "Status message: a :spawn command was refused, with the reason."),
    line("msg_director", &[], "Status message: what the :director command reports."),
    line("msg_later", &["cmd"], "Status message: a known command that is not built yet."),
    line("msg_unknown", &["line"], "Status message: an unknown command, with a list of the known ones."),
    line("msg_learned", &["node"], "Status message: a skill tree node was learned."),
    line("msg_spend_owned", &[], "Status message: the cursor node is learned already."),
    line("msg_spend_points", &["cost", "have"], "Status message: not enough points for the cursor node."),
    line("msg_spend_locked", &["missing"], "Status message: the cursor node needs another node first (its name)."),
    line("msg_no_power_up", &[], "Status message: p was pressed with no power-up waiting."),
    line("msg_applied", &[], "Status message: a typed command was applied."),
    line("msg_rejected", &["reason"], "Status message: a typed command was refused, with the reason."),
    line("msg_win", &[], "Status message: the run was won (keys for a new run and quit)."),
    line("msg_dead", &["cause"], "Status message: the hero died, with what killed it (cause_*)."),
    line("msg_no_browser", &["url"], "Status message: the sponsor link was clicked but no browser was found."),
    line("msg_opening", &["url"], "Status message: the sponsor link is opening."),
    line("msg_open_failed", &["url", "error"], "Status message: the browser did not start."),
    line("whichkey_more", &[], "Which-key popup: the label of a key that opens a further group."),
    line("shop_piece", &["slot", "item", "plus"], "Shop popup, smith and jeweler: the piece the next upgrade or enchant works on (tab picks the slot)."),
    line("shop_upgrade", &["gold", "ore"], "Shop popup, blacksmith: the upgrade key and what the next plus costs."),
    line("shop_enchant", &["gold"], "Shop popup, jeweler: the enchant key and its price."),
    line("shop_sell", &["item", "gold"], "Shop popup, merchant: the sell key, the first bag item and what it fetches."),
    line("deed_sold", &["item", "gold"], "Shop popup, last line: a sale went through."),
    line("deed_upgraded", &["item", "plus"], "Shop popup, last line: the smith raised a piece."),
    line("deed_botched", &["item"], "Shop popup, last line: a deceiving smith took the pay and failed."),
    line("deed_enchanted", &["item", "stat", "value"], "Shop popup, last line: the jeweler enchanted a piece (stat_* name, signed value)."),
    line("deed_no_piece", &[], "Shop popup, last line: no piece worn in that slot, or nothing in the bag to sell."),
    line("deed_poor", &["gold", "ore"], "Shop popup, last line: the hero cannot pay for the service."),
    line("deed_maxed", &[], "Shop popup, last line: the piece is at its last plus."),
    line("stat_max_hp", &[], "Name of the max HP stat (enchantments)."),
    line("stat_regen", &[], "Name of the regeneration stat."),
    line("stat_armour", &[], "Name of the armour stat."),
    line("stat_pickup", &[], "Name of the pickup radius stat."),
    line("stat_speed", &[], "Name of the speed stat."),
    line("stat_damage", &[], "Name of the damage stat."),
    line("stat_haste", &[], "Name of the haste stat."),
    line("stat_area", &[], "Name of the area stat."),
    line("stat_crit", &[], "Name of the critical chance stat."),
    line("legacy_title", &["souls", "runs", "best"], "Legacy popup, top line: souls to spend, runs played, best level reached."),
    line("legacy_row", &["name", "rank", "max", "cost"], "Legacy popup: one boon line: its name (boon_*), rank, last rank, price of the next rank."),
    line("legacy_locked", &["needs", "rank"], "Legacy popup: after a boon line whose branch is still shut: the boon and rank it needs."),
    line("legacy_keys", &[], "Legacy popup, last line: the keys."),
    line("boon_vitality", &[], "Name of the boon that adds max HP."),
    line("boon_might", &[], "Name of the boon that adds damage."),
    line("boon_fortune", &[], "Name of the boon that adds starting gold."),
    line("boon_swiftness", &[], "Name of the boon that adds speed (needs Vitality)."),
    line("boon_kinship", &[], "Name of the boon that strengthens crew pals (needs Might)."),
    line("boon_wisdom", &[], "Name of the boon that adds XP (needs Fortune)."),
    line("boon_insight", &[], "Name of the crown boon that adds skill points at the start (needs Wisdom)."),
    line("boon_vitality_desc", &[], "Legacy popup: under the cursor boon: what one rank of Vitality gives."),
    line("boon_might_desc", &[], "Legacy popup: what one rank of Might gives."),
    line("boon_fortune_desc", &[], "Legacy popup: what one rank of Fortune gives."),
    line("boon_swiftness_desc", &[], "Legacy popup: what one rank of Swiftness gives."),
    line("boon_kinship_desc", &[], "Legacy popup: what one rank of Kinship gives."),
    line("boon_wisdom_desc", &[], "Legacy popup: what one rank of Wisdom gives."),
    line("boon_insight_desc", &[], "Legacy popup: what one rank of Insight gives."),
    line("msg_boon_bought", &["boon", "rank"], "Status message: a boon rank was bought."),
    line("msg_boon_locked", &["boon", "rank"], "Status message: the boon's branch is still shut."),
    line("msg_boon_poor", &["cost", "have"], "Status message: not enough souls."),
    line("msg_boon_maxed", &[], "Status message: the boon is at its last rank."),
    line("msg_souls", &["earned", "total"], "Status message at the end of a run: souls earned and the total."),
    line("msg_legacy_error", &["error"], "Status message: the legacy file could not be read or written."),
    line("prompt_title", &[], "Prompt popup (P), top line."),
    line("prompt_placeholder", &[], "Prompt popup: the dim example in the empty line."),
    line("prompt_to", &["agent"], "Prompt popup: the live local Claude session the line goes to."),
    line("prompt_no_agents", &[], "Prompt popup: shown when no local Claude session runs."),
    line("prompt_keys", &[], "Prompt popup, last line: the keys."),
    line("msg_prompt_sent", &["agent"], "Status message: the line went to an agent."),
    line("msg_prompt_failed", &["error"], "Status message: the line could not be sent."),
    line("chat_you", &[], "Chat log: the speaker name of the player."),
    line("chat_game", &[], "Chat log: the speaker name of the game itself (joins, falls, refusals)."),
    line("crew_title", &["n", "max"], "Crew pane (right, under the stats): its title with members and the cap."),
    line("crew_row", &["name", "class", "hp", "max"], "Crew pane: one member: name, class word, HP and max HP (all from its agent)."),
    line("crew_refused", &["error"], "Chat log, game line: an agent's line the game refused."),
    line("crew_down", &["name"], "Chat log, game line: a crew member fell."),
    line("crew_joined", &["name", "class"], "Chat log, game line: a crew member joined."),
];

pub const POOLS: [Spec; 13] = [
    line("thief_stole", &[], "Over a thief that ate loot: one picked per bite (then its power)."),
    line("level_taunt", &[], "Under the hero on level up: one picked per level."),
    big("roar_champion", "Over a champion that appears: one picked, in the big font."),
    big("roar_mini", "Over a mini boss that appears: one picked, in the big font."),
    big("roar_super", "Over a super boss that appears: one picked, in the big font."),
    line("roar_uber", &[], "Under an uber boss's double-size ROAR: one taunt picked, in quotes."),
    line("foe_quip", &[], "Look tooltip, after what a foe ate: one picked per foe."),
    line("cheer", &[], "Death screen: a short cheer hopping over every third foe of the crowd (keep it within 6 chars)."),
    line("death_taunt", &[], "Death screen: one taunt line picked per run, under slain_by."),
    line("rescued_name", &[], "A pal the hero rescued (or won over) joins the party: one name picked per pal. Up to 16 letters, digits, spaces, - or _ (other chars are dropped); its first letter is its head glyph."),
    line("rescued_class", &[], "A rescued pal's class word (crew pane, chat): one picked per pal. Up to 16 letters, digits, spaces, - or _."),
    line("rescued_say", &[], "Over a rescued pal as it joins the party: one greeting picked per pal (60 chars)."),
    line("wall_lore", &[], "Engraved in the lit faces of the walls as shadow only, never shown as letters: one line per wall block, read left to right then row after row; every non-space char darkens that wall cell, a space leaves it bare. Write lore whose letter shapes and gaps make a pattern of the fantasy (runes, rows of script, circuit lines)."),
];

/// One art piece: rows of width-1 glyphs. `rows` = (fewest, most) rows; `cols` = the width every row must have
/// (`exact`) or must not pass.
pub struct ArtSpec {
    pub id: &'static str,
    pub rows: (usize, usize),
    pub cols: usize,
    pub exact: bool,
    pub doc: &'static str,
}

const fn face(id: &'static str, doc: &'static str) -> ArtSpec {
    ArtSpec { id, rows: (3, 3), cols: 7, exact: true, doc }
}

pub const ARTS: [ArtSpec; 19] = [
    ArtSpec { id: "title", rows: (1, 10), cols: 60, exact: false, doc: "Start screen top: the game title, rows centred as one block from row 2, in the Legendary role colour." },
    // the hero's face in the middle of the bottom bar (Doom style): 3 rows x 7 cols, drawn in the mood's colour
    face("face_calm", "Hero face, calm (nothing near), eyes ahead."),
    face("face_calm_left", "Hero face, calm, eyes glancing left (toward a foe on the left)."),
    face("face_calm_right", "Hero face, calm, eyes glancing right."),
    face("face_fencing", "Hero face, fighting (foes close), eyes ahead."),
    face("face_fencing_left", "Hero face, fighting, eyes left."),
    face("face_fencing_right", "Hero face, fighting, eyes right."),
    face("face_grim", "Hero face, low HP, eyes ahead."),
    face("face_grim_left", "Hero face, low HP, eyes left."),
    face("face_grim_right", "Hero face, low HP, eyes right."),
    face("face_pain", "Hero face, just hit hard."),
    face("face_laugh", "Hero face, a kill streak or a level up."),
    face("face_frozen", "Hero face, frozen by the cold."),
    face("face_dead", "Hero face, dead (death screen and after)."),
    ArtSpec { id: "death_banner", rows: (3, 3), cols: 40, exact: false, doc: "Death screen: the big banner (3 rows), centred over the crowd, shaking, in the alarm colour." },
    ArtSpec { id: "roar_sign", rows: (3, 3), cols: 15, exact: true, doc: "Over an uber boss that appears: its sign (3 rows x 15 cols), drawn at double size." },
    ArtSpec { id: "chest", rows: (2, 2), cols: 3, exact: true, doc: "A treasure chest on the floor (2 rows x 3 cols), glowing gold." },
    ArtSpec { id: "kill_burst", rows: (3, 3), cols: 3, exact: true, doc: "Where a foe dies: 3 frames of 3 cols played in order (one row per frame), over the foe's head row." },
    ArtSpec { id: "agent_loading", rows: (2, 16), cols: 40, exact: false, doc: "Agentic step 3, while the agent writes: an animation, one frame per row, played in a loop (one frame every 150 ms), centred under the asking line in the accent colour." },
];

/// An art piece (see `ARTS`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArtId(pub(crate) usize);

impl ArtId {
    pub const fn named(id: &str) -> ArtId {
        let mut i = 0;
        while i < ARTS.len() {
            if same(ARTS[i].id, id) {
                return ArtId(i);
            }
            i += 1;
        }
        panic!("art id not in the table")
    }
}

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
    pub const AGENTIC_NAME: Txt<0> = Txt::named("agentic_name");
    pub const AGENTIC_BLURB: Txt<0> = Txt::named("agentic_blurb");
    pub const DESCRIBE_HEADER: Txt<0> = Txt::named("describe_header");
    pub const DESCRIBE_PLACEHOLDER: Txt<0> = Txt::named("describe_placeholder");
    pub const PICK_AGENT: Txt<0> = Txt::named("pick_agent");
    pub const NO_AGENTS: Txt<0> = Txt::named("no_agents");
    pub const AGENT_BUSY: Txt<0> = Txt::named("agent_busy");
    pub const ASKING: Txt<1> = Txt::named("asking");
    pub const AGENT_WAIT: Txt<0> = Txt::named("agent_wait");
    pub const AGENT_ELAPSED: Txt<1> = Txt::named("agent_elapsed");
    pub const AGENT_REFUSED: Txt<1> = Txt::named("agent_refused");
    pub const AGENT_FAILED: Txt<1> = Txt::named("agent_failed");
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
    pub const PANEL_HP: Txt<0> = Txt::named("panel_hp");
    pub const PANEL_LV: Txt<0> = Txt::named("panel_lv");
    pub const PANEL_TO_GO: Txt<0> = Txt::named("panel_to_go");
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
    pub const MODE_PROMPT: Txt<0> = Txt::named("mode_prompt");
    pub const MODE_LEGACY: Txt<0> = Txt::named("mode_legacy");
    pub const MODE_START: Txt<0> = Txt::named("mode_start");
    pub const HINT_MOVE: Txt<0> = Txt::named("hint_move");
    pub const HINT_TURN: Txt<0> = Txt::named("hint_turn");
    pub const HINT_SPECIAL: Txt<0> = Txt::named("hint_special");
    pub const HINT_TAKE: Txt<0> = Txt::named("hint_take");
    pub const HINT_BUY: Txt<0> = Txt::named("hint_buy");
    pub const HINT_NEW_RUN: Txt<0> = Txt::named("hint_new_run");
    pub const HINT_QUIT: Txt<0> = Txt::named("hint_quit");
    /// hero pane tabs: armor, skills, bags
    pub const TAB: [Txt<0>; 3] = [Txt::named("tab_armor"), Txt::named("tab_skills"), Txt::named("tab_bags")];
    /// gear slots in `Slot` order: armor, ring, jewel, weapon, charm
    pub const SLOT: [Txt<0>; 5] = [Txt::named("slot_armor"), Txt::named("slot_ring"), Txt::named("slot_jewel"), Txt::named("slot_weapon"), Txt::named("slot_charm")];
    pub const PANE_NO_TREE: Txt<0> = Txt::named("pane_no_tree");
    pub const TREE_TITLE: Txt<4> = Txt::named("tree_title");
    pub const TREE_DETAIL: Txt<4> = Txt::named("tree_detail");
    pub const BAGS_USED: Txt<2> = Txt::named("bags_used");
    pub const BAGS_ORE: Txt<0> = Txt::named("bags_ore");
    /// power tags in `PowerTag` order
    pub const TAG: [Txt<0>; 6] = [Txt::named("tag_magic"), Txt::named("tag_melee"), Txt::named("tag_necro"), Txt::named("tag_thunder"), Txt::named("tag_speed"), Txt::named("tag_dex")];
    /// weather in `Weather` order: names, icons, banners
    pub const WEATHER: [Txt<0>; 8] = [
        Txt::named("weather_clear"), Txt::named("weather_rain"), Txt::named("weather_acid_rain"), Txt::named("weather_snow"),
        Txt::named("weather_heat"), Txt::named("weather_wind"), Txt::named("weather_full_moon"), Txt::named("weather_blood_moon"),
    ];
    pub const ICON: [Txt<0>; 8] = [
        Txt::named("icon_clear"), Txt::named("icon_rain"), Txt::named("icon_acid_rain"), Txt::named("icon_snow"),
        Txt::named("icon_heat"), Txt::named("icon_wind"), Txt::named("icon_full_moon"), Txt::named("icon_blood_moon"),
    ];
    pub const BANNER: [Txt<0>; 8] = [
        Txt::named("banner_clear"), Txt::named("banner_rain"), Txt::named("banner_acid_rain"), Txt::named("banner_snow"),
        Txt::named("banner_heat"), Txt::named("banner_wind"), Txt::named("banner_full_moon"), Txt::named("banner_blood_moon"),
    ];
    pub const CAUSE_CONTACT: Txt<2> = Txt::named("cause_contact");
    pub const CAUSE_SHOT: Txt<1> = Txt::named("cause_shot");
    pub const CAUSE_BOSS: Txt<1> = Txt::named("cause_boss");
    pub const CAUSE_BLAST: Txt<0> = Txt::named("cause_blast");
    pub const CAUSE_TIMEOUT: Txt<0> = Txt::named("cause_timeout");
    /// compass letters, quarter turns from north: N, E, S, W
    pub const DIR: [Txt<0>; 4] = [Txt::named("dir_n"), Txt::named("dir_e"), Txt::named("dir_s"), Txt::named("dir_w")];
    pub const MSG_AUTOFIRE: [Txt<0>; 2] = [Txt::named("msg_autofire_off"), Txt::named("msg_autofire_on")];
    pub const MSG_AUTOWALK: [Txt<0>; 2] = [Txt::named("msg_autowalk_off"), Txt::named("msg_autowalk_on")];
    pub const MSG_CAMERA: Txt<1> = Txt::named("msg_camera");
    pub const MSG_QUIT: Txt<0> = Txt::named("msg_quit");
    pub const MSG_LOOK: Txt<0> = Txt::named("msg_look");
    pub const MSG_SPAWN_QUEUED: Txt<0> = Txt::named("msg_spawn_queued");
    pub const MSG_SPAWN_ERROR: Txt<1> = Txt::named("msg_spawn_error");
    pub const MSG_DIRECTOR: Txt<0> = Txt::named("msg_director");
    pub const MSG_LATER: Txt<1> = Txt::named("msg_later");
    pub const MSG_UNKNOWN: Txt<1> = Txt::named("msg_unknown");
    pub const MSG_LEARNED: Txt<1> = Txt::named("msg_learned");
    pub const MSG_SPEND_OWNED: Txt<0> = Txt::named("msg_spend_owned");
    pub const MSG_SPEND_POINTS: Txt<2> = Txt::named("msg_spend_points");
    pub const MSG_SPEND_LOCKED: Txt<1> = Txt::named("msg_spend_locked");
    pub const MSG_NO_POWER_UP: Txt<0> = Txt::named("msg_no_power_up");
    pub const MSG_APPLIED: Txt<0> = Txt::named("msg_applied");
    pub const MSG_REJECTED: Txt<1> = Txt::named("msg_rejected");
    pub const MSG_WIN: Txt<0> = Txt::named("msg_win");
    pub const MSG_DEAD: Txt<1> = Txt::named("msg_dead");
    pub const MSG_NO_BROWSER: Txt<1> = Txt::named("msg_no_browser");
    pub const MSG_OPENING: Txt<1> = Txt::named("msg_opening");
    pub const MSG_OPEN_FAILED: Txt<2> = Txt::named("msg_open_failed");
    pub const WHICHKEY_MORE: Txt<0> = Txt::named("whichkey_more");
    pub const SHOP_PIECE: Txt<3> = Txt::named("shop_piece");
    pub const SHOP_UPGRADE: Txt<2> = Txt::named("shop_upgrade");
    pub const SHOP_ENCHANT: Txt<1> = Txt::named("shop_enchant");
    pub const SHOP_SELL: Txt<2> = Txt::named("shop_sell");
    pub const DEED_SOLD: Txt<2> = Txt::named("deed_sold");
    pub const DEED_UPGRADED: Txt<2> = Txt::named("deed_upgraded");
    pub const DEED_BOTCHED: Txt<1> = Txt::named("deed_botched");
    pub const DEED_ENCHANTED: Txt<3> = Txt::named("deed_enchanted");
    pub const DEED_NO_PIECE: Txt<0> = Txt::named("deed_no_piece");
    pub const DEED_POOR: Txt<2> = Txt::named("deed_poor");
    pub const DEED_MAXED: Txt<0> = Txt::named("deed_maxed");
    pub const LEGACY_TITLE: Txt<3> = Txt::named("legacy_title");
    pub const LEGACY_ROW: Txt<4> = Txt::named("legacy_row");
    pub const LEGACY_LOCKED: Txt<2> = Txt::named("legacy_locked");
    pub const LEGACY_KEYS: Txt<0> = Txt::named("legacy_keys");
    pub const MSG_BOON_BOUGHT: Txt<2> = Txt::named("msg_boon_bought");
    pub const MSG_BOON_LOCKED: Txt<2> = Txt::named("msg_boon_locked");
    pub const MSG_BOON_POOR: Txt<2> = Txt::named("msg_boon_poor");
    pub const MSG_BOON_MAXED: Txt<0> = Txt::named("msg_boon_maxed");
    pub const MSG_SOULS: Txt<2> = Txt::named("msg_souls");
    pub const MSG_LEGACY_ERROR: Txt<1> = Txt::named("msg_legacy_error");
    pub const PROMPT_TITLE: Txt<0> = Txt::named("prompt_title");
    pub const PROMPT_PLACEHOLDER: Txt<0> = Txt::named("prompt_placeholder");
    pub const PROMPT_TO: Txt<1> = Txt::named("prompt_to");
    pub const PROMPT_NO_AGENTS: Txt<0> = Txt::named("prompt_no_agents");
    pub const PROMPT_KEYS: Txt<0> = Txt::named("prompt_keys");
    pub const MSG_PROMPT_SENT: Txt<1> = Txt::named("msg_prompt_sent");
    pub const MSG_PROMPT_FAILED: Txt<1> = Txt::named("msg_prompt_failed");
    pub const CHAT_YOU: Txt<0> = Txt::named("chat_you");
    pub const CHAT_GAME: Txt<0> = Txt::named("chat_game");
    pub const CREW_TITLE: Txt<2> = Txt::named("crew_title");
    pub const CREW_ROW: Txt<4> = Txt::named("crew_row");
    pub const CREW_REFUSED: Txt<1> = Txt::named("crew_refused");
    pub const CREW_DOWN: Txt<1> = Txt::named("crew_down");
    pub const CREW_JOINED: Txt<2> = Txt::named("crew_joined");
    /// stat names in `Stat` order: max_hp, regen, armour, pickup, speed, damage, haste, area, crit
    pub const STAT: [Txt<0>; 9] = [Txt::named("stat_max_hp"), Txt::named("stat_regen"), Txt::named("stat_armour"), Txt::named("stat_pickup"), Txt::named("stat_speed"), Txt::named("stat_damage"), Txt::named("stat_haste"), Txt::named("stat_area"), Txt::named("stat_crit")];
    /// boon names and what one rank gives, in `Boon::ALL` order
    pub const BOON: [Txt<0>; 7] = [Txt::named("boon_vitality"), Txt::named("boon_might"), Txt::named("boon_fortune"), Txt::named("boon_swiftness"), Txt::named("boon_kinship"), Txt::named("boon_wisdom"), Txt::named("boon_insight")];
    pub const BOON_DESC: [Txt<0>; 7] = [Txt::named("boon_vitality_desc"), Txt::named("boon_might_desc"), Txt::named("boon_fortune_desc"), Txt::named("boon_swiftness_desc"), Txt::named("boon_kinship_desc"), Txt::named("boon_wisdom_desc"), Txt::named("boon_insight_desc")];

    pub const STOLE_LINES: Pool = Pool::named("thief_stole");
    pub const LEVEL_TAUNTS: Pool = Pool::named("level_taunt");
    pub const ROAR_CHAMPION: Pool = Pool::named("roar_champion");
    pub const ROAR_MINI: Pool = Pool::named("roar_mini");
    pub const ROAR_SUPER: Pool = Pool::named("roar_super");
    pub const ROAR_UBER: Pool = Pool::named("roar_uber");
    pub const FOE_QUIPS: Pool = Pool::named("foe_quip");
    pub const CHEERS: Pool = Pool::named("cheer");
    pub const DEATH_TAUNTS: Pool = Pool::named("death_taunt");
    pub const RESCUED_NAMES: Pool = Pool::named("rescued_name");
    pub const RESCUED_CLASSES: Pool = Pool::named("rescued_class");
    pub const RESCUED_SAYS: Pool = Pool::named("rescued_say");
    pub const WALL_LORE: Pool = Pool::named("wall_lore");
}

/// Named art handles.
pub mod a {
    use super::ArtId;
    pub const TITLE: ArtId = ArtId::named("title");
    /// faces by mood; the moods with eyes have [ahead, left, right]
    pub const FACE_CALM: [ArtId; 3] = [ArtId::named("face_calm"), ArtId::named("face_calm_left"), ArtId::named("face_calm_right")];
    pub const FACE_FENCING: [ArtId; 3] = [ArtId::named("face_fencing"), ArtId::named("face_fencing_left"), ArtId::named("face_fencing_right")];
    pub const FACE_GRIM: [ArtId; 3] = [ArtId::named("face_grim"), ArtId::named("face_grim_left"), ArtId::named("face_grim_right")];
    pub const FACE_PAIN: ArtId = ArtId::named("face_pain");
    pub const FACE_LAUGH: ArtId = ArtId::named("face_laugh");
    pub const FACE_FROZEN: ArtId = ArtId::named("face_frozen");
    pub const FACE_DEAD: ArtId = ArtId::named("face_dead");
    pub const DEATH_BANNER: ArtId = ArtId::named("death_banner");
    pub const ROAR_SIGN: ArtId = ArtId::named("roar_sign");
    pub const CHEST: ArtId = ArtId::named("chest");
    pub const KILL_BURST: ArtId = ArtId::named("kill_burst");
    pub const AGENT_LOADING: ArtId = ArtId::named("agent_loading");
}

/// One sprite glyph: a single width-1 cell glyph the world draws (`glyph` records).
pub struct GlyphSpec {
    pub id: &'static str,
    pub doc: &'static str,
}

const fn gl(id: &'static str, doc: &'static str) -> GlyphSpec {
    GlyphSpec { id, doc }
}

pub const GLYPHS: [GlyphSpec; 31] = [
    gl("hero_head", "The hero's head in the arena (top of its body column), on the ring colour."),
    gl("hero_armor", "The hero's torso when it wears armor, in the armor's rarity colour."),
    gl("ore", "Ore lying on the floor (the blacksmith forges with it)."),
    gl("food", "Food lying on the floor (fills the hunger bar)."),
    gl("magnet", "The pickup that pulls every xp gem to the hero, glowing."),
    gl("xp_gem", "An xp gem on the floor."),
    gl("foe_shot", "A foe's projectile in flight, in its family colour."),
    gl("gold_pile", "A pile of gold on the floor, glowing gold."),
    gl("npc_pal", "A pal to rescue (arena and map)."),
    gl("npc_merchant", "A merchant (arena and map)."),
    gl("npc_blacksmith", "A blacksmith (arena and map)."),
    gl("npc_jeweler", "A jeweler who sells trinkets and enchants worn pieces (arena and map)."),
    gl("npc_neutral", "A neutral bystander (arena and map)."),
    gl("map_hero", "The hero on the map (top left box)."),
    gl("map_chest", "A treasure on the map."),
    gl("map_boss", "A boss (or where one will rise) on the map."),
    gl("map_goblin", "A treasure goblin on the map."),
    gl("map_rock", "Explored rock on the map."),
    gl("map_floor", "Explored floor on the map."),
    gl("rain", "A rain streak falling over the arena."),
    gl("acid_rain", "An acid rain streak."),
    gl("snow", "A snowflake drifting over the arena."),
    gl("wind", "A gust streak blowing over the arena."),
    gl("star", "A star: the middle of the map compass and the points of the level-up ring."),
    gl("cheer_hop", "Death screen: the mark over each foe of the cheering crowd."),
    gl("call_left", "A pal's call for help: the sound waves on its left side."),
    gl("call_right", "A pal's call for help: the sound waves on its right side."),
    gl("chest_tell", "Over a chest that is a trap (a mimic): the warning mark one row above it."),
    gl("trap", "A floor trap, in its state colour."),
    gl("blast", "A blast going off (traps, mines)."),
    gl("goblin", "A treasure goblin running in the arena."),
];

/// A sprite glyph (see `GLYPHS`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GlyphId(pub(crate) usize);

impl GlyphId {
    pub const fn named(id: &str) -> GlyphId {
        let mut i = 0;
        while i < GLYPHS.len() {
            if same(GLYPHS[i].id, id) {
                return GlyphId(i);
            }
            i += 1;
        }
        panic!("glyph id not in the table")
    }
}

/// Named sprite glyph handles.
pub mod g {
    use super::GlyphId;
    pub const HERO_HEAD: GlyphId = GlyphId::named("hero_head");
    pub const HERO_ARMOR: GlyphId = GlyphId::named("hero_armor");
    pub const ORE: GlyphId = GlyphId::named("ore");
    pub const FOOD: GlyphId = GlyphId::named("food");
    pub const MAGNET: GlyphId = GlyphId::named("magnet");
    pub const XP_GEM: GlyphId = GlyphId::named("xp_gem");
    pub const FOE_SHOT: GlyphId = GlyphId::named("foe_shot");
    pub const GOLD_PILE: GlyphId = GlyphId::named("gold_pile");
    pub const NPC_PAL: GlyphId = GlyphId::named("npc_pal");
    pub const NPC_MERCHANT: GlyphId = GlyphId::named("npc_merchant");
    pub const NPC_BLACKSMITH: GlyphId = GlyphId::named("npc_blacksmith");
    pub const NPC_JEWELER: GlyphId = GlyphId::named("npc_jeweler");
    pub const NPC_NEUTRAL: GlyphId = GlyphId::named("npc_neutral");
    pub const MAP_HERO: GlyphId = GlyphId::named("map_hero");
    pub const MAP_CHEST: GlyphId = GlyphId::named("map_chest");
    pub const MAP_BOSS: GlyphId = GlyphId::named("map_boss");
    pub const MAP_GOBLIN: GlyphId = GlyphId::named("map_goblin");
    pub const MAP_ROCK: GlyphId = GlyphId::named("map_rock");
    pub const MAP_FLOOR: GlyphId = GlyphId::named("map_floor");
    pub const RAIN: GlyphId = GlyphId::named("rain");
    pub const ACID_RAIN: GlyphId = GlyphId::named("acid_rain");
    pub const SNOW: GlyphId = GlyphId::named("snow");
    pub const WIND: GlyphId = GlyphId::named("wind");
    pub const STAR: GlyphId = GlyphId::named("star");
    pub const CHEER_HOP: GlyphId = GlyphId::named("cheer_hop");
    pub const CALL_LEFT: GlyphId = GlyphId::named("call_left");
    pub const CALL_RIGHT: GlyphId = GlyphId::named("call_right");
    pub const CHEST_TELL: GlyphId = GlyphId::named("chest_tell");
    pub const TRAP: GlyphId = GlyphId::named("trap");
    pub const BLAST: GlyphId = GlyphId::named("blast");
    pub const GOBLIN: GlyphId = GlyphId::named("goblin");
}
