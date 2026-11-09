//! Typed content (design sec 3 CONTENT, sec 11 knobs [E]). Every number the sim uses comes from
//! `data/*.jsonl`; seconds and metres are converted once here to ticks and milli-metres.
use crate::input::{Action, Key, Mode};
use crate::json::{Rec, records};
use crate::{Error, Result};
use std::num::{NonZeroU16, NonZeroU32, NonZeroU8};
use std::path::Path;

// ---------- small typed values ----------

/// Sim rate: closed set (design sec 18 item 3: OPEN, default 30).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hz {
    H30 = 30,
    H60 = 60,
}

impl Hz {
    pub fn get(self) -> u32 {
        self as u32
    }
    pub fn ticks(self, s: f64) -> u32 {
        (s * f64::from(self.get())).round() as u32
    }
    /// metres/second -> milli-metres/tick
    pub fn mmpt(self, mps: f64) -> i32 {
        (mps * 1000.0 / f64::from(self.get())).round() as i32
    }
}

pub fn mm(m: f64) -> i32 {
    (m * 1000.0).round() as i32
}
/// HP -> milli-HP (sim stores all HP as integer milli-HP).
pub fn mhp(hp: f64) -> i32 {
    (hp * 1000.0).round() as i32
}
pub fn permille(f: f64) -> u32 {
    (f * 1000.0).round() as u32
}

/// One terminal cell glyph: single codepoint, width 1 (design sec 5.3). Built only by `parse`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Glyph(char);

impl Glyph {
    pub const fn ch(self) -> char {
        self.0
    }
    pub const SPACE: Glyph = Glyph(' ');
    /// The hidden right half of a wide glyph: never drawn (the terminal paints the wide glyph over two cells).
    pub const CONT: Glyph = Glyph('\u{1}');
    /// Always-wide emoji the frame may draw (two cells each); used sparingly, where no ASCII art fits.
    const WIDE: [char; 9] = ['🌞', '☔', '🧪', '⛄', '🔥', '💨', '🌕', '🩸', '🌙'];
    pub fn wide(c: char) -> Option<Glyph> {
        Self::WIDE.contains(&c).then_some(Glyph(c))
    }
    pub fn is_wide(self) -> bool {
        Self::WIDE.contains(&self.0)
    }
    pub fn of(c: char) -> Option<Glyph> {
        let u = u32::from(c);
        let ok = (0x20..0x7f).contains(&u)
            || u == 0xb7
            || u == 0x2022
            || (0x2500..=0x25ff).contains(&u)
            || u == 0x2726
            || (0xff61..=0xff9f).contains(&u);
        ok.then_some(Glyph(c))
    }
    /// Text to glyphs; non-cell chars become '?'.
    pub fn text(s: &str) -> impl Iterator<Item = Glyph> + '_ {
        s.chars().map(|c| Glyph::of(c).or_else(|| Glyph::wide(c)).unwrap_or(Glyph('?')))
    }
}

fn glyph(r: &Rec, k: &str) -> Result<Glyph> {
    let s = r.text(k)?;
    let mut it = s.chars();
    match (it.next().and_then(Glyph::of), it.next()) {
        (Some(g), None) => Ok(g),
        _ => Err(r.err(k, "need exactly one width-1 glyph")),
    }
}

/// Fixed-length integer list, each in [lo, hi].
fn ints<const N: usize>(r: &Rec, k: &str, lo: i64, hi: i64) -> Result<[u8; N]> {
    let v: Option<Vec<u8>> = r.list(k)?.iter().map(|x| x.as_i64().filter(|x| (lo..=hi).contains(x)).and_then(|x| u8::try_from(x).ok())).collect();
    v.and_then(|v| v.try_into().ok()).ok_or_else(|| r.err(k, format!("need {N} integers in [{lo}, {hi}]")))
}

fn glyphs(r: &Rec, k: &str) -> Result<Vec<Glyph>> {
    let v: Option<Vec<Glyph>> = r.text(k)?.chars().map(Glyph::of).collect();
    v.filter(|v| !v.is_empty()).ok_or_else(|| r.err(k, "need >= 1 width-1 glyphs"))
}

/// Printable ASCII label, 1..=max chars.
fn label(r: &Rec, k: &str, max: usize) -> Result<String> {
    let s = r.text(k)?;
    if (1..=max).contains(&s.len()) && s.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        Ok(s.to_string())
    } else {
        Err(r.err(k, format!("need 1..={max} printable ASCII chars")))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// `#rrggbb` (either hex case).
fn hex_rgb(s: &str) -> Option<Rgb> {
    let hex = s.strip_prefix('#').filter(|h| h.len() == 6)?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some(Rgb(byte(0)?, byte(2)?, byte(4)?))
}

fn rgb(r: &Rec, k: &str) -> Result<Rgb> {
    hex_rgb(r.text(k)?).ok_or_else(|| r.err(k, "need #rrggbb"))
}

/// Damage type = hue (design sec 11.3, 4 types, closed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hue {
    Kin,
    Therm,
    Emp,
    Code,
}
const HUES: [(&str, Hue); 4] = [("kin", Hue::Kin), ("therm", Hue::Therm), ("emp", Hue::Emp), ("code", Hue::Code)];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FamId(u8);
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkillId(u8);

impl FamId {
    pub fn ix(self) -> usize {
        self.0.into()
    }
}
impl SkillId {
    pub fn ix(self) -> usize {
        self.0.into()
    }
}

// ---------- options (the 5 OPEN rulings, design sec 18) ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rewind {
    /// FP1: rewind not built (design sec 17.3). Only legal value.
    Off,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Terminal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotNumbers {
    FetchedOnly,
}

pub struct Options {
    pub pick_pauses: bool,
    pub hz: Hz,
    pub rewind: Rewind,
    pub rewind_charges: u8,
    pub backend: Backend,
    pub hot_numbers: HotNumbers,
    pub whichkey_ms: u64,
    pub autofire: bool,
}

fn options(r: &Rec) -> Result<Options> {
    r.allow(&["t", "pick_pauses", "tick_hz", "rewind", "rewind_charges", "backend", "hot_numbers", "whichkey_ms", "autofire"])?;
    Ok(Options {
        pick_pauses: r.flag("pick_pauses")?,
        hz: match r.int("tick_hz", 30, 60)? {
            30 => Hz::H30,
            60 => Hz::H60,
            _ => return Err(r.err("tick_hz", "only 30 or 60")),
        },
        rewind: r.pick("rewind", &[("off", Rewind::Off)]).map_err(|_| r.err("rewind", "only \"off\": rewind is not in FP1"))?,
        rewind_charges: r.int("rewind_charges", 0, 9)? as u8,
        backend: r.pick("backend", &[("terminal", Backend::Terminal)]).map_err(|_| r.err("backend", "only \"terminal\" in FP1"))?,
        hot_numbers: r.pick("hot_numbers", &[("F-only", HotNumbers::FetchedOnly)])?,
        whichkey_ms: r.int("whichkey_ms", 0, 2000)? as u64,
        autofire: r.flag("autofire")?,
    })
}

// ---------- tuning ----------

pub struct PlayerDef {
    pub hp: i32,
    pub regen: i32, // milli-HP per tick
    pub armour: i32,
    pub speed: i32,
    pub radius: i32,
    pub pickup: i32,
    pub crit_chance: u32,
    pub crit_bonus: u32,
    pub flux: i32,
    pub flux_regen: i32, // milli-flux per tick
    pub flux_per_kill: i32,
    pub dash: i32, // mm per dash tick
    pub dash_ticks: u16,
    pub iframes: u16,
    pub dodge_cd: u16,
    pub contact_cd: u8,
    pub skills: Vec<SkillId>,
    pub skill_slots: usize,
    pub trait_slots: usize,
}

pub struct SpawnDef {
    pub trash_hp: i32,
    /// permille multiplier per second of run time (m(t) table, design sec 11.4)
    pub hp_mult: Vec<u32>,
    /// trash spawn rate, milli-spawns per second, per second of run time
    pub rate: Vec<u32>,
    pub dmg_mult: Vec<u32>,
    pub dmg0: i32,
    pub live_cap: usize,
    pub ring_min: i32,
    pub ring_max: i32,
    pub despawn: i32,
    pub cone_cos: i32, // permille
    pub elite_hp: u32,
    pub elite_dmg: u32,
    pub champion_hp: u32,
    pub champion_dmg: u32,
    pub swift: u32, // elite speed permille
    pub xp_trash: u16,
    pub xp_elite: u16,
    pub xp_champion: u16,
    pub pack_bank: u32,
    pub pack_min: u16,
    pub separation: i32, // permille push
    pub timeout: u32,
    pub purge_every: u16,
    pub purge_lead: u16,
    pub purge_r: i32,
    /// mob speed multiplier per second of run time, permille (trash speeds up; capped below player)
    pub speed_mult: Vec<u32>,
    pub speed_cap: i32,
}

/// How the director reacts to the hero levelling (tot-751.9). All permille; "excess" = levels above par.
#[derive(Clone, Copy, Debug)]
pub struct ScaleDef {
    /// par level (permille levels) gained per minute of run time
    par_per_min: u32,
    /// extra pacing-clock speed per excess level, and its cap
    speedup: u32,
    speedup_cap: u32,
    /// miniboss cadence speed-up per level above 1, and its cap
    mini: u32,
    mini_cap: u32,
    /// horde size bonus per level above 1, and its cap
    horde: u32,
    horde_cap: u32,
}

impl ScaleDef {
    /// Pacing-clock rate (1000 = normal): faster the more the hero out-levels par at run tick `t`.
    pub fn phase_rate(&self, hz: Hz, t: u32, level: u16) -> u32 {
        let par = 1000 + u64::from(self.par_per_min) * u64::from(t) / (60 * u64::from(hz.get()));
        let excess = (u64::from(level) * 1000).saturating_sub(par);
        1000 + (excess * u64::from(self.speedup) / 1000).min(u64::from(self.speedup_cap)) as u32
    }

    /// Miniboss spacing at `level`: base cadence shrunk by the level bonus (never under 1 tick).
    pub fn mini_every(&self, base: NonZeroU32, level: u16) -> NonZeroU32 {
        let bonus = (u64::from(self.mini) * u64::from(level.saturating_sub(1))).min(u64::from(self.mini_cap));
        NonZeroU32::new((u64::from(base.get()) * 1000 / (1000 + bonus)) as u32).unwrap_or(NonZeroU32::MIN)
    }

    /// Horde size at `level`, kept inside the live cap.
    pub fn horde_n(&self, n: NonZeroU16, level: u16, live_cap: usize) -> NonZeroU16 {
        let bonus = (u64::from(self.horde) * u64::from(level.saturating_sub(1))).min(u64::from(self.horde_cap));
        let scaled = (u64::from(n.get()) * (1000 + bonus) / 1000).min(live_cap as u64).min(u64::from(u16::MAX));
        NonZeroU16::new(scaled as u16).unwrap_or(n)
    }
}

fn scaling(r: &Rec) -> Result<ScaleDef> {
    r.allow(&["t", "par_levels_per_min", "speedup_per_level", "speedup_cap", "mini_per_level", "mini_cap", "horde_per_level", "horde_cap"])?;
    let pm = |k: &str, hi: f64| r.num(k, 0.0, hi).map(permille);
    Ok(ScaleDef {
        par_per_min: pm("par_levels_per_min", 20.0)?,
        speedup: pm("speedup_per_level", 5.0)?,
        speedup_cap: pm("speedup_cap", 4.0)?,
        mini: pm("mini_per_level", 1.0)?,
        mini_cap: pm("mini_cap", 4.0)?,
        horde: pm("horde_per_level", 1.0)?,
        horde_cap: pm("horde_cap", 4.0)?,
    })
}

#[derive(Clone, Copy)]
pub struct PacePhase {
    pub len: u32,
    pub from: u32,
    pub to: u32,
}

pub struct PicksDef {
    pub cards: usize,
    pub rarity: [u32; 4],
    pub gain: [u32; 4],
    pub pity_every: u16,
    pub owned_bias: u32,
    pub tag_bonus: u32,
    pub max_rank: u8,
}

pub struct HijackDef {
    pub cap_base: u16,
    pub cap_per_rank: u16,
    pub burst_hp: u32,
    pub burst_r: i32,
    pub dmg_mult: u32,
    pub enemy_weight: u32,
    pub elite_ice: u8,
    pub champion_ice: u8,
    pub elite_s: u16,
    pub champion_s: u16,
    pub glitch_decay: u16,
    pub seek: i32,
}

pub struct ViewDef {
    pub fog_start: i32,
    pub fog_end: i32,
    pub fog_max: u32,
    pub fog_bands: u32,
    pub floor_cell: i32,
    pub min_cols: u16,
    pub min_rows: u16,
    pub crowd_n: u8,
    pub crowd_r: i32,
    pub blink: u32, // ticks per blink half-period
    pub view_r: i32,
    /// extrusion height in rows: [pillar, rubble]; 0 = flat
    pub block_rows: [u8; 2],
    /// standing stack rows above the shadow cell: [player, trash, elite]; 0 = flat (glyph on the foot, no shadow)
    pub actor_rows: [u8; 3],
    /// face brightness, percent floor -> text: [top, lit side, dark side]
    pub face_pct: [u8; 3],
    /// half-width (cols) of the cutaway window around the player; 0 = no cutaway
    pub cut_cols: u8,
    /// render frame rate cap (sim stays at tick_hz; render interpolates between the last 2 ticks)
    pub fps: u16,
    /// camera follow time constant (ms); 0 = snap
    pub follow_ms: u32,
    /// camera deadzone radius (mm)
    pub deadzone: i32,
    /// q/e turn animation (ms); 0 = instant
    pub turn_ms: u32,
}

/// Arena generator ranges (floorgen.rs draws inside them per run seed).
pub struct ArenaDef {
    pub glyphs: (u32, u32),
    pub density: (u32, u32),
    pub fam_mult: (u32, u32),
    pub decor: (u32, u32),
    pub decor_cell: i32,
    pub decor_r: (i32, i32),
}

pub struct Bars {
    pub kiter_tag_win: (f64, f64),
    pub kiter_random_win: (f64, f64),
    pub oracle_win_min: f64,
    pub none_median_max_s: f64,
    pub still_median_max_s: f64,
    pub n2_earlier_min: f64,
    pub skill_share_max: f64,
    pub degenerate_dmg_share: f64,
    pub meter_ratio_min: f64,
}

