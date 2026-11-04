//! Typed content (design sec 3 CONTENT, sec 11 knobs [E]). Every number the sim uses comes from
//! `data/*.jsonl`; seconds and metres are converted once here to ticks and milli-metres.
use crate::input::{Action, Key, Mode};
use crate::json::{Rec, records};
use crate::{Error, Result};
use std::num::{NonZeroU16, NonZeroU32};
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
        s.chars().map(|c| Glyph::of(c).unwrap_or(Glyph('?')))
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

fn rgb(r: &Rec, k: &str) -> Result<Rgb> {
    let s = r.text(k)?;
    let hex = s.strip_prefix('#').filter(|h| h.len() == 6).ok_or_else(|| r.err(k, "need #rrggbb"))?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("x"), 16).map_err(|_| r.err(k, "need #rrggbb"));
    Ok(Rgb(byte(0)?, byte(2)?, byte(4)?))
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
}

impl Palette {
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

// ---------- content ----------

pub struct Content {
    pub opt: Options,
    pub player: PlayerDef,
    pub spawn: SpawnDef,
    pub pace: [PacePhase; 4],
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
    pub binds: Vec<Bind>,
    pub groups: Vec<Group>,
}

pub const MAX_SKILLS: usize = 32;
const FILES: [&str; 8] =
    ["options.jsonl", "tuning.jsonl", "palette.jsonl", "enemies.jsonl", "skills.jsonl", "bosses.jsonl", "waves.jsonl", "bindings.jsonl"];

impl Content {
    pub fn load(dir: &Path) -> Result<Content> {
        let mut texts = Vec::new();
        for f in FILES {
            texts.push(std::fs::read_to_string(dir.join(f)).map_err(|e| Error::Data {
                file: f.into(),
                line: 0,
                path: String::new(),
                msg: e.to_string(),
            })?);
        }
        Content::parse(&texts)
    }

