//! Agentic crew link: the game as a chat room for agents. The player types a line (P); the game sends it with the
//! crew protocol and the state of the fight to a live local Claude session (peer sockets). Any agent that writes
//! to the game's socket (that session, or subagents it spawned) can make crew members from its own kit data, order
//! them, and chat; the game applies every order line and relays the chat, what the crew did, refused lines and the
//! state to every agent that ever wrote, so one prompt can field a whole team (or a rival band).
use crate::peer::{Inbox, Session};
use crate::sim::{Feat, Allegiance, CrewErr, CrewEv, Kit, Rgb8, Sim, Stance, Verb, Word};
use crate::{Error, Result};
use std::path::Path;

/// One line of an agent's answer, parsed: only a well-formed order is representable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Order {
    Join { key: Word, name: Word, side: Allegiance, kit: Kit, say: Option<Say> },
    Stance { key: Word, stance: Stance },
    Move { key: Word, dx: i32, dy: i32 },
    /// a crew member speaks (shown over it and in the chat)
    Say { key: Word, text: Say },
    /// an agent speaks in the chat without a body
    Chat { from: Word, text: Say },
    Leave { key: Word },
}

/// A chat line: 1..=SAY_MAX printable ASCII chars (the terminal draws every one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Say(String);

const SAY_MAX: usize = 60;