pub struct Palette {
    pub bg: Rgb,
    pub fog: Rgb,
    pub floor: Rgb,
    pub player: Rgb,
    pub ring: Rgb,
    pub alarm: Rgb,
    pub text: Rgb,
    pub dim: Rgb,
    pub xp: Rgb,
    pub chest: Rgb,
    pub hud_bg: Rgb,
    pub hue: [Rgb; 4],
    pub floor_glyphs: Vec<Glyph>,
    pub density: [Glyph; 3],
    /// block faces: top, lit side, dark side
    pub face: [Glyph; 3],
    pub shadow: Glyph,
    pub body: Glyph,
    /// base24 scheme id named by the record (file `schemes/<id>.yaml`), applied by `Content::load`
    pub scheme: Option<SchemeId>,
    roles: RoleTable,
}

impl Palette {
    /// The one colour per role every widget reads (design: tot-z6a.1).
    pub fn role(&self, r: Role) -> Rgb {
        self.roles.get(r)
    }

    pub fn of(&self, h: Hue) -> Rgb {
        self.hue[h as usize]
    }
}

// ---------- enemies ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MobRole {
    Melee,
    Ranged { range: i32, shot_v: i32, every: u16, dmg: u32 },
    Aura { radius: i32, ice: u8 },
    /// runs from the player inside `flee` mm, dives for loot, eats it (heals), steals it from the player
    Thief { flee: i32 },
    /// bursts for `dmg` permille refHP inside `radius` mm on death or contact
    Exploder { radius: i32, dmg: u32 },
    /// below `below` permille HP its speed is multiplied by `mult` permille
    Berserker { below: u32, mult: u32 },
    /// ranged shot that freezes (slows) the player for `freeze` ticks
    FrostCaster { range: i32, shot_v: i32, every: u16, dmg: u32, freeze: u16 },
    /// takes `reduce` permille less damage
    Shield { reduce: u32 },
    /// spawns in packs of `pack`
    Swarm { pack: NonZeroU8 },
}

pub struct Mob {
    pub role: MobRole,
    pub hp: u32, // permille of trash HP(t)
    pub ice: u8,
    pub hijack: u16,
    pub cost: u32, // milli-TP
    pub weight: u32,
    pub unlock: u32,
}

/// Bosses are hack-immune by construction (no ICE field, design sec 10.3).
pub enum FamKind {
    Mob(Mob),
    Boss { hp: i32 },
}

pub struct Fam {
    pub key: String,
    pub name: String,
    pub glyph: Glyph,
    pub hue: Hue,
    pub speed: i32,
    pub dmg: u32,
    pub radius: i32,
    pub kind: FamKind,
}

// ---------- skills ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkillKind {
    Bolt,
    Hack,
    Orbit,
    Pulse,
    Chain,
    Mine,
    Might,
    Haste,
    Plating,
    Magnet,
    Regen,
}

impl SkillKind {
    pub fn is_trait(self) -> bool {
        matches!(self, SkillKind::Might | SkillKind::Haste | SkillKind::Plating | SkillKind::Magnet | SkillKind::Regen)
    }

    /// Has a number-key special (a boosted shot); traits and orbit blades are passive.
    pub fn has_special(self) -> bool {
        matches!(self, SkillKind::Bolt | SkillKind::Hack | SkillKind::Pulse | SkillKind::Chain | SkillKind::Mine)
    }
}

/// Auto-aim mode per skill (design sec 6.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Nearest,
    Densest,
    SelfCentered,
}

/// Tag bitset over a closed tag list (design sec 12.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Tags(u16);
const TAGS: [&str; 10] = ["bolt", "area", "chain", "aura", "summon", "hijack", "on_hit", "on_kill", "on_hack", "on_dodge"];

impl Tags {
    pub fn overlap(self, o: Tags) -> u32 {
        (self.0 & o.0).count_ones()
    }
    pub fn or(self, o: Tags) -> Tags {
        Tags(self.0 | o.0)
    }
}

#[derive(Default)]
pub struct SkillBase {
    pub dmg: i32,
    pub cd: u16,
    pub count: u8,
    pub pierce: u8,
    pub speed: i32,
    pub range: i32,
    pub area: i32,
    pub glitch: u8,
    pub flux: i32,
    pub arc_cos: i32,
    pub duration: u16,
    pub value: u32,
}

#[derive(Default)]
pub struct PerRank {
    pub dmg: u32,
    pub haste: u32,
    pub area: u32,
    pub value: u32,
}

#[derive(Default)]
pub struct R5 {
    pub count: u8,
    pub pierce: u8,
    pub glitch: u8,
}

pub struct Skill {
    pub key: String,
    pub name: String,
    pub hue: Hue,
    pub kind: SkillKind,
    pub target: Target,
    pub tags: Tags,
    pub base: SkillBase,
    pub per: PerRank,
    pub r5: R5,
    pub desc: String,
}

// ---------- bosses ----------

/// Telegraph lead. Fairness by construction (design sec 8.4): >= 0.4 s always,
/// >= 0.5 s when the hit can deal >= 15% refHP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lead(u16);

impl Lead {
    pub fn ticks(self) -> u16 {
        self.0
    }
    fn parse(r: &Rec, hz: Hz, dmg: u32) -> Result<Lead> {
        let s = r.num("lead_s", 0.0, 5.0)?;
        let min = if dmg >= 150 { 0.5 } else { 0.4 };
        if s < min {
            return Err(r.err("lead_s", format!("{s} s < {min} s fairness floor for a {} permille refHP hit", dmg)));
        }
        Ok(Lead(hz.ticks(s) as u16))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeDef {
    Band { half: i32 },
    Cross { half: i32 },
    Lattice { pitch: i32, half: i32 },
    Circle { r: i32 },
    Fan { n: u8, r: i32, spread: i32 },
    Arena,
}

#[derive(Clone, Copy, Debug)]
pub enum AttackKind {
    Area { shape: ShapeDef, dmg: u32 },
    Spawn { fam: FamId, n: u8 },
}

#[derive(Clone, Copy, Debug)]
pub struct Attack {
    pub every: u16,
    pub lead: Lead,
    pub what: AttackKind,
}

pub struct BossPhase {
    pub until: u32, // hp permille at which the next phase starts
    pub attacks: Vec<Attack>,
}

pub struct BossDef {
    pub fam: FamId,
    pub ref_hp: i32,
    pub cloud: Vec<Glyph>,
    pub w: u8,
    pub h: u8,
    pub phases: Vec<BossPhase>,
    pub enrage: u32,
    pub enrage_cadence: u32,
    pub hardcap: u32,
    pub hard: Attack,
}

// ---------- waves ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Formation {
    Scatter,
    Ring,
    Wall,
    Spiral,
}

#[derive(Clone, Debug)]
pub enum WaveEv {
    Horde { at: u32, pattern: Formation, n: NonZeroU16, fam: FamId },
    Boss { at: u32, fam: FamId, last: bool },
    Thin { at: u32, mult: u32 },
    Narrate { at: u32, text: String },
}

impl WaveEv {
    pub fn at(&self) -> u32 {
        match self {
            WaveEv::Horde { at, .. } | WaveEv::Boss { at, .. } | WaveEv::Thin { at, .. } | WaveEv::Narrate { at, .. } => *at,
        }
    }
}

/// Champion-grade named family: its own glyph, hue and size (the `Fam`), plus a telegraphed attack mix.
/// The family is a pool-excluded mob (weight 0) so only the miniboss cadence spawns it.
pub struct MiniDef {
    pub fam: FamId,
    pub attacks: Vec<Attack>,
}

/// Smallest body of a miniboss: must read bigger than trash on screen.
const MINI_MIN_RADIUS_M: f64 = 0.6;

pub struct WavePlan {
    evs: Vec<WaveEv>,
    elite: Cadence,
    champion: Cadence,
    mini: Option<MiniPlan>,
}

/// Miniboss records with their spawn cadence. At least one record by construction (`head`), so
/// round-robin needs no emptiness guard.
pub struct MiniPlan {
    pub cadence: Cadence,
    head: MiniDef,
    tail: Vec<MiniDef>,
}

impl MiniPlan {
    pub fn len(&self) -> usize {
        1 + self.tail.len()
    }

    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = &MiniDef> {
        std::iter::once(&self.head).chain(&self.tail)
    }

    /// `k`-th miniboss of the run: round-robin over the records.
    pub fn nth(&self, k: u32) -> &MiniDef {
        match k as usize % self.len() {
            0 => &self.head,
            i => self.tail.get(i - 1).unwrap_or(&self.head),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Cadence {
    pub from: u32,
    pub every: NonZeroU32,
}

pub struct Bind {
    pub mode: Mode,
    pub keys: Vec<Key>,
    pub action: Action,
    pub label: String,
}

pub struct Group {
    pub mode: Mode,
    pub keys: Vec<Key>,
    pub label: String,
}

// ---------- base24 schemes + role colours (tot-z6a.1) ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    Dark,
    Light,
}

/// A base24 scheme: exactly 24 slots base00..base17 (slot index = the hex number).
#[derive(Debug)]
pub struct Scheme {
    pub variant: Variant,
    slots: [Rgb; 24],
}

fn scheme_err(file: &str, line: usize, path: &str, msg: impl Into<String>) -> Error {
    Error::Data { file: file.into(), line, path: path.into(), msg: msg.into() }
}

/// YAML scalar of the line subset the schemes use: `"quoted"` or bare, then an optional `# comment`.
fn scalar(v: &str) -> &str {
    let v = v.trim();
    match v.strip_prefix('"') {
        Some(q) => q.split('"').next().unwrap_or(""),
        None => v.split(" #").next().unwrap_or("").trim(),
    }
}

impl Scheme {
    /// Line parser (no YAML crate): top-level `name:` / `variant:`, then `  baseXX: "#rrggbb"` under `palette:`.
    pub fn parse(file: &str, text: &str) -> Result<Scheme> {
        let (mut name, mut variant) = (None, None);
        let mut slots: [Option<Rgb>; 24] = [None; 24];
        let mut in_palette = false;
        for (i, l) in text.lines().enumerate() {
            let n = i + 1;
            if l.trim().is_empty() || l.trim_start().starts_with('#') || l.trim_end() == "---" {
                continue;
            }
            let Some((k, v)) = l.split_once(':') else { return Err(scheme_err(file, n, "", "expected `key: value`")) };
            if !l.starts_with(' ') {
                in_palette = k == "palette";
                match k {
                    "name" => name = Some(scalar(v).to_string()),
                    "variant" => {
                        variant = Some(match scalar(v) {
                            "dark" => Variant::Dark,
                            "light" => Variant::Light,
                            o => return Err(scheme_err(file, n, ".variant", format!("'{o}' not dark|light"))),
                        })
                    }
                    _ => {}
                }
            } else if in_palette {
                let k = k.trim();
                let ix = k.strip_prefix("base").filter(|h| h.len() == 2).and_then(|h| usize::from_str_radix(h, 16).ok()).filter(|ix| *ix < 24);
                let Some(ix) = ix else { return Err(scheme_err(file, n, &format!(".palette.{k}"), "slot is base00..base17")) };
                let c = hex_rgb(scalar(v)).ok_or_else(|| scheme_err(file, n, &format!(".palette.{k}"), "need #rrggbb"))?;
                if slots[ix].replace(c).is_some() {
                    return Err(scheme_err(file, n, &format!(".palette.{k}"), "duplicate slot"));
                }
            }
        }
        let got = slots.iter().flatten().count();
        let all: Option<Vec<Rgb>> = slots.iter().copied().collect();
        let slots = all.and_then(|v| <[Rgb; 24]>::try_from(v).ok()).ok_or_else(|| scheme_err(file, 0, ".palette", format!("need exactly 24 slots, got {got}")))?;
        name.filter(|n| !n.is_empty()).ok_or_else(|| scheme_err(file, 0, ".name", "missing"))?;
        Ok(Scheme {
            variant: variant.ok_or_else(|| scheme_err(file, 0, ".variant", "missing"))?,
            slots,
        })
    }
}

/// Name of a file under `schemes/`: 1..=40 chars of [a-z0-9-], so it can never leave that directory.
/// Built only by `parse`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemeId(String);

impl SchemeId {
    fn parse(r: &Rec, k: &str) -> Result<SchemeId> {
        let id = label(r, k, 40)?;
        if id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            Ok(SchemeId(id))
        } else {
            Err(r.err(k, "scheme id is [a-z0-9-]+ (file name under schemes/)"))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The scheme the world is drawn in: dark by construction (light schemes are for UI regions only).
pub struct WorldScheme(Scheme);

impl WorldScheme {
    pub fn new(file: &str, s: Scheme) -> Result<WorldScheme> {
        match s.variant {
            Variant::Dark => Ok(WorldScheme(s)),
            Variant::Light => Err(scheme_err(file, 0, ".variant", "the world scheme must be dark")),
        }
    }
}

/// What every widget colours with: foe class, power type, loot rarity, chrome. Closed set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Bg,
    Panel,
    Void,
    Select,
    Fog,
    HudDim,
    Text,
    Danger,
    Legendary,
    Rare,
    Heal,
    Info,
    Magic,
    Unique,
    Accent,
}

const ROLES: [Role; 15] = [
    Role::Bg,
    Role::Panel,
    Role::Void,
    Role::Select,
    Role::Fog,
    Role::HudDim,
    Role::Text,
    Role::Danger,
    Role::Legendary,
    Role::Rare,
    Role::Heal,
    Role::Info,
    Role::Magic,
    Role::Unique,
    Role::Accent,
];

impl Role {
    fn ix(self) -> usize {
        self as usize
    }
    /// base24 slot of the role.
    fn slot(self) -> usize {
        match self {
            Role::Bg => 0x00,
            Role::Panel => 0x10,
            Role::Void => 0x11,
            Role::Select => 0x02,
            Role::Fog => 0x03,
            Role::HudDim => 0x04,
            Role::Text => 0x05,
            Role::Danger => 0x08,
            Role::Legendary => 0x09,
            Role::Rare => 0x0a,
            Role::Heal => 0x0b,
            Role::Info => 0x0c,
            Role::Magic => 0x0d,
            Role::Unique => 0x0e,
            Role::Accent => 0x0f,
        }
    }
    /// Bright twin slot (flash colour), where the scheme has one.
    fn twin(self) -> Option<usize> {
        match self {
            Role::Danger => Some(0x12),
            Role::Rare => Some(0x13),
            Role::Heal => Some(0x14),
            Role::Info => Some(0x15),
            Role::Magic => Some(0x16),
            Role::Unique => Some(0x17),
            _ => None,
        }
    }
    /// Drawn over the background (must keep 3:1); the rest are surfaces and tints.
    fn is_ink(self) -> bool {
        !matches!(self, Role::Bg | Role::Panel | Role::Void | Role::Select | Role::Fog)
    }
}

/// WCAG contrast ratio of two colours.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let lin = |c: u8| {
        let v = f64::from(c) / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let y = |c: Rgb| 0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2);
    let (hi, lo) = (y(a).max(y(b)), y(a).min(y(b)));
    (hi + 0.05) / (lo + 0.05)
}

/// Ink must keep this ratio against the background (WCAG large-text / UI-component floor).
const MIN_CONTRAST: f64 = 3.0;

#[derive(Clone, Copy, Debug)]
pub struct RoleTable {
    rgb: [Rgb; 15],
}

impl RoleTable {
    /// Table straight from colours (no scheme): trusted fixed data.
    fn of_legacy(rgb: [Rgb; 15]) -> RoleTable {
        RoleTable { rgb }
    }

