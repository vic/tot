//! Doom-style hero face for the bottom bar: a mood from what just happened, eyes that track the nearest foe
//! or glance around, and short barks on events. Pure over sim state plus its own tick-stamped memory, so
//! frames stay deterministic.
use crate::sim::{Outcome, Sim, SpecialTry, Status};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    Calm,
    Fencing,
    Pain,
    Laugh,
    Grim,
    Frozen,
    Dead,
}

/// What a bark answers. Order = priority: a later event never talks over an earlier one still showing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Event {
    LowHp,
    Hurt,
    Reloading,
    Passive,
    NoTarget,
    Empty,
    Level,
    Streak,
    Special,
    Frozen,
    Hungry,
    Gold,
}

impl Event {
    /// The one word the hero yells big over its head with the line.
    fn shout(self) -> &'static str {
        match self {
            Event::LowHp => "HELP!",
            Event::Hurt => "OW!",
            Event::Reloading | Event::Passive | Event::NoTarget | Event::Empty => "HUH?",
            Event::Level => "LEVEL UP!",
            Event::Streak => "HA HA!",
            Event::Special => "BOOM!",
            Event::Frozen => "BRR!",
            Event::Hungry => "FOOD!",
            Event::Gold => "SHINY!",
        }
    }

    fn lines(self) -> &'static [&'static str] {
        match self {
            Event::Level => &["I feel... SPICY.", "Patch notes: me, buffed.", "Level up! Mom would be proud.", "More power. Less sense.", "New level, same bad decisions.", "I can feel my build coming together!", "Ding! Somebody call the bard."],
            Event::Streak => &["Combo! Do I get a sticker?", "Bits everywhere. Not cleaning that.", "Ha! Next!", "Is that all you have got?", "Five! Count them! Five!", "Someone call their manager.", "I am the horde now."],
            Event::Hurt => &["Ow. Rude.", "That is coming out of your pay!", "My good tunic!", "Hey! I was walking here!", "That one I felt in my soul.", "Not the face!", "I bruise like a peach!"],
            Event::LowHp => &["Medic! ...anyone?", "I am fine. Totally fine.", "Is that my blood? It is.", "Tell my cat I tried.", "I see a light. Is it loot?", "Running is a strategy, right?"],
            Event::Gold => &["Shiny!", "Cha-ching.", "Retirement fund +1.", "Mine. All mine.", "Coin! My one true love.", "The merchant will hear about this."],
            Event::Special => &["Special delivery!", "Ka-BOOM.", "Say hello to my reload timer!", "Behold! ...ok, behold faster."],
            Event::Frozen => &["B-b-brr!", "Who left the door open?", "C-can't feel my t-toes."],
            Event::Hungry => &["My stomach is louder than the boss.", "Is that a sandwich? Please be a sandwich.", "Hero needs food, badly.", "I could eat a mimic."],
            Event::Reloading => &["Still reloading!", "Patience, I am not a vending machine.", "Give it a second!"],
            Event::Passive => &["That one works on its own.", "Passive. Like me on Mondays.", "No button for that, friend."],
            Event::NoTarget => &["Nothing to hit!", "Save it for a crowd.", "Who am I aiming at, the air?"],
            Event::Empty => &["Nothing in that pocket.", "Empty slot. Go find loot!", "That hand is empty."],
        }
    }
}

/// Seconds a bark shows, a kill streak window and its size, the big-hit share of max HP (permille), low HP.
const BARK_S: u32 = 3;
const STREAK_S: u32 = 2;
const STREAK_KILLS: u32 = 5;
const LAUGH_S: u32 = 2;
const HURT_BIG: i64 = 80;
const LOW_HP_PCT: i64 = 30;
/// Pain shows this many ticks after a hit; idle eyes move every GLANCE_S seconds.
const PAIN_TICKS: u32 = 8;
const GLANCE_S: u32 = 2;

#[derive(Clone, Copy, Default)]
struct Seen {
    level: u16,
    hp: i32,
    gold: u32,
    low: bool,
    frozen: bool,
    hungry: bool,
    reloads: u64,
    tried_at: Option<u32>,
    win_tick: u32,
    win_kills: u32,
}

#[derive(Default)]
pub struct Face {
    /// None until the first look: the starting state raises no barks
    last: Option<Seen>,
    laugh_until: u32,
    bark: Option<(Event, u32)>,
}

fn seen(sim: &Sim) -> Seen {
    Seen {
        level: sim.p.level,
        hp: sim.p.hp,
        gold: sim.p.gold,
        low: i64::from(sim.p.hp) * 100 < i64::from(sim.p.max_hp) * LOW_HP_PCT,
        frozen: matches!(sim.p.status, Status::Frozen { .. }),
        hungry: sim.p.food < crate::sim::FOOD_LOW,
        reloads: sim.owned.iter().map(|o| u64::from(o.sp)).sum(),
        tried_at: sim.special_try.map(|(_, at)| at),
        win_tick: sim.tick,
        win_kills: sim.stats.kills,
    }
}

