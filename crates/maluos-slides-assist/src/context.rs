//! The deck as the model sees it: an outline of the open presentation for the prompt.
//!
//! The outline is text, not JSON: sections, every slide's title, its placeholder text with
//! bullet levels, the other shapes in one line each, notes, the theme, and the selection in full
//! (the selected shapes with ids, boxes and text, the text range being edited). It is cut to a
//! byte budget with markers that say what was left out, slide by slide, so a 200-slide deck
//! still shows its shape and the current slide stays whole. [`ContextCache`] keeps the last
//! outline and rebuilds it only when the document revision or the selection changes.
//!
//! Reading the engine state is fine; only edits must go through commands.

use std::fmt::Write as _;

use maluos_slides_engine::cmd::inspect::shape_summary;
use maluos_slides_engine::model::resolve::Ctx;
use maluos_slides_engine::model::{Presentation, Shape, ShapeKind, Slide};
use maluos_slides_engine::{DocState, Selection, Session, Target};
use serde_json::Value;

/// How much of the deck goes into the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Bytes for the whole outline (the selection and the current slide are written first and
    /// never cut below [`Budget::slide_bytes`]).
    pub total_bytes: usize,
    /// Bytes per slide; a slide past it keeps its title and gets a marker.
    pub slide_bytes: usize,
    /// Characters of one shape's text; longer text is cut with `…`.
    pub text_chars: usize,
    /// Characters of one shape's text on the current slide (the slide being edited is shown
    /// whole as far as this allows).
    pub current_text_chars: usize,
    /// Characters of a slide's notes.
    pub notes_chars: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { total_bytes: 24 * 1024, slide_bytes: 1200, text_chars: 400, current_text_chars: 4000, notes_chars: 300 }
    }
}

/// The outline and what it was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub text: String,
    /// Document uid and revision the outline reflects.
    pub uid: u64,
    pub revision: u64,
    /// Slides whose body was cut or left out; their count tells the model to inspect.
    pub cut_slides: Vec<usize>,
}

impl Context {
    /// Builds the outline of the session's active document, or a one-line note without one.
    pub fn build(session: &Session, budget: &Budget) -> Context {
        match session.active() {
            Some(st) => Self::of(st, budget),
            None => Context {
                text: "No presentation is open. new_presentation or open_presentation starts one.\n".into(),
                uid: 0,
                revision: 0,
                cut_slides: vec![],
            },
        }
    }

    pub fn of(st: &DocState, budget: &Budget) -> Context {
        let mut w = Writer { out: String::new(), budget: *budget, cut_slides: vec![] };
        w.document(st);
        Context { text: w.out, uid: st.uid, revision: st.revision, cut_slides: w.cut_slides }
    }
}

/// Keeps the last [`Context`] and rebuilds only when the document or the selection changed.
#[derive(Debug, Default)]
pub struct ContextCache {
    last: Option<(u64, u64, String, Context)>,
    pub builds: usize,
}

impl ContextCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The outline for the session's current state.
    pub fn get(&mut self, session: &Session, budget: &Budget) -> &Context {
        let key = match session.active() {
            Some(st) => (st.uid, st.revision, selection_key(&st.selection)),
            None => (0, 0, String::new()),
        };
        let fresh = match &self.last {
            Some((u, r, s, _)) => !(*u == key.0 && *r == key.1 && *s == key.2),
            None => true,
        };
        if fresh {
            self.builds += 1;
            let ctx = Context::build(session, budget);
            self.last = Some((key.0, key.1, key.2, ctx));
        }
        match &self.last {
            Some((_, _, _, c)) => c,
            // `last` was just set above; this arm cannot be reached.
            None => EMPTY,
        }
    }
}

static EMPTY: &Context = &Context { text: String::new(), uid: 0, revision: 0, cut_slides: Vec::new() };

fn selection_key(sel: &Selection) -> String {
    serde_json::to_string(sel).unwrap_or_default()
}

struct Writer {
    out: String,
    budget: Budget,
    cut_slides: Vec<usize>,
}