    /// Resolve `s` for a region whose fallbacks are the world scheme: own slot, then bright twin,
    /// then the world's colour, then the world's text, then white/black; first one >= 3:1 over `s`'s bg.
    pub fn of(s: &Scheme, world: &WorldScheme) -> RoleTable {
        let bg = s.slots[Role::Bg.slot()];
        let w = &world.0.slots;
        let mut t = RoleTable::of_legacy([bg; 15]);
        for r in ROLES {
            let own = s.slots[r.slot()];
            let chain = [
                Some(own),
                r.twin().map(|i| s.slots[i]),
                Some(w[r.slot()]),
                Some(w[Role::Text.slot()]),
            ];
            let white_black = if contrast(Rgb(255, 255, 255), bg) >= contrast(Rgb(0, 0, 0), bg) { Rgb(255, 255, 255) } else { Rgb(0, 0, 0) };
            let pick = chain.into_iter().flatten().find(|c| !r.is_ink() || contrast(*c, bg) >= MIN_CONTRAST);
            t.rgb[r.ix()] = if r.is_ink() { pick.unwrap_or(white_black) } else { own };
        }
        t
    }

    pub fn get(&self, r: Role) -> Rgb {
        self.rgb[r.ix()]
    }
}

// ---------- classes, skill trees (tot-rcn.7) ----------

/// Power type of a tree or node (closed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PowerTag {
    Magic,
    Melee,
    Necro,
    Thunder,
    Speed,
    Dex,
}
const POWER_TAGS: [(&str, PowerTag); 6] = [
    ("magic", PowerTag::Magic),
    ("melee", PowerTag::Melee),
    ("necro", PowerTag::Necro),
    ("thunder", PowerTag::Thunder),
    ("speed", PowerTag::Speed),
    ("dex", PowerTag::Dex),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ClassId(u8);

impl ClassId {
    pub fn ix(self) -> usize {
        self.0.into()
    }
}

pub struct Class {
    pub key: String,
    pub name: String,
}

/// Nodes per tree: `Owned` is one u64.
pub const MAX_TREE_NODES: usize = 64;

/// One purchasable node. `requires` indexes the same tree's `nodes`; `col`/`row` place it on the
/// screen grid (unique per tree) for hjkl navigation.
pub struct Node {
    pub name: String,
    pub tag: PowerTag,
    pub cost: u8,
    pub requires: Vec<u8>,
    pub effect: SkillId,
    pub col: u8,
    pub row: u8,
}

pub struct Tree {
    pub key: String,
    pub name: String,
    pub class: ClassId,
    pub tag: PowerTag,
    pub nodes: Vec<Node>,
}

/// Nodes a hero owns in one tree (bit i = node i).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Owned(u64);

impl Owned {
    pub fn has(self, node: usize) -> bool {
        node < MAX_TREE_NODES && self.0 >> node & 1 == 1
    }
    pub fn count(self) -> u32 {
        self.0.count_ones()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpendErr {
    NoSuchNode,
    AlreadyOwned,
    NeedPoints { cost: u8, have: u16 },
    /// first unowned prerequisite
    Locked { missing: u8 },
}

impl Tree {
    /// Node at a grid cell.
    pub fn at(&self, col: u8, row: u8) -> Option<usize> {
        self.nodes.iter().position(|n| n.col == col && n.row == row)
    }

    /// Spend `points` on `node`: the new ownership and the points left, or why not.
    pub fn spend(&self, owned: Owned, points: u16, node: usize) -> std::result::Result<(Owned, u16), SpendErr> {
        let n = self.nodes.get(node).ok_or(SpendErr::NoSuchNode)?;
        if owned.has(node) {
            return Err(SpendErr::AlreadyOwned);
        }
        if let Some(&missing) = n.requires.iter().find(|&&q| !owned.has(q.into())) {
            return Err(SpendErr::Locked { missing });
        }
        let left = points.checked_sub(n.cost.into()).ok_or(SpendErr::NeedPoints { cost: n.cost, have: points })?;
        Ok((Owned(owned.0 | 1 << node), left))
    }
}

fn index_of<'a>(keys: impl Iterator<Item = &'a str>, k: &str) -> Option<usize> {
    keys.enumerate().find(|(_, x)| *x == k).map(|(i, _)| i)
}

/// `class`, `tree`, `node` records. Per tree the requirement graph must be a DAG with every node
/// reachable from a root (a node with no requirements); effect refs must name existing skills.
fn trees(recs: &[(String, Rec)], skill_id: &dyn Fn(&Rec, &str, &str) -> Result<SkillId>) -> Result<(Vec<Class>, Vec<Tree>)> {
    let mut classes: Vec<Class> = Vec::new();
    for (_, r) in recs.iter().filter(|(t, _)| t == "class") {
        r.allow(&["t", "id", "name"])?;
        let key = label(r, "id", 24)?;
        if classes.iter().any(|c| c.key == key) {
            return Err(r.err("id", "duplicate class"));
        }
        classes.push(Class { key, name: label(r, "name", 24)? });
    }
    if classes.is_empty() || classes.len() > 255 {
        return Err(scheme_err("trees.jsonl", 0, "", "need 1..=255 class records"));
    }
    let mut out: Vec<Tree> = Vec::new();
    let mut node_recs: Vec<Vec<&Rec>> = Vec::new();
    for (t, r) in recs {
        match t.as_str() {
            "class" => {}
            "tree" => {
                r.allow(&["t", "id", "name", "class", "tag"])?;
                let key = label(r, "id", 24)?;
                if out.iter().any(|x| x.key == key) {
                    return Err(r.err("id", "duplicate tree"));
                }
                let cls = r.text("class")?;
                let class = index_of(classes.iter().map(|c| c.key.as_str()), cls).ok_or_else(|| r.err("class", format!("no class '{cls}'")))?;
                out.push(Tree { key, name: label(r, "name", 24)?, class: ClassId(class as u8), tag: r.pick("tag", &POWER_TAGS)?, nodes: Vec::new() });
                node_recs.push(Vec::new());
            }
            "node" => {
                let k = r.text("tree")?;
                let ti = index_of(out.iter().map(|x| x.key.as_str()), k).ok_or_else(|| r.err("tree", format!("no tree '{k}' (declare it before its nodes)")))?;
                node_recs[ti].push(r);
            }
            _ => return Err(r.err("t", "one of class tree node")),
        }
    }
    for (ci, c) in classes.iter().enumerate() {
        if !out.iter().any(|t| t.class.ix() == ci) {
            return Err(scheme_err("trees.jsonl", 0, "", format!("class '{}' has no tree", c.key)));
        }
    }
    for (tree, rs) in out.iter_mut().zip(&node_recs) {
        if rs.is_empty() || rs.len() > MAX_TREE_NODES {
            return Err(scheme_err("trees.jsonl", 0, "", format!("tree '{}' needs 1..={MAX_TREE_NODES} nodes", tree.key)));
        }
        let keys: Vec<&str> = rs.iter().map(|r| r.text("id")).collect::<Result<_>>()?;
        for (i, r) in rs.iter().enumerate() {
            r.allow(&["t", "tree", "id", "name", "tag", "cost", "requires", "effect", "col", "row"])?;
            if keys[..i].contains(&keys[i]) {
                return Err(r.err("id", "duplicate node in this tree"));
            }
            let requires = r
                .strs("requires")?
                .iter()
                .map(|q| index_of(keys.iter().copied(), q).map(|x| x as u8).ok_or_else(|| r.err("requires", format!("no node '{q}' in tree '{}'", tree.key))))
                .collect::<Result<Vec<_>>>()?;
            let (col, row) = (r.int("col", 0, 20)? as u8, r.int("row", 0, 20)? as u8);
            if tree.nodes.iter().any(|n| (n.col, n.row) == (col, row)) {
                return Err(r.err("col", "grid cell already used by another node"));
            }
            let tag = if r.has("tag") { r.pick("tag", &POWER_TAGS)? } else { tree.tag };
            let effect = skill_id(r, "effect", r.text("effect")?)?;
            label(r, "id", 24)?;
            tree.nodes.push(Node { name: label(r, "name", 24)?, tag, cost: r.int("cost", 1, 9)? as u8, requires, effect, col, row });
        }
        // reachability: a node unlocks once all its requirements did; a cycle never unlocks
        let mut seen = Owned::default();
        loop {
            let next = tree.nodes.iter().enumerate().find(|(i, n)| !seen.has(*i) && n.requires.iter().all(|&q| seen.has(q.into())));
            match next {
                Some((i, _)) => seen = Owned(seen.0 | 1 << i),
                None => break,
            }
        }
        if let Some(i) = (0..rs.len()).find(|&i| !seen.has(i)) {
            return Err(rs[i].err("requires", "node is not reachable from a root (requirement cycle)"));
        }
        if !tree.nodes.iter().any(|n| n.requires.is_empty()) {
            return Err(rs[0].err("requires", "tree has no root node"));
        }
    }
    Ok((classes, out))
}

// ---------- loot items (tot-fnk.2) ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Armor,
    Ring,
    Jewel,
    Weapon,
    Charm,
}
const SLOTS: [(&str, Slot); 5] = [("armor", Slot::Armor), ("ring", Slot::Ring), ("jewel", Slot::Jewel), ("weapon", Slot::Weapon), ("charm", Slot::Charm)];

impl Slot {
    pub const ALL: [Slot; 5] = [Slot::Armor, Slot::Ring, Slot::Jewel, Slot::Weapon, Slot::Charm];
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Rarity {
    Common,
    Rare,
    Epic,
    Legendary,
    Unique,
}
const RARITIES: [(&str, Rarity); 5] =
    [("common", Rarity::Common), ("rare", Rarity::Rare), ("epic", Rarity::Epic), ("legendary", Rarity::Legendary), ("unique", Rarity::Unique)];

impl Rarity {
    /// Colour role every widget draws this rarity in.
    pub fn role(self) -> Role {
        match self {
            Rarity::Common => Role::Text,
            Rarity::Rare => Role::Rare,
            Rarity::Epic => Role::Magic,
            Rarity::Legendary => Role::Legendary,
            Rarity::Unique => Role::Unique,
        }
    }
    /// Most stat mods an item of this rarity may carry.
    fn max_mods(self) -> usize {
        match self {
            Rarity::Common => 1,
            Rarity::Rare => 2,
            Rarity::Epic | Rarity::Legendary => 3,
            Rarity::Unique => 4,
        }
    }
}

/// Signed permille (item mods may subtract).
fn pm(v: f64) -> i32 {
    (v * 1000.0).round() as i32
}

/// (json name, stat, allowed magnitude in data units, scaler to stored units)
type StatRow = (&'static str, Stat, f64, fn(f64) -> i32);

/// Stat an item can change. Values are stored scaled: hp as milli-HP, regen as milli-HP per second,
/// pickup as mm, the rest as permille.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stat {
    MaxHp,
    Regen,
    Armour,
    Pickup,
    Speed,
    Damage,
    Haste,
    Area,
    Crit,
}

impl Stat {
    /// (json name, allowed magnitude in data units, scaler)
    const ALL: [StatRow; 9] = [
        ("max_hp", Stat::MaxHp, 500.0, mhp),
        ("regen", Stat::Regen, 20.0, mhp),
        ("armour", Stat::Armour, 50.0, |v| v.round() as i32),
        ("pickup", Stat::Pickup, 10.0, mm),
        ("speed", Stat::Speed, 0.5, pm),
        ("damage", Stat::Damage, 2.0, pm),
        ("haste", Stat::Haste, 1.0, pm),
        ("area", Stat::Area, 1.0, pm),
        ("crit", Stat::Crit, 0.5, pm),
    ];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mod {
    pub stat: Stat,
    pub value: i32,
}

pub struct Item {
    pub key: String,
    pub name: String,
    pub slot: Slot,
    pub rarity: Rarity,
    pub glyph: Glyph,
    pub mods: Vec<Mod>,
    pub desc: String,
}

fn item(r: &Rec) -> Result<Item> {
    r.allow(&["t", "id", "name", "slot", "rarity", "glyph", "mods", "desc"])?;
    let rarity = r.pick("rarity", &RARITIES)?;
    let mut mods: Vec<Mod> = Vec::new();
    for (i, m) in r.list("mods")?.iter().enumerate() {
        let m = r.at(m, &format!("mods[{i}]"))?;
        m.allow(&["stat", "value"])?;
        let name = m.text("stat")?;
        let &(_, stat, max, scale) = Stat::ALL.iter().find(|s| s.0 == name).ok_or_else(|| m.err("stat", format!("'{name}' not one of: {}", Stat::ALL.map(|s| s.0).join(" "))))?;
        if mods.iter().any(|x| x.stat == stat) {
            return Err(m.err("stat", "stat repeated on this item"));
        }
        let v = m.num("value", -max, max)?;
        if v == 0.0 {
            return Err(m.err("value", "a zero mod does nothing"));
        }
        mods.push(Mod { stat, value: scale(v) });
    }
    if !(1..=rarity.max_mods()).contains(&mods.len()) {
        return Err(r.err("mods", format!("{rarity:?} items carry 1..={} mods, got {}", rarity.max_mods(), mods.len())));
    }
    Ok(Item {
        key: label(r, "id", 24)?,
        name: label(r, "name", 24)?,
        slot: r.pick("slot", &SLOTS)?,
        rarity,
        glyph: glyph(r, "glyph")?,
        mods,
        desc: label(r, "desc", 70)?,
    })
}

fn items(recs: &[(String, Rec)]) -> Result<Vec<Item>> {
    let mut out: Vec<Item> = Vec::new();
    for (t, r) in recs {
        want(t, r, "item")?;
        let it = item(r)?;
        if out.iter().any(|o| o.key == it.key) {
            return Err(r.err("id", "duplicate item"));
        }
        out.push(it);
    }
    Ok(out)
}

// ---------- content ----------

pub struct Content {
    pub opt: Options,
    pub player: PlayerDef,
    pub spawn: SpawnDef,
    pub pace: [PacePhase; 4],
    pub scale: ScaleDef,
    pub xp_need: Vec<u32>,
    pub picks: PicksDef,
    pub hijack: HijackDef,
    pub view: ViewDef,
    pub bars: Bars,
    pub arena: ArenaDef,
    pub pal: Palette,
    pub fams: Vec<Fam>,
    pub skills: Vec<Skill>,
    pub bosses: Vec<BossDef>,
    pub waves: Vec<WaveEv>,
    pub elite: Cadence,
    pub champion: Cadence,
    /// named miniboss families (waves.jsonl `miniboss` records) and their spawn cadence; both or neither
    pub mini: Option<MiniPlan>,
    pub binds: Vec<Bind>,
    pub groups: Vec<Group>,
    pub classes: Vec<Class>,
    pub trees: Vec<Tree>,
    pub items: Vec<Item>,
}

pub const MAX_SKILLS: usize = 32;
const FILES: [&str; 10] = [
    "options.jsonl",
    "tuning.jsonl",
    "palette.jsonl",
    "enemies.jsonl",
    "skills.jsonl",
    "bosses.jsonl",
    "waves.jsonl",
    "bindings.jsonl",
    "trees.jsonl",
    "items.jsonl",
];

/// Files a profile may replace: the content, never the options or the key bindings.
const PROFILE_FILES: [&str; 8] =
    ["tuning.jsonl", "palette.jsonl", "enemies.jsonl", "skills.jsonl", "bosses.jsonl", "waves.jsonl", "trees.jsonl", "items.jsonl"];

/// Header of a profile directory (`profile.jsonl`).
#[derive(Debug, PartialEq, Eq)]
pub struct ProfileInfo {
    pub id: String,
    pub name: String,
    pub blurb: String,
}

/// Profiles installed under `dir/profiles`, sorted by id; each manifest is validated.
pub fn profiles(dir: &Path) -> Result<Vec<ProfileInfo>> {
    let root = dir.join("profiles");
    let mut ids: Vec<String> = match std::fs::read_dir(&root) {
        Ok(rd) => rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect(),
        Err(_) => Vec::new(),
    };
    ids.sort();
    ids.iter().map(|id| manifest(&root.join(id), id)).collect()
}

fn manifest(pdir: &Path, id: &str) -> Result<ProfileInfo> {
    let file = format!("profiles/{id}/profile.jsonl");
    let text = std::fs::read_to_string(pdir.join("profile.jsonl")).map_err(|e| scheme_err(&file, 0, "", e.to_string()))?;
    let mut vals = Vec::new();
    let recs = records("profile", &text, &mut vals).map_err(|e| match e {
        Error::Data { line, path, msg, .. } => scheme_err(&file, line, &path, msg),
        o => o,
    })?;
    let [(t, r)] = recs.as_slice() else { return Err(scheme_err(&file, 0, "", "need exactly one \"profile\" record")) };
    want(t, r, "profile")?;
    r.allow(&["t", "id", "name", "blurb"])?;
    let got = label(r, "id", 40)?;
    if got != id {
        return Err(r.err("id", format!("must equal the directory name '{id}'")));
    }
    Ok(ProfileInfo { id: got, name: label(r, "name", 40)?, blurb: label(r, "blurb", 80)? })
}

/// Directory of an installed profile; a name that is not one is a usage error that lists the real ones.
fn profile_dir(dir: &Path, name: &str) -> Result<std::path::PathBuf> {
    let plain = !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let p = dir.join("profiles").join(name);
    if plain && p.join("profile.jsonl").exists() {
        manifest(&p, name)?;
        return Ok(p);
    }
    let have = profiles(dir).map(|v| v.into_iter().map(|i| i.id).collect::<Vec<_>>().join(" ")).unwrap_or_default();
    Err(Error::Usage(format!("--profile '{name}' is not installed; available: {have}")))
}

impl Content {
    pub fn load(dir: &Path) -> Result<Content> {
        Content::load_profile(dir, None)
    }

    /// `profile` = a theme under `dir/profiles/<name>/`: any content file it ships replaces the base
    /// file whole (same schema); the rest, plus options and bindings always, come from `dir`.
    pub fn load_profile(dir: &Path, profile: Option<&str>) -> Result<Content> {
        let pdir = match profile {
            None => None,
            Some(name) => Some(profile_dir(dir, name)?),
        };
        let mut texts = Vec::new();
        for f in FILES {
            let over = pdir.as_ref().filter(|_| PROFILE_FILES.contains(&f)).map(|p| p.join(f)).filter(|p| p.exists());
            let path = over.unwrap_or_else(|| dir.join(f));
            texts.push(std::fs::read_to_string(&path).map_err(|e| Error::Data {
                file: path.display().to_string(),
                line: 0,
                path: String::new(),
                msg: e.to_string(),
            })?);
        }
        let mut c = Content::parse(&texts)?;
        if let Some(id) = c.pal.scheme.clone() {
            let file = format!("schemes/{}.yaml", id.as_str());
            let text = std::fs::read_to_string(dir.join(&file)).map_err(|e| scheme_err(&file, 0, "", e.to_string()))?;
            let world = WorldScheme::new(&file, Scheme::parse(&file, &text)?)?;
            c.pal.roles = RoleTable::of(&world.0, &world);
        }
        Ok(c)
    }

    /// Parse the 9 files given as texts in FILES order (tests feed edited copies).
    pub fn parse(texts: &[String]) -> Result<Content> {
        let mut vals: Vec<Vec<serde_json::Value>> = FILES.iter().map(|_| Vec::new()).collect();
        let mut recs = Vec::new();
        for ((f, t), v) in FILES.iter().zip(texts).zip(vals.iter_mut()) {
            recs.push(records(f, t, v)?);
        }
        let mut it = recs.into_iter();
        let mut next = || it.next().unwrap_or_default();
        let (o, tu, pa, en, sk, bo, wa, bi, tr, it) = (next(), next(), next(), next(), next(), next(), next(), next(), next(), next());
        let opt = options(only(&o, "options", "options.jsonl")?)?;
        let hz = opt.hz;
        let fams = en.iter().map(|(t, r)| want(t, r, "family").and_then(|_| family(r, hz))).collect::<Result<Vec<_>>>()?;
        let skills = sk.iter().map(|(t, r)| want(t, r, "skill").and_then(|_| skill(r, hz))).collect::<Result<Vec<_>>>()?;
        if skills.len() > MAX_SKILLS || fams.len() > 255 {
            return Err(Error::Data { file: "skills.jsonl".into(), line: 0, path: String::new(), msg: format!("max {MAX_SKILLS} skills") });
        }
        let fam_id = |r: &Rec, k: &str| -> Result<FamId> {
            let s = r.text(k)?;
            fams.iter().position(|f| f.key == s).map(|i| FamId(i as u8)).ok_or_else(|| r.err(k, format!("no family '{s}'")))
        };
        let skill_id = |r: &Rec, k: &str, s: &str| -> Result<SkillId> {
            skills.iter().position(|f| f.key == s).map(|i| SkillId(i as u8)).ok_or_else(|| r.err(k, format!("no skill '{s}'")))
        };
        let player = player(only(&tu, "player", "tuning.jsonl")?, hz, &skill_id)?;
        let spawn = spawn(only(&tu, "spawn", "tuning.jsonl")?, hz)?;
        for (f, (_, r)) in fams.iter().zip(&en) {
            if let FamKind::Mob(_) = f.kind {
                if f.speed >= player.speed {
                    return Err(r.err("speed_mps", "mobs must be slower than the player (design sec 8.4)"));
                }
                if i64::from(f.speed) * i64::from(spawn.swift) / 1000 > i64::from(hz.mmpt(4.8)) {
                    return Err(r.err("speed_mps", "swift elite variant would exceed 4.8 m/s"));
                }
                if let FamKind::Mob(Mob { role: MobRole::Berserker { mult, .. }, .. }) = &f.kind
                {
                    // top speed = enraged AND swift elite
                    let top = i64::from(f.speed) * i64::from(*mult) / 1000 * i64::from(spawn.swift) / 1000;
                    if top >= i64::from(player.speed) || top > i64::from(hz.mmpt(4.8)) {
                        return Err(r.err("role", "enraged swift speed must stay below the player and 4.8 m/s (design sec 8.4)"));
                    }
                }
            }
        }
        if spawn.speed_cap >= player.speed {
            return Err(Error::Data { file: "tuning.jsonl".into(), line: 0, path: ".spawn.speed_cap_mps".into(), msg: "must stay below player speed (design sec 8.4)".into() });
        }
        let bosses = bo.iter().map(|(t, r)| want(t, r, "boss").and_then(|_| boss(r, hz, &fam_id, &fams))).collect::<Result<Vec<_>>>()?;
        let WavePlan { evs: waves, elite, champion, mini } = waves(&wa, hz, &fam_id, &fams, &bosses, &spawn)?;
        let (binds, groups) = binds(&bi)?;
        let (classes, trees) = trees(&tr, &skill_id)?;
        Ok(Content {
            pace: pacing(only(&tu, "pacing", "tuning.jsonl")?, hz)?,
            scale: scaling(only(&tu, "scaling", "tuning.jsonl")?)?,
            xp_need: xp(only(&tu, "xp", "tuning.jsonl")?)?,
            picks: picks(only(&tu, "picks", "tuning.jsonl")?)?,
            hijack: hijack(only(&tu, "hijack", "tuning.jsonl")?, hz)?,
            view: view(only(&tu, "view", "tuning.jsonl")?, hz, &spawn)?,
            bars: bars(only(&tu, "bars", "tuning.jsonl")?)?,
            arena: arena_def(only(&tu, "arena", "tuning.jsonl")?)?,
            pal: palette(only(&pa, "palette", "palette.jsonl")?)?,
            opt,
            player,
            spawn,
            fams,
            skills,
            bosses,
            waves,
            elite,
            champion,
            mini,
            binds,
            groups,
            classes,
            trees,
            items: items(&it)?,
        })
    }

    pub fn hz(&self) -> Hz {
        self.opt.hz
    }

    pub fn fam(&self, f: FamId) -> &Fam {
        &self.fams[f.ix()]
    }

    pub fn skill(&self, s: SkillId) -> &Skill {
        &self.skills[s.ix()]
    }

    pub fn skill_ids(&self) -> impl Iterator<Item = SkillId> + '_ {
        (0..self.skills.len()).map(|i| SkillId(i as u8))
    }

    pub fn fam_ids(&self) -> impl Iterator<Item = FamId> + '_ {
        (0..self.fams.len()).map(|i| FamId(i as u8))
    }

    pub fn boss_of(&self, f: FamId) -> Option<&BossDef> {
        self.bosses.iter().find(|b| b.fam == f)
    }

    pub fn skill_by_key(&self, k: &str) -> Option<SkillId> {
        self.skill_ids().find(|&s| self.skill(s).key == k)
    }
}

fn want(t: &str, r: &Rec, kind: &str) -> Result<()> {
    if t == kind { Ok(()) } else { Err(r.err("t", format!("expected \"{kind}\""))) }
}

/// Exactly one record of `kind` in a file.
fn only<'a, 'b>(recs: &'b [(String, Rec<'a>)], kind: &str, file: &str) -> Result<&'b Rec<'a>> {
    let mut it = recs.iter().filter(|(t, _)| t == kind);
    match (it.next(), it.next()) {
        (Some((_, r)), None) => Ok(r),
        (Some(_), Some((_, r))) => Err(r.err("t", format!("second \"{kind}\" record"))),
        _ => Err(Error::Data { file: file.into(), line: 0, path: String::new(), msg: format!("missing \"{kind}\" record") }),
    }
}

