//! People: townsfolk and wanderers an agent puts on a level. Each has a name, a look and a job; the engine gives the
//! job its gameplay (a shop, a forge, a jeweler's bench, a healer's touch, or words). The hero meets one by walking up
//! to it; the play loop shows its words and tells the agents.
use super::*;
use crate::sim::level::Level;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Job {
    /// sells items (a merchant's shop)
    Trade,
    /// sells, forges and upgrades (a blacksmith's shop)
    Forge,
    /// sells and enchants (a jeweler's shop)
    Enchant,
    /// heals the hero to full when met
    Heal,
    /// only talks
    Talk,
}

impl Job {
    pub const ALL: [Job; 5] = [Job::Trade, Job::Forge, Job::Enchant, Job::Heal, Job::Talk];

    pub fn key(self) -> &'static str {
        match self {
            Job::Trade => "trade",
            Job::Forge => "forge",
            Job::Enchant => "enchant",
            Job::Heal => "heal",
            Job::Talk => "talk",
        }
    }

    pub fn parse(s: &str) -> Option<Job> {
        Job::ALL.into_iter().find(|j| j.key() == s)
    }

    /// The honest keeper whose shop this job runs, if it keeps one.
    fn keeper(self) -> Option<npc::Npc> {
        match self {
            Job::Trade => Some(npc::Npc::Merchant { honest: true }),
            Job::Forge => Some(npc::Npc::Blacksmith { honest: true }),
            Job::Enchant => Some(npc::Npc::Jeweler { honest: true }),
            Job::Heal | Job::Talk => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Person {
    pub key: crew::Word,
    pub name: crew::Word,
    pub job: Job,
    pub glyph: char,
    pub rgb: Option<crew::Rgb8>,
    pub level: Level,
    /// where the agent asked for it, tiles from the level's start (None: a spot the game picks)
    at: Option<(i32, i32)>,
    pub x: i32,
    pub y: i32,
    /// the hero stands by it now (a meeting counts once per approach)
    near: bool,
}

/// People at once over all levels; meeting radius (mm), and how far the hero must go before the next meeting.
pub const PEOPLE_MAX: usize = 24;
const MEET_R: i32 = 1_500;
const PART_R: i32 = 4_000;
/// A person with no spot of its own stands this far from the level's start, in a direction hashed from its key.
const SPOT_R: i32 = 9_000;

impl Sim<'_> {
    /// An agent puts a person on `level` (at `at` tiles from its start, else on a spot the game picks).
    #[allow(clippy::too_many_arguments)]
    pub fn add_person(&mut self, key: crew::Word, name: crew::Word, job: Job, glyph: char, rgb: Option<crew::Rgb8>, level: Level, at: Option<(i32, i32)>) -> Result<(), crew::CrewErr> {
        if self.people.iter().any(|p| p.key == key) {
            return Err(crew::CrewErr::Taken);
        }
        if self.people.len() >= PEOPLE_MAX {
            return Err(crew::CrewErr::Full);
        }
        self.people.push(Person { key, name, job, glyph, rgb, level, at, x: 0, y: 0, near: false });
        if level == self.level {
            self.seat_people();
        }
        Ok(())
    }

    /// Place the current level's people on free floor and open their shops (after a level change or a new person).
    pub(super) fn seat_people(&mut self) {
        let t = crate::floorgen::TILE_MM;
        for k in 0..self.people.len() {
            let p = self.people[k];
            if p.level != self.level {
                continue;
            }
            let (wx, wy) = match p.at {
                Some((dx, dy)) => (dx.clamp(-40, 40) * t, dy.clamp(-40, 40) * t),
                None => {
                    let h = p.key.as_str().bytes().fold(0u64, |h, b| mix(h ^ u64::from(b)));
                    let (cx, cy) = self.trig[(h % u64::from(TRIG_N)) as usize];
                    (scale(cx, i64::from(SPOT_R)), scale(cy, i64::from(SPOT_R)))
                }
            };
            let (x, y) = self.free_near(wx, wy, 400);
            (self.people[k].x, self.people[k].y) = (x, y);
            if let Some(npc) = p.job.keeper()
                && !self.world.npcs.iter().any(|n| (n.x, n.y) == (x, y))
            {
                self.world.npcs.push(npc::Placed { npc, x, y });
                self.world.shops.push(Vec::new());
            }
        }
        // a level with an agent's people is theirs: the engine's default keepers there leave
        let spots: Vec<(i32, i32)> = self.people_here().map(|p| (p.x, p.y)).collect();
        if !spots.is_empty() {
            let mut i = 0;
            while i < self.world.npcs.len() {
                let n = self.world.npcs[i];
                if n.npc.keeper_honest().is_some() && !spots.contains(&(n.x, n.y)) {
                    self.world.remove_npc(i);
                } else {
                    i += 1;
                }
            }
        }
        self.world.stock_shops(self.c);
    }

    /// The people of the level the hero is on.
    pub fn people_here(&self) -> impl Iterator<Item = &Person> {
        self.people.iter().filter(|p| p.level == self.level)
    }

    /// The hero walking up to a person meets it (once per approach); a healer heals it to full.
    pub(super) fn people_step(&mut self) {
        let (px, py) = (self.p.x, self.p.y);
        for k in 0..self.people.len() {
            let p = self.people[k];
            if p.level != self.level {
                continue;
            }
            let d2 = dist2(p.x - px, p.y - py);
            if !p.near && d2 <= i64::from(MEET_R).pow(2) {
                self.people[k].near = true;
                if p.job == Job::Heal {
                    self.p.hp = self.p.max_hp;
                }
                if self.people_ev.len() < PEOPLE_MAX {
                    self.people_ev.push(p.key);
                }
            } else if p.near && d2 > i64::from(PART_R).pow(2) {
                self.people[k].near = false;
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
    fn people_stand_on_their_level_keep_their_trade_and_are_met_once_per_approach() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        s.travel(Level::Town);
        let keepers = s.world.npcs.len();
        s.add_person(w("bran"), w("BRAN"), Job::Forge, 'B', None, Level::Town, Some((3, 0))).unwrap();
        s.add_person(w("mae"), w("MAE"), Job::Heal, 'M', None, Level::Town, Some((-3, 0))).unwrap();
        s.add_person(w("deep"), w("OLD ONE"), Job::Talk, 'O', None, Level::FIRST, None).unwrap();
        assert_eq!(s.add_person(w("mae"), w("X"), Job::Talk, 'X', None, Level::Town, None), Err(crew::CrewErr::Taken));
        assert_eq!(s.people_here().count(), 2, "only this level's people stand here");
        let bran = *s.people.iter().find(|p| p.key == w("bran")).unwrap();
        assert!(keepers > 0, "control: the town had default keepers");
        assert_eq!(s.world.npcs.len(), 1, "the agent's people replace the default keepers; the smith keeps a shop, the healer does not");
        assert!(s.world.npcs.iter().any(|n| (n.x, n.y) == (bran.x, bran.y) && n.npc == npc::Npc::Blacksmith { honest: true }));
        // the healer: walk up, met once, healed to full
        let mae = *s.people.iter().find(|p| p.key == w("mae")).unwrap();
        s.p.hp = 1_000;
        (s.p.x, s.p.y) = (mae.x + 500, mae.y);
        s.people_step();
        s.people_step();
        assert_eq!((s.people_ev.clone(), s.p.hp), (vec![w("mae")], s.p.max_hp), "met once, healed");
        // NEG: walk away and back: a second meeting; no meeting while far
        s.people_ev.clear();
        s.p.x += PART_R + 1_000;
        s.people_step();
        assert!(s.people_ev.is_empty(), "control: far away, nobody met");
        s.p.x = mae.x + 500;
        s.people_step();
        assert_eq!(s.people_ev, vec![w("mae")]);
        // the deep's person stands there once the hero goes down
        s.travel(Level::FIRST);
        assert_eq!(s.people_here().map(|p| p.key).collect::<Vec<_>>(), vec![w("deep")]);
        assert!(!s.world.npcs.iter().any(|n| (n.x, n.y) == (bran.x, bran.y) && n.npc == npc::Npc::Blacksmith { honest: true }), "the smith stayed in town");
    }
}
