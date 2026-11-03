//! A deck from a document: Markdown (or a JSON list of slides) → an outline → slides, sections,
//! notes and tables, built with engine commands through a [`Backend`] (step 4.1).
//!
//! Markdown rules: `# Title` is the deck title and the paragraph under it the subtitle;
//! `## Heading` starts a section (and makes its section-header slide); `### Heading` is a slide;
//! bullets (`-`, `*`, `1.`) with two spaces or a tab per level are the body; plain paragraphs are
//! body text too; a `Notes:` line or a `> Notes:` quote is the speaker notes; a pipe table becomes
//! a table on the slide. A document with `##` headings but no `###` uses the `##` as slides and
//! has no sections. Layouts: `title` for the deck title, `sectionHeader` for sections,
//! `titleAndContent` for a slide with body, `titleOnly` for one with a table or nothing.

use maluos_slides_mcp::Backend;
use serde_json::{Value, json};

/// The eight built-in themes (`design.themes`).
pub const THEMES: &[&str] = &["MaluOS Slides", "Harbor", "Ember", "Meadow", "Nocturne", "Paper", "Coral Reef", "Slate"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outline {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Section {
    /// `None` for slides before any `##`, or for a document without sections.
    pub name: Option<String>,
    pub slides: Vec<SlideSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlideSpec {
    /// `slide.new`'s layout name or kind; empty picks one from the content.
    pub layout: String,
    pub title: String,
    /// Body paragraphs, leading tabs for bullet levels.
    pub body: Vec<String>,
    pub notes: Option<String>,
    /// Rows of cells; the first row is the header.
    pub table: Option<Vec<Vec<String>>>,
}

impl SlideSpec {
    fn layout(&self) -> &str {
        if !self.layout.is_empty() {
            return &self.layout;
        }
        if self.table.is_some() || self.body.is_empty() { "titleOnly" } else { "titleAndContent" }
    }
}

impl Outline {
    pub fn slide_count(&self) -> usize {
        let title = usize::from(self.title.is_some());
        title + self.sections.iter().map(|s| s.slides.len() + usize::from(s.name.is_some())).sum::<usize>()
    }

    /// The outline of a Markdown document.
    pub fn from_markdown(text: &str) -> Outline {
        let has_h3 = text.lines().any(|l| l.trim_start().starts_with("### "));
        let mut out = Outline::default();
        let mut section = Section::default();
        let mut slide: Option<SlideSpec> = None;
        let mut after_title = false;
        let mut in_fence = false;
        let flush_slide = |section: &mut Section, slide: &mut Option<SlideSpec>| {
            if let Some(s) = slide.take() {
                section.slides.push(s);
            }
        };
        for raw in text.lines() {
            let line = raw.trim_end();
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                if let Some(s) = slide.as_mut() {
                    s.body.push(format!("\t{}", trimmed));
                }
                continue;
            }
            if trimmed.is_empty() {
                after_title = false;
                continue;
            }
            if let Some(t) = trimmed.strip_prefix("# ") {
                if out.title.is_none() {
                    out.title = Some(t.trim().to_string());
                    after_title = true;
                } else {
                    // A second H1 is a section like `##`.
                    flush_slide(&mut section, &mut slide);
                    if section.name.is_some() || !section.slides.is_empty() {
                        out.sections.push(std::mem::take(&mut section));
                    }
                    section.name = Some(t.trim().to_string());
                }
                continue;
            }
            if let Some(t) = trimmed.strip_prefix("## ") {
                flush_slide(&mut section, &mut slide);
                after_title = false;
                if has_h3 {
                    if section.name.is_some() || !section.slides.is_empty() {
                        out.sections.push(std::mem::take(&mut section));
                    }
                    section.name = Some(t.trim().to_string());
                } else {
                    slide = Some(SlideSpec { title: t.trim().to_string(), ..SlideSpec::default() });
                }
                continue;
            }
            if let Some(t) = trimmed.strip_prefix("### ") {
                flush_slide(&mut section, &mut slide);
                after_title = false;
                slide = Some(SlideSpec { title: t.trim().to_string(), ..SlideSpec::default() });
                continue;
            }
            if after_title && slide.is_none() && out.subtitle.is_none() {
                out.subtitle = Some(trimmed.to_string());
                continue;
            }
            let Some(s) = slide.as_mut() else {
                // Text before the first slide and after the subtitle: a slide of its own.
                slide = Some(SlideSpec { title: out.title.clone().unwrap_or_else(|| "Overview".into()), ..SlideSpec::default() });
                if let Some(s) = slide.as_mut() {
                    push_line(s, line, trimmed);
                }
                continue;
            };
            push_line(s, line, trimmed);
        }
        flush_slide(&mut section, &mut slide);
        if section.name.is_some() || !section.slides.is_empty() {
            out.sections.push(section);
        }
        out
    }

    /// The outline of the tool's JSON form: `{title?, subtitle?, slides: [{title, body?: text |
    /// [lines], notes?, layout?, section?, table?: [[cells]]}]}`; a slide's `section` starts a
    /// section with that name.
    pub fn from_json(v: &Value) -> Result<Outline, String> {
        let mut out = Outline {
            title: v.get("title").and_then(Value::as_str).map(String::from),
            subtitle: v.get("subtitle").and_then(Value::as_str).map(String::from),
            sections: vec![],
        };
        let slides = v.get("slides").and_then(Value::as_array).ok_or("build_deck needs `markdown` or `slides: [...]`")?;
        let mut section = Section::default();
        for (i, s) in slides.iter().enumerate() {
            let title = s.get("title").and_then(Value::as_str).ok_or_else(|| format!("slides[{i}] needs a `title`"))?;
            if let Some(name) = s.get("section").and_then(Value::as_str) {
                if section.name.is_some() || !section.slides.is_empty() {
                    out.sections.push(std::mem::take(&mut section));
                }
                section.name = Some(name.to_string());
            }
            let body: Vec<String> = match s.get("body").or_else(|| s.get("bullets")) {
                Some(Value::String(t)) => t.lines().map(String::from).collect(),
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(String::from).collect(),
                Some(_) => return Err(format!("slides[{i}].body must be text or a list of lines")),
                None => vec![],
            };
            let table = match s.get("table") {
                Some(Value::Array(rows)) => {
                    Some(rows.iter().map(|r| r.as_array().map(|c| c.iter().map(cell_text).collect()).unwrap_or_default()).collect())
                }
                Some(_) => return Err(format!("slides[{i}].table must be rows of cells")),
                None => None,
            };
            section.slides.push(SlideSpec {
                layout: s.get("layout").and_then(Value::as_str).unwrap_or("").to_string(),
                title: title.to_string(),
                body,
                notes: s.get("notes").and_then(Value::as_str).map(String::from),
                table,
            });
        }
        if section.name.is_some() || !section.slides.is_empty() {
            out.sections.push(section);
        }
        Ok(out)
    }
}

fn cell_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn push_line(s: &mut SlideSpec, line: &str, trimmed: &str) {
    if let Some(n) = trimmed
        .strip_prefix("Notes:")
        .or_else(|| trimmed.strip_prefix("> Notes:"))
        .or_else(|| trimmed.strip_prefix("> notes:"))
        .or_else(|| trimmed.strip_prefix("notes:"))
    {
        let n = n.trim();
        s.notes = Some(match s.notes.take() {
            Some(prev) if !n.is_empty() => format!("{prev}\n{n}"),
            Some(prev) => prev,
            None => n.to_string(),
        });
        return;
    }
    if trimmed.starts_with('|') {
        let cells: Vec<String> = trimmed.trim_matches('|').split('|').map(|c| c.trim().to_string()).collect();
        if cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')) {
            return; // the header rule
        }
        s.table.get_or_insert_with(Vec::new).push(cells);
        return;
    }
    let indent = line.len() - trimmed.len();
    let tabs = line[..indent].chars().filter(|c| *c == '\t').count();
    let spaces = line[..indent].chars().filter(|c| *c == ' ').count();
    let level = tabs + spaces / 2;
    let text = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
        .or_else(|| {
            let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
            if digits > 0 { trimmed[digits..].strip_prefix(". ").or_else(|| trimmed[digits..].strip_prefix(") ")) } else { None }
        })
        .unwrap_or(trimmed);
    let text = strip_emphasis(text);
    s.body.push(format!("{}{}", "\t".repeat(level), text));
}

fn strip_emphasis(t: &str) -> String {
    t.replace("**", "").replace("__", "")
}

/// What `apply` built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Indexes of the slides added, in order.
    pub slides: Vec<usize>,
    pub sections: usize,
    pub tables: usize,
    pub notes: usize,
}