fn player(r: &Rec, hz: Hz, skill_id: &dyn Fn(&Rec, &str, &str) -> Result<SkillId>) -> Result<PlayerDef> {
    r.allow(&[
        "t", "hp", "regen_s", "armour", "speed_mps", "radius_m", "pickup_m", "crit_chance", "crit_bonus", "flux", "flux_regen_s",
        "flux_per_kill", "dash_m", "dash_ticks", "iframe_s", "dodge_cd_s", "contact_cd_s", "skills", "skill_slots", "trait_slots",
    ])?;
    let dash_ticks = r.int("dash_ticks", 1, 30)? as u16;
    let skills = r.strs("skills")?.iter().map(|s| skill_id(r, "skills", s)).collect::<Result<Vec<_>>>()?;
    let skill_slots = r.int("skill_slots", 1, 8)? as usize;
    if skills.is_empty() || skills.len() > skill_slots {
        return Err(r.err("skills", "need 1..=skill_slots base skills"));
    }
    Ok(PlayerDef {
        hp: mhp(r.num("hp", 1.0, 10000.0)?),
        regen: mhp(r.num("regen_s", 0.0, 100.0)?) / hz.get() as i32,
        armour: r.num("armour", 0.0, 1000.0)? as i32,
        speed: hz.mmpt(r.num("speed_mps", 0.5, 20.0)?),
        radius: mm(r.num("radius_m", 0.05, 2.0)?),
        pickup: mm(r.num("pickup_m", 0.1, 20.0)?),
        crit_chance: permille(r.num("crit_chance", 0.0, 5.0)?),
        crit_bonus: permille(r.num("crit_bonus", 0.0, 10.0)?),
        flux: mhp(r.num("flux", 0.0, 1000.0)?),
        flux_regen: mhp(r.num("flux_regen_s", 0.0, 100.0)?) / hz.get() as i32,
        flux_per_kill: mhp(r.num("flux_per_kill", 0.0, 100.0)?),
        dash: mm(r.num("dash_m", 0.0, 10.0)?) / i32::from(dash_ticks),
        dash_ticks,
        iframes: hz.ticks(r.num("iframe_s", 0.0, 2.0)?) as u16,
        dodge_cd: hz.ticks(r.num("dodge_cd_s", 0.1, 30.0)?) as u16,
        contact_cd: hz.ticks(r.num("contact_cd_s", 0.1, 5.0)?) as u8,
        skills,
        skill_slots,
        trait_slots: r.int("trait_slots", 0, 8)? as usize,
    })
}

/// Per-second permille tables, integer only. Linear: from + slope * t_min. Growth: g^t_min
/// by exact per-minute recurrence, linear inside the minute.
fn linear(len: u32, from: u32, slope: u32) -> Vec<u32> {
    (0..=len).map(|s| from + slope * s / 60).collect()
}

fn growth(len: u32, g: u32) -> Vec<u32> {
    let mins: Vec<u64> = std::iter::successors(Some(1_000_000u64), |m| Some(m * u64::from(g) / 1000)).take((len / 60 + 2) as usize).collect();
    (0..=len as usize).map(|s| (mins[s / 60] + (mins[s / 60 + 1] - mins[s / 60]) * (s % 60) as u64 / 60) / 1000).map(|v| v.min(u64::from(u32::MAX)) as u32).collect()
}


