//! `tot skill`: the agent skill an AI author reads to write adventures for this engine: what the game is, the
//! exact answer format, the shown-content schema, and every shipped adventure in full as an example.
use crate::Result;
use std::path::Path;

/// Line that opens and closes one file in an answer.
pub const FILE_OPEN: &str = "=== FILE ";
pub const FILE_CLOSE: &str = "=== END";

const INTRO: &str = "---
name: tot-adventure
description: Write and keep updating an adventure for the tot terminal game engine: every word, colour, foe, skill, item and story beat the player sees.
---

# tot adventure author

tot is a terminal action roguelike (horde survival in a Diablo-like dungeon, drawn with text glyphs in an isometric view). The engine has no content of its own: everything the player sees comes from an adventure. An adventure is a set of JSON Lines files: classes and skill trees, foes and bosses, waves, items, tuning numbers, colours, and every message on screen. The game loads it, checks it, and refuses it with a precise error if one record is wrong.

## your job

1. The user names the kind of adventure they want (a theme, a mood, a setting). Make a NEW adventure of that kind: new names, new foes, new skills, new words, new colours. Use the shipped adventures below for the format and the balance, never copy their content.
2. Answer ONLY with files, in the exact format below. No prose before, between or after the files.
3. Keep sending updates while the run goes on: when the user or the game tells you what happened, answer again with the files you change (whole files, same format). The game swaps them in, so the adventure can turn: new foes, a new weather, a darker story, new words.

## answer format

Each file: a line `=== FILE <name>`, the file's JSON Lines, a line `=== END`. Names: profile.jsonl and the content files listed in the examples. The first line of every file is its head record, as in the examples. The profile id is [a-z0-9-], at most 40 chars, and it is also the adventure's directory name.

Rules the loader checks:
- every id a record refers to must exist (a tree's class, a node's skill, a wave's family);
- numbers stay inside their ranges (the examples show working values; the loader names the field and the range when one is out);
- every text, pool, art and bark id of the schema below must appear exactly once in texts.jsonl, and every `{slot}` a text lists must appear in it;
- shown text uses only glyphs the terminal draws: printable ASCII, box drawing and blocks (U+2500..U+25FF), a few symbols, half-width katakana, and a short list of wide emoji (the examples use them); the big block font takes only letters, ! ? . and space.

";

/// The skill text: intro, schema, then each installed adventure's files in the answer format.
pub fn skill(dir: &Path) -> Result<String> {
    skill_with(dir, |_| true)
}

/// The skill text with only the adventures `pick` keeps as examples.
pub fn skill_with(dir: &Path, pick: impl Fn(&str) -> bool) -> Result<String> {
    let mut s = String::from(INTRO);
    s += &crate::data::schema_doc();
    s += "\n## shipped adventures (examples: the format and the balance, not the content)\n";
    let advs: Vec<_> = crate::data::profiles(dir)?.into_iter().filter(|p| pick(&p.id)).collect();
    if advs.is_empty() {
        return Err(crate::Error::Usage(format!("no adventures under {}/profiles: the skill needs at least one example", dir.display())));
    }
    for p in advs {
        s += &format!("\n### {} ({})\n\n{}\n\n", p.name, p.id, p.blurb);
        for (name, text) in crate::data::adventure_files(dir, &p.id)? {
            s += &format!("{FILE_OPEN}{name}\n{}\n{FILE_CLOSE}\n", text.trim_end());
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_tells_the_format_and_carries_every_adventure_whole() {
        let d = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/data"));
        let s = skill(d).unwrap();
        let advs = crate::data::profiles(d).unwrap();
        assert!(advs.len() >= 2 && advs.iter().all(|p| s.contains(&format!("({})", p.id))), "every adventure");
        for p in &advs {
            // every file of every adventure, whole, in the answer format
            for (name, text) in crate::data::adventure_files(d, &p.id).unwrap() {
                assert!(s.contains(&format!("{FILE_OPEN}{name}\n{}\n{FILE_CLOSE}", text.trim_end())), "{} {name}", p.id);
            }
        }
        assert!(s.contains("Answer ONLY with files") && s.contains("Keep sending updates") && s.contains("NEW adventure"));
        assert!(s.contains("pick_adventure") && s.contains("base24"), "the schema is in it");
        assert!(s.contains("BONE ABBOT") && s.contains("SCRAPPER"), "each adventure with its own foes (hellish ships its own enemies)");
        assert!(skill(Path::new("/nonexistent-tot-data")).is_err(), "control: no data, no skill");
    }
}
