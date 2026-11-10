//! Quests: an agent gives the hero a goal and a reward; the engine counts the progress and pays when it is done. The
//! engine knows only the goal kinds and the payment, never the story (the agent's title and its scenes carry that).
use super::*;
use crate::sim::level::Level;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Goal {
    /// kill this many foes from now on
    Slay(u32),
    /// stand on this level
    Reach(Level),
    /// win this many pals over (rescued or turned) from now on
    Rescue(u16),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Reward {
    pub gold: u32,
    pub xp: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Quest {
    pub key: crew::Word,
    pub title: crew::Word,
    pub goal: Goal,
    pub reward: Reward,
    /// where its objective is (level, world mm), when an agent gave a spot
    pub at: Option<(Level, i32, i32)>,
    /// the run's counter (kills or pals) when the quest was given
    base: u32,
    pub done: bool,
}

impl Quest {
    /// (progress, target) as the player reads it.
    pub fn progress(&self, sim: &Sim) -> (u32, u32) {
        match self.goal {
            Goal::Slay(n) => (sim.stats.kills.saturating_sub(self.base).min(n), n),
            Goal::Rescue(n) => ((u32::from(sim.world.pals)).saturating_sub(self.base).min(u32::from(n)), u32::from(n)),
            Goal::Reach(l) => (u32::from(sim.level == l || self.done), 1),
        }
    }
}

/// A spot an agent gives: whole tiles from a level's start (dx, dy each -SPOT_MAX..=SPOT_MAX), on that level (None:
/// the hero's level when it is given).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Spot {
    pub level: Option<Level>,
    dx: i32,
    dy: i32,
}

pub const SPOT_MAX: i32 = 200;

impl Spot {
    pub fn new(level: Option<Level>, dx: i32, dy: i32) -> Option<Spot> {
        (dx.abs() <= SPOT_MAX && dy.abs() <= SPOT_MAX).then_some(Spot { level, dx, dy })
    }
}

/// Quests at once; reward caps (a reward an agent asks for above them is refused).
pub const QUESTS_MAX: usize = 8;
pub const GOLD_MAX: u32 = 10_000;
pub const XP_MAX: u32 = 5_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuestErr {
    Full,
    Taken,
    /// a goal of zero, or a reward over its cap
    Bounds,
}

impl Sim<'_> {
    pub fn give_quest(&mut self, key: crew::Word, title: crew::Word, goal: Goal, reward: Reward, at: Option<Spot>) -> Result<(), QuestErr> {
        if matches!(goal, Goal::Slay(0) | Goal::Rescue(0)) || reward.gold > GOLD_MAX || reward.xp > XP_MAX {
            return Err(QuestErr::Bounds);
        }
        if self.quests.iter().any(|q| q.key == key) {
            return Err(QuestErr::Taken);
        }
        if self.quests.iter().filter(|q| !q.done).count() >= QUESTS_MAX {
            return Err(QuestErr::Full);
        }
        self.quests.retain(|q| !q.done);
        let base = match goal {
            Goal::Slay(_) => self.stats.kills,
            Goal::Rescue(_) => u32::from(self.world.pals),
            Goal::Reach(_) => 0,
        };
        let at = at.map(|s| self.place_spot(s));
        self.quests.push(Quest { key, title, goal, reward, base, done: false, at });
        Ok(())
    }

    /// An agent moves quest `key`'s objective to `at` (None: back to the engine's own pick).
    pub fn quest_at(&mut self, key: crew::Word, at: Option<Spot>) -> Result<(), QuestErr> {
        let at = at.map(|s| self.place_spot(s));
        let q = self.quests.iter_mut().find(|q| q.key == key && !q.done).ok_or(QuestErr::Taken)?;
        q.at = at;
        Ok(())
    }

    /// A spot an agent names, made concrete: on its level (the hero's when unnamed), in world mm from the level's start.
    fn place_spot(&self, s: Spot) -> (Level, i32, i32) {
        let t = crate::floorgen::TILE_MM;
        (s.level.unwrap_or(self.level), s.dx * t, s.dy * t)
    }

    /// Where quest `q` points on the hero's level now: the agent's spot when it lies here, else for a level to reach
    /// the stair toward it; None when the objective is nowhere on this level (slay and rescue without a spot).
    pub fn objective(&self, q: &Quest) -> Option<(i32, i32)> {
        if q.done {
            return None;
        }
        if let Some((l, x, y)) = q.at
            && l == self.level
        {
            return Some((x, y));
        }
        let Goal::Reach(l) = q.goal else { return None };
        if l == self.level {
            return None;
        }
        let up = l == Level::Town;
        self.stairs.iter().find(|s| s.to == l).or_else(|| self.stairs.iter().find(|s| (s.to == Level::Town) == up)).map(|s| (s.x, s.y))
    }

    /// Finished quests pay out (gold, XP) once and are told to the play loop.
    pub(super) fn quests_step(&mut self) {
        for k in 0..self.quests.len() {
            let q = self.quests[k];
            let (now, of) = q.progress(self);
            if !q.done && now >= of {
                self.quests[k].done = true;
                self.p.gold = self.p.gold.saturating_add(q.reward.gold);
                self.p.xp = self.p.xp.saturating_add(q.reward.xp);
                if self.quest_ev.len() < QUESTS_MAX {
                    self.quest_ev.push(q.key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;

    fn w(s: &str) -> crew::Word {
        crew::Word::parse(s).unwrap()
    }

    #[test]
    fn quests_count_from_when_given_and_pay_once() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        s.stats.kills = 40;
        s.give_quest(w("rats"), w("Rat Purge"), Goal::Slay(3), Reward { gold: 100, xp: 0 }, None).unwrap();
        s.give_quest(w("town"), w("Go Home"), Goal::Reach(Level::Town), Reward { gold: 0, xp: 7 }, None).unwrap();
        assert_eq!(s.give_quest(w("rats"), w("x"), Goal::Slay(1), Reward::default(), None), Err(QuestErr::Taken));
        assert_eq!(s.give_quest(w("big"), w("x"), Goal::Slay(1), Reward { gold: GOLD_MAX + 1, xp: 0 }, None), Err(QuestErr::Bounds), "NEG: reward over the cap");
        assert_eq!(s.give_quest(w("zero"), w("x"), Goal::Slay(0), Reward::default(), None), Err(QuestErr::Bounds), "NEG: an empty goal");
        let gold = s.p.gold;
        s.stats.kills = 42;
        s.quests_step();
        assert_eq!((s.quests[0].progress(&s), s.quests[0].done, s.p.gold), ((2, 3), false, gold), "old kills do not count");
        s.stats.kills = 43;
        s.quests_step();
        s.quests_step();
        assert_eq!((s.quests[0].done, s.p.gold, s.quest_ev.clone()), (true, gold + 100, vec![w("rats")]), "paid once");
        let xp = s.p.xp;
        s.travel(Level::Town);
        s.quests_step();
        assert_eq!((s.p.xp, s.quest_ev.len()), (xp + 7, 2), "reached the town");
    }
}