fn spawn(r: &Rec, hz: Hz) -> Result<SpawnDef> {
    r.allow(&[
        "t", "trash_hp", "hp_growth_min", "rate0_s", "rate_slope", "dmg0", "dmg_slope", "live_cap", "ring_min_m", "ring_max_m",
        "despawn_m", "cone_deg", "elite_hp", "elite_dmg", "champion_hp", "champion_dmg", "swift", "xp_trash", "xp_elite",
        "xp_champion", "pack_bank", "pack_min", "separation", "run_s", "timeout_s", "purge_every_s", "purge_lead_s", "purge_m",
        "speed_slope", "speed_cap_mps",
    ])?;
    let run = r.int("run_s", 60, 3600)? as u32;
    let (g, r0, rs, ds) = (permille(r.num("hp_growth_min", 1.0, 2.0)?), permille(r.num("rate0_s", 0.0, 50.0)?), permille(r.num("rate_slope", 0.0, 10.0)?), permille(r.num("dmg_slope", 0.0, 1.0)?));
    let ring_min = mm(r.num("ring_min_m", 5.0, 200.0)?);
    let ring_max = mm(r.num("ring_max_m", 5.0, 200.0)?);
    if ring_max < ring_min {
        return Err(r.err("ring_max_m", "below ring_min_m"));
    }
    let despawn = mm(r.num("despawn_m", 10.0, 400.0)?);
    if despawn <= ring_max {
        return Err(r.err("despawn_m", "must exceed ring_max_m"));
    }
    let timeout = r.int("timeout_s", 60, 7200)? as u32;
    if timeout <= run {
        return Err(r.err("timeout_s", "must exceed run_s"));
    }
    Ok(SpawnDef {
        trash_hp: mhp(r.num("trash_hp", 1.0, 1e5)?),
        hp_mult: growth(timeout + 600, g),
        rate: linear(timeout + 600, r0, rs),
        dmg_mult: linear(timeout + 600, 1000, ds),
        dmg0: mhp(r.num("dmg0", 0.0, 1e4)?),
        live_cap: r.int("live_cap", 1, 50_000)? as usize,
        ring_min,
        ring_max,
        despawn,
        cone_cos: crate::trig::cos(crate::trig::deg(r.num("cone_deg", 0.0, 180.0)? / 2.0)),
        elite_hp: permille(r.num("elite_hp", 1.0, 100.0)?),
        elite_dmg: permille(r.num("elite_dmg", 0.1, 20.0)?),
        champion_hp: permille(r.num("champion_hp", 1.0, 500.0)?),
        champion_dmg: permille(r.num("champion_dmg", 0.1, 20.0)?),
        swift: permille(r.num("swift", 1.0, 3.0)?),
        xp_trash: r.int("xp_trash", 0, 1000)? as u16,
        xp_elite: r.int("xp_elite", 0, 1000)? as u16,
        xp_champion: r.int("xp_champion", 0, 10000)? as u16,
        pack_bank: permille(r.num("pack_bank", 1.0, 100.0)?),
        pack_min: r.int("pack_min", 1, 200)? as u16,
        separation: permille(r.num("separation", 0.0, 1.0)?) as i32,
        timeout: hz.ticks(f64::from(timeout)),
        purge_every: hz.ticks(r.num("purge_every_s", 0.5, 60.0)?) as u16,
        purge_lead: hz.ticks(r.num("purge_lead_s", 0.4, 5.0)?) as u16,
        purge_r: mm(r.num("purge_m", 0.5, 20.0)?),
        speed_mult: linear(timeout + 600, 1000, permille(r.num("speed_slope", 0.0, 1.0)?)),
        speed_cap: hz.mmpt(r.num("speed_cap_mps", 0.5, 4.8)?),
    })
}

fn pacing(r: &Rec, hz: Hz) -> Result<[PacePhase; 4]> {
    r.allow(&["t", "build_s", "build_mult", "peak_s", "peak_mult", "fade_s", "fade_mult", "relax_s", "relax_mult"])?;
    let m = |k: &str| r.num(k, 0.0, 5.0).map(permille);
    let s = |k: &str| r.num(k, 1.0, 600.0).map(|v| hz.ticks(v));
    let b = r.list("build_mult")?;
    let bm = |i: usize| b.get(i).and_then(serde_json::Value::as_f64).filter(|v| (0.0..=5.0).contains(v)).map(permille);
    let (Some(b0), Some(b1), 2) = (bm(0), bm(1), b.len()) else {
        return Err(r.err("build_mult", "need [from, to] in [0, 5]"));
    };
    let relax = m("relax_mult")?;
    if relax == 0 {
        return Err(r.err("relax_mult", "floor never 0 (design sec 11.6)"));
    }
    Ok([
        PacePhase { len: s("build_s")?, from: b0, to: b1 },
        PacePhase { len: s("peak_s")?, from: m("peak_mult")?, to: m("peak_mult")? },
        PacePhase { len: s("fade_s")?, from: m("fade_mult")?, to: m("fade_mult")? },
        PacePhase { len: s("relax_s")?, from: relax, to: relax },
    ])
}

fn xp(r: &Rec) -> Result<Vec<u32>> {
    r.allow(&["t", "need"])?;
    // [[until_level, need], ...] -> v[L] = xp to go from level L to L+1 (v[0] unused)
    let mut out = vec![0];
    for (i, v) in r.list("need")?.iter().enumerate() {
        let pair = v.as_array().map(|a| (a.first().and_then(serde_json::Value::as_u64), a.get(1).and_then(serde_json::Value::as_u64), a.len()));
        let Some((Some(until), Some(need), 2)) = pair else {
            return Err(r.err(&format!("need[{i}]"), "need [until_level, xp]"));
        };
        if until < out.len() as u64 || until > 200 || need == 0 {
            return Err(r.err(&format!("need[{i}]"), "levels must rise, xp > 0"));
        }
        while (out.len() as u64) <= until {
            out.push(need as u32);
        }
    }
    Ok(out)
}

fn picks(r: &Rec) -> Result<PicksDef> {
    r.allow(&["t", "cards", "rarity", "gain", "pity_every", "owned_bias", "tag_bonus", "max_rank"])?;
    let four = |k: &str| -> Result<[u32; 4]> {
        let l = r.list(k)?;
        let v: Option<Vec<u32>> = l.iter().map(|x| x.as_f64().filter(|f| (0.0..=1000.0).contains(f)).map(permille)).collect();
        match v.as_deref() {
            Some(&[a, b, c, d]) => Ok([a, b, c, d]),
            _ => Err(r.err(k, "need 4 numbers (common rare epic legendary)")),
        }
    };
    let rarity = four("rarity")?;
    if rarity.iter().sum::<u32>() != 1000 * 1000 {
        return Err(r.err("rarity", "weights must sum to 1000"));
    }
    Ok(PicksDef {
        cards: r.int("cards", 3, 3)? as usize,
        rarity: rarity.map(|w| w / 1000),
        gain: four("gain")?,
        pity_every: r.int("pity_every", 1, 50)? as u16,
        owned_bias: permille(r.num("owned_bias", 0.0, 1.0)?),
        tag_bonus: permille(r.num("tag_bonus", 0.0, 5.0)?),
        max_rank: r.int("max_rank", 1, 9)? as u8,
    })
}

fn hijack(r: &Rec, hz: Hz) -> Result<HijackDef> {
    r.allow(&[
        "t", "cap_base", "cap_per_rank", "burst_hp", "burst_m", "dmg_mult", "enemy_weight", "elite_ice", "champion_ice", "elite_s", "champion_s",
        "glitch_decay_s", "seek_m",
    ])?;
    let s = |k: &str| r.num(k, 0.5, 120.0).map(|v| hz.ticks(v) as u16);
    Ok(HijackDef {
        elite_ice: r.int("elite_ice", 1, 200)? as u8,
        champion_ice: r.int("champion_ice", 1, 250)? as u8,
        elite_s: s("elite_s")?,
        champion_s: s("champion_s")?,
        glitch_decay: s("glitch_decay_s")?.max(1),
        seek: mm(r.num("seek_m", 1.0, 40.0)?),
        cap_base: r.int("cap_base", 0, 500)? as u16,
        cap_per_rank: r.int("cap_per_rank", 0, 100)? as u16,
        burst_hp: permille(r.num("burst_hp", 0.0, 10.0)?),
        burst_r: mm(r.num("burst_m", 0.0, 20.0)?),
        dmg_mult: permille(r.num("dmg_mult", 0.0, 20.0)?),
        enemy_weight: permille(r.num("enemy_weight", 0.01, 10.0)?),
    })
}

fn view(r: &Rec, hz: Hz, s: &SpawnDef) -> Result<ViewDef> {
    r.allow(&[
        "t", "fog_start_m", "fog_end_m", "fog_max", "fog_bands", "floor_cell_m", "min_cols", "min_rows", "crowd_n", "crowd_m",
        "tele_blink_hz", "view_m", "block_rows", "actor_rows", "face_pct", "cut_cols", "fps", "follow_ms", "deadzone_m", "turn_ms",
    ])?;
    let view_r = mm(r.num("view_m", 5.0, 100.0)?);
    if view_r >= s.ring_min {
        return Err(r.err("view_m", "spawn ring must start outside view (design sec 8.4)"));
    }
    // actors are drawn only within sight (fog end): a foe spawned or recycled inside it would pop in
    let sight = mm(r.num("fog_end_m", 1.0, 200.0)?);
    if sight > s.ring_min {
        return Err(r.err("fog_end_m", "spawn ring must start at or beyond sight (fog end)"));
    }
    let blink = r.num("tele_blink_hz", 0.5, 15.0)?;
    Ok(ViewDef {
        fog_start: mm(r.num("fog_start_m", 0.0, 100.0)?),
        fog_end: sight,
        fog_max: permille(r.num("fog_max", 0.0, 1.0)?),
        fog_bands: r.int("fog_bands", 1, 256)? as u32,
        floor_cell: mm(r.num("floor_cell_m", 0.5, 20.0)?),
        min_cols: r.int("min_cols", 20, 1000)? as u16,
        min_rows: r.int("min_rows", 10, 1000)? as u16,
        crowd_n: r.int("crowd_n", 1, 50)? as u8,
        crowd_r: mm(r.num("crowd_m", 0.1, 10.0)?),
        blink: (f64::from(hz.get()) / (2.0 * blink)).round().max(1.0) as u32,
        view_r,
        block_rows: ints(r, "block_rows", 0, 8)?,
        actor_rows: ints(r, "actor_rows", 0, 4)?,
        face_pct: ints(r, "face_pct", 0, 100)?,
        cut_cols: r.int("cut_cols", 0, 60)? as u8,
        fps: r.int("fps", 1, 240)? as u16,
        follow_ms: r.int("follow_ms", 0, 5000)? as u32,
        deadzone: mm(r.num("deadzone_m", 0.0, 10.0)?),
        turn_ms: r.int("turn_ms", 0, 2000)? as u32,
    })
}

/// `[lo, hi]` with lo <= hi inside [min, max].
fn pair(r: &Rec, k: &str, min: f64, max: f64) -> Result<(f64, f64)> {
    let l = r.list(k)?;
    match (l.first().and_then(serde_json::Value::as_f64), l.get(1).and_then(serde_json::Value::as_f64), l.len()) {
        (Some(a), Some(b), 2) if min <= a && a <= b && b <= max => Ok((a, b)),
        _ => Err(r.err(k, format!("need [lo, hi] with {min} <= lo <= hi <= {max}"))),
    }
}

fn arena_def(r: &Rec) -> Result<ArenaDef> {
    r.allow(&["t", "glyphs", "density", "fam_mult", "decor", "decor_cell_m", "decor_r_m"])?;
    let pm = |k: &str, max: f64| pair(r, k, 0.0, max).map(|(a, b)| (permille(a), permille(b)));
    let g = pair(r, "glyphs", 1.0, 64.0)?;
    let dr = pair(r, "decor_r_m", 0.1, 20.0)?;
    Ok(ArenaDef {
        glyphs: (g.0 as u32, g.1 as u32),
        density: pm("density", 1.0)?,
        fam_mult: pm("fam_mult", 10.0)?,
        decor: pm("decor", 1.0)?,
        decor_cell: mm(r.num("decor_cell_m", 2.0, 100.0)?),
        decor_r: (mm(dr.0), mm(dr.1)),
    })
}

fn bars(r: &Rec) -> Result<Bars> {
    r.allow(&[
        "t", "kiter_tag_win", "kiter_random_win", "oracle_win_min", "none_median_max_s", "still_median_max_s", "n2_earlier_min",
        "skill_share_max", "degenerate_dmg_share", "meter_ratio_min",
    ])?;
    let pair = |k: &str| -> Result<(f64, f64)> {
        let l = r.list(k)?;
        match (l.first().and_then(serde_json::Value::as_f64), l.get(1).and_then(serde_json::Value::as_f64), l.len()) {
            (Some(a), Some(b), 2) if a <= b => Ok((a, b)),
            _ => Err(r.err(k, "need [lo, hi]")),
        }
    };
    Ok(Bars {
        kiter_tag_win: pair("kiter_tag_win")?,
        kiter_random_win: pair("kiter_random_win")?,
        oracle_win_min: r.num("oracle_win_min", 0.0, 1.0)?,
        none_median_max_s: r.num("none_median_max_s", 0.0, 3600.0)?,
        still_median_max_s: r.num("still_median_max_s", 0.0, 3600.0)?,
        n2_earlier_min: r.num("n2_earlier_min", 0.0, 1.0)?,
        skill_share_max: r.num("skill_share_max", 0.0, 1.0)?,
        degenerate_dmg_share: r.num("degenerate_dmg_share", 0.0, 1.0)?,
        meter_ratio_min: r.num("meter_ratio_min", 0.0, 100.0)?,
    })
}