impl Face {
    /// Update memory from this tick's sim state; raises barks on changes. Call once per drawn frame.
    pub fn observe(&mut self, sim: &Sim) {
        let hz = sim.c.hz().get();
        let now = seen(sim);
        let Some(old) = self.last else {
            self.last = Some(now);
            return;
        };
        let say = |f: &mut Self, e: Event| {
            let free = f.bark.is_none_or(|(b, at)| sim.tick.saturating_sub(at) >= BARK_S * hz || e < b);
            if free {
                f.bark = Some((e, sim.tick));
            }
        };
        if now.level > old.level {
            say(self, Event::Level);
        }
        if i64::from(old.hp - now.hp) * 1000 >= i64::from(sim.p.max_hp) * HURT_BIG {
            say(self, Event::Hurt);
        }
        if now.low && !old.low {
            say(self, Event::LowHp);
        }
        if now.gold > old.gold {
            say(self, Event::Gold);
        }
        if now.hungry && !old.hungry {
            say(self, Event::Hungry);
        }
        if now.frozen && !old.frozen {
            say(self, Event::Frozen);
        }
        // a special just went off: some slot's reload jumped up
        if now.reloads > old.reloads {
            say(self, Event::Special);
        }
        // a number key that did nothing: say why
        if let Some((t, _)) = sim.special_try && now.tried_at != old.tried_at {
            match t {
                SpecialTry::Cast => {}
                SpecialTry::Reloading => say(self, Event::Reloading),
                SpecialTry::Passive => say(self, Event::Passive),
                SpecialTry::NoTarget => say(self, Event::NoTarget),
                SpecialTry::Empty => say(self, Event::Empty),
            }
        }
        // kill streak: STREAK_KILLS kills inside one STREAK_S window
        let (mut win_tick, mut win_kills) = (old.win_tick, old.win_kills);
        if sim.stats.kills.saturating_sub(win_kills) >= STREAK_KILLS {
            self.laugh_until = sim.tick + LAUGH_S * hz;
            say(self, Event::Streak);
            (win_tick, win_kills) = (sim.tick, sim.stats.kills);
        } else if sim.tick.saturating_sub(win_tick) >= STREAK_S * hz {
            (win_tick, win_kills) = (sim.tick, sim.stats.kills);
        }
        self.last = Some(Seen { win_tick, win_kills, ..now });
    }

    /// Current mood; `fighting` = the hero is shooting at a foe close by.
    pub fn mood(&self, sim: &Sim, fighting: bool) -> Mood {
        if matches!(sim.outcome, Some(Outcome::Dead { .. })) {
            Mood::Dead
        } else if sim.p.unhurt < PAIN_TICKS {
            Mood::Pain
        } else if matches!(sim.p.status, Status::Frozen { .. }) {
            Mood::Frozen
        } else if sim.tick < self.laugh_until {
            Mood::Laugh
        } else if fighting {
            Mood::Fencing
        } else if self.last.is_some_and(|s| s.low) {
            Mood::Grim
        } else {
            Mood::Calm
        }
    }

    /// The shout (big word), the bark (line) showing now, and the sim tick it began, if any.
    pub fn shout(&self, sim: &Sim) -> Option<(&'static str, &'static str, u32)> {
        let (e, at) = self.bark?;
        self.bark(sim).map(|line| (e.shout(), line, at))
    }

    /// The bark showing now, if any.
    pub fn bark(&self, sim: &Sim) -> Option<&'static str> {
        let (e, at) = self.bark?;
        let hz = sim.c.hz().get();
        (sim.tick.saturating_sub(at) < BARK_S * hz).then(|| {
            let lines = e.lines();
            lines[(at as usize).wrapping_mul(7919) % lines.len()]
        })
    }

    /// Eyes: -1 left, 0 ahead, 1 right. Toward the nearest foe (`toward`, from the screen) or an idle glance.
    pub fn eyes(sim: &Sim, toward: Option<i32>) -> i32 {
        toward.map_or_else(|| [0, -1, 0, 1][(sim.tick / (GLANCE_S * sim.c.hz().get()) % 4) as usize], i32::signum)
    }
}

