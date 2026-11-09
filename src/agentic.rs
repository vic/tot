//! Agentic adventures: ask a local Claude Code session to write a whole new adventure from the player's own
//! description, collect the files it sends back, and install them as an adventure the game can load.
use crate::data::{Library, PROFILE_FILES};
use crate::skill::{FILE_CLOSE, FILE_OPEN};
use crate::{Error, Result};
use std::path::Path;

/// The adventures sent along as examples of the format and the balance.
pub const EXAMPLES: [&str; 2] = ["cyberpunk", "hellish"];
/// The line that ends an answer: every file was sent.
pub const DONE: &str = "=== DONE";
/// Longest message the agent should send (characters); a longer answer is split over several messages.
const MAX_MESSAGE: usize = 200_000;
/// Every file an answer must hold.
fn wanted() -> impl Iterator<Item = &'static str> {
    std::iter::once("profile.jsonl").chain(PROFILE_FILES)
}

/// The whole request the agent reads: the skill with the example adventures, the player's description, and how
/// to answer (`reply_to` = the game's own address).
pub fn request(lib: &Library, brief: &str, reply_to: &str, with_skill: bool) -> Result<String> {
    // a batch sends the skill once: later requests point back to it
    let mut s = if with_skill { crate::skill::skill_with(&lib.base, |id| EXAMPLES.contains(&id))? } else { "Use the tot-adventure skill and answer format sent with the first request of this batch.\n".into() };
    let taken = lib.profiles()?.into_iter().map(|p| p.id).collect::<Vec<_>>().join(", ");
    let files = wanted().collect::<Vec<_>>().join(", ");
    s += &format!(
        "\n## the request\n\nThe player wants this adventure: \"{brief}\"\n\nWrite it as a NEW adventure in the answer format above: new names, foes, skills, items, story, words, voices, art, colours and gameplay numbers that fit it. Send every file: {files}. The profile id must be new (taken: {taken}).\n\nAnswer with the SendMessage tool to `{reply_to}` (that address is the game itself, not a person). Put only files in your messages, no other text. If the files pass {MAX_MESSAGE} characters, split them over several messages, whole files per message. After the last file, send a line `{DONE}`. If the game answers with a load error, fix the file it names and send the fixed files again, then `{DONE}`.\n"
    );
    Ok(s)
}

/// Text the agent sent, collected until it says it is done.
#[derive(Default)]
pub struct Answer(String);

impl Answer {
    /// Add one received message; Some(files) once the agent sent `DONE`.
    pub fn push(&mut self, body: &str) -> Option<Result<Vec<(String, String)>>> {
        self.0.push_str(body);
        self.0.push('\n');
        let done = self.0.lines().any(|l| l.trim() == DONE);
        done.then(|| parse(&std::mem::take(&mut self.0)))
    }
}

/// The `FILE` blocks of `text`: every wanted file exactly once, nothing else.
fn parse(text: &str) -> Result<Vec<(String, String)>> {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut open: Option<(String, String)> = None;
    for line in text.lines() {
        match (&mut open, line.strip_prefix(FILE_OPEN)) {
            (None, Some(name)) => open = Some((name.trim().to_string(), String::new())),
            (Some(_), _) if line.trim() == FILE_CLOSE => files.extend(open.take()),
            (Some((_, body)), _) => {
                body.push_str(line);
                body.push('\n');
            }
            // text outside the blocks (a greeting, the DONE line) is not part of any file
            (None, None) => {}
        }
    }
    if let Some((name, _)) = open {
        return Err(Error::Peer(format!("file {name} has no {FILE_CLOSE} line")));
    }
    for (name, _) in &files {
        if !wanted().any(|w| w == name) {
            return Err(Error::Peer(format!("{name} is not an adventure file")));
        }
        if files.iter().filter(|(n, _)| n == name).count() > 1 {
            return Err(Error::Peer(format!("{name} was sent twice")));
        }
    }
    match wanted().find(|w| !files.iter().any(|(n, _)| n == w)) {
        Some(missing) => Err(Error::Peer(format!("{missing} is missing"))),
        None => Ok(files),
    }
}