impl Say {
    pub fn parse(s: &str) -> Option<Say> {
        let t: String = s.trim().chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(SAY_MAX).collect();
        (!t.trim().is_empty()).then_some(Say(t))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A number given in agent units (seconds, metres, shares), as thousandths.
fn milli(v: &serde_json::Value, f: &str) -> std::result::Result<i64, String> {
    v[f].as_f64().filter(|x| x.is_finite() && x.abs() < 1e6).map(|x| (x * 1000.0).round() as i64).ok_or_else(|| format!("needs \"{f}\" as a number"))
}

fn colour(s: &str) -> Option<Rgb8> {
    let h = s.strip_prefix('#').filter(|h| h.len() == 6)?;
    let b = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some(Rgb8(b(0)?, b(2)?, b(4)?))
}

fn ability(v: &serde_json::Value, k: usize) -> std::result::Result<Feat, String> {
    let verb = v["verb"].as_str().and_then(Verb::parse).ok_or_else(|| format!("abilities[{k}].verb: one of {}", Verb::ALL.map(Verb::key).join(", ")))?;
    let n = |f: &str| milli(v, f).map_err(|e| format!("abilities[{k}]: {e}"));
    let (every, reach, power, radius) = (n("every_s")?, n("reach_m")?, n("power")?, milli(v, "radius_m").unwrap_or(0));
    let fit = |x: i64| i32::try_from(x).unwrap_or(i32::MAX);
    Feat::new(verb, u32::try_from(every).unwrap_or(0), fit(reach), fit(power), fit(radius)).ok_or_else(|| {
        let (e, r, p, d) = (crate::sim::crew_ranges::EVERY_MS, crate::sim::crew_ranges::REACH_MM, crate::sim::crew_ranges::POWER_PM, crate::sim::crew_ranges::RADIUS_MM);
        format!("abilities[{k}] out of range: every_s {}..{}, reach_m {}..{}, power {}..{}, radius_m {}..{}", e.0 / 1000, e.1 / 1000, r.0 / 1000, r.1 / 1000, p.0 / 1000, p.1 / 1000, d.0 / 1000, d.1 / 1000)
    })
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
    let say = |f: &str| s(f).and_then(|x| Say::parse(x).ok_or_else(|| format!("\"{f}\": printable ASCII, not blank")));
    let tiles = |f: &str| v[f].as_i64().and_then(|n| i32::try_from(n).ok()).filter(|n| n.abs() <= 40).ok_or_else(|| format!("\"{f}\": whole tiles, -40..=40"));
    let t = s("t")?;
    if t == "chat" {
        return Ok(Some(Order::Chat { from: word("from")?, text: say("text")? }));
    }
    let key = word("id")?;
    Ok(Some(match t {
        "join" => {
            let side = match s("side")? {
                "pal" => Allegiance::Pal,
                "foe" => Allegiance::Foe,
                x => return Err(format!("side \"{x}\": pal or foe")),
            };
            let glyph = s("glyph")?.chars().collect::<Vec<_>>();
            let abilities = v["abilities"].as_array().ok_or("needs \"abilities\" as a list")?.iter().enumerate().map(|(k, a)| ability(a, k)).collect::<std::result::Result<Vec<_>, _>>()?;
            let rgb = match v["color"].as_str() {
                Some(c) => Some(colour(c).ok_or("color: #rrggbb")?),
                None => None,
            };
            let (hp, pace) = (milli(&v, "hp")?, milli(&v, "pace").unwrap_or(1000));
            let r = (crate::sim::crew_ranges::HP_PM, crate::sim::crew_ranges::PACE_PM);
            let kit = match glyph.as_slice() {
                [g] => Kit::new(word("class")?, *g, rgb, i32::try_from(hp).unwrap_or(0), i32::try_from(pace).unwrap_or(0), &abilities),
                _ => None,
            }
            .ok_or_else(|| format!("kit refused: glyph one printable ASCII char, hp {}..{}, pace {}..{}, 1-{} abilities", f64::from(r.0.0) / 1000.0, f64::from(r.0.1) / 1000.0, f64::from(r.1.0) / 1000.0, f64::from(r.1.1) / 1000.0, crate::sim::crew_ranges::ABILITIES))?;
            Order::Join { key, name: word("name")?, side, kit, say: v["say"].as_str().and_then(Say::parse) }
        }
        "order" => Order::Stance { key, stance: Stance::parse(s("do")?).ok_or_else(|| format!("do: one of {}", Stance::ALL.map(Stance::key).join(", ")))? },
        "move" => Order::Move { key, dx: tiles("dx")?, dy: tiles("dy")? },
        "say" => Order::Say { key, text: say("text")? },
        "leave" => Order::Leave { key },
        x => return Err(format!("t \"{x}\": join, order, move, say, chat or leave")),
    }))
}

/// Apply one order to the sim. `say` and `chat` change nothing in the sim (the view shows them).
pub fn apply(sim: &mut Sim, o: &Order) -> std::result::Result<(), CrewErr> {
    match *o {
        Order::Join { key, name, side, kit, .. } => sim.crew_join(key, name, kit, side),
        Order::Stance { key, stance } => sim.crew_order(key, stance),
        Order::Move { key, dx, dy } => sim.crew_goal(key, dx, dy),
        Order::Say { key, .. } => sim.crew.iter().any(|m| m.key == key).then_some(()).ok_or(CrewErr::NoSuchMate),
        Order::Chat { .. } => Ok(()),
        Order::Leave { key } => sim.crew_leave(key),
    }
}

/// The chat line an applied order adds: (speaker, text).
pub fn chat_line(sim: &Sim, o: &Order) -> Option<(String, String)> {
    match o {
        Order::Join { name, say: Some(t), .. } => Some((name.as_str().to_string(), t.as_str().to_string())),
        Order::Say { key, text } => sim.crew.iter().find(|m| m.key == *key).map(|m| (m.name.as_str().to_string(), text.as_str().to_string())),
        Order::Chat { from, text } => Some((from.as_str().to_string(), text.as_str().to_string())),
        _ => None,
    }
}

/// The fight as an agent sees it, one JSON line: positions in whole tiles from the hero, HP in whole points.
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
            Some(serde_json::json!({"id": m.key.as_str(), "name": m.name.as_str(), "class": m.kit.class.as_str(), "side": side, "do": m.stance.key(), "dx": dx, "dy": dy, "hp": hp / 1000, "max": max / 1000}))
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

/// One crew event as an agent reads it.
fn event(e: &CrewEv) -> String {
    match e {
        CrewEv::Joined(k) => format!("{} joined", k.as_str()),
        CrewEv::Down(k) => format!("{} is down", k.as_str()),
        CrewEv::Healed { by, hp } => format!("{} healed {} HP", by.as_str(), hp / 1000),
        CrewEv::Raised { by } => format!("{} raised a foe", by.as_str()),
        CrewEv::Struck { by, dmg } => format!("{} hit for {}", by.as_str(), dmg / 1000),
        CrewEv::Cast { by, verb } => format!("{} used {} on the hero", by.as_str(), verb.key()),
    }
}

const PROTOCOL: &str = r##"You play crew members in a live game of tot (a terminal Diablo-like horde survival). The game has no classes of its own: you invent each member (its name, class, look, numbers and abilities) to fit the player's prompt below, and you keep ordering them as the fight goes on. Pals fight beside the hero; foes hunt the hero and its pals. The game is also a chat room: the player, you, and any subagents you spawn talk there.

Send messages with the SendMessage tool to the reply address below (it is the game itself, not a person). You may spawn subagents and give them that address: each one can run its own members and chat; the game answers whoever writes to it. Put one JSON object per line; other lines are ignored.

{"t":"join","id":"mira","name":"MIRA","side":"pal","class":"lifebinder","glyph":"M","color":"#66ffcc","hp":0.8,"pace":0.9,"say":"I have you!","abilities":[{"verb":"heal","every_s":2,"reach_m":9,"power":0.15}]}
  side: pal or foe. glyph: one printable ASCII char drawn as its head. color: #rrggbb. hp: multiple of the hero's base HP (0.1..4). pace: multiple of the hero's speed (0.2..1.5).
  abilities: 1-4 of {"verb", "every_s" 0.2..30, "reach_m" 0.5..15, "power" 0..5, "radius_m" 0..6}. verbs:
    heal: a pal heals the hero or the most hurt pal, a foe the most hurt foe; power = share of the target's max HP.
    strike: a pal hits the nearest foe in reach; a foe shoots at the hero in reach; power = multiple of the hero's hit (pal) or the body's hit (foe).
    blast: a pal hits all foes within radius_m of the nearest foe in reach; a foe drops a telegraphed blast of radius_m on the hero.
    raise: a pal turns a foe in reach below power share of its HP to fight for the hero; a foe raises power (1..5) foes around itself.
    taunt: a pal draws foes within radius_m onto itself; a foe flies into a rage when the hero is in reach.
{"t":"order","id":"mira","do":"follow"}   do: follow, hold, charge or flee
{"t":"move","id":"mira","dx":3,"dy":-2}   go to a spot in whole tiles from the hero (-40..40)
{"t":"say","id":"mira","text":"Behind you!"}   a member speaks (60 chars)
{"t":"chat","from":"captain","text":"Tank left, I heal."}   you speak in the chat without a body
{"t":"leave","id":"mira"}
ids, names, classes: 1-16 of A-Z a-z 0-9 _ - space. At most 6 members at once. The game sends reports: chat lines, what the crew did, refused lines with the reason, and a state line (positions in tiles from the hero). Answer with new orders when the fight calls for it; stay silent otherwise."##;

/// The first message to a session: the protocol, the player's line, the fight now.
pub fn request(prompt: &str, reply_to: &str, sim: &Sim) -> String {
    format!("{PROTOCOL}\n\nReply address: {reply_to}\n\nThe player says: \"{prompt}\"\n\nNow: {}\n", snapshot(sim))
}

/// A report: chat since the last one, crew events, refused lines, the state.
pub fn report(chat: &[(String, String)], evs: &[CrewEv], refused: &[String], sim: &Sim) -> String {
    let mut s = String::from("tot report.\n");
    for (who, text) in chat {
        s += &format!("Chat {who}: {text}\n");
    }
    if !evs.is_empty() {
        s += &format!("Crew: {}.\n", evs.iter().map(event).collect::<Vec<_>>().join("; "));
    }
    for r in refused {
        s += &format!("Refused: {r}\n");
    }
    s + &snapshot(sim) + "\n"
}

/// Shortest gap between two reports (s); news (a member down, chat, a refusal) goes out after NEWS_S.
const REPORT_S: u32 = 20;
const NEWS_S: u32 = 2;
/// The name the game signs crew messages with.
const SENDER: &str = "tot";

/// What one poll heard, for the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    Applied(Order),
    Refused(String),
}

/// The running link: our inbox, every agent that may hear us, what waits to be reported.
pub struct Link {
    inbox: Inbox,
    to: Vec<Session>,
    chat: Vec<(String, String)>,
    evs: Vec<CrewEv>,
    refused: Vec<String>,
    last_report: u32,
}

impl Link {
    pub fn open(socks: &Path) -> Result<Link> {
        Ok(Link { inbox: Inbox::open(socks)?, to: Vec::new(), chat: Vec::new(), evs: Vec::new(), refused: Vec::new(), last_report: 0 })
    }