/// The 3 face rows (7 columns) for a mood and eye direction.
pub fn rows(mood: Mood, eyes: i32) -> [String; 3] {
    let hair = match mood {
        Mood::Grim => " .-%-. ",
        Mood::Frozen => " .***. ",
        Mood::Fencing => " \\---/ ",
        _ => " .---. ",
    };
    let eye = match mood {
        Mood::Calm => Some('o'),
        Mood::Fencing => Some('O'),
        Mood::Grim => Some('.'),
        _ => None,
    };
    let eyes_row = match (mood, eye) {
        (_, Some(e)) => match eyes {
            -1 => format!(" ({e}{e} ) "),
            1 => format!(" ( {e}{e}) "),
            _ => format!(" ({e} {e}) "),
        },
        (Mood::Pain, _) => " (> <) ".into(),
        (Mood::Laugh, _) => " (^ ^) ".into(),
        (Mood::Frozen, _) => " (* *) ".into(),
        _ => " (x x) ".into(),
    };
    let mouth = match mood {
        Mood::Calm => "  \\_/  ",
        Mood::Fencing => "  =#=  ",
        Mood::Pain => "  ~O~  ",
        Mood::Laugh => " \\vvv/ ",
        Mood::Grim => "  ---  ",
        Mood::Frozen => "  ~~~  ",
        Mood::Dead => "  ___  ",
    };
    [hair.into(), eyes_row, mouth.into()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;

    #[test]
    fn events_bark_and_moods_follow_what_happened() {
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let mut f = Face::default();
        s.p.unhurt = 1000;
        f.observe(&s);
        assert_eq!((f.mood(&s, false), f.bark(&s)), (Mood::Calm, None), "control: start is calm and quiet");
        s.p.gold += 10;
        s.tick += 1;
        f.observe(&s);
        assert!(Event::Gold.lines().contains(&f.bark(&s).unwrap()), "gold bark");
        // a bigger event talks over a smaller one
        s.p.level += 1;
        s.tick += 1;
        f.observe(&s);
        assert!(Event::Level.lines().contains(&f.bark(&s).unwrap()), "level bark wins");
        // kill streak: laugh
        s.stats.kills += STREAK_KILLS;
        s.tick += 1;
        f.observe(&s);
        assert_eq!(f.mood(&s, true), Mood::Laugh);
        // a big hit: pain face and a hurt bark (hurt outranks level)
        s.tick += 1;
        s.p.hp -= s.p.max_hp / 5;
        s.p.unhurt = 0;
        f.observe(&s);
        assert_eq!(f.mood(&s, false), Mood::Pain);
        assert!(Event::Hurt.lines().contains(&f.bark(&s).unwrap()));
        // barks fade
        s.tick += BARK_S * c.hz().get();
        s.p.unhurt = 1000;
        assert_eq!(f.bark(&s), None);
        // fencing while fighting once the laugh is over; eyes follow the foe side
        s.tick += LAUGH_S * c.hz().get();
        assert_eq!(f.mood(&s, true), Mood::Fencing);
        assert_eq!(Face::eyes(&s, Some(-40)), -1);
        assert!(rows(Mood::Fencing, -1)[1].contains("OO "), "{:?}", rows(Mood::Fencing, -1));
        // every row is FACE_W wide
        for m in [Mood::Calm, Mood::Fencing, Mood::Pain, Mood::Laugh, Mood::Grim, Mood::Frozen, Mood::Dead] {
            for e in -1..=1 {
                assert!(rows(m, e).iter().all(|r| r.chars().count() == 7), "{m:?} {e}");
            }
        }
    }

    /// A number key that does nothing gets an answer: reloading, passive, empty slot.
    #[test]
    fn refused_special_makes_the_hero_talk() {
        use crate::data::SkillKind;
        use crate::sim::Input;
        let c = content();
        let mut s = Sim::new(&c, 7, None);
        let mut f = Face::default();
        s.p.unhurt = 1000;
        f.observe(&s);
        let bolt = s.owned.iter().position(|o| c.skill(o.id).kind == SkillKind::Bolt).unwrap() as u8;
        let press = |s: &mut Sim, f: &mut Face, k: u8| {
            s.step(&Input { special: Some(k), ..Input::default() });
            f.observe(s);
            f.bark(s)
        };
        let first = press(&mut s, &mut f, bolt);
        assert!(first.is_some_and(|l| Event::Special.lines().contains(&l)), "cast: special bark {first:?}");
        let again = press(&mut s, &mut f, bolt);
        assert!(again.is_some_and(|l| Event::Reloading.lines().contains(&l)), "reloading: {again:?}");
        let empty = press(&mut s, &mut f, 7);
        // reloading (higher priority) still shows; after it fades the empty slot answers
        assert!(empty.is_some_and(|l| Event::Reloading.lines().contains(&l)), "{empty:?}");
        s.tick += BARK_S * c.hz().get();
        let empty = press(&mut s, &mut f, 7);
        assert!(empty.is_some_and(|l| Event::Empty.lines().contains(&l)), "empty slot: {empty:?}");
    }
}