/// Builds the outline into the backend's presentation with engine commands. `replace` starts a
/// new presentation (with `theme`); otherwise the slides go after the last slide (or after
/// `after`) and `theme`, when given, is applied to the whole deck.
pub fn apply(b: &mut dyn Backend, outline: &Outline, theme: Option<&str>, replace: bool, after: Option<usize>) -> Result<Applied, String> {
    if let Some(t) = theme
        && !THEMES.iter().any(|x| x.eq_ignore_ascii_case(t))
    {
        return Err(format!("unknown theme `{t}` (one of: {})", THEMES.join(", ")));
    }
    let theme = theme.and_then(|t| THEMES.iter().find(|x| x.eq_ignore_ascii_case(t)).copied());
    let exec = |b: &mut dyn Backend, id: &str, params: Value| -> Result<Value, String> {
        b.call("engine.execute", json!({"command": id, "params": params}))
    };
    let mut applied = Applied::default();
    if replace {
        let mut p = json!({"blank": true});
        if let Some(t) = theme {
            p["theme"] = json!(t);
        }
        exec(b, "file.new", p)?;
    } else {
        let doc = b.call("document.inspect", json!({}))?;
        let n = doc.get("slides").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        if n > 0 {
            let at = after.unwrap_or(n - 1).min(n - 1);
            exec(b, "slide.go", json!({"index": at}))?;
        }
        if let Some(t) = theme {
            exec(b, "design.theme", json!({"name": t}))?;
        }
    }
    let add = |b: &mut dyn Backend, layout: &str, title: &str, body: &[String]| -> Result<usize, String> {
        let mut p = json!({"layout": layout, "title": title});
        if !body.is_empty() {
            p["body"] = json!(body.join("\n"));
        }
        let r = exec(b, "slide.new", p)?;
        r.get("index").and_then(Value::as_u64).map(|i| i as usize).ok_or_else(|| "slide.new returned no index".to_string())
    };
    if let Some(t) = &outline.title {
        let body: Vec<String> = outline.subtitle.iter().cloned().collect();
        let i = add(b, "title", t, &body)?;
        applied.slides.push(i);
    }
    for section in &outline.sections {
        if let Some(name) = &section.name {
            let i = add(b, "sectionHeader", name, &[])?;
            applied.slides.push(i);
            exec(b, "section.add", json!({"name": name, "at": i}))?;
            applied.sections += 1;
        }
        for s in &section.slides {
            let i = add(b, s.layout(), &s.title, &s.body)?;
            applied.slides.push(i);
            if let Some(rows) = &s.table
                && let Some(first) = rows.first()
            {
                let cols = first.len().max(1);
                let data: Vec<Vec<String>> = rows.iter().map(|r| (0..cols).map(|c| r.get(c).cloned().unwrap_or_default()).collect()).collect();
                exec(
                    b,
                    "insert.table",
                    json!({"rows": rows.len(), "cols": cols, "data": data, "rect": [60, 120, 840, (28 * rows.len() + 20).min(380)]}),
                )?;
                applied.tables += 1;
            }
            if let Some(n) = &s.notes {
                exec(b, "slide.notes", json!({"text": n}))?;
                applied.notes += 1;
            }
        }
    }
    Ok(applied)
}