    fn hear(&mut self, s: Session) {
        if !self.to.iter().any(|t| t.socket_eq(&s)) {
            self.to.push(s);
        }
    }

    /// Send the player's line to `to` (with the protocol) and relay it as chat to every agent at the next report.
    pub fn prompt(&mut self, to: &Session, prompt: &str, sim: &Sim) -> Result<()> {
        if prompt.trim().is_empty() {
            return Err(Error::Peer("empty prompt".into()));
        }
        crate::peer::send(to, &self.inbox, SENDER, &request(prompt, &self.inbox.addr(), sim))?;
        self.hear(to.clone());
        self.last_report = sim.secs();
        Ok(())
    }

    /// Read what agents sent: apply every order line, keep chat and refusals for the next report, and answer
    /// every sender from now on.
    pub fn poll(&mut self, sim: &mut Sim) -> Vec<Heard> {
        let mut out = Vec::new();
        for letter in self.inbox.letters() {
            if let Some(s) = letter.from {
                self.hear(s);
            }
            for line in letter.text.lines() {
                let heard = match parse_line(line) {
                    Ok(None) => continue,
                    Ok(Some(o)) => match apply(sim, &o) {
                        Ok(()) => Heard::Applied(o),
                        Err(e) => Heard::Refused(format!("{}: {e:?}", line.trim())),
                    },
                    Err(why) => Heard::Refused(format!("{}: {why}", line.trim())),
                };
                match &heard {
                    Heard::Refused(r) => self.refused.push(r.clone()),
                    Heard::Applied(o) => self.chat.extend(chat_line(sim, o)),
                }
                out.push(heard);
            }
        }
        out
    }

