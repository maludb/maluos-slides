//! Memory across sessions (step 4.6): what the assistant remembers about one deck, stored in
//! the deck itself and nowhere else.
//!
//! The notes live in one presentation-level add-in tag (`p:custDataLst` → `ppt/tags/tagN.xml`
//! in PPTX, `tags` in `presentation.json` in the native package), named [`TAG`], as a small JSON
//! document. Both formats round-trip tags, so the memory travels with the file, survives a
//! save as PPTX and comes back when the deck is opened in another session or on another desk.
//! Reading and writing go through the engine command `document.tags`, so a remembered note is
//! an undoable change like any other and marks the deck dirty until saved.
//!
//! What belongs here: lasting facts about this deck (the audience, the tone, the time limit,
//! what must not change, decisions taken). What never does: secrets, keys, anything about other
//! decks or the user in general.

use maluos_slides_mcp::Backend;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The tag that holds the memory.
pub const TAG: &str = "maluos.assistant.memory";
/// Most notes kept; the oldest goes when a new one arrives past this.
pub const MAX_NOTES: usize = 24;
/// Characters of one note; longer notes are cut with `…`.
pub const MAX_NOTE_CHARS: usize = 240;
/// The JSON's `v`, for a later shape change.
pub const VERSION: u32 = 1;

/// One remembered note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub text: String,
    /// The day it was written, `YYYY-MM-DD` (UTC).
    #[serde(default)]
    pub when: String,
}

/// The notes of one deck.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeckMemory {
    #[serde(default)]
    pub notes: Vec<Note>,
}

impl DeckMemory {
    /// From the presentation's tags: the memory tag parsed, or empty when absent or unreadable.
    pub fn from_tags(tags: &[(String, String)]) -> DeckMemory {
        tags.iter().find(|(n, _)| n == TAG).map(|(_, v)| Self::from_tag_value(v)).unwrap_or_default()
    }

    /// From the tag's value.
    pub fn from_tag_value(value: &str) -> DeckMemory {
        serde_json::from_str::<Value>(value)
            .ok()
            .and_then(|v| serde_json::from_value::<DeckMemory>(v.get("memory").cloned().unwrap_or(v)).ok())
            .unwrap_or_default()
    }

    /// The tag's value.
    pub fn to_tag_value(&self) -> String {
        json!({"v": VERSION, "memory": self}).to_string()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Add a note (trimmed, cut at [`MAX_NOTE_CHARS`]); a note already there (case-insensitive)
    /// is not added twice. Past [`MAX_NOTES`] the oldest goes. Returns the note's index.
    pub fn remember(&mut self, text: &str, when: &str) -> Result<usize, String> {
        let text = cut(text.split_whitespace().collect::<Vec<_>>().join(" ").as_str(), MAX_NOTE_CHARS);
        if text.is_empty() {
            return Err("remember: the note is empty".into());
        }
        if let Some(i) = self.notes.iter().position(|n| n.text.eq_ignore_ascii_case(&text)) {
            return Ok(i);
        }
        self.notes.push(Note { text, when: when.into() });
        while self.notes.len() > MAX_NOTES {
            self.notes.remove(0);
        }
        Ok(self.notes.len() - 1)
    }

    /// Drop the note at `index`.
    pub fn forget(&mut self, index: usize) -> Option<Note> {
        (index < self.notes.len()).then(|| self.notes.remove(index))
    }

    /// The notes as the prompt shows them: one line each, numbered from 0 for `forget`.
    pub fn lines(&self) -> String {
        let mut s = String::new();
        for (i, n) in self.notes.iter().enumerate() {
            s.push_str(&format!("- [{i}] {}{}\n", n.text, if n.when.is_empty() { String::new() } else { format!(" ({})", n.when) }));
        }
        s
    }

    /// The `document.tags` params that store this memory (or remove the tag when empty).
    pub fn tag_params(&self) -> Value {
        if self.is_empty() { json!({"remove": [TAG]}) } else { json!({"set": {TAG: self.to_tag_value()}}) }
    }
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max - 1).collect::<String>() + "…" }
}

/// Today as `YYYY-MM-DD` (UTC), without a calendar crate.
pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86_400) as i64;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// The memory of the backend's presentation, read through `document.tags`.
pub fn read(backend: &mut dyn Backend) -> Result<DeckMemory, String> {
    let v = backend.call("engine.execute", json!({"command": "document.tags", "params": {}}))?;
    Ok(v.pointer(&format!("/tags/{TAG}")).and_then(Value::as_str).map(DeckMemory::from_tag_value).unwrap_or_default())
}

/// Store `memory` through `document.tags` (one undoable change).
pub fn write(backend: &mut dyn Backend, memory: &DeckMemory) -> Result<(), String> {
    backend.call("engine.execute", json!({"command": "document.tags", "params": memory.tag_params()})).map(|_| ())
}

#[cfg(test)]
mod unit {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn notes_round_trip_dedupe_and_cap() {
        let mut m = DeckMemory::default();
        assert_eq!(m.remember("  The audience is   the board. ", "2026-11-03"), Ok(0));
        assert_eq!(m.remember("the audience is the board.", "2026-11-04"), Ok(0), "same note, once");
        assert_eq!(m.remember("Keep it under ten minutes", "2026-11-03"), Ok(1));
        assert!(m.remember("   ", "x").is_err());
        let back = DeckMemory::from_tag_value(&m.to_tag_value());
        assert_eq!(back, m);
        assert_eq!(back.lines(), "- [0] The audience is the board. (2026-11-03)\n- [1] Keep it under ten minutes (2026-11-03)\n");
        assert_eq!(DeckMemory::from_tags(&[("other".into(), "x".into()), (TAG.into(), m.to_tag_value())]), m);
        assert!(DeckMemory::from_tag_value("not json").is_empty());
        assert!(DeckMemory::from_tags(&[]).is_empty());
        assert_eq!(m.forget(0).map(|n| n.text), Some("The audience is the board.".into()));
        assert_eq!(m.forget(5), None);
        assert_eq!(m.notes.len(), 1);
        for i in 0..40 {
            m.remember(&format!("note {i}"), "d").unwrap();
        }
        assert_eq!(m.notes.len(), MAX_NOTES);
        assert_eq!(m.notes[0].text, format!("note {}", 40 - MAX_NOTES));
        let long = "x".repeat(400);
        let i = m.remember(&long, "d").unwrap();
        assert_eq!(m.notes[i].text.chars().count(), MAX_NOTE_CHARS);
        assert!(m.notes[i].text.ends_with('…'));
        assert_eq!(DeckMemory::default().tag_params(), json!({"remove": [TAG]}));
        assert!(m.tag_params()["set"][TAG].is_string());
    }

    #[test]
    fn today_is_a_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"), "{t}");
        assert_eq!(&t[4..5], "-");
    }
}