impl Writer {
    fn document(&mut self, st: &DocState) {
        let d = &st.doc;
        let current = st.selection.slide;
        let _ = writeln!(self.out, "# {}", title_line(st));
        let _ = writeln!(
            self.out,
            "{} slides, {}×{} pt, theme {}{}{}",
            d.slides.len(),
            d.slide_size.width,
            d.slide_size.height,
            theme_line(d),
            if st.is_dirty() { ", unsaved changes" } else { "" },
            if d.sections.is_empty() { String::new() } else { format!(", {} sections", d.sections.len()) }
        );
        self.selection(st);
        let memory = crate::memory::DeckMemory::from_tags(&d.tags);
        if !memory.is_empty() {
            let _ = writeln!(self.out, "Remembered about this deck (kept in the file; remember adds, forget drops by index):");
            self.out.push_str(&memory.lines());
        }
        let _ = writeln!(self.out);

        // The current slide first and whole, so it is never the one that gets cut.
        if let Some(sl) = d.slides.get(current) {
            let _ = writeln!(self.out, "## Current slide");
            self.slide(d, current, sl, usize::MAX, self.budget.current_text_chars);
            let _ = writeln!(self.out);
        }

        let _ = writeln!(self.out, "## Outline");
        let section_of = section_starts(d);
        let total = self.budget.total_bytes;
        let mut left_out = 0usize;
        for (i, sl) in d.slides.iter().enumerate() {
            if let Some(name) = section_of.iter().find(|(id, _)| *id == sl.id).map(|(_, n)| n.as_str()) {
                let _ = writeln!(self.out, "### Section: {name}");
            }
            if i == current {
                let _ = writeln!(self.out, "{}. {} (current, above)", i + 1, title_or(sl, d));
                continue;
            }
            // Past the total budget only the title line of each remaining slide is written; past
            // the budget plus a margin even those stop, with one count at the end.
            if self.out.len() > total + 2048 {
                left_out += 1;
                self.cut_slides.push(i);
                continue;
            }
            if self.out.len() > total {
                let _ = writeln!(self.out, "{}. {} [body left out: inspect_slide {i}]", i + 1, title_or(sl, d));
                self.cut_slides.push(i);
                continue;
            }
            self.slide(d, i, sl, self.budget.slide_bytes, self.budget.text_chars);
        }
        if left_out > 0 {
            let _ = writeln!(self.out, "… {left_out} more slides (inspect_document lists them)");
        }
    }

    fn selection(&mut self, st: &DocState) {
        let sel = &st.selection;
        match sel.target {
            Target::Slides => {}
            _ => {
                let _ = writeln!(self.out, "Editing a master or layout, not a slide.");
            }
        }
        let _ = write!(self.out, "Current slide: {}", sel.slide + 1);
        if sel.slides.len() > 1 {
            let _ = write!(self.out, "; {} slides selected", sel.slides.len());
        }
        if let Some(t) = &sel.text {
            if t.notes {
                let _ = write!(self.out, "; editing the notes");
            } else {
                let _ = write!(self.out, "; editing text in shape {}", t.shape);
                if let Some((r, c)) = t.cell {
                    let _ = write!(self.out, " cell ({r},{c})");
                }
                let (a, b) = (t.anchor.min(t.caret), t.anchor.max(t.caret));
                if a == b {
                    let _ = write!(self.out, ", caret at paragraph {} char {}", a.0, a.1);
                } else {
                    let _ = write!(self.out, ", selected from paragraph {} char {} to paragraph {} char {}", a.0, a.1, b.0, b.1);
                    if let Some(sh) = st.shape(t.shape)
                        && let Some(body) = text_of(sh, t.cell)
                    {
                        let s = selected_text(body, a, b);
                        if !s.is_empty() {
                            let _ = write!(self.out, ": {:?}", cut_chars(&s, self.budget.text_chars));
                        }
                    }
                }
            }
        } else if let Some((id, (r0, c0), (r1, c1))) = sel.cells {
            let _ = write!(self.out, "; table {id} cells ({r0},{c0})–({r1},{c1}) selected");
        }
        let _ = writeln!(self.out);
        if sel.shapes.is_empty() {
            let _ = writeln!(self.out, "Nothing selected on it.");
            return;
        }
        let _ = writeln!(self.out, "Selected shapes ({}), the target of an edit:", sel.shapes.len());
        let ctx = st.doc.slides.get(sel.slide).and_then(|sl| Ctx::for_slide(&st.doc, sl));
        for id in &sel.shapes {
            match st.shape(*id) {
                Some(sh) => {
                    let line = shape_line(&shape_summary(ctx.as_ref(), sh, 0), self.budget.text_chars);
                    let _ = writeln!(self.out, "- {line}");
                }
                None => {
                    let _ = writeln!(self.out, "- shape {id} (not on this slide)");
                }
            }
        }
    }

