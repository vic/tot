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
    /// a scene card over the game (the world waits for the player)
    Scene(Scene),
    /// what a level is: its name, safe (town: no waves) or a dungeon, its danger, its look (agents make new dungeons)
    Place { level: crate::sim::Level, def: crate::sim::LevelDef },
    /// a goal for the hero with a reward the engine pays when it is done
    Quest { key: Word, title: Word, goal: crate::sim::Goal, reward: crate::sim::Reward },
    /// someone on a level with a job (its shop, a healer, words); its lines show when the hero meets it
    Person { key: Word, name: Word, job: crate::sim::Job, glyph: char, rgb: Option<Rgb8>, level: crate::sim::Level, at: Option<(i32, i32)>, lines: Vec<Say> },
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

/// A scene card an agent puts over the game: a title, 1..=SCENE_LINES lines, 0..=SCENE_CHOICES choices. The world
/// waits while it is open; the player's choice goes back to the agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scene {
    pub key: Word,
    pub title: Say,
    pub lines: Vec<Say>,
    pub choices: Vec<(Word, Say)>,
}

pub const SCENE_LINES: usize = 8;
pub const SCENE_CHOICES: usize = 4;

fn scene(v: &serde_json::Value, key: Word) -> std::result::Result<Scene, String> {
    let title = v["title"].as_str().and_then(Say::parse).ok_or("\"title\": printable ASCII, not blank")?;
    let lines = v["lines"].as_array().filter(|a| (1..=SCENE_LINES).contains(&a.len())).ok_or(format!("\"lines\": a list of 1-{SCENE_LINES} strings"))?;
    let lines = lines.iter().enumerate().map(|(k, l)| l.as_str().and_then(Say::parse).ok_or(format!("lines[{k}]: printable ASCII, not blank"))).collect::<std::result::Result<Vec<_>, _>>()?;
    let choices = match &v["choices"] {
        serde_json::Value::Null => Vec::new(),
        serde_json::Value::Array(a) if a.len() <= SCENE_CHOICES => a
            .iter()
            .enumerate()
            .map(|(k, c)| match (c["id"].as_str().and_then(Word::parse), c["text"].as_str().and_then(Say::parse)) {
                (Some(id), Some(t)) => Ok((id, t)),
                _ => Err(format!("choices[{k}]: {{\"id\": 1-16 of A-Z a-z 0-9 _ - space, \"text\": printable ASCII}}")),
            })
            .collect::<std::result::Result<Vec<_>, _>>()?,
        _ => return Err(format!("\"choices\": a list of at most {SCENE_CHOICES}")),
    };
    Ok(Scene { key, title, lines, choices })
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
    let level = || match &v["level"] {
        serde_json::Value::String(s) if s == "town" => Ok(crate::sim::Level::Town),
        n => n.as_u64().and_then(|d| u8::try_from(d).ok()).and_then(std::num::NonZeroU8::new).map(crate::sim::Level::Deep).ok_or("\"level\": \"town\" or a depth 1-255".to_string()),
    };
    if t == "place" {
        let name = match v.get("name") {
            None => None,
            Some(_) => Some(word("name")?),
        };
        let safe = match v["kind"].as_str() {
            None => None,
            Some("town") => Some(true),
            Some("dungeon") => Some(false),
            Some(k) => return Err(format!("kind \"{k}\": town or dungeon")),
        };
        let danger = match &v["danger"] {
            serde_json::Value::Null => None,
            d => Some(d.as_u64().and_then(|d| u8::try_from(d).ok()).and_then(crate::sim::Danger::new).ok_or(format!("\"danger\": 0-{}", crate::sim::DANGER_MAX))?),
        };
        let look = match &v["look"] {
            serde_json::Value::Null => None,
            l => Some(match (l["floor"].as_str().and_then(colour), l["wall"].as_str().and_then(colour)) {
                (Some(f), Some(w)) => (f, w),
                _ => return Err("\"look\": {\"floor\": \"#rrggbb\", \"wall\": \"#rrggbb\"}".into()),
            }),
        };
        if (name, safe, danger, look) == (None, None, None, None) {
            return Err("place needs a name, kind, danger or look".into());
        }
        return Ok(Some(Order::Place { level: level()?, def: crate::sim::LevelDef { name, safe, danger, look } }));
    }
    if t == "quest" {
        let g = &v["goal"];
        let count = |f: &str| g[f].as_u64().filter(|n| (1..=10_000).contains(n)).map(|n| n as u32);
        let goal = if let Some(n) = count("slay") {
            crate::sim::Goal::Slay(n)
        } else if let Some(n) = count("rescue").filter(|n| *n <= 100) {
            crate::sim::Goal::Rescue(n as u16)
        } else if !g["reach"].is_null() {
            let reach = &g["reach"];
            crate::sim::Goal::Reach(match reach {
                serde_json::Value::String(s) if s == "town" => crate::sim::Level::Town,
                n => n.as_u64().and_then(|d| u8::try_from(d).ok()).and_then(std::num::NonZeroU8::new).map(crate::sim::Level::Deep).ok_or("goal.reach: \"town\" or a depth 1-255")?,
            })
        } else {
            return Err("\"goal\": {\"slay\": 1-10000} or {\"rescue\": 1-100} or {\"reach\": \"town\" or a depth}".into());
        };
        let r = &v["reward"];
        let cap = |f: &str, max: u32| match &r[f] {
            serde_json::Value::Null => Ok(0),
            n => n.as_u64().filter(|n| *n <= u64::from(max)).map(|n| n as u32).ok_or(format!("reward.{f}: 0-{max}")),
        };
        let reward = crate::sim::Reward { gold: cap("gold", crate::sim::quest_caps::GOLD_MAX)?, xp: cap("xp", crate::sim::quest_caps::XP_MAX)? };
        return Ok(Some(Order::Quest { key: word("id")?, title: word("title")?, goal, reward }));
    }
    if t == "person" {
        let glyph = s("glyph")?.chars().collect::<Vec<_>>();
        let glyph = match glyph.as_slice() {
            [g] if g.is_ascii_graphic() => *g,
            _ => return Err("\"glyph\": one printable ASCII char".into()),
        };
        let rgb = match v["color"].as_str() {
            Some(c) => Some(colour(c).ok_or("color: #rrggbb")?),
            None => None,
        };
        let lines = v["lines"].as_array().filter(|a| (1..=SCENE_LINES).contains(&a.len())).ok_or(format!("\"lines\": a list of 1-{SCENE_LINES} strings"))?;
        let lines = lines.iter().enumerate().map(|(k, l)| l.as_str().and_then(Say::parse).ok_or(format!("lines[{k}]: printable ASCII, not blank"))).collect::<std::result::Result<Vec<_>, _>>()?;
        let at = match (v.get("dx"), v.get("dy")) {
            (None, None) => None,
            _ => Some((tiles("dx")?, tiles("dy")?)),
        };
        return Ok(Some(Order::Person {
            key: word("id")?,
            name: word("name")?,
            job: crate::sim::Job::parse(s("job")?).ok_or_else(|| format!("job: one of {}", crate::sim::Job::ALL.map(crate::sim::Job::key).join(", ")))?,
            glyph,
            rgb,
            level: level()?,
            at,
            lines,
        }));
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
        "scene" => Order::Scene(scene(&v, key)?),
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
        Order::Chat { .. } | Order::Scene(_) => Ok(()),
        Order::Person { key, name, job, glyph, rgb, level, at, .. } => sim.add_person(key, name, job, glyph, rgb, level, at),
        Order::Quest { key, title, goal, reward } => sim.give_quest(key, title, goal, reward).map_err(|e| match e {
            crate::sim::QuestErr::Full => CrewErr::Full,
            crate::sim::QuestErr::Taken | crate::sim::QuestErr::Bounds => CrewErr::Taken,
        }),
        Order::Place { level, def } => {
            sim.shape_level(level, def);
            Ok(())
        }
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
        "t": "state", "time_s": sim.secs(), "place": crate::render::level_name(sim, sim.level), "safe": sim.safe(sim.level),
        "hero": {"hp": sim.p.hp / 1000, "max": sim.p.max_hp / 1000, "level": sim.p.level, "gold": sim.p.gold},
        "foes_in_view": near, "foes_alive": sim.e.len(), "crew": crew,
        "quests": sim.quests.iter().filter(|q| !q.done).map(|q| { let (n, of) = q.progress(sim); serde_json::json!({"id": q.key.as_str(), "title": q.title.as_str(), "now": n, "of": of}) }).collect::<Vec<_>>(),
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
{"t":"scene","id":"gate","title":"The Iron Gate","lines":["A guard bars the road.","He wants coin."],"choices":[{"id":"bribe","text":"Pay him"},{"id":"fight","text":"Draw steel"}]}
  a card over the game: 1-8 lines, 0-4 choices; the world waits while the player reads; the report tells you the choice. Use scenes for story, talk, quests and choices between fights.
{"t":"place","level":3,"name":"Drowned Crypt","kind":"dungeon","danger":7,"look":{"floor":"#1a3040","wall":"#5a8aa0"}}
  make and shape levels: "level" is "town" or a depth 1-255; all other fields optional. kind: town (safe: no waves, keepers) or dungeon. danger 0-10 sets foe toughness (unset: deeper is tougher). look: floor and wall colours. The hero takes stairs: the town leads down, each depth leads down and back to the town. Put people on a level and the default keepers there leave: it is yours.
{"t":"person","id":"bran","name":"BRAN","level":"town","job":"forge","glyph":"B","color":"#ffaa55","lines":["Steel for coin, friend.","The deep forges are cold now."],"dx":3,"dy":0}
  someone who lives on a level (dx/dy optional: tiles from its start). job: trade (sells), forge (sells, forges, upgrades), enchant (sells, enchants), heal (heals the hero to full), talk (its lines open as a card). Reports tell you when the hero meets someone.
{"t":"quest","id":"rats","title":"Rat Purge","goal":{"slay":20},"reward":{"gold":150,"xp":40}}
  a goal the engine counts from now: {"slay":n} foes, {"rescue":n} pals won, or {"reach":"town" or a depth}. reward: gold 0-10000, xp 0-5000, paid when done; the report tells you. Give quests through people and scenes.
{"t":"leave","id":"mira"}
ids, names, classes: 1-16 of A-Z a-z 0-9 _ - space. At most 6 members at once. The game sends reports: chat lines, what the crew did, refused lines with the reason, and a state line (positions in tiles from the hero). You direct this world; the engine has no story of its own. Be proactive: when the hero enters a level, give it a name and people with jobs; give quests through people and scenes; send scenes between fights; bring pals, rivals and foes that fit the story; greet the player in the chat and tell them what you can do. Answer reports with new orders when the world calls for it."##;

/// The first message to a session: the protocol, the player's line, the fight now.
pub fn request(prompt: &str, reply_to: &str, sim: &Sim) -> String {
    format!("{PROTOCOL}\n\nReply address: {reply_to}\n\nThe player says: \"{prompt}\"\n\nNow: {}\n", snapshot(sim))
}

/// A report: chat since the last one, crew events, refused lines, the state.
pub fn report(chat: &[(String, String)], evs: &[CrewEv], refused: &[String], news: &[String], sim: &Sim) -> String {
    let mut s = String::from("tot report.\n");
    for n in news {
        s += &format!("{n}\n");
    }
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
    /// what the player did that the agents asked about (scene choices)
    news: Vec<String>,
    evs: Vec<CrewEv>,
    refused: Vec<String>,
    last_report: u32,
}

impl Link {
    pub fn open(socks: &Path) -> Result<Link> {
        Ok(Link { inbox: Inbox::open(socks)?, to: Vec::new(), chat: Vec::new(), news: Vec::new(), evs: Vec::new(), refused: Vec::new(), last_report: 0 })
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

    /// Quest `key` is done (the engine paid its reward).
    pub fn quest_done(&mut self, key: &str, title: &str) {
        self.news.push(format!("Quest {key} ({title}) is done; its reward is paid."));
    }

    /// The hero walked up to the person named `name`.
    pub fn met(&mut self, name: &str) {
        self.news.push(format!("The hero met {name}."));
    }

    /// The hero took a stair to the level named `name` (in the adventure's words).
    pub fn entered(&mut self, name: &str) {
        self.news.push(format!("The hero entered {name}."));
    }

    /// The player closed scene `scene` with `choice` (None: a scene with no choices, read and closed).
    pub fn chose(&mut self, scene: Word, choice: Option<Word>) {
        self.news.push(match choice {
            Some(c) => format!("Scene {}: the player chose {}.", scene.as_str(), c.as_str()),
            None => format!("Scene {}: the player read it and closed it.", scene.as_str()),
        });
    }

    /// Take the sim's crew events; send a report to every agent when one is due. Ok(true) when it went out.
    pub fn tick(&mut self, sim: &mut Sim) -> Result<bool> {
        self.evs.extend(sim.crew_ev.drain(..).filter(|e| !matches!(e, CrewEv::Struck { .. })));
        let gap = sim.secs().saturating_sub(self.last_report);
        let news = !self.news.is_empty() || !self.chat.is_empty() || !self.refused.is_empty() || self.evs.iter().any(|e| matches!(e, CrewEv::Down(_)));
        let quiet = self.evs.is_empty() && !news && sim.crew.is_empty();
        if self.to.is_empty() || quiet || !(gap >= REPORT_S || (news && gap >= NEWS_S)) {
            return Ok(false);
        }
        let text = report(&self.chat, &self.evs, &self.refused, &self.news, sim);
        self.news.clear();
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
        let gate = r#"{"t":"scene","id":"gate","title":"The Iron Gate","lines":["A gate bars the road.","A guard waits."],"choices":[{"id":"bribe","text":"Pay him"},{"id":"fight","text":"Draw steel"}]}"#;
        let Ok(Some(Order::Scene(s))) = parse_line(gate) else { panic!("scene") };
        assert!(s.key.as_str() == "gate" && s.lines.len() == 2 && s.choices[1].0.as_str() == "fight" && s.choices[1].1.as_str() == "Draw steel");
        assert!(matches!(parse_line(&gate.replace(r#","choices":[{"id":"bribe","text":"Pay him"},{"id":"fight","text":"Draw steel"}]"#, "")), Ok(Some(Order::Scene(s))) if s.choices.is_empty()), "choices are optional");
        assert!(matches!(parse_line(r#"{"t":"place","level":"town","name":"Emberhold"}"#), Ok(Some(Order::Place { level: crate::sim::Level::Town, .. }))));
        assert!(matches!(parse_line(r#"{"t":"place","level":3,"name":"The Ossuary"}"#), Ok(Some(Order::Place { level: crate::sim::Level::Deep(d), .. })) if d.get() == 3));
        assert!(parse_line(r#"{"t":"place","level":0,"name":"x"}"#).unwrap_err().contains("\"level\""), "NEG: depth 0 is no level");
        assert!(parse_line(r#"{"t":"place","level":"heaven","name":"x"}"#).unwrap_err().contains("\"level\""));
        let crypt = r##"{"t":"place","level":3,"name":"Drowned Crypt","kind":"dungeon","danger":7,"look":{"floor":"#1a3040","wall":"#5a8aa0"}}"##;
        assert!(matches!(parse_line(crypt), Ok(Some(Order::Place { def: crate::sim::LevelDef { safe: Some(false), danger: Some(d), look: Some(_), name: Some(_) }, .. })) if d.get() == 7));
        for (bad, why) in [(crypt.replace(r#""danger":7"#, r#""danger":11"#), "\"danger\""), (crypt.replace("dungeon", "castle"), "kind \"castle\""), (crypt.replace("#5a8aa0", "blue"), "\"look\""), (r#"{"t":"place","level":2}"#.into(), "place needs")] {
            assert!(parse_line(&bad).unwrap_err().contains(why), "{bad}: {:?}", parse_line(&bad));
        }
        let rats = r#"{"t":"quest","id":"rats","title":"Rat Purge","goal":{"slay":20},"reward":{"gold":150,"xp":40}}"#;
        assert!(matches!(parse_line(rats), Ok(Some(Order::Quest { goal: crate::sim::Goal::Slay(20), reward: crate::sim::Reward { gold: 150, xp: 40 }, .. }))));
        assert!(matches!(parse_line(&rats.replace(r#"{"slay":20}"#, r#"{"reach":"town"}"#)), Ok(Some(Order::Quest { goal: crate::sim::Goal::Reach(crate::sim::Level::Town), .. }))));
        assert!(matches!(parse_line(&rats.replace(r#"{"slay":20}"#, r#"{"rescue":2}"#)), Ok(Some(Order::Quest { goal: crate::sim::Goal::Rescue(2), .. }))));
        for (bad, why) in [(rats.replace(r#"{"slay":20}"#, r#"{"slay":0}"#), "\"goal\""), (rats.replace(r#"{"slay":20}"#, r#"{"dance":1}"#), "\"goal\""), (rats.replace("150", "99999"), "reward.gold"), (rats.replace(r#"{"slay":20}"#, r#"{"reach":"moon"}"#), "goal.reach")] {
            assert!(parse_line(&bad).unwrap_err().contains(why), "{bad}: {:?}", parse_line(&bad));
        }
        let bran = r##"{"t":"person","id":"bran","name":"BRAN","level":"town","job":"forge","glyph":"B","color":"#ffaa55","lines":["Steel for coin."],"dx":3,"dy":0}"##;
        assert!(matches!(parse_line(bran), Ok(Some(Order::Person { job: crate::sim::Job::Forge, glyph: 'B', level: crate::sim::Level::Town, at: Some((3, 0)), .. }))));
        assert!(matches!(parse_line(&bran.replace(r#","dx":3,"dy":0"#, "")), Ok(Some(Order::Person { at: None, .. }))), "a spot is optional");
        for (bad, why) in [(bran.replace("forge", "bake"), "job: one of"), (bran.replace(r#""glyph":"B""#, r#""glyph":"BB""#), "\"glyph\""), (bran.replace(r#"["Steel for coin."]"#, "[]"), "\"lines\""), (bran.replace(r#""dx":3,"#, ""), "\"dx\"")] {
            assert!(parse_line(&bad).unwrap_err().contains(why), "{bad}: {:?}", parse_line(&bad));
        }
        for (bad, why) in [
            (gate.replace(r#""title":"The Iron Gate""#, r#""title":"  ""#), "\"title\""),
            (gate.replace(r#"["A gate bars the road.","A guard waits."]"#, "[]"), "\"lines\": a list of 1-8"),
            (gate.replace(r#""id":"fight""#, r#""id":"<x>""#), "choices[1]"),
            (gate.replace(r#"{"id":"bribe","text":"Pay him"}"#, r#"{"id":"a","text":"a"},{"id":"b","text":"b"},{"id":"c","text":"c"},{"id":"d","text":"d"}"#), "\"choices\": a list of at most 4"),
        ] {
            assert!(parse_line(&bad).unwrap_err().contains(why), "{bad}: {:?}", parse_line(&bad));
        }
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
        // the player's scene choice goes back to every agent as news
        link.chose(Word::parse("gate").unwrap(), Some(Word::parse("open").unwrap()));
        sim.tick += sim.c.hz().get() * NEWS_S;
        assert!(link.tick(&mut sim).unwrap(), "a choice is news");
        assert!(lead_in.poll().concat().contains("Scene gate: the player chose open."));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