/// Install the answer in the player's adventures, `<user>/<id>` (id from its profile.jsonl, new among every
/// adventure), and load it once: an
/// adventure the loader refuses is removed again and its error returned. Ok(id) when it loads.
pub fn install(lib: &Library, files: &[(String, String)]) -> Result<String> {
    let profile = files.iter().find(|(n, _)| n == "profile.jsonl").map(|(_, t)| t.as_str()).unwrap_or_default();
    let id = profile.lines().find_map(|l| serde_json::from_str::<serde_json::Value>(l).ok().filter(|v| v["t"] == "profile")).and_then(|v| v["id"].as_str().map(String::from));
    let id = id.filter(|i| !i.is_empty() && i.len() <= 40 && i.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')).ok_or_else(|| Error::Peer("profile.jsonl needs a profile record with an id of [a-z0-9-], at most 40 chars".into()))?;
    let user = lib.user.as_ref().ok_or_else(|| Error::Peer("no place for player adventures: set HOME or XDG_DATA_HOME".into()))?;
    let pdir = user.join(&id);
    if pdir.exists() || lib.profiles()?.iter().any(|p| p.id == id) {
        return Err(Error::Peer(format!("the adventure id '{id}' is taken: choose a new one")));
    }
    std::fs::create_dir_all(&pdir)?;
    let written = files.iter().try_for_each(|(n, t)| std::fs::write(pdir.join(n), t));
    match written.map_err(Error::from).and_then(|()| lib.load(Some(&id))) {
        Ok(_) => Ok(id),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&pdir);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir() -> std::path::PathBuf {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/data")).to_path_buf()
    }

    /// An answer that repeats an installed adventure under a new id, split over two messages.
    fn answer_as(id: &str) -> Vec<String> {
        let files = crate::data::adventure_files(&data_dir(), "kawai").unwrap();
        let block = |(n, t): &(&str, String)| {
            let t = if *n == "profile.jsonl" { t.replace("\"id\":\"kawai\"", &format!("\"id\":\"{id}\"")) } else { t.clone() };
            format!("{FILE_OPEN}{n}\n{}\n{FILE_CLOSE}\n", t.trim_end())
        };
        let (a, b) = files.split_at(4);
        vec![format!("Here you go!\n{}", a.iter().map(block).collect::<String>()), format!("{}{DONE}", b.iter().map(block).collect::<String>())]
    }

    #[test]
    fn the_request_carries_the_skill_two_examples_the_brief_and_the_reply_rules() {
        let r = request(&Library::shipped(&data_dir()), "a sunken jungle temple", "uds:/r/tot-1.sock", true).unwrap();
        assert!(r.contains("(cyberpunk)") && r.contains("(hellish)") && !r.contains("(kawai)"), "only the two examples");
        assert!(r.contains("a sunken jungle temple") && r.contains("uds:/r/tot-1.sock") && r.contains(DONE) && r.contains("pick_adventure"));
        assert!(r.contains("taken: cyberpunk, dnd, hellish, kawai, moriah"), "the ids it must not reuse");
        let later = request(&Library::shipped(&data_dir()), "a sunken jungle temple", "uds:/r/tot-1.sock", false).unwrap();
        assert!(!later.contains("(cyberpunk)") && !later.contains("pick_adventure"), "a later batch request leaves the skill out");
        assert!(later.contains("a sunken jungle temple") && later.contains("uds:/r/tot-1.sock") && later.contains(DONE) && later.contains("taken: cyberpunk"));
    }

    #[test]
    fn an_answer_installs_once_done_and_loads() {
        // the shipped data, and a fresh player directory
        let user = std::env::temp_dir().join(format!("tot-agentic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&user);
        let lib = Library { base: data_dir(), user: Some(user.clone()) };
        let msgs = answer_as("jungle");
        let mut a = Answer::default();
        assert!(a.push(&msgs[0]).is_none(), "not done after the first message");
        let files = a.push(&msgs[1]).expect("done").unwrap();
        assert_eq!(install(&lib, &files).unwrap(), "jungle");
        assert!(user.join("jungle/profile.jsonl").exists() && !data_dir().join("profiles/jungle").exists(), "written to the player's directory, never the shipped one");
        assert!(lib.profiles().unwrap().iter().any(|p| p.id == "jungle") && lib.load(Some("jungle")).is_ok(), "listed and loaded with the shipped ones");
        assert!(install(&lib, &files).unwrap_err().to_string().contains("taken"), "an id is never overwritten");
        // a player folder named like a shipped adventure never hides it
        std::fs::create_dir_all(user.join("cyberpunk")).unwrap();
        std::fs::copy(user.join("jungle/profile.jsonl"), user.join("cyberpunk/profile.jsonl")).unwrap();
        assert_eq!(lib.profiles().unwrap().iter().filter(|p| p.id == "cyberpunk").count(), 1, "the shipped one wins");
        std::fs::remove_dir_all(user.join("cyberpunk")).unwrap();
        let shipped_id: Vec<_> = answer_as("kawai").into_iter().collect();
        let mut a = Answer::default();
        a.push(&shipped_id[0]);
        let files = a.push(&shipped_id[1]).unwrap().unwrap();
        assert!(install(&lib, &files).unwrap_err().to_string().contains("taken"), "a shipped id is taken too");
        // a file the loader refuses: nothing stays installed
        let broken: Vec<_> = answer_as("broken").into_iter().map(|m| m.replace("\"t\":\"palette\",", "\"t\":\"palette\",\"bogus\":1,")).collect();
        let mut a = Answer::default();
        a.push(&broken[0]);
        let files = a.push(&broken[1]).unwrap().unwrap();
        assert!(install(&lib, &files).is_err() && !user.join("broken").exists(), "a refused adventure is removed");
        assert!(install(&Library::shipped(&data_dir()), &files).unwrap_err().to_string().contains("no place"), "control: no player directory, no install");
        let _ = std::fs::remove_dir_all(&user);
    }

    #[test]
    fn answers_must_hold_every_file_once() {
        let msgs = answer_as("x").concat();
        assert!(parse(&msgs).is_ok(), "control");
        let no_texts = msgs.replace(&format!("{FILE_OPEN}texts.jsonl"), &format!("{FILE_OPEN}notes.jsonl"));
        assert!(parse(&no_texts).unwrap_err().to_string().contains("notes.jsonl is not an adventure file"));
        let cut = &msgs[..msgs.rfind(FILE_CLOSE).unwrap()];
        assert!(parse(cut).unwrap_err().to_string().contains("no === END"));
    }
}

/// One running request: the agent asked, its messages collected, its answer installed. A refused answer goes
/// back to the agent with the error, and the link keeps waiting for the fixed files.
pub struct Link {
    inbox: crate::peer::Inbox,
    to: crate::peer::Session,
    answer: Answer,
}

/// What a poll saw.
#[derive(Debug, PartialEq, Eq)]
pub enum Heard {
    Nothing,
    /// the answer was refused; the error went back to the agent
    Refused(String),
    /// installed under this id
    Installed(String),
}

/// The name the game signs its messages with.
const SENDER: &str = "tot";

impl Link {
    /// Ask `to` for the adventure `brief` describes (`with_skill`: send the skill along, see `request`).
    pub fn ask(lib: &Library, socks: &Path, to: crate::peer::Session, brief: &str, with_skill: bool) -> Result<Link> {
        let inbox = crate::peer::Inbox::open(socks)?;
        crate::peer::send(&to, &inbox, SENDER, &request(lib, brief, &inbox.addr(), with_skill)?)?;
        Ok(Link { inbox, to, answer: Answer::default() })
    }

    pub fn poll(&mut self, lib: &Library) -> Result<Heard> {
        for body in self.inbox.poll() {
            let Some(files) = self.answer.push(&body) else { continue };
            match files.and_then(|f| install(lib, &f)) {
                Ok(id) => return Ok(Heard::Installed(id)),
                Err(e) => {
                    let why = e.to_string();
                    crate::peer::send(&self.to, &self.inbox, SENDER, &format!("The game refused the adventure: {why}\nFix it and send every file again, then `{DONE}`."))?;
                    return Ok(Heard::Refused(why));
                }
            }
        }
        Ok(Heard::Nothing)
    }
}