fn palette(r: &Rec) -> Result<Palette> {
    r.allow(&[
        "t", "bg", "fog", "floor", "player", "ring", "alarm", "text", "dim", "xp", "chest", "hud_bg", "kin", "therm", "emp", "code", "floor_glyphs",
        "density_glyphs", "face_glyphs", "shadow_glyph", "body_glyph", "scheme",
    ])?;
    let scheme = r.has("scheme").then(|| SchemeId::parse(r, "scheme")).transpose()?;
    let d = glyphs(r, "density_glyphs")?;
    let [a, b, c] = d.as_slice() else { return Err(r.err("density_glyphs", "need 3 glyphs")) };
    let e = glyphs(r, "face_glyphs")?;
    let [d0, d1, d2] = e.as_slice() else { return Err(r.err("face_glyphs", "need 3 glyphs (top, lit side, dark side)")) };
    let (bg, fog, hue) = (rgb(r, "bg")?, rgb(r, "fog")?, [rgb(r, "kin")?, rgb(r, "therm")?, rgb(r, "emp")?, rgb(r, "code")?]);
    let (hud_bg, ring, player, text, dim) = (rgb(r, "hud_bg")?, rgb(r, "ring")?, rgb(r, "player")?, rgb(r, "text")?, rgb(r, "dim")?);
    let (alarm, chest, xp) = (rgb(r, "alarm")?, rgb(r, "chest")?, rgb(r, "xp")?);
    // without a scheme each role reads its legacy field; the match is exhaustive over Role
    let roles = RoleTable::of_legacy(ROLES.map(|role| match role {
        Role::Bg | Role::Void => bg,
        Role::Panel => hud_bg,
        Role::Select => ring,
        Role::Fog => fog,
        Role::HudDim => dim,
        Role::Text => text,
        Role::Danger => alarm,
        Role::Legendary => chest,
        Role::Rare => xp,
        Role::Heal => hue[Hue::Kin as usize],
        Role::Info => hue[Hue::Emp as usize],
        Role::Magic => hue[Hue::Code as usize],
        Role::Unique => hue[Hue::Therm as usize],
        Role::Accent => player,
    }));
    Ok(Palette {
        scheme,
        roles,
        bg,
        fog,
        floor: rgb(r, "floor")?,
        player,
        ring,
        alarm,
        text,
        dim,
        xp,
        chest,
        hud_bg,
        hue,
        floor_glyphs: glyphs(r, "floor_glyphs")?,
        density: [*a, *b, *c],
        face: [*d0, *d1, *d2],
        shadow: glyph(r, "shadow_glyph")?,
        body: glyph(r, "body_glyph")?,
    })
}

/// Shared projectile fields of ranged-like roles: (range, shot speed, period ticks, dmg).
fn shot(b: &Rec, hz: Hz) -> Result<(i32, i32, u16, u32)> {
    Ok((
        mm(b.num("range_m", 1.0, 30.0)?),
        hz.mmpt(b.num("shot_mps", 0.5, 8.0)?),
        hz.ticks(b.num("every_s", 0.2, 60.0)?) as u16,
        permille(b.num("dmg", 0.0, 20.0)?),
    ))
}

fn family(r: &Rec, hz: Hz) -> Result<Fam> {
    r.allow(&[
        "t", "id", "name", "glyph", "hue", "role", "hp", "hp_abs", "speed_mps", "dmg", "cost", "ice", "hijack_s", "radius_m", "unlock_s", "weight",
    ])?;
    let (role, body) = r.variant("role")?;
    let kind = match (role, &body) {
        ("boss", None) => FamKind::Boss { hp: mhp(r.num("hp_abs", 1.0, 1e7)?) },
        (_, _) => {
            if r.has("hp_abs") {
                return Err(r.err("hp_abs", "only bosses have absolute HP"));
            }
            let role = match (role, body) {
                ("melee", None) => MobRole::Melee,
                ("ranged", Some(b)) => {
                    b.allow(&["range_m", "shot_mps", "every_s", "dmg"])?;
                    let (range, shot_v, every, dmg) = shot(&b, hz)?;
                    MobRole::Ranged { range, shot_v, every, dmg }
                }
                ("aura", Some(b)) => {
                    b.allow(&["radius_m", "ice"])?;
                    MobRole::Aura { radius: mm(b.num("radius_m", 0.5, 20.0)?), ice: b.int("ice", 1, 50)? as u8 }
                }
                ("thief", Some(b)) => {
                    b.allow(&["flee_m"])?;
                    MobRole::Thief { flee: mm(b.num("flee_m", 1.0, 30.0)?) }
                }
                ("exploder", Some(b)) => {
                    b.allow(&["radius_m", "dmg"])?;
                    MobRole::Exploder { radius: mm(b.num("radius_m", 0.5, 10.0)?), dmg: permille(b.num("dmg", 0.0, 1.0)?) }
                }
                ("berserker", Some(b)) => {
                    b.allow(&["below", "mult"])?;
                    MobRole::Berserker { below: permille(b.num("below", 0.05, 0.95)?), mult: permille(b.num("mult", 1.0, 3.0)?) }
                }
                ("frost_caster", Some(b)) => {
                    b.allow(&["range_m", "shot_mps", "every_s", "dmg", "freeze_s"])?;
                    let (range, shot_v, every, dmg) = shot(&b, hz)?;
                    MobRole::FrostCaster { range, shot_v, every, dmg, freeze: hz.ticks(b.num("freeze_s", 0.1, 5.0)?) as u16 }
                }
                ("shield", Some(b)) => {
                    b.allow(&["reduce"])?;
                    MobRole::Shield { reduce: permille(b.num("reduce", 0.05, 0.9)?) }
                }
                ("swarm", Some(b)) => {
                    b.allow(&["pack"])?;
                    MobRole::Swarm { pack: NonZeroU8::MIN.saturating_add(b.int("pack", 2, 20)? as u8 - 1) }
                }
                _ => {
                    return Err(r.err("role", "one of: \"melee\" ranged aura thief exploder berserker frost_caster shield swarm (as {\"name\":{..}}) \"boss\""));
                }
            };
            FamKind::Mob(Mob {
                role,
                hp: permille(r.num("hp", 0.05, 100.0)?),
                ice: r.int("ice", 1, 100)? as u8,
                hijack: hz.ticks(r.num("hijack_s", 0.5, 120.0)?) as u16,
                cost: permille(r.num("cost", 0.1, 100.0)?),
                weight: r.int("weight", 0, 1000)? as u32,
                unlock: hz.ticks(r.num("unlock_s", 0.0, 3600.0)?),
            })
        }
    };
    Ok(Fam {
        key: label(r, "id", 24)?,
        name: label(r, "name", 24)?,
        glyph: glyph(r, "glyph")?,
        hue: r.pick("hue", &HUES)?,
        speed: hz.mmpt(r.num("speed_mps", 0.1, 20.0)?),
        dmg: permille(r.num("dmg", 0.0, 50.0)?),
        radius: mm(r.num("radius_m", 0.1, 5.0)?),
        kind,
    })
}

const KINDS: [(&str, SkillKind); 11] = [
    ("bolt", SkillKind::Bolt),
    ("hack", SkillKind::Hack),
    ("orbit", SkillKind::Orbit),
    ("pulse", SkillKind::Pulse),
    ("chain", SkillKind::Chain),
    ("mine", SkillKind::Mine),
    ("might", SkillKind::Might),
    ("haste", SkillKind::Haste),
    ("plating", SkillKind::Plating),
    ("magnet", SkillKind::Magnet),
    ("regen", SkillKind::Regen),
];

fn skill(r: &Rec, hz: Hz) -> Result<Skill> {
    r.allow(&[
        "t", "id", "name", "hue", "kind", "target", "tags", "dmg", "cd_s", "count", "pierce", "speed_mps", "range_m", "area_m", "glitch", "flux",
        "arc_deg", "duration_s", "value", "per_rank", "r5", "desc",
    ])?;
    let mut tags = Tags::default();
    for t in r.strs("tags")? {
        let i = TAGS.iter().position(|x| *x == t).ok_or_else(|| r.err("tags", format!("'{t}' not one of {}", TAGS.join(" "))))?;
        tags = tags.or(Tags(1 << i));
    }
    let per = r.sub("per_rank")?;
    per.allow(&["dmg", "haste", "area", "value"])?;
    let r5 = match r.has("r5") {
        true => {
            let b = r.sub("r5")?;
            b.allow(&["count", "pierce", "glitch"])?;
            R5 { count: b.int_or("count", 0, 0, 16)? as u8, pierce: b.int_or("pierce", 0, 0, 50)? as u8, glitch: b.int_or("glitch", 0, 0, 50)? as u8 }
        }
        false => R5::default(),
    };
    let kind = r.pick("kind", &KINDS)?;
    let cd = hz.ticks(r.num_or("cd_s", 1.0, 0.1, 60.0)?) as u16;
    Ok(Skill {
        key: label(r, "id", 24)?,
        name: label(r, "name", 24)?,
        hue: r.pick("hue", &HUES)?,
        kind,
        target: r.pick("target", &[("nearest", Target::Nearest), ("densest", Target::Densest), ("self", Target::SelfCentered)])?,
        tags,
        base: SkillBase {
            dmg: mhp(r.num_or("dmg", 0.0, 0.0, 1e5)?),
            cd: cd.max(3),
            count: r.int_or("count", 1, 1, 16)? as u8,
            pierce: r.int_or("pierce", 0, 0, 50)? as u8,
            speed: hz.mmpt(r.num_or("speed_mps", 10.0, 0.5, 100.0)?),
            range: mm(r.num_or("range_m", 8.0, 0.5, 60.0)?),
            area: mm(r.num_or("area_m", 1.0, 0.1, 30.0)?),
            glitch: r.int_or("glitch", 0, 0, 50)? as u8,
            flux: mhp(r.num_or("flux", 0.0, 0.0, 1000.0)?),
            arc_cos: crate::trig::cos(crate::trig::deg(r.num_or("arc_deg", 60.0, 1.0, 360.0)? / 2.0)),
            duration: hz.ticks(r.num_or("duration_s", 1.0, 0.1, 60.0)?) as u16,
            value: permille(r.num_or("value", 0.0, 0.0, 100.0)?),
        },
        per: PerRank {
            dmg: permille(per.num_or("dmg", 0.0, 0.0, 10.0)?),
            haste: permille(per.num_or("haste", 0.0, 0.0, 10.0)?),
            area: permille(per.num_or("area", 0.0, 0.0, 10.0)?),
            value: permille(per.num_or("value", 0.0, 0.0, 100.0)?),
        },
        r5,
        desc: label(r, "desc", 70)?,
    })
}

fn attack(r: &Rec, hz: Hz, fam_id: &dyn Fn(&Rec, &str) -> Result<FamId>) -> Result<Attack> {
    r.allow(&["shape", "dmg", "lead_s", "every_s", "spawn", "n"])?;
    let every = hz.ticks(r.num("every_s", 0.5, 120.0)?) as u16;
    if r.has("spawn") {
        if r.has("shape") || r.has("dmg") {
            return Err(r.err("spawn", "spawn attack has no shape/dmg"));
        }
        let fam = fam_id(r, "spawn")?;
        return Ok(Attack { every, lead: Lead::parse(r, hz, 0)?, what: AttackKind::Spawn { fam, n: r.int("n", 1, 50)? as u8 } });
    }
    let dmg = permille(r.num("dmg", 0.0, 1.0)?);
    let (name, body) = r.variant("shape")?;
    let m = |k: &str| match &body {
        Some(b) => b.num(k, 0.1, 100.0).map(mm),
        None => Err(r.err("shape", format!("'{name}' needs {{{k}}}"))),
    };
    let shape = match name {
        "band" => ShapeDef::Band { half: m("half_m")? },
        "cross" => ShapeDef::Cross { half: m("half_m")? },
        "lattice" => ShapeDef::Lattice { pitch: m("pitch_m")?, half: m("half_m")? },
        "circle" => ShapeDef::Circle { r: m("r_m")? },
        "fan" => ShapeDef::Fan {
            n: body.as_ref().map(|b| b.int("n", 1, 12)).transpose()?.unwrap_or(3) as u8,
            r: m("r_m")?,
            spread: m("spread_m")?,
        },
        "arena" => ShapeDef::Arena,
        _ => return Err(r.err("shape", "one of band cross lattice circle fan arena")),
    };
    Ok(Attack { every, lead: Lead::parse(r, hz, dmg)?, what: AttackKind::Area { shape, dmg } })
}

fn boss(r: &Rec, hz: Hz, fam_id: &dyn Fn(&Rec, &str) -> Result<FamId>, fams: &[Fam]) -> Result<BossDef> {
    r.allow(&["t", "family", "ref_hp", "cloud", "w", "h", "phases", "enrage_s", "enrage_cadence", "hardcap_s", "hardcap"])?;
    let fam = fam_id(r, "family")?;
    if !matches!(fams[fam.ix()].kind, FamKind::Boss { .. }) {
        return Err(r.err("family", "family role must be \"boss\""));
    }
    let mut phases = Vec::new();
    let mut last = 1001;
    for (i, p) in r.list("phases")?.iter().enumerate() {
        let p = r.at(p, &format!("phases[{i}]"))?;
        p.allow(&["until", "attacks"])?;
        let until = permille(p.num("until", 0.0, 1.0)?);
        if until >= last {
            return Err(p.err("until", "phase thresholds must fall"));
        }
        last = until;
        let attacks = p.list("attacks")?.iter().enumerate().map(|(j, a)| attack(&p.at(a, &format!("attacks[{j}]"))?, hz, fam_id)).collect::<Result<Vec<_>>>()?;
        phases.push(BossPhase { until, attacks });
    }
    if last != 0 {
        return Err(r.err("phases", "last phase must have until = 0"));
    }
    Ok(BossDef {
        fam,
        ref_hp: mhp(r.num("ref_hp", 1.0, 1e5)?),
        cloud: glyphs(r, "cloud")?,
        w: r.int("w", 1, 21)? as u8,
        h: r.int("h", 1, 11)? as u8,
        phases,
        enrage: hz.ticks(r.num("enrage_s", 1.0, 3600.0)?),
        enrage_cadence: permille(r.num("enrage_cadence", 0.1, 1.0)?),
        hardcap: hz.ticks(r.num("hardcap_s", 1.0, 3600.0)?),
        hard: attack(&r.sub("hardcap")?, hz, fam_id)?,
    })
}

#[derive(Clone, Copy)]
enum CadTier {
    Elite,
    Champion,
    Mini,
}

/// `miniboss` record: a weight-0 mob family with a size floor, a glyph of its own, and 1..=4 attacks.
fn miniboss(r: &Rec, hz: Hz, fam_id: &dyn Fn(&Rec, &str) -> Result<FamId>, fams: &[Fam], seen: &[MiniDef]) -> Result<MiniDef> {
    r.allow(&["t", "family", "attacks"])?;
    let fam = fam_id(r, "family")?;
    let f = &fams[fam.ix()];
    match &f.kind {
        FamKind::Mob(m) if m.weight == 0 => {}
        _ => return Err(r.err("family", "miniboss family must be a mob with weight 0 (kept out of the trash pool)")),
    }
    if f.radius < mm(MINI_MIN_RADIUS_M) {
        return Err(r.err("family", format!("miniboss radius_m must be >= {MINI_MIN_RADIUS_M}")));
    }
    if seen.iter().any(|m| m.fam == fam) {
        return Err(r.err("family", "second miniboss record for this family"));
    }
    if fams.iter().enumerate().any(|(i, o)| i != fam.ix() && o.glyph == f.glyph) {
        return Err(r.err("family", "miniboss glyph must be unique among families"));
    }
    let attacks = r.list("attacks")?.iter().enumerate().map(|(j, a)| attack(&r.at(a, &format!("attacks[{j}]"))?, hz, fam_id)).collect::<Result<Vec<_>>>()?;
    if !(1..=4).contains(&attacks.len()) {
        return Err(r.err("attacks", "need 1..=4 attacks"));
    }
    Ok(MiniDef { fam, attacks })
}

