//! Legacy: what outlives the hero. Every run ends with souls earned (kills, level, bosses, a win); souls buy
//! ranks of boons in a small permanent tree; the boons make every later run stronger. Saved as JSON next to the
//! player's adventures, so a death never loses it.
use crate::data::Stat;
use crate::{Error, Result};
use std::path::{Path, PathBuf};

/// A permanent boon. The tree: three roots, three branches that each need rank 2 of one root, one crown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Boon {
    Vitality,
    Might,
    Fortune,
    Swiftness,
    Kinship,
    Wisdom,
    Insight,
}

/// Ranks per boon.
pub const MAX_RANK: u8 = 5;
/// Souls per run: one per KILLS_PER_SOUL kills, SOULS_PER_LEVEL per hero level, SOULS_PER_BOSS per boss, WIN_SOULS on a win.
const KILLS_PER_SOUL: u32 = 10;
const SOULS_PER_LEVEL: u32 = 5;
const SOULS_PER_BOSS: u32 = 50;
const WIN_SOULS: u32 = 200;

impl Boon {
    pub const ALL: [Boon; 7] = [Boon::Vitality, Boon::Might, Boon::Fortune, Boon::Swiftness, Boon::Kinship, Boon::Wisdom, Boon::Insight];

    /// Name in the save file.
    pub fn key(self) -> &'static str {
        match self {
            Boon::Vitality => "vitality",
            Boon::Might => "might",
            Boon::Fortune => "fortune",
            Boon::Swiftness => "swiftness",
            Boon::Kinship => "kinship",
            Boon::Wisdom => "wisdom",
            Boon::Insight => "insight",
        }
    }

    /// The boon and the rank of it this one needs first (the edges of the tree).
    pub fn needs(self) -> Option<(Boon, u8)> {
        match self {
            Boon::Vitality | Boon::Might | Boon::Fortune => None,
            Boon::Swiftness => Some((Boon::Vitality, 2)),
            Boon::Kinship => Some((Boon::Might, 2)),
            Boon::Wisdom => Some((Boon::Fortune, 2)),
            Boon::Insight => Some((Boon::Wisdom, 3)),
        }
    }

    /// Souls for the rank after `rank`: deeper boons cost more, every rank more than the last.
    pub fn cost(self, rank: u8) -> u32 {
        let base = match self.needs() {
            None => 20,
            Some(_) if self == Boon::Insight => 80,
            Some(_) => 40,
        };
        base * (u32::from(rank) + 1)
    }

    fn ix(self) -> usize {
        self as usize
    }
}

/// What one run earned souls with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunEnd {
    pub kills: u32,
    pub level: u16,
    pub bosses: u32,
    pub won: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpendErr {
    Maxed,
    Locked { needs: Boon, rank: u8 },
    NeedSouls { cost: u32, have: u32 },
}

/// The save: souls to spend, the rank of every boon, and the record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Legacy {
    pub souls: u32,
    ranks: [u8; Boon::ALL.len()],
    pub runs: u32,
    pub best_level: u16,
}

/// The numbers the boons add to a run. Stat bonuses are in the stat's data units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Boons {
    /// max HP, permille of the class's base HP
    pub hp: i64,
    pub damage: i64,
    pub speed: i64,
    /// XP gained, permille extra
    pub xp: u32,
    pub gold: u32,
    /// crew pals hit and heal harder, permille extra
    pub kin: i64,
    /// skill tree points at the start of a run
    pub points: u16,
}

impl Boons {
    /// The stat bonus of a stat, in its data units (`base_hp` = the class's base max HP, milli-HP).
    pub fn stat(&self, s: Stat, base_hp: i32) -> i64 {
        match s {
            Stat::MaxHp => i64::from(base_hp) * self.hp / 1000,
            Stat::Damage => self.damage,
            Stat::Speed => self.speed,
            Stat::Regen | Stat::Armour | Stat::Pickup | Stat::Haste | Stat::Area | Stat::Crit => 0,
        }
    }
}

impl Legacy {
    pub fn rank(&self, b: Boon) -> u8 {
        self.ranks[b.ix()]
    }

    /// Buy the next rank of `b`. Ok(new rank).
    pub fn spend(&mut self, b: Boon) -> std::result::Result<u8, SpendErr> {
        let r = self.rank(b);
        if r >= MAX_RANK {
            return Err(SpendErr::Maxed);
        }
        if let Some((needs, rank)) = b.needs().filter(|(n, k)| self.rank(*n) < *k) {
            return Err(SpendErr::Locked { needs, rank });
        }
        let cost = b.cost(r);
        self.souls = self.souls.checked_sub(cost).ok_or(SpendErr::NeedSouls { cost, have: self.souls })?;
        self.ranks[b.ix()] = r + 1;
        Ok(r + 1)
    }

    /// Souls a run earns.
    pub fn earned(r: &RunEnd) -> u32 {
        r.kills / KILLS_PER_SOUL + u32::from(r.level) * SOULS_PER_LEVEL + r.bosses * SOULS_PER_BOSS + if r.won { WIN_SOULS } else { 0 }
    }

    /// Bank a finished run. Returns the souls it earned.
    pub fn award(&mut self, r: &RunEnd) -> u32 {
        let n = Self::earned(r);
        self.souls = self.souls.saturating_add(n);
        self.runs = self.runs.saturating_add(1);
        self.best_level = self.best_level.max(r.level);
        n
    }