    fn slide(&mut self, d: &Presentation, i: usize, sl: &Slide, max_bytes: usize, text_chars: usize) {
        let start = self.out.len();
        let layout = d.layout(sl.layout).map(|(_, l)| l.name.clone()).unwrap_or_default();
        let _ = write!(self.out, "{}. {}", i + 1, title_or(sl, d));
        let mut flags: Vec<String> = vec![];
        if !layout.is_empty() {
            flags.push(format!("layout {layout}"));
        }
        flags.push(format!("id {}", sl.id));
        if sl.hidden {
            flags.push("hidden".into());
        }
        if !sl.animations.is_empty() {
            flags.push(format!("{} animations", sl.animations.len()));
        }
        if let Some(t) = &sl.transition {
            flags.push(format!("transition {}", t.kind));
        }
        let _ = writeln!(self.out, " ({})", flags.join(", "));
        let ctx = Ctx::for_slide(d, sl);
        let mut cut = false;
        for sh in &sl.shapes {
            if self.out.len() - start > max_bytes {
                cut = true;
                break;
            }
            if sh.ph.as_ref().is_some_and(|p| p.kind.is_title()) && sh.text.as_ref().is_some_and(|t| !t.is_empty()) {
                continue; // the title line has it
            }
            self.shape(ctx.as_ref(), sh, text_chars);
        }
        if cut {
            let _ = writeln!(self.out, "   … more shapes [cut: inspect_slide {i}]");
            self.cut_slides.push(i);
        }
        let notes = sl.notes_text();
        if !notes.trim().is_empty() {
            let _ = writeln!(self.out, "   notes: {:?}", cut_chars(notes.trim(), self.budget.notes_chars));
        }
    }

    /// Placeholder text as indented lines (the model reads and edits it); any other shape as
    /// one summary line.
    fn shape(&mut self, ctx: Option<&Ctx>, sh: &Shape, text_chars: usize) {
        if sh.hidden {
            return;
        }
        let is_text_ph = sh.ph.is_some() && matches!(sh.kind, ShapeKind::Shape) && sh.text.as_ref().is_some_and(|t| !t.is_empty());
        if is_text_ph {
            let kind = sh.ph.as_ref().map(|p| p.kind.xml()).unwrap_or("body");
            let _ = writeln!(self.out, "   [{kind} id {}]", sh.id);
            let Some(body) = &sh.text else { return };
            let mut used = 0usize;
            for p in &body.paragraphs {
                let t = p.text().replace('\u{b}', " ");
                if used >= text_chars {
                    let _ = writeln!(self.out, "   …");
                    break;
                }
                let shown = cut_chars(&t, text_chars - used);
                used += shown.chars().count();
                let _ = writeln!(
                    self.out,
                    "   {}{}{}",
                    "  ".repeat(p.level as usize),
                    if p.level > 0 || body.paragraphs.len() > 1 { "- " } else { "" },
                    shown
                );
            }
            return;
        }
        let line = shape_line(&shape_summary(ctx, sh, 0), text_chars);
        let _ = writeln!(self.out, "   - {line}");
    }
}

fn title_line(st: &DocState) -> String {
    let t = st.title();
    match &st.path {
        Some(p) if !p.is_empty() => format!("{t} ({p})"),
        _ => t,
    }
}

fn theme_line(d: &Presentation) -> String {
    let Some(m) = d.masters.first() else { return "none".into() };
    let th = &m.theme;
    let c = &th.colors.colors;
    let hex = |i: usize| c.get(i).map(|c| format!("#{}", c.hex())).unwrap_or_default();
    format!(
        "{} (fonts {}/{}; dark {}, light {}, accents {} {} {} {} {} {})",
        th.name,
        th.fonts.major.latin,
        th.fonts.minor.latin,
        hex(0),
        hex(1),
        hex(4),
        hex(5),
        hex(6),
        hex(7),
        hex(8),
        hex(9)
    )
}

fn title_or(sl: &Slide, d: &Presentation) -> String {
    let t = sl.title();
    if !t.trim().is_empty() {
        return cut_chars(t.trim(), 120);
    }
    if !sl.name.trim().is_empty() {
        return format!("(untitled: {})", sl.name.trim());
    }
    let layout = d.layout(sl.layout).map(|(_, l)| l.name.clone()).unwrap_or_default();
    if layout.is_empty() { "(untitled)".into() } else { format!("(untitled {layout} slide)") }
}

