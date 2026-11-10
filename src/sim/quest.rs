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
    pub fn give_quest(&mut self, key: crew::Word, title: crew::Word, goal: Goal, reward: Reward) -> Result<(), QuestErr> {
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
        self.quests.push(Quest { key, title, goal, reward, base, done: false });
        Ok(())
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
        s.give_quest(w("rats"), w("Rat Purge"), Goal::Slay(3), Reward { gold: 100, xp: 0 }).unwrap();
        s.give_quest(w("town"), w("Go Home"), Goal::Reach(Level::Town), Reward { gold: 0, xp: 7 }).unwrap();
        assert_eq!(s.give_quest(w("rats"), w("x"), Goal::Slay(1), Reward::default()), Err(QuestErr::Taken));
        assert_eq!(s.give_quest(w("big"), w("x"), Goal::Slay(1), Reward { gold: GOLD_MAX + 1, xp: 0 }), Err(QuestErr::Bounds), "NEG: reward over the cap");
        assert_eq!(s.give_quest(w("zero"), w("x"), Goal::Slay(0), Reward::default()), Err(QuestErr::Bounds), "NEG: an empty goal");
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