    /// Parse the 8 files given as texts in FILES order (tests feed edited copies).
    pub fn parse(texts: &[String]) -> Result<Content> {
        let mut vals: Vec<Vec<serde_json::Value>> = FILES.iter().map(|_| Vec::new()).collect();
        let mut recs = Vec::new();
        for ((f, t), v) in FILES.iter().zip(texts).zip(vals.iter_mut()) {
            recs.push(records(f, t, v)?);
        }
        let mut it = recs.into_iter();
        let mut next = || it.next().unwrap_or_default();
        let (o, tu, pa, en, sk, bo, wa, bi) = (next(), next(), next(), next(), next(), next(), next(), next());
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
            }
        }
        if spawn.speed_cap >= player.speed {
            return Err(Error::Data { file: "tuning.jsonl".into(), line: 0, path: ".spawn.speed_cap_mps".into(), msg: "must stay below player speed (design sec 8.4)".into() });
        }
        let bosses = bo.iter().map(|(t, r)| want(t, r, "boss").and_then(|_| boss(r, hz, &fam_id, &fams))).collect::<Result<Vec<_>>>()?;
        let (waves, elite, champion) = waves(&wa, hz, &fam_id, &fams, &bosses, &spawn)?;
        let (binds, groups) = binds(&bi)?;
        Ok(Content {
            pace: pacing(only(&tu, "pacing", "tuning.jsonl")?, hz)?,
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
            binds,
            groups,
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
    let blink = r.num("tele_blink_hz", 0.5, 15.0)?;
    Ok(ViewDef {
        fog_start: mm(r.num("fog_start_m", 0.0, 100.0)?),
        fog_end: mm(r.num("fog_end_m", 1.0, 200.0)?),
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
        "density_glyphs", "face_glyphs", "shadow_glyph", "body_glyph",
    ])?;
    let d = glyphs(r, "density_glyphs")?;
    let [a, b, c] = d.as_slice() else { return Err(r.err("density_glyphs", "need 3 glyphs")) };
    let e = glyphs(r, "face_glyphs")?;
    let [d0, d1, d2] = e.as_slice() else { return Err(r.err("face_glyphs", "need 3 glyphs (top, lit side, dark side)")) };
    Ok(Palette {
        bg: rgb(r, "bg")?,
        fog: rgb(r, "fog")?,
        floor: rgb(r, "floor")?,
        player: rgb(r, "player")?,
        ring: rgb(r, "ring")?,
        alarm: rgb(r, "alarm")?,
        text: rgb(r, "text")?,
        dim: rgb(r, "dim")?,
        xp: rgb(r, "xp")?,
        chest: rgb(r, "chest")?,
        hud_bg: rgb(r, "hud_bg")?,
        hue: [rgb(r, "kin")?, rgb(r, "therm")?, rgb(r, "emp")?, rgb(r, "code")?],
        floor_glyphs: glyphs(r, "floor_glyphs")?,
        density: [*a, *b, *c],
        face: [*d0, *d1, *d2],
        shadow: glyph(r, "shadow_glyph")?,
        body: glyph(r, "body_glyph")?,
    })
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
                    MobRole::Ranged {
                        range: mm(b.num("range_m", 1.0, 30.0)?),
                        shot_v: hz.mmpt(b.num("shot_mps", 0.5, 8.0)?),
                        every: hz.ticks(b.num("every_s", 0.2, 60.0)?) as u16,
                        dmg: permille(b.num("dmg", 0.0, 20.0)?),
                    }
                }
                ("aura", Some(b)) => {
                    b.allow(&["radius_m", "ice"])?;
                    MobRole::Aura { radius: mm(b.num("radius_m", 0.5, 20.0)?), ice: b.int("ice", 1, 50)? as u8 }
                }
                ("thief", Some(b)) => {
                    b.allow(&["flee_m"])?;
                    MobRole::Thief { flee: mm(b.num("flee_m", 1.0, 30.0)?) }
                }
                _ => return Err(r.err("role", "one of: \"melee\" {\"ranged\":{..}} {\"aura\":{..}} {\"thief\":{..}} \"boss\"")),
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

type Waves = (Vec<WaveEv>, Cadence, Cadence);

fn waves(recs: &[(String, Rec)], hz: Hz, fam_id: &dyn Fn(&Rec, &str) -> Result<FamId>, fams: &[Fam], bosses: &[BossDef], s: &SpawnDef) -> Result<Waves> {
    let mut evs = Vec::new();
    let (mut elite, mut champ) = (None, None);
    for (t, r) in recs {
        let at = || r.num("at_s", 0.0, 7200.0).map(|v| hz.ticks(v));
        match t.as_str() {
            "cadence" => {
                r.allow(&["t", "tier", "from_s", "every_s"])?;
                let c = Cadence {
                    from: hz.ticks(r.num("from_s", 0.0, 7200.0)?),
                    every: NonZeroU32::new(hz.ticks(r.num("every_s", 1.0, 7200.0)?)).ok_or_else(|| r.err("every_s", "> 0"))?,
                };
                match r.pick("tier", &[("elite", true), ("champion", false)])? {
                    true => elite = Some(c),
                    false => champ = Some(c),
                }
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
            _ => return Err(r.err("t", "one of cadence horde boss thin narrate")),
        }
    }
    let finals = evs.iter().filter(|e| matches!(e, WaveEv::Boss { last: true, .. })).count();
    if finals != 1 {
        return Err(Error::Data { file: "waves.jsonl".into(), line: 0, path: String::new(), msg: "need exactly one final boss".into() });
    }
    evs.sort_by_key(WaveEv::at);
    match (elite, champ) {
        (Some(e), Some(c)) => Ok((evs, e, c)),
        _ => Err(Error::Data { file: "waves.jsonl".into(), line: 0, path: String::new(), msg: "need one elite and one champion cadence".into() }),
    }
}

fn binds(recs: &[(String, Rec)]) -> Result<(Vec<Bind>, Vec<Group>)> {
    let (mut bs, mut gs) = (Vec::new(), Vec::new());
    for (t, r) in recs {
        let mode = r.pick("mode", &[("play", Mode::Play), ("pick", Mode::Pick), ("look", Mode::Look), ("cmd", Mode::Cmd)])?;
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
        assert!(c.opt.pick_pauses);
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