    /// Take the sim's crew events; send a report to every agent when one is due. Ok(true) when it went out.
    pub fn tick(&mut self, sim: &mut Sim) -> Result<bool> {
        self.evs.extend(sim.crew_ev.drain(..).filter(|e| !matches!(e, CrewEv::Struck { .. })));
        let gap = sim.secs().saturating_sub(self.last_report);
        let news = !self.chat.is_empty() || !self.refused.is_empty() || self.evs.iter().any(|e| matches!(e, CrewEv::Down(_)));
        let quiet = self.evs.is_empty() && !news && sim.crew.is_empty();
        if self.to.is_empty() || quiet || !(gap >= REPORT_S || (news && gap >= NEWS_S)) {
            return Ok(false);
        }
        let text = report(&self.chat, &self.evs, &self.refused, sim);
        self.chat.clear();
        self.evs.clear();
        self.refused.clear();
        self.last_report = sim.secs();
        // an agent that went away is dropped, the others still hear
        self.to.retain(|s| crate::peer::send(s, &self.inbox, SENDER, &text).is_ok());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tests::content;

    const HEALER: &str = r##"{"t":"join","id":"doc","name":"DOC","side":"pal","class":"lifebinder","glyph":"D","color":"#66ffcc","hp":0.8,"say":"I have you!","abilities":[{"verb":"heal","every_s":2,"reach_m":9,"power":0.15}]}"##;

    #[test]
    fn order_lines_parse_into_closed_orders_or_a_reason() {
        let j = parse_line(HEALER).unwrap().unwrap();
        assert!(matches!(&j, Order::Join { side: Allegiance::Pal, kit, say: Some(s), .. } if s.as_str() == "I have you!" && kit.glyph == 'D' && kit.rgb == Some(Rgb8(0x66, 0xff, 0xcc)) && kit.class.as_str() == "lifebinder"));
        assert_eq!(parse_line("Sure, here are the orders:"), Ok(None), "prose is ignored");
        assert!(matches!(parse_line(r#"{"t":"order","id":"mira","do":"hold"}"#), Ok(Some(Order::Stance { stance: Stance::Hold, .. }))));
        assert!(matches!(parse_line(r#"{"t":"move","id":"mira","dx":3,"dy":-2}"#), Ok(Some(Order::Move { dx: 3, dy: -2, .. }))));
        assert!(matches!(parse_line(r#"{"t":"chat","from":"captain","text":"tank left"}"#), Ok(Some(Order::Chat { .. }))));
        // NEG: every malformed field names itself
        for (bad, why) in [
            (HEALER.replace("\"heal\"", "\"dance\""), "abilities[0].verb"),
            (HEALER.replace("\"every_s\":2", "\"every_s\":0.01"), "abilities[0] out of range"),
            (HEALER.replace("\"hp\":0.8", "\"hp\":80"), "kit refused"),
            (HEALER.replace("\"glyph\":\"D\"", "\"glyph\":\"DD\""), "kit refused"),
            (HEALER.replace("#66ffcc", "green"), "color: #rrggbb"),
            (HEALER.replace("\"side\":\"pal\"", "\"side\":\"both\""), "side \"both\""),
            (r#"{"t":"move","id":"m","dx":99,"dy":0}"#.into(), "\"dx\": whole tiles"),
            (r#"{"t":"order","id":"<script>","do":"hold"}"#.into(), "\"id\": 1-16"),
            (r#"{"t":"dance","id":"m"}"#.into(), "t \"dance\""),
            (r#"{"t":"say","id":"m","text":"   "}"#.into(), "\"text\": printable"),
            ("{not json".into(), "not JSON"),
        ] {
            assert!(parse_line(&bad).unwrap_err().contains(why), "{bad}: {:?}", parse_line(&bad));
        }
        assert_eq!(Say::parse("héllo\u{1b}[2J wörld").map(|s| s.0), Some("hllo[2J wrld".into()), "only printable ASCII reaches the screen");
    }

    #[test]
    fn agents_and_their_subagents_drive_the_crew_through_the_socket_and_hear_the_chat() {
        let c = content();
        let mut sim = Sim::new(&c, 7, None);
        let dir = std::env::temp_dir().join(format!("tot-crew-{}", std::process::id()));
        for d in ["game", "lead", "sub"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        let mut link = Link::open(&dir.join("game")).unwrap();
        // the lead agent and a subagent it spawned: sockets we read ourselves
        let (lead_in, sub_in) = (Inbox::open(&dir.join("lead")).unwrap(), Inbox::open(&dir.join("sub")).unwrap());
        let lead = crate::peer::tests::session_at("lead", &lead_in);
        let game = crate::peer::tests::session_at("game", &link.inbox);
        link.prompt(&lead, "a healer for me and a rival band", &sim).unwrap();
        let req = lead_in.poll().concat();
        assert!(req.contains("a healer for me and a rival band") && req.contains(&link.inbox.addr()) && req.contains("\"t\":\"state\"") && req.contains("subagents"), "protocol, prompt, reply address, state");
        assert!(link.prompt(&lead, "  ", &sim).is_err(), "NEG: an empty line is not sent");
        crate::peer::send(&game, &lead_in, "lead", &format!("On it.\n{HEALER}\n{{\"t\":\"order\",\"id\":\"ghost\",\"do\":\"hold\"}}")).unwrap();
        // the subagent was never prompted by the game: it writes on its own
        let archer = HEALER.replace("\"doc\"", "\"bow\"").replace("\"pal\"", "\"foe\"").replace("\"heal\"", "\"strike\"");
        crate::peer::send(&game, &sub_in, "sub", &format!("{archer}\n{{\"t\":\"chat\",\"from\":\"bow\",\"text\":\"I hunt you, hero\"}}")).unwrap();
        let heard = link.poll(&mut sim);
        assert_eq!(heard.iter().filter(|h| matches!(h, Heard::Applied(_))).count(), 3, "{heard:?}");
        assert!(heard.iter().any(|h| matches!(h, Heard::Refused(r) if r.contains("NoSuchMate"))));
        assert_eq!(sim.crew.len(), 2);
        sim.tick = sim.c.hz().get() * NEWS_S;
        assert!(link.tick(&mut sim).unwrap(), "news is reported after NEWS_S");
        for (who, inbox) in [("lead", &lead_in), ("sub", &sub_in)] {
            let rep = inbox.poll().concat();
            assert!(rep.contains("Chat DOC: I have you!") && rep.contains("Chat bow: I hunt you, hero") && rep.contains("doc joined") && rep.contains("Refused:") && rep.contains("\"class\":\"lifebinder\""), "{who} hears all: {rep}");
        }
        assert!(!link.tick(&mut sim).unwrap(), "control: nothing new, no report");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