    pub fn boons(&self) -> Boons {
        let r = |b: Boon| i64::from(self.rank(b));
        Boons {
            hp: 100 * r(Boon::Vitality),
            damage: 80 * r(Boon::Might),
            speed: 40 * r(Boon::Swiftness),
            xp: 100 * u32::from(self.rank(Boon::Wisdom)),
            gold: 50 * u32::from(self.rank(Boon::Fortune)),
            kin: 150 * r(Boon::Kinship),
            points: u16::from(self.rank(Boon::Insight)),
        }
    }

    /// `$XDG_DATA_HOME/tot/legacy.json`, else `$HOME/.local/share/tot/legacy.json`.
    pub fn default_path() -> Option<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
        data.map(|d| d.join("tot/legacy.json"))
    }

    /// The save at `path`; a missing file is a fresh legacy, a damaged one an error (never silently reset).
    pub fn load(path: &Path) -> Result<Legacy> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Legacy::default()),
            Err(e) => return Err(e.into()),
        };
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let bad = |f: &str| Error::Data { file: path.display().to_string(), line: 1, path: format!(".{f}"), msg: "missing or out of range".into() };
        let num = |f: &str| v[f].as_u64().ok_or_else(|| bad(f));
        let mut l = Legacy {
            souls: u32::try_from(num("souls")?).map_err(|_| bad("souls"))?,
            ranks: [0; Boon::ALL.len()],
            runs: u32::try_from(num("runs")?).map_err(|_| bad("runs"))?,
            best_level: u16::try_from(num("best_level")?).map_err(|_| bad("best_level"))?,
        };
        for b in Boon::ALL {
            let f = format!("ranks.{}", b.key());
            let r = v["ranks"][b.key()].as_u64().unwrap_or(0);
            l.ranks[b.ix()] = u8::try_from(r).ok().filter(|r| *r <= MAX_RANK).ok_or_else(|| bad(&f))?;
        }
        Ok(l)
    }

    /// Write the save (to a temp file first, then rename: a crash never leaves half a file).
    pub fn save(&self, path: &Path) -> Result<()> {
        let ranks: serde_json::Map<String, serde_json::Value> = Boon::ALL.iter().map(|b| (b.key().to_string(), self.rank(*b).into())).collect();
        let v = serde_json::json!({"souls": self.souls, "runs": self.runs, "best_level": self.best_level, "ranks": ranks});
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, format!("{v}\n"))?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn souls_buy_ranks_along_the_tree_only() {
        let mut l = Legacy { souls: 1000, ..Legacy::default() };
        assert_eq!(l.spend(Boon::Swiftness), Err(SpendErr::Locked { needs: Boon::Vitality, rank: 2 }), "NEG: a branch needs its root");
        assert_eq!(l.spend(Boon::Vitality), Ok(1));
        assert_eq!(l.souls, 980);
        assert_eq!(l.spend(Boon::Vitality), Ok(2));
        assert_eq!(l.souls, 940, "rank 2 costs twice rank 1");
        assert_eq!(l.spend(Boon::Swiftness), Ok(1), "unlocked at root rank 2");
        assert_eq!(l.spend(Boon::Insight), Err(SpendErr::Locked { needs: Boon::Wisdom, rank: 3 }));
        for _ in 2..MAX_RANK {
            l.spend(Boon::Vitality).unwrap();
        }
        assert_eq!(l.spend(Boon::Vitality), Err(SpendErr::Maxed));
        let mut poor = Legacy { souls: 19, ..Legacy::default() };
        assert_eq!(poor.spend(Boon::Might), Err(SpendErr::NeedSouls { cost: 20, have: 19 }));
        assert_eq!(poor.rank(Boon::Might), 0, "NEG: a refused buy changes nothing");
    }

    #[test]
    fn a_run_banks_souls_and_boons_grow_with_ranks() {
        let mut l = Legacy::default();
        let r = RunEnd { kills: 123, level: 7, bosses: 1, won: false };
        assert_eq!(l.award(&r), 12 + 35 + 50);
        assert_eq!((l.souls, l.runs, l.best_level), (97, 1, 7));
        assert_eq!(Legacy::earned(&RunEnd { won: true, ..r }), 97 + WIN_SOULS);
        assert_eq!(l.boons(), Boons::default(), "control: no ranks, no boons");
        l.spend(Boon::Vitality).unwrap();
        l.spend(Boon::Fortune).unwrap();
        let b = l.boons();
        assert_eq!((b.hp, b.gold), (100, 50));
        assert_eq!(b.stat(Stat::MaxHp, 100_000), 10_000, "+10% of the base HP");
        assert_eq!(b.stat(Stat::Armour, 100_000), 0);
    }

    #[test]
    fn the_save_survives_a_round_trip_and_damage_is_an_error() {
        let dir = std::env::temp_dir().join(format!("tot-legacy-{}", std::process::id()));
        let path = dir.join("legacy.json");
        assert_eq!(Legacy::load(&path).unwrap(), Legacy::default(), "no file: a fresh legacy");
        let mut l = Legacy { souls: 500, runs: 3, best_level: 9, ..Legacy::default() };
        l.spend(Boon::Might).unwrap();
        l.spend(Boon::Might).unwrap();
        l.spend(Boon::Kinship).unwrap();
        l.save(&path).unwrap();
        assert_eq!(Legacy::load(&path).unwrap(), l);
        std::fs::write(&path, r#"{"souls":1,"runs":1,"best_level":1,"ranks":{"might":9}}"#).unwrap();
        assert!(Legacy::load(&path).unwrap_err().to_string().contains("ranks.might"), "NEG: a rank past the cap is refused");
        std::fs::write(&path, "{").unwrap();
        assert!(Legacy::load(&path).is_err(), "NEG: a damaged save is an error, not a reset");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