fn waves(recs: &[(String, Rec)], hz: Hz, fam_id: &dyn Fn(&Rec, &str) -> Result<FamId>, fams: &[Fam], bosses: &[BossDef], s: &SpawnDef) -> Result<WavePlan> {
    let mut evs = Vec::new();
    let (mut elite, mut champ, mut mini) = (None, None, None);
    let mut minis: Vec<MiniDef> = Vec::new();
    for (t, r) in recs {
        let at = || r.num("at_s", 0.0, 7200.0).map(|v| hz.ticks(v));
        match t.as_str() {
            "cadence" => {
                r.allow(&["t", "tier", "from_s", "every_s"])?;
                let c = Cadence {
                    from: hz.ticks(r.num("from_s", 0.0, 7200.0)?),
                    every: NonZeroU32::new(hz.ticks(r.num("every_s", 1.0, 7200.0)?)).ok_or_else(|| r.err("every_s", "> 0"))?,
                };
                match r.pick("tier", &[("elite", CadTier::Elite), ("champion", CadTier::Champion), ("miniboss", CadTier::Mini)])? {
                    CadTier::Elite => elite = Some(c),
                    CadTier::Champion => champ = Some(c),
                    CadTier::Mini => mini = Some(c),
                }
            }
            "miniboss" => {
                let m = miniboss(r, hz, fam_id, fams, &minis)?;
                minis.push(m);
            }
            "horde" => {
                r.allow(&["t", "at_s", "pattern", "n", "family"])?;
                let n = r.int("n", 1, 2000)?;
                if n as usize > s.live_cap {
                    return Err(r.err("n", format!("horde {n} > live cap {} (O-cap)", s.live_cap)));
                }
                let fam = fam_id(r, "family")?;
                if !matches!(fams[fam.ix()].kind, FamKind::Mob(_)) {
                    return Err(r.err("family", "hordes are mobs"));
                }
                evs.push(WaveEv::Horde {
                    at: at()?,
                    pattern: r.pick("pattern", &[("ring", Formation::Ring), ("wall", Formation::Wall), ("spiral", Formation::Spiral)])?,
                    n: NonZeroU16::new(n as u16).ok_or_else(|| r.err("n", "> 0"))?,
                    fam,
                });
            }
            "boss" => {
                r.allow(&["t", "at_s", "family", "final"])?;
                let fam = fam_id(r, "family")?;
                if !bosses.iter().any(|b| b.fam == fam) {
                    return Err(r.err("family", "no boss record for this family"));
                }
                evs.push(WaveEv::Boss { at: at()?, fam, last: r.flag("final")? });
            }
            "thin" => {
                r.allow(&["t", "at_s", "mult"])?;
                evs.push(WaveEv::Thin { at: at()?, mult: permille(r.num("mult", 0.0, 1.0)?) });
            }
            "narrate" => {
                r.allow(&["t", "at_s", "text"])?;
                evs.push(WaveEv::Narrate { at: at()?, text: label(r, "text", 80)? });
            }
            _ => return Err(r.err("t", "one of cadence horde boss thin narrate miniboss")),
        }
    }
    let finals = evs.iter().filter(|e| matches!(e, WaveEv::Boss { last: true, .. })).count();
    if finals != 1 {
        return Err(Error::Data { file: "waves.jsonl".into(), line: 0, path: String::new(), msg: "need exactly one final boss".into() });
    }
    evs.sort_by_key(WaveEv::at);
    let mut minis = minis.into_iter();
    let mini = match (mini, minis.next()) {
        (Some(cadence), Some(head)) => Some(MiniPlan { cadence, head, tail: minis.collect() }),
        (None, None) => None,
        _ => return Err(Error::Data { file: "waves.jsonl".into(), line: 0, path: String::new(), msg: "miniboss records and the miniboss cadence come together".into() }),
    };
    match (elite, champ) {
        (Some(elite), Some(champion)) => Ok(WavePlan { evs, elite, champion, mini }),
        _ => Err(Error::Data { file: "waves.jsonl".into(), line: 0, path: String::new(), msg: "need one elite and one champion cadence".into() }),
    }
}

