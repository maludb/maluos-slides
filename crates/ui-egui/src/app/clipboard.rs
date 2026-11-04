use super::{SlideApp, Value, json};

/// Text put on the system clipboard when the copied objects (a rectangle, a picture, slides…)
/// carry no text. egui-winit drops Ctrl+V when the system clipboard is empty, so without it the
/// shortcut never reached the session and only the context menu could paste objects (#182). An
/// invisible word joiner, so a paste into another app adds nothing visible.
pub(crate) const OBJECTS_MARKER: &str = "\u{2060}";

/// What a copy/cut leaves on the system clipboard for the session's plain-text rendering `t`.
pub(crate) fn system_clipboard_text(t: &str) -> String {
    if t.is_empty() { OBJECTS_MARKER.to_string() } else { t.to_string() }
}

impl SlideApp {
    /// Offer the copy or cut on the system clipboard.
    ///
    /// Slides go out as plain text. Text goes out as HTML plus the same plain words, so a paste
    /// back into MaluOS Slides still uses the internal clipboard. A textless shape leaves the
    /// invisible [`OBJECTS_MARKER`] rather than a picture: with only an image on the clipboard
    /// egui-winit drops Ctrl+V, and the shortcut could no longer paste the shape here (#182).
    pub(super) fn publish_clipboard(&mut self, ctx: &egui::Context, cut: bool) {
        let cmd = if cut { "edit.cut" } else { "edit.copy" };
        let Ok(v) = self.session.execute(cmd, &json!({})) else { return };
        let text = v.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        let html = v.get("html").and_then(Value::as_str).unwrap_or("").to_string();
        if self.session.clipboard.slides.is_empty() && !html.is_empty() && self.services.clipboard_set_html.as_mut().is_some_and(|f| f(&html, &text))
        {
            return;
        }
        ctx.copy_text(system_clipboard_text(&text));
    }

    /// Paste what the system clipboard holds, as Ctrl+V does: `text` is its plain text (our own
    /// [`OBJECTS_MARKER`] means "what the session copied"); a picture (a screenshot, Keynote's
    /// TIFF) lands when there is no text; text keeps its HTML (bold, italic, underline) when the
    /// host can read it. `image_already_pasted` is set when egui delivered the picture itself
    /// (`PasteImage`), so it is never inserted twice.
    pub(crate) fn paste_system(&mut self, text: String, image_already_pasted: bool) -> Result<Value, String> {
        let text = if text == OBJECTS_MARKER { String::new() } else { text };
        let html = self.services.clipboard_html.as_mut().and_then(|f| f()).filter(|h| !h.is_empty());
        let img = if text.is_empty() && !image_already_pasted { self.services.clipboard_image.as_mut().and_then(|f| f()) } else { None };
        let r = match img {
            Some(png) => {
                self.session.execute("insert.picture", &json!({"name": "Pasted image.png", "data": maluos_slides_engine::cmd::base64_encode(&png)}))
            }
            None => {
                let mut params = json!({"text": text});
                if let Some(h) = html {
                    params["html"] = Value::String(h);
                }
                self.session.execute("edit.paste", &params)
            }
        };
        r.map_err(|e| e.to_string())
    }

    /// The ribbon's Paste, Edit › Paste, the context menu and `edit.paste {}` from the control
    /// channel or the assistant: the same paste as Ctrl+V, from the system clipboard when the
    /// host can read it. Without a clipboard reader (tests, the web), the session's own clipboard.
    pub(crate) fn paste_from_system(&mut self) -> Result<Value, String> {
        let text = self.services.clipboard_text.as_mut().and_then(|f| f()).unwrap_or_default();
        self.paste_system(text, false)
    }
}
