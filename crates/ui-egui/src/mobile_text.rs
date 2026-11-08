//! On-screen keyboards (phones and tablets): the browser shows one only where the app says text
//! is being entered. While a cell draft is open (typed into the cell or the formula bar), say so
//! every frame, at the cell, so the keyboard comes up and the page keeps the cell in view.

use egui::{Id, Rect, pos2, vec2};

use crate::SheetApp;

/// Call after the frame's UI (a double tap opens the draft during it).
pub fn report(app: &SheetApp, ctx: &egui::Context) {
    let Some(ed) = app.editor.as_ref() else { return };
    let screen = ctx.content_rect();
    let cell = app
        .session
        .documents()
        .iter()
        .find(|d| d.uid == ed.doc)
        .and_then(|d| ed.sheet_in(&d.wb).and_then(|i| d.wb.sheet(i)).map(|sh| (d, sh)))
        .and_then(|(_, sh)| app.grid.geo.as_ref().map(|g| g.cell_rect(sh, ed.cell)))
        .filter(|r| r.is_finite() && screen.intersects(*r));
    // Off screen (scrolled away) or not laid out yet: the top of the window, where the formula bar is
    let rect = cell.unwrap_or_else(|| Rect::from_min_size(screen.min + vec2(0.0, 120.0), vec2(screen.width(), 24.0)));
    let cursor = Rect::from_min_size(pos2(rect.min.x + 2.0, rect.min.y), vec2(2.0, rect.height().max(12.0)));
    // A new context for each draft (another cell, or a new edit of the same one), so a composition
    // never carries over between cells.
    let generation = (u64::from(ed.cell.row) << 20) ^ u64::from(ed.cell.col) ^ (ed.sheet_uid << 40);
    ctx.output_mut(|o| {
        if o.ime.is_none() {
            o.ime = Some(egui::output::IMEOutput::new(Id::new("gridcraft.cell-draft"), generation, egui::IMEPurpose::Normal, rect, cursor));
        }
    });
}