fn binds(recs: &[(String, Rec)]) -> Result<(Vec<Bind>, Vec<Group>)> {
    let (mut bs, mut gs) = (Vec::new(), Vec::new());
    for (t, r) in recs {
        let mode = r.pick("mode", &[("play", Mode::Play), ("pick", Mode::Pick), ("look", Mode::Look), ("cmd", Mode::Cmd), ("tree", Mode::Tree), ("gear", Mode::Gear), ("shop", Mode::Shop)])?;
        let keys = r.strs("keys")?.iter().map(|k| Key::parse(k).ok_or_else(|| r.err("keys", format!("unknown key '{k}'")))).collect::<Result<Vec<_>>>()?;
        if keys.is_empty() {
            return Err(r.err("keys", "empty"));
        }
        match t.as_str() {
            "bind" => {
                r.allow(&["t", "mode", "keys", "action", "label"])?;
                let action = Action::parse(r.text("action")?).ok_or_else(|| r.err("action", "unknown action"))?;
                if bs.iter().any(|b: &Bind| b.mode == mode && (b.keys.starts_with(&keys) || keys.starts_with(&b.keys))) {
                    return Err(r.err("keys", "prefix clash with an earlier binding in this mode"));
                }
                bs.push(Bind { mode, keys, action, label: label(r, "label", 20)? });
            }
            "group" => {
                r.allow(&["t", "mode", "keys", "label"])?;
                gs.push(Group { mode, keys, label: label(r, "label", 20)? });
            }
            _ => return Err(r.err("t", "one of bind group")),
        }
    }
    Ok((bs, gs))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn texts() -> Vec<String> {
        FILES.iter().map(|f| std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("data").join(f)).unwrap_or_default()).collect()
    }

    pub fn content() -> Content {
        Content::parse(&texts()).unwrap()
    }

    fn edit(file: usize, from: &str, to: &str) -> String {
        let mut t = texts();
        assert!(t[file].contains(from), "fixture text '{from}' not found");
        t[file] = t[file].replacen(from, to, 1);
        match Content::parse(&t) {
            Ok(_) => "OK".into(),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn shipped_data_loads() {
        let c = content();
        assert_eq!(c.hz(), Hz::H30);
        assert!(!c.opt.pick_pauses, "picks keep play going (popups force auto-walk and auto-fire)");
        assert_eq!(c.player.speed, 167);
        assert!(c.skills.len() >= 6 && c.fams.len() >= 5);
    }

    #[test]
    fn zero_telegraph_boss_attack_rejected() {
        let e = edit(5, "\"lead_s\":1.0", "\"lead_s\":0.0");
        assert!(e.contains("bosses.jsonl") && e.contains("lead_s") && e.contains("fairness floor"), "{e}");
    }

    #[test]
    fn mob_faster_than_player_rejected() {
        let e = edit(3, "\"speed_mps\":3.4", "\"speed_mps\":5.5");
        assert!(e.contains("enemies.jsonl:2 .speed_mps: mobs must be slower"), "{e}");
    }

    #[test]
    fn new_foe_roles_load_and_reject_bad_params() {
        let c = content();
        let roles: Vec<MobRole> = c.fams.iter().filter_map(|f| if let FamKind::Mob(m) = &f.kind { Some(m.role) } else { None }).collect();
        for want in ["Exploder", "Berserker", "FrostCaster", "Shield", "Swarm"] {
            assert!(roles.iter().any(|r| format!("{r:?}").starts_with(want)), "no {want} family shipped");
        }
        // file 3 = enemies.jsonl; rager is line 12
        let msg = "enemies.jsonl:12 .role: enraged swift speed must stay below the player and 4.8 m/s (design sec 8.4)";
        assert_eq!(edit(3, "\"mult\":1.5", "\"mult\":3"), msg);
        // 2.6 m/s x 1.6 x 1.2 swift = 4.99 m/s: below the 5.0 player, only the 4.8 cap catches it
        assert_eq!(edit(3, "\"mult\":1.5", "\"mult\":1.6"), msg);
        assert_eq!(edit(3, "\"mult\":1.5", "\"mult\":1.5"), "OK", "control");
        assert!(edit(3, "\"pack\":6", "\"pack\":1").contains(".pack"));
        assert!(edit(3, "\"reduce\":0.5", "\"reduce\":0.99").contains(".reduce"));
        assert!(edit(3, "\"freeze_s\":1.2", "\"freeze_s\":60").contains(".freeze_s"));
        assert!(edit(3, "\"radius_m\":2.5", "\"radius\":2.5").contains("exploder"));
        assert!(edit(3, "\"berserker\"", "\"berzerk\"").contains("one of"));
        assert_eq!(edit(3, "\"mult\":1.5", "\"mult\":1.4"), "OK", "control");
    }

    #[test]
    fn miniboss_families_load_and_reject_bad_records() {
        let c = content();
        let plan = c.mini.as_ref().unwrap();
        assert_eq!(plan.len(), 3);
        assert!(plan.iter().all(|m| c.fam(m.fam).radius >= mm(MINI_MIN_RADIUS_M) && !m.attacks.is_empty()));
        let fams: Vec<FamId> = plan.iter().map(|m| m.fam).collect();
        assert_eq!((plan.nth(0).fam, plan.nth(1).fam, plan.nth(2).fam, plan.nth(3).fam), (fams[0], fams[1], fams[2], fams[0]), "round robin");
        let w = 6;
        assert!(edit(w, "\"family\":\"chute_warden\"", "\"family\":\"brute\"").contains("weight 0"));
        assert!(edit(3, "\"radius_m\":0.8", "\"radius_m\":0.4").contains("radius_m must be >= 0.6"));
        assert!(edit(3, "\"glyph\":\"W\"", "\"glyph\":\"B\"").contains("glyph must be unique"));
        assert!(edit(w, "\"lead_s\":0.8,\"dmg\":0.08,\"every_s\":5.0", "\"lead_s\":0.1,\"dmg\":0.08,\"every_s\":5.0").contains("fairness floor"));
        assert!(edit(w, "\"tier\":\"miniboss\",\"from_s\":120,\"every_s\":75", "\"tier\":\"champion\",\"from_s\":120,\"every_s\":75").contains("come together"));
        assert!(edit(w, "\"attacks\":[{\"spawn\":\"mite\"", "\"attacks\":[],\"x\":[{\"spawn\":\"mite\"").contains("unknown key"));
        assert!(edit(w, "\"spawn\":\"mite\"", "\"spawn\":\"nope\"").contains("no family"));
    }

    fn scheme_text(id: &str) -> String {
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("data/schemes").join(format!("{id}.yaml"))).unwrap_or_default()
    }

    fn scheme(id: &str) -> Scheme {
        Scheme::parse(id, &scheme_text(id)).unwrap()
    }

    #[test]
    fn scheme_parser_accepts_dracula_and_rejects_bad_files() {
        let d = scheme("dracula");
        assert_eq!((d.variant, d.slots[0], d.slots[0x17]), (Variant::Dark, Rgb(0x28, 0x2a, 0x36), Rgb(0xff, 0x92, 0xdf)));
        let t = scheme_text("dracula");
        assert!(Scheme::parse("d.yaml", &t.replacen("name: \"Dracula\"", "", 1)).unwrap_err().to_string().contains(".name: missing"));
        let short = t.replacen("  base17:", "  #base17:", 1);
        assert_eq!(Scheme::parse("d.yaml", &short).err().map(|e| e.to_string()), Some("d.yaml:0 .palette: need exactly 24 slots, got 23".into()));
        let dup = t.replacen("  base17:", "  base16:", 1);
        assert!(Scheme::parse("d.yaml", &dup).unwrap_err().to_string().contains("d.yaml:37 .palette.base16: duplicate slot"));
        assert!(Scheme::parse("d.yaml", &t.replacen("#282a36", "#28", 1)).unwrap_err().to_string().contains(".palette.base00: need #rrggbb"));
        assert!(Scheme::parse("d.yaml", &t.replacen("base08", "base18", 1)).unwrap_err().to_string().contains("slot is base00..base17"));
        assert!(Scheme::parse("d.yaml", &t.replacen("\"dark\"", "\"dim\"", 1)).unwrap_err().to_string().contains(".variant"));
    }

    #[test]
    fn every_vendored_scheme_parses_and_resolves_to_readable_ink() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/schemes");
        let world = WorldScheme::new("dracula", scheme("dracula")).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.ends_with(".yaml")).collect();
        assert_eq!(names.len(), 243);
        for n in names {
            let s = Scheme::parse(&n, &scheme_text(n.trim_end_matches(".yaml"))).unwrap();
            let t = RoleTable::of(&s, &world);
            let bg = t.get(Role::Bg);
            for r in ROLES.into_iter().filter(|r| r.is_ink()) {
                assert!(contrast(t.get(r), bg) >= MIN_CONTRAST, "{n} {r:?} {:.2}", contrast(t.get(r), bg));
            }
        }
    }

    #[test]
    fn low_contrast_danger_falls_back_to_bright_twin_then_extremes() {
        let world = WorldScheme::new("dracula", scheme("dracula")).unwrap();
        let g = scheme("gruvbox-dark");
        assert!(contrast(g.slots[0x08], g.slots[0]) < 3.0, "fixture: gruvbox danger is below 3:1");
        let t = RoleTable::of(&g, &world);
        assert_eq!(t.get(Role::Danger), g.slots[0x12], "twin");
        assert_eq!(t.get(Role::Text), g.slots[Role::Text.slot()], "control: readable roles are untouched");
        // a scheme whose twin is also unreadable falls to the world, never below 3:1
        let mut bad = scheme("gruvbox-dark");
        bad.slots[0x08] = bad.slots[0];
        bad.slots[0x12] = bad.slots[0];
        let t = RoleTable::of(&bad, &world);
        assert_eq!(t.get(Role::Danger), world.0.slots[0x08], "world");
        // light region over a dark world: bg is light, world colours are light-on-dark so they fail too
        let l = scheme("gruvbox-light");
        let t = RoleTable::of(&l, &world);
        assert!(ROLES.into_iter().filter(|r| r.is_ink()).all(|r| contrast(t.get(r), t.get(Role::Bg)) >= 3.0));
    }

    #[test]
    fn light_scheme_rejected_as_world() {
        let e = WorldScheme::new("schemes/gruvbox-light.yaml", scheme("gruvbox-light")).err().map(|e| e.to_string());
        assert_eq!(e, Some("schemes/gruvbox-light.yaml:0 .variant: the world scheme must be dark".into()));
        assert!(WorldScheme::new("d", scheme("dracula")).is_ok(), "control");
    }

    #[test]
    fn palette_scheme_id_is_a_plain_file_name() {
        let t = texts();
        let with = |id: &str| {
            let mut t = t.clone();
            t[2] = t[2].replacen("\"t\":\"palette\",", &format!("\"t\":\"palette\",\"scheme\":\"{id}\","), 1);
            Content::parse(&t).map(|c| c.pal.scheme.map(|s| s.as_str().to_string())).map_err(|e| e.to_string())
        };
        assert_eq!(with("dracula"), Ok(Some("dracula".into())));
        assert!(with("../x").unwrap_err().contains("palette.jsonl:2 .scheme: scheme id is"));
        assert!(with("").unwrap_err().contains(".scheme: need 1..=40"), "empty id");
        assert!(with("Dracula").unwrap_err().contains("scheme id is"), "upper case");
    }

    #[test]
    fn legacy_palette_roles_read_the_old_fields() {
        let c = content();
        assert_eq!((c.pal.role(Role::Danger), c.pal.role(Role::Bg)), (c.pal.alarm, c.pal.bg));
    }

    #[test]
    fn scaling_rises_with_level_and_stays_capped() {
        let c = content();
        let (s, hz) = (c.scale, c.hz());
        let min = hz.ticks(60.0);
        // par at 2 min = level 2.6: a level-2 hero is on the clock, level 10 is far ahead
        assert_eq!(s.phase_rate(hz, 2 * min, 2), 1000, "at or under par: normal clock");
        let (r5, r10, r99) = (s.phase_rate(hz, 2 * min, 5), s.phase_rate(hz, 2 * min, 10), s.phase_rate(hz, 2 * min, 99));
        assert!(1000 < r5 && r5 < r10 && r10 <= r99 && r99 == 2000, "{r5} {r10} {r99}: monotone, capped at 1 + speedup_cap");
        let base = NonZeroU32::new(2250).unwrap();
        assert_eq!(s.mini_every(base, 1), base);
        assert!(s.mini_every(base, 20) < s.mini_every(base, 5) && s.mini_every(base, 5) < base);
        assert_eq!(s.mini_every(base, 200).get(), 2250 * 1000 / 2500, "cap 1.5");
        let n = NonZeroU16::new(24).unwrap();
        assert_eq!(s.horde_n(n, 1, 600), n);
        assert!(s.horde_n(n, 20, 600) > n && s.horde_n(n, 200, 600).get() == 60);
        assert_eq!(s.horde_n(n, 200, 30).get(), 30, "never above the live cap");
    }

    #[test]
    fn trees_ship_two_classes_three_trees_and_validate() {
        let c = content();
        assert_eq!((c.classes.len(), c.trees.len()), (2, 6));
        assert!((0..2).all(|k| c.trees.iter().filter(|t| t.class.ix() == k).count() == 3));
        let tags: Vec<_> = c.trees.iter().map(|t| t.tag).collect();
        assert!([PowerTag::Magic, PowerTag::Melee, PowerTag::Necro, PowerTag::Thunder, PowerTag::Speed, PowerTag::Dex].iter().all(|t| tags.contains(t)));
        let f = 8; // trees.jsonl
        let e = edit(f, "\"requires\":[\"packet0\"],\"col\":0", "\"requires\":[\"nope\"],\"col\":0");
        assert_eq!(e, "trees.jsonl:6 .requires: no node 'nope' in tree 'packet'");
        assert!(edit(f, "\"effect\":\"chain\",\"cost\":1,\"tag\":\"thunder\"", "\"effect\":\"nothing\",\"cost\":1,\"tag\":\"thunder\"").contains(".effect: no skill 'nothing'"));
        assert!(edit(f, "\"cost\":3,\"requires\":[\"packet3\"]", "\"cost\":0,\"requires\":[\"packet3\"]").contains(".cost: 0 outside [1, 9]"));
        assert!(edit(f, "\"class\":\"scripter\",\"tag\":\"magic\"", "\"class\":\"wizard\",\"tag\":\"magic\"").contains("no class 'wizard'"));
        assert!(edit(f, "\"tag\":\"magic\"", "\"tag\":\"fire\"").contains("not one of"));
        assert!(edit(f, "\"id\":\"packet2\"", "\"id\":\"packet1\"").contains("duplicate node"));
        assert!(edit(f, "\"col\":2,\"row\":1}", "\"col\":0,\"row\":1}").contains("grid cell already used"));
        // cycle: the root now needs its own descendant, so nothing is reachable
        let cyc = edit(f, "\"id\":\"packet0\",\"name\":\"SYN FLOOD\",\"effect\":\"bolt\",\"cost\":1,\"requires\":[]", "\"id\":\"packet0\",\"name\":\"SYN FLOOD\",\"effect\":\"bolt\",\"cost\":1,\"requires\":[\"packet4\"]");
        assert!(cyc.contains("not reachable from a root"), "{cyc}");
        assert_eq!(edit(f, "\"cost\":1,\"requires\":[]", "\"cost\":1,\"requires\":[]"), "OK", "control");
    }

    #[test]
    fn spend_follows_requirements_and_points() {
        let c = content();
        let t = &c.trees[0];
        let none = Owned::default();
        assert_eq!(t.spend(none, 5, 3), Err(SpendErr::Locked { missing: 1 }));
        assert_eq!(t.spend(none, 0, 0), Err(SpendErr::NeedPoints { cost: 1, have: 0 }));
        assert_eq!(t.spend(none, 1, 99), Err(SpendErr::NoSuchNode));
        let (o, left) = t.spend(none, 5, 0).unwrap();
        assert_eq!((o.has(0), o.count(), left), (true, 1, 4));
        assert_eq!(t.spend(o, 5, 0), Err(SpendErr::AlreadyOwned));
        let (o, _) = t.spend(o, 4, 1).and_then(|(o, p)| t.spend(o, p, 2)).unwrap();
        assert_eq!(t.spend(o, 3, 3).map(|(o, p)| (o.count(), p)), Ok((4, 1)));
        assert_eq!(t.at(1, 2), Some(3));
        assert_eq!(t.at(9, 9), None);
    }

    #[test]
    fn items_ship_every_slot_and_rarity_and_validate() {
        let c = content();
        for s in [Slot::Armor, Slot::Ring, Slot::Jewel, Slot::Weapon, Slot::Charm] {
            assert!(c.items.iter().any(|i| i.slot == s), "no {s:?} item");
        }
        for r in [Rarity::Common, Rarity::Rare, Rarity::Epic, Rarity::Legendary, Rarity::Unique] {
            assert!(c.items.iter().any(|i| i.rarity == r), "no {r:?} item");
        }
        let hp = c.items.iter().find(|i| i.key == "hazmat_plate").unwrap();
        assert_eq!(hp.mods, vec![Mod { stat: Stat::Armour, value: 6 }, Mod { stat: Stat::MaxHp, value: 40_000 }]);
        assert_eq!(Rarity::Unique.role(), Role::Unique);
        let f = 9; // items.jsonl
        assert_eq!(edit(f, "\"stat\":\"armour\",\"value\":3", "\"stat\":\"luck\",\"value\":3"), "items.jsonl:2 .mods[0].stat: 'luck' not one of: max_hp regen armour pickup speed damage haste area crit");
        assert!(edit(f, "\"stat\":\"armour\",\"value\":3", "\"stat\":\"armour\",\"value\":300").contains(".mods[0].value: 300 outside [-50, 50]"));
        assert!(edit(f, "\"stat\":\"armour\",\"value\":3", "\"stat\":\"armour\",\"value\":0").contains("zero mod"));
        assert!(edit(f, "\"slot\":\"armor\"", "\"slot\":\"boots\"").contains("not one of"));
        assert!(edit(f, "\"rarity\":\"common\"", "\"rarity\":\"mythic\"").contains("not one of"));
        // common carries 1 mod
        assert!(edit(f, "\"mods\":[{\"stat\":\"armour\",\"value\":3}]", "\"mods\":[{\"stat\":\"armour\",\"value\":3},{\"stat\":\"haste\",\"value\":0.1}]").contains("Common items carry 1..=1 mods, got 2"));
        assert!(edit(f, "{\"stat\":\"armour\",\"value\":6},{\"stat\":\"max_hp\"", "{\"stat\":\"armour\",\"value\":6},{\"stat\":\"armour\"").contains("stat repeated"));
        assert!(edit(f, "\"id\":\"lint_chip\"", "\"id\":\"mesh_vest\"").contains("duplicate item"));
        assert_eq!(edit(f, "\"value\":3}", "\"value\":4}"), "OK", "control");
    }

    fn data_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("data")
    }

    #[test]
    fn every_installed_profile_loads_and_default_is_the_base_data() {
        let d = data_dir();
        let ps = profiles(&d).unwrap();
        assert_eq!(ps.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["cyber-sanitation", "dark-fantasy"]);
        let base = Content::load(&d).unwrap();
        let cyber = Content::load_profile(&d, Some("cyber-sanitation")).unwrap();
        let names = |c: &Content| c.fams.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&cyber), names(&base), "cyber-sanitation = the base data");
        let dark = Content::load_profile(&d, Some("dark-fantasy")).unwrap();
        assert_ne!(names(&dark), names(&base));
        assert!(names(&dark).contains(&"BONE ABBOT".to_string()) && dark.mini.as_ref().map(MiniPlan::len) == Some(3) && dark.trees.len() == 6 && dark.items.len() == 14);
        // dark-fantasy names its colour scheme: roles come from it, not from the legacy fields
        assert_eq!(dark.pal.role(Role::Bg), scheme("lumina-oxblood-dark").slots[0]);
        assert_eq!(cyber.pal.role(Role::Bg), cyber.pal.bg);
        // options and bindings are never themed
        assert_eq!(dark.binds.len(), base.binds.len());
    }

    #[test]
    fn unknown_or_traversing_profile_is_a_usage_error_listing_the_real_ones() {
        let d = data_dir();
        for bad in ["nope", "../data", "", "Dark-Fantasy"] {
            let e = Content::load_profile(&d, Some(bad)).err().map(|e| e.to_string()).unwrap_or_default();
            assert!(e.contains(&format!("--profile '{bad}' is not installed; available: cyber-sanitation dark-fantasy")), "{e}");
        }
    }

    #[test]
    fn every_profile_tells_its_story_from_minute_one_without_overlap() {
        let d = data_dir();
        for p in ["cyber-sanitation", "dark-fantasy"] {
            let c = Content::load_profile(&d, Some(p)).unwrap();
            let at: Vec<u32> = c.waves.iter().filter_map(|w| if let WaveEv::Narrate { at, .. } = w { Some(*at) } else { None }).collect();
            let minute = c.hz().ticks(60.0);
            assert!(at.iter().filter(|t| **t < minute).count() >= 3, "{p}: 3 story lines in the first minute");
            let shown = c.hz().get() * 6; // render keeps a line 6 s
            assert!(at.windows(2).all(|w| w[1] - w[0] >= shown), "{p}: a line must not cut off the previous one {at:?}");
        }
    }

    #[test]
    fn profile_manifest_id_must_match_its_directory() {
        let d = data_dir().join("profiles/dark-fantasy");
        let e = manifest(&d, "other").err().map(|e| e.to_string());
        assert_eq!(e, Some("profile:2 .id: must equal the directory name 'other'".into()));
        assert!(manifest(&d, "dark-fantasy").is_ok(), "control");
    }

    #[test]
    fn scaling_record_rejects_out_of_range() {
        assert!(edit(1, "\"speedup_cap\":1.0", "\"speedup_cap\":9").contains("tuning.jsonl:") && edit(1, "\"speedup_cap\":1.0", "\"speedup_cap\":9").contains(".speedup_cap: 9 outside [0, 4]"));
        assert!(edit(1, "\"mini_cap\":1.5", "\"mini_cap\":-1").contains(".mini_cap"));
    }

    #[test]
    fn arena_decor_cell_must_be_positive() {
        // floorgen divides by this cell: 0 must never load (floor of the range is 2 m)
        let e = edit(1, "\"decor_cell_m\":14", "\"decor_cell_m\":0");
        assert!(e.contains("tuning.jsonl:") && e.contains(".decor_cell_m: 0 outside [2, 100]"), "{e}");
        assert_eq!(edit(1, "\"decor_cell_m\":14", "\"decor_cell_m\":14"), "OK", "control");
    }

    #[test]
    fn closed_enums_reject_unknown() {
        assert!(edit(0, "\"rewind\":\"off\"", "\"rewind\":\"on\"").contains("rewind is not in FP1"));
        assert!(edit(0, "\"tick_hz\":30", "\"tick_hz\":45").contains("only 30 or 60"));
        assert!(edit(4, "\"hue\":\"code\"", "\"hue\":\"fire\"").contains("not one of"));
    }

    #[test]
    fn horde_over_cap_rejected_and_ring_inside_view_rejected() {
        assert!(edit(6, "\"n\":24", "\"n\":1500").contains("O-cap"));
        assert!(edit(1, "\"ring_min_m\":26", "\"ring_min_m\":10").contains("outside view"));
    }

    #[test]
    fn iso_knobs_blame_their_path() {
        let e = edit(1, "\"block_rows\":[7,2]", "\"block_rows\":[7,9]");
        assert!(e.contains("tuning.jsonl") && e.contains("block_rows") && e.contains("need 2 integers in [0, 8]"), "{e}");
        assert!(edit(1, "\"actor_rows\":[2,2,3]", "\"actor_rows\":[2,2]").contains("need 3 integers"));
        assert!(edit(2, "\"face_glyphs\":\"▓█▒\"", "\"face_glyphs\":\"▓█\"").contains("top, lit side, dark side"));
        assert_eq!(edit(1, "\"cut_cols\":10", "\"cut_cols\":0"), "OK", "control: 0 = no cutaway is valid");
    }

    #[test]
    fn sixty_hz_option_rescales_ticks() {
        let mut t = texts();
        t[0] = t[0].replacen("\"tick_hz\":30", "\"tick_hz\":60", 1);
        let c = Content::parse(&t).unwrap();
        assert_eq!(c.player.speed, 83);
        assert_eq!(c.player.dodge_cd, 180);
    }
}
