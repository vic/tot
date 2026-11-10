//! Agentic crew link. The player types a prompt (P); the game sends it with the crew protocol and a snapshot of the
//! fight to a live local Claude session (peer sockets). The agent answers with JSON order lines that make and drive
//! crew members, pals or foes (sim/crew.rs); the game applies them, and reports back what the crew did, the state
//! of the fight and the lines it refused, so the agent keeps playing.
use crate::peer::{Inbox, Session};
use crate::sim::{Allegiance, CrewErr, CrewEv, Role, Sim, Stance, Word};
use crate::{Error, Result};
use std::path::Path;

/// One line of an agent's answer, parsed: only a well-formed order is representable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Order {
    Join { key: Word, name: Word, side: Allegiance, role: Role, say: Option<Say> },
    Stance { key: Word, stance: Stance },
    Move { key: Word, dx: i32, dy: i32 },
    Say { key: Word, text: Say },
    Leave { key: Word },
}

/// A line a crew member says: 1..=SAY_MAX printable ASCII chars (the terminal draws every one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Say(String);

const SAY_MAX: usize = 48;

impl Say {
    pub fn parse(s: &str) -> Option<Say> {
        let t: String = s.trim().chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(SAY_MAX).collect();
        (!t.trim().is_empty()).then_some(Say(t))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Order {
    pub fn key(&self) -> Word {
        match self {
            Order::Join { key, .. } | Order::Stance { key, .. } | Order::Move { key, .. } | Order::Say { key, .. } | Order::Leave { key } => *key,
        }
    }
}

/// One answer line: Ok(None) for prose (a line that does not start with `{`), Err(why) for a bad order.
pub fn parse_line(line: &str) -> std::result::Result<Option<Order>, String> {
    let line = line.trim();
    if !line.starts_with('{') {
        return Ok(None);
    }
    let v: serde_json::Value = serde_json::from_str(line).map_err(|e| format!("not JSON: {e}"))?;
    let s = |f: &str| v[f].as_str().ok_or_else(|| format!("needs \"{f}\" as a string"));
    let word = |f: &str| s(f).and_then(|x| Word::parse(x).ok_or_else(|| format!("\"{f}\": 1-16 of A-Z a-z 0-9 _ - space")));
    let key = word("id")?;
    let tiles = |f: &str| v[f].as_i64().and_then(|n| i32::try_from(n).ok()).filter(|n| n.abs() <= 40).ok_or_else(|| format!("\"{f}\": whole tiles, -40..=40"));
    Ok(Some(match s("t")? {
        "join" => Order::Join {
            key,
            name: word("name")?,
            side: match s("side")? {
                "pal" => Allegiance::Pal,
                "foe" => Allegiance::Foe,
                x => return Err(format!("side \"{x}\": pal or foe")),
            },
            role: Role::parse(s("role")?).ok_or_else(|| format!("role: one of {}", Role::ALL.map(Role::key).join(", ")))?,
            say: v["say"].as_str().and_then(Say::parse),
        },
        "order" => Order::Stance { key, stance: Stance::parse(s("do")?).ok_or_else(|| format!("do: one of {}", Stance::ALL.map(Stance::key).join(", ")))? },
        "move" => Order::Move { key, dx: tiles("dx")?, dy: tiles("dy")? },
        "say" => Order::Say { key, text: Say::parse(s("text")?).ok_or("text: printable ASCII, not blank")? },
        "leave" => Order::Leave { key },
        x => return Err(format!("t \"{x}\": join, order, move, say or leave")),
    }))
}

/// Apply one order to the sim. A `say` changes nothing in the sim (the view shows it).
pub fn apply(sim: &mut Sim, o: &Order) -> std::result::Result<(), CrewErr> {
    match *o {
        Order::Join { key, name, side, role, .. } => sim.crew_join(key, name, role, side),
        Order::Stance { key, stance } => sim.crew_order(key, stance),
        Order::Move { key, dx, dy } => sim.crew_goal(key, dx, dy),
        Order::Say { key, .. } => sim.crew.iter().any(|m| m.key == key).then_some(()).ok_or(CrewErr::NoSuchMate),
        Order::Leave { key } => sim.crew_leave(key),
    }
}

/// The fight as the agent sees it, one JSON line: positions in whole tiles from the hero, HP in whole points.
pub fn snapshot(sim: &Sim) -> String {
    let t = crate::floorgen::TILE_MM;
    let rel = |x: i32, y: i32| ((x - sim.p.x).div_euclid(t), (y - sim.p.y).div_euclid(t));
    let crew: Vec<serde_json::Value> = sim
        .crew
        .iter()
        .filter_map(|m| {
            let (x, y, hp, max) = sim.mate_at(m)?;
            let (dx, dy) = rel(x, y);
            let side = match m.side() {
                Allegiance::Pal => "pal",
                Allegiance::Foe => "foe",
            };
            Some(serde_json::json!({"id": m.key.as_str(), "name": m.name.as_str(), "side": side, "role": m.role.key(), "do": m.stance.key(), "dx": dx, "dy": dy, "hp": hp / 1000, "max": max / 1000}))
        })
        .collect();
    let near = (0..sim.e.len()).filter(|&i| crate::sim::dist2(sim.e.x[i] - sim.p.x, sim.e.y[i] - sim.p.y) <= i64::from(sim.c.view.view_r).pow(2)).count();
    serde_json::json!({
        "t": "state", "time_s": sim.secs(),
        "hero": {"hp": sim.p.hp / 1000, "max": sim.p.max_hp / 1000, "level": sim.p.level, "gold": sim.p.gold},
        "foes_in_view": near, "foes_alive": sim.e.len(), "crew": crew,
    })
    .to_string()
}

/// One crew event as the agent reads it.
fn event(e: &CrewEv) -> String {
    match e {
        CrewEv::Joined(k) => format!("{} joined", k.as_str()),
        CrewEv::Down(k) => format!("{} is down", k.as_str()),
        CrewEv::Healed { by, hp } => format!("{} healed {} HP", by.as_str(), hp / 1000),
        CrewEv::Raised { by } => format!("{} raised a foe", by.as_str()),
        CrewEv::Struck { by, dmg } => format!("{} hit for {}", by.as_str(), dmg / 1000),
        CrewEv::Cast { by } => format!("{} cast at the hero", by.as_str()),
    }
}

const PROTOCOL: &str = "You run crew members in a live game of tot (a terminal Diablo-like horde survival). The player wrote the prompt below. Play it: make crew members and keep giving them orders as the fight goes on. Pals fight beside the hero; foes hunt the hero and its pals.

Answer with the SendMessage tool to the address below (it is the game itself, not a person). Put one JSON object per line; other lines are ignored. Orders:
{\"t\":\"join\",\"id\":\"mira\",\"name\":\"MIRA\",\"side\":\"pal\",\"role\":\"healer\",\"say\":\"I have you!\"}
  side: pal or foe. role: healer (heals the hero and pals; a foe heals foes), tank (pal: draws foes onto itself; foe: very hard to kill), necro (pal: raises hurt foes to fight for the hero; foe: raises the dead around the hero), mage (pal: blasts crowds; foe: telegraphed blasts at the hero), warrior (strikes hard up close; a foe rages), archer (shoots from range).
{\"t\":\"order\",\"id\":\"mira\",\"do\":\"follow\"}   do: follow, hold, charge or flee
{\"t\":\"move\",\"id\":\"mira\",\"dx\":3,\"dy\":-2}   go to a spot in whole tiles from the hero (-40..40)
{\"t\":\"say\",\"id\":\"mira\",\"text\":\"Behind you!\"}   a line shown over the member (48 chars)
{\"t\":\"leave\",\"id\":\"mira\"}
ids and names: 1-16 of A-Z a-z 0-9 _ - space. At most 6 members at once. The game sends you reports: what the crew did, the refused lines with the reason, and a state line (positions in tiles from the hero). Answer reports with new orders when the fight calls for it; stay silent otherwise.";

/// The first message to a session: the protocol, the player's prompt, the fight now.
pub fn request(prompt: &str, reply_to: &str, sim: &Sim) -> String {
    format!("{PROTOCOL}\n\nReply address: {reply_to}\n\nThe player's prompt: \"{prompt}\"\n\nNow: {}\n", snapshot(sim))
}

/// A report: crew events since the last one, refused lines, the state.
pub fn report(evs: &[CrewEv], refused: &[String], sim: &Sim) -> String {
    let mut s = String::from("tot report.\n");
    if !evs.is_empty() {
        s += &format!("Crew: {}.\n", evs.iter().map(event).collect::<Vec<_>>().join("; "));
    }
    for r in refused {
        s += &format!("Refused: {r}\n");
    }
    s + &snapshot(sim) + "\n"
}

/// Shortest gap between two reports (s); a crew member falling reports after DOWN_S at the earliest.
const REPORT_S: u32 = 20;
const DOWN_S: u32 = 3;
/// The name the game signs crew messages with.
const SENDER: &str = "tot-crew";

/// What one poll heard, for the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    Applied(Order),
    Refused(String),
}

/// The running link: our inbox, the sessions the player prompted, what waits to be reported.
pub struct Link {
    inbox: Inbox,
    to: Vec<Session>,
    evs: Vec<CrewEv>,
    refused: Vec<String>,
    last_report: u32,
}

impl Link {
    pub fn open(socks: &Path) -> Result<Link> {
        Ok(Link { inbox: Inbox::open(socks)?, to: Vec::new(), evs: Vec::new(), refused: Vec::new(), last_report: 0 })
    }

    /// Send the player's prompt to `to` (it joins the sessions that get reports).
    pub fn prompt(&mut self, to: &Session, prompt: &str, sim: &Sim) -> Result<()> {
        if prompt.trim().is_empty() {
            return Err(Error::Peer("empty prompt".into()));
        }
        crate::peer::send(to, &self.inbox, SENDER, &request(prompt, &self.inbox.addr(), sim))?;
        if !self.to.contains(to) {
            self.to.push(to.clone());
        }
        self.last_report = sim.secs();
        Ok(())
    }

    /// Read what the agents sent, apply every order line, keep the refusals for the next report.
    pub fn poll(&mut self, sim: &mut Sim) -> Vec<Heard> {
        let mut out = Vec::new();
        for body in self.inbox.poll() {
            for line in body.lines() {
                let heard = match parse_line(line) {
                    Ok(None) => continue,
                    Ok(Some(o)) => match apply(sim, &o) {
                        Ok(()) => Heard::Applied(o),
                        Err(e) => Heard::Refused(format!("{}: {e:?}", line.trim())),
                    },
                    Err(why) => Heard::Refused(format!("{}: {why}", line.trim())),
                };
                if let Heard::Refused(r) = &heard {
                    self.refused.push(r.clone());
                }
                out.push(heard);
            }
        }
        out
    }

    /// Take the sim's crew events; send a report when one is due. Ok(true) when a report went out.
    pub fn tick(&mut self, sim: &mut Sim) -> Result<bool> {
        self.evs.extend(sim.crew_ev.drain(..).filter(|e| !matches!(e, CrewEv::Struck { .. })));
        let gap = sim.secs().saturating_sub(self.last_report);
        let down = self.evs.iter().any(|e| matches!(e, CrewEv::Down(_)));
        let due = !self.to.is_empty() && (gap >= REPORT_S || (down && gap >= DOWN_S) || (!self.refused.is_empty() && gap >= DOWN_S)) && !(self.evs.is_empty() && self.refused.is_empty() && sim.crew.is_empty());
        if !due {
            return Ok(false);
        }
        let text = report(&self.evs, &self.refused, sim);
        self.evs.clear();
        self.refused.clear();
        self.last_report = sim.secs();
        for s in &self.to {
            crate::peer::send(s, &self.inbox, SENDER, &text)?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;

    #[test]
    fn order_lines_parse_into_closed_orders_or_a_reason() {
        let j = parse_line(r#"{"t":"join","id":"mira","name":"MIRA","side":"pal","role":"healer","say":"I have you!"}"#).unwrap().unwrap();
        assert!(matches!(&j, Order::Join { side: Allegiance::Pal, role: Role::Healer, say: Some(s), .. } if s.as_str() == "I have you!"));
        assert_eq!(parse_line("Sure, here are the orders:"), Ok(None), "prose is ignored");
        assert!(matches!(parse_line(r#"{"t":"order","id":"mira","do":"hold"}"#), Ok(Some(Order::Stance { stance: Stance::Hold, .. }))));
        assert!(matches!(parse_line(r#"{"t":"move","id":"mira","dx":3,"dy":-2}"#), Ok(Some(Order::Move { dx: 3, dy: -2, .. }))));
        // NEG: every malformed field names itself
        for (bad, why) in [
            (r#"{"t":"join","id":"m","name":"M","side":"pal","role":"bard"}"#, "role: one of healer"),
            (r#"{"t":"join","id":"m","name":"M","side":"both","role":"tank"}"#, "side \"both\""),
            (r#"{"t":"move","id":"m","dx":99,"dy":0}"#, "\"dx\": whole tiles"),
            (r#"{"t":"order","id":"<script>","do":"hold"}"#, "\"id\": 1-16"),
            (r#"{"t":"dance","id":"m"}"#, "t \"dance\""),
            (r#"{"t":"say","id":"m","text":"   "}"#, "text: printable"),
            ("{not json", "not JSON"),
        ] {
            assert!(parse_line(bad).unwrap_err().contains(why), "{bad}");
        }
        assert_eq!(Say::parse("héllo\u{1b}[2J wörld").map(|s| s.0), Some("hllo[2J wrld".into()), "only printable ASCII reaches the screen");
    }

    #[test]
    fn an_agent_drives_the_crew_through_a_socket_and_gets_reports() {
        let c = content();
        let mut sim = Sim::new(&c, 7, None);
        let dir = std::env::temp_dir().join(format!("tot-crew-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut link = Link::open(&dir).unwrap();
        // the "agent" is a socket we read ourselves: what the game sends is what we see
        std::fs::create_dir_all(dir.join("agent")).unwrap();
        let agent_inbox = Inbox::open(&dir.join("agent")).unwrap();
        let agent = crate::peer::tests::session_at("agent", &agent_inbox);
        let game = crate::peer::tests::session_at("game", &link.inbox);
        link.prompt(&agent, "send a healer and a rival archer", &sim).unwrap();
        let req = agent_inbox.poll().concat();
        assert!(req.contains("send a healer and a rival archer") && req.contains(&link.inbox.addr()) && req.contains("\"t\":\"state\""), "protocol, prompt, reply address, state");
        assert!(link.prompt(&agent, "  ", &sim).is_err(), "NEG: an empty prompt is not sent");
        let orders = "On it.\n{\"t\":\"join\",\"id\":\"doc\",\"name\":\"DOC\",\"side\":\"pal\",\"role\":\"healer\"}\n{\"t\":\"join\",\"id\":\"bow\",\"name\":\"BOW\",\"side\":\"foe\",\"role\":\"archer\"}\n{\"t\":\"order\",\"id\":\"ghost\",\"do\":\"hold\"}";
        crate::peer::send(&game, &agent_inbox, "agent", orders).unwrap();
        let heard = link.poll(&mut sim);
        assert_eq!(heard.iter().filter(|h| matches!(h, Heard::Applied(_))).count(), 2);
        assert!(matches!(heard.last(), Some(Heard::Refused(r)) if r.contains("NoSuchMate")), "{heard:?}");
        assert_eq!(sim.crew.len(), 2);
        sim.tick = sim.c.hz().get() * DOWN_S;
        assert!(link.tick(&mut sim).unwrap(), "a refusal is reported once DOWN_S passed");
        let rep = agent_inbox.poll().concat();
        assert!(rep.contains("doc joined") && rep.contains("Refused:") && rep.contains("\"id\":\"bow\""), "{rep}");
        assert!(!link.tick(&mut sim).unwrap(), "control: nothing new, no report");
        drop(agent_inbox);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
