//! How long a deck takes to present (step 4.4): a spoken-pace estimate per slide from its notes
//! and its text, and a cue line for presenter view.

use maluos_slides_engine::model::{Presentation, Shape, ShapeKind};
use serde_json::{Value, json};

/// Words per minute for notes read aloud.
pub const NOTES_WPM: f64 = 130.0;
/// Words per minute for text on the slide: the presenter paraphrases, the audience reads.
pub const SLIDE_WPM: f64 = 60.0;
/// Seconds every slide costs: the transition, a breath, "next".
pub const BASE_SECONDS: f64 = 8.0;
/// No slide is quicker than this.
pub const MIN_SECONDS: f64 = 15.0;

#[derive(Debug, Clone, PartialEq)]
pub struct SlideTiming {
    pub index: usize,
    pub title: String,
    pub slide_words: usize,
    pub notes_words: usize,
    pub seconds: f64,
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeckTiming {
    pub slides: Vec<SlideTiming>,
    /// Seconds for the slides that are shown.
    pub total_seconds: f64,
}

impl DeckTiming {
    pub fn of(doc: &Presentation) -> DeckTiming {
        let slides: Vec<SlideTiming> = doc
            .slides
            .iter()
            .enumerate()
            .map(|(i, sl)| {
                let slide_words: usize = sl.shapes.iter().map(shape_words).sum();
                let notes_words = words(&sl.notes_text());
                SlideTiming { index: i, title: sl.title(), slide_words, notes_words, seconds: estimate(slide_words, notes_words), hidden: sl.hidden }
            })
            .collect();
        let total_seconds = slides.iter().filter(|s| !s.hidden).map(|s| s.seconds).sum();
        DeckTiming { slides, total_seconds }
    }

    pub fn slide(&self, index: usize) -> Option<&SlideTiming> {
        self.slides.get(index)
    }

    /// The longest shown slide.
    pub fn longest(&self) -> Option<&SlideTiming> {
        self.slides.iter().filter(|s| !s.hidden).max_by(|a, b| a.seconds.total_cmp(&b.seconds))
    }

    /// For the `deck_timing` tool.
    pub fn to_json(&self) -> Value {
        json!({
            "totalSeconds": self.total_seconds.round(),
            "total": clock(self.total_seconds),
            "slides": self.slides.iter().map(|s| json!({"index": s.index, "title": s.title, "slideWords": s.slide_words, "notesWords": s.notes_words, "seconds": s.seconds.round(), "hidden": s.hidden})).collect::<Vec<_>>(),
            "longest": self.longest().map(|s| json!({"index": s.index, "title": s.title, "seconds": s.seconds.round()})),
            "assumptions": {"notesWpm": NOTES_WPM, "slideWpm": SLIDE_WPM, "baseSeconds": BASE_SECONDS, "minSeconds": MIN_SECONDS},
        })
    }
}

/// Seconds for one slide.
pub fn estimate(slide_words: usize, notes_words: usize) -> f64 {
    let spoken = notes_words as f64 / NOTES_WPM * 60.0 + slide_words as f64 / SLIDE_WPM * 60.0;
    (BASE_SECONDS + spoken).max(MIN_SECONDS)
}

/// `m:ss`, or `h:mm:ss` past an hour.
pub fn clock(seconds: f64) -> String {
    let s = seconds.round().max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// The cue for presenter view: the first sentences of the notes, at most `max_words` words.
pub fn cue(notes: &str, max_words: usize) -> Option<String> {
    let text = notes.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut n = 0;
    for sentence in split_sentences(&text) {
        let w = words(sentence);
        if n > 0 && n + w > max_words {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(sentence);
        n += w;
        if n >= max_words {
            break;
        }
    }
    if n > max_words {
        let cut: Vec<&str> = out.split_whitespace().take(max_words).collect();
        out = format!("{}…", cut.join(" "));
    }
    Some(out)
}

fn split_sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for (i, c) in text.char_indices() {
        if matches!(c, '.' | '!' | '?') && bytes.get(i + 1).is_none_or(|b| *b == b' ') {
            let s = text[start..=i].trim();
            if !s.is_empty() {
                out.push(s);
            }
            start = i + 1;
        }
    }
    let rest = text[start..].trim();
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

pub fn words(text: &str) -> usize {
    text.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count()
}

fn shape_words(sh: &Shape) -> usize {
    let mut n = sh.text.as_ref().map(|t| words(&t.text())).unwrap_or(0);
    match &sh.kind {
        ShapeKind::Table(t) => n += t.rows.iter().map(|r| r.cells.iter().map(|c| words(&c.text.text())).sum::<usize>()).sum::<usize>(),
        ShapeKind::Group { children, .. } => n += children.iter().map(shape_words).sum::<usize>(),
        _ => {}
    }
    n
}