/// Section names keyed by their first slide id.
fn section_starts(d: &Presentation) -> Vec<(maluos_slides_engine::model::SlideId, String)> {
    d.sections.iter().filter_map(|s| s.slides.first().map(|id| (*id, s.name.clone()))).collect()
}

/// One line from `shape_summary`'s JSON: `rect id 12 "Goal" at [600,300,200,100] fill #ff0000`.
fn shape_line(v: &Value, text_chars: usize) -> String {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");
    let mut line = String::new();
    let kind = s("kind");
    let preset = s("preset");
    let ph = s("placeholder");
    let _ = write!(
        line,
        "{}",
        if !ph.is_empty() {
            format!("{ph} placeholder")
        } else if !preset.is_empty() {
            preset.to_string()
        } else {
            kind.to_string()
        }
    );
    if let Some(id) = v.get("id") {
        let _ = write!(line, " id {id}");
    }
    let name = s("name");
    if !name.is_empty() {
        let _ = write!(line, " \"{name}\"");
    }
    if let Some(b) = v.get("box").and_then(Value::as_array) {
        let nums: Vec<String> = b.iter().map(|n| n.as_f64().map(|f| format!("{}", f.round() as i64)).unwrap_or_default()).collect();
        let _ = write!(line, " at [{}]", nums.join(","));
    }
    if let Some(r) = v.get("rotation").and_then(Value::as_f64).filter(|r| *r != 0.0) {
        let _ = write!(line, " rot {r}");
    }
    let fill = s("fill");
    if !fill.is_empty() && fill != "none" {
        let _ = write!(line, " fill {fill}");
    }
    if let Some(t) = v.get("table") {
        let _ = write!(line, " table {}×{}", t["rows"], t["cols"]);
        if let Some(rows) = t.get("cells").and_then(Value::as_array) {
            let first: Vec<String> = rows
                .iter()
                .take(3)
                .map(|r| r.as_array().map(|c| c.iter().map(|x| x.as_str().unwrap_or("")).collect::<Vec<_>>().join(" | ")).unwrap_or_default())
                .collect();
            let _ = write!(line, " rows: {}{}", first.join(" / "), if rows.len() > 3 { " / …" } else { "" });
        }
    }
    if let Some(c) = v.get("chart") {
        let series = c.get("series").and_then(Value::as_array).map(|s| s.len()).unwrap_or(0);
        let cats = c.get("categories").and_then(Value::as_array).map(|s| s.len()).unwrap_or(0);
        let _ = write!(
            line,
            " chart {} \"{}\" ({series} series, {cats} categories)",
            c["type"].as_str().unwrap_or(""),
            c["title"].as_str().unwrap_or("")
        );
    }
    if let Some(ch) = v.get("children").and_then(Value::as_array) {
        let _ = write!(line, " group of {}", ch.len());
    }
    let text = s("text");
    if !text.is_empty() {
        let _ = write!(line, " text {:?}", cut_chars(&text.replace('\u{b}', " "), text_chars));
    }
    let alt = s("altText");
    if !alt.is_empty() && text.is_empty() {
        let _ = write!(line, " alt {:?}", cut_chars(alt, 80));
    }
    line
}

fn text_of(sh: &Shape, cell: Option<(usize, usize)>) -> Option<&maluos_slides_engine::model::TextBody> {
    match (cell, &sh.kind) {
        (Some((r, c)), ShapeKind::Table(t)) => t.rows.get(r).and_then(|row| row.cells.get(c)).map(|cell| &cell.text),
        _ => sh.text.as_ref(),
    }
}

/// The text between two (paragraph, char) positions.
fn selected_text(body: &maluos_slides_engine::model::TextBody, a: (usize, usize), b: (usize, usize)) -> String {
    let mut out = String::new();
    for (pi, p) in body.paragraphs.iter().enumerate() {
        if pi < a.0 || pi > b.0 {
            continue;
        }
        let t = p.text();
        let start = if pi == a.0 { a.1 } else { 0 };
        let end = if pi == b.0 { b.1 } else { t.chars().count() };
        if pi > a.0 {
            out.push('\n');
        }
        out.extend(t.chars().skip(start).take(end.saturating_sub(start)));
    }
    out
}

/// At most `max` chars, with `…` when cut.
fn cut_chars(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
