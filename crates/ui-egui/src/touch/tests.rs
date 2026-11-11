//! The gesture filter on its own, then fingers on the grid through the raw input hook as eframe
//! runs it, with the events a browser's touch handlers send: the emulated pointer press, moves
//! and release next to each [`egui::Event::Touch`].

use super::*;
use crate::{Services, SheetApp};
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, RangeRef};

fn touch(id: u64, phase: TouchPhase, pos: Pos2) -> Event {
    Event::Touch { device_id: egui::TouchDeviceId(1), id: TouchId(id), phase, pos, force: None }
}

fn press(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

/// One frame at `t` through the filter, over the grid everywhere.
fn run(g: &mut TouchGesture, t: f64, events: Vec<Event>) -> Vec<Event> {
    let mut raw = egui::RawInput { events, time: Some(t), ..Default::default() };
    g.filter(&mut raw, |_| true, || false);
    raw.events
}

fn presses(events: &[Event]) -> Vec<bool> {
    events.iter().filter_map(|e| if let Event::PointerButton { pressed, .. } = e { Some(*pressed) } else { None }).collect()
}

#[test]
fn a_drag_scrolls_and_holds_the_press_back() {
    let mut g = TouchGesture::default();
    let p = egui::pos2(100.0, 300.0);
    let out = run(&mut g, 0.0, vec![press(p, true), touch(1, TouchPhase::Start, p)]);
    assert!(presses(&out).is_empty(), "the press waits: {out:?}");
    let mut total = Vec2::ZERO;
    for i in 1..=10 {
        let at = p - egui::vec2(6.0 * i as f32, 12.0 * i as f32);
        let out = run(&mut g, f64::from(i) / 60.0, vec![Event::PointerMoved(at), touch(1, TouchPhase::Move, at)]);
        assert!(presses(&out).is_empty());
        total += g.take_scroll(Target::Canvas);
    }
    assert_eq!(total, egui::vec2(-60.0, -120.0), "1:1 with the finger, both ways");
    let end = p - egui::vec2(60.0, 120.0);
    let out = run(&mut g, 11.0 / 60.0, vec![press(end, false), Event::PointerGone, touch(1, TouchPhase::End, end)]);
    assert!(presses(&out).is_empty(), "no release either: {out:?}");
    assert!(out.contains(&Event::PointerGone));
    // It coasts on, slowing down.
    run(&mut g, 12.0 / 60.0, Vec::new());
    let first = g.take_scroll(Target::Canvas);
    assert!(first.y < -1.0 && first.x < -0.5, "{first:?}");
    let mut frames = 0;
    while g.repaint_after().is_some() && frames < 1000 {
        frames += 1;
        run(&mut g, f64::from(12 + frames) / 60.0, Vec::new());
    }
    assert!(frames < 1000, "the coast stops");
    // A mouse press after it waits a frame for a finger (this is a touch screen), then goes
    // through as it was; a press and release in one frame (a click) at once.
    let mouse = vec![Event::PointerMoved(p), press(p, true)];
    assert_eq!(run(&mut g, 30.0, mouse.clone()), [Event::PointerMoved(p)]);
    assert_eq!(run(&mut g, 30.02, Vec::new()), [Event::PointerMoved(p), press(p, true)]);
    assert!(!g.press_was_touch());
    let click = vec![press(p, true), press(p, false)];
    assert_eq!(run(&mut g, 30.04, click.clone()), click);
    // On a device that has shown no finger the mouse is never held.
    let mut desk = TouchGesture::default();
    assert_eq!(run(&mut desk, 0.0, mouse.clone()), mouse);
}

/// The browser's `pointerdown` comes in a frame of its own before the `touchstart`: on a touch
/// screen it is held and dropped for the finger; where no finger was expected it went through,
/// and is released as the finger takes over.
#[test]
fn a_fingers_early_pointerdown_never_stays_pressed_in_egui() {
    let p = egui::pos2(100.0, 300.0);
    let mut g = TouchGesture::default();
    g.expect_touch();
    assert!(presses(&run(&mut g, 0.0, vec![Event::PointerMoved(p), press(p, true)])).is_empty(), "held a frame");
    assert!(g.repaint_after().is_some());
    let out = run(&mut g, 0.02, vec![press(p, true), touch(1, TouchPhase::Start, p)]);
    assert!(presses(&out).is_empty(), "the finger's: dropped {out:?}");

    let mut g = TouchGesture::default();
    assert_eq!(presses(&run(&mut g, 0.0, vec![press(p, true)])), [true], "not known to be a finger: through");
    let out = run(&mut g, 0.02, vec![press(p, true), touch(1, TouchPhase::Start, p)]);
    assert_eq!(presses(&out), [false], "released as the finger takes over: {out:?}");
}

#[test]
fn a_tap_is_a_click_where_the_finger_came_down() {
    let mut g = TouchGesture::default();
    let p = egui::pos2(50.0, 60.0);
    run(&mut g, 0.0, vec![touch(1, TouchPhase::Start, p), press(p, true)]);
    let q = p + egui::vec2(3.0, 2.0);
    run(&mut g, 0.05, vec![touch(1, TouchPhase::Move, q), Event::PointerMoved(q)]);
    let out = run(&mut g, 0.1, vec![touch(1, TouchPhase::End, q), press(q, false), Event::PointerGone]);
    assert_eq!(presses(&out), [true, false]);
    assert!(out.iter().any(|e| matches!(e, Event::PointerButton { pos, .. } if *pos == p)));
    assert_eq!(out.last(), Some(&Event::PointerGone), "the hover ends after the click");
    assert!(g.press_was_touch());
    assert_eq!(g.take_scroll(Target::Canvas), Vec2::ZERO);
}

#[test]
fn a_long_press_lets_the_press_through_and_a_second_finger_ends_it() {
    let mut g = TouchGesture::default();
    let p = egui::pos2(50.0, 60.0);
    run(&mut g, 0.0, vec![touch(1, TouchPhase::Start, p), press(p, true)]);
    assert!(g.repaint_after().is_some_and(|d| d.as_secs_f64() <= LONG_PRESS));
    assert!(presses(&run(&mut g, 0.3, Vec::new())).is_empty());
    assert_eq!(presses(&run(&mut g, 0.5, Vec::new())), [true], "held: the press goes through");
    let q = p + egui::vec2(80.0, 0.0);
    let out = run(&mut g, 0.55, vec![touch(1, TouchPhase::Move, q), Event::PointerMoved(q)]);
    assert!(out.contains(&Event::PointerMoved(q)), "the drag reaches egui");
    assert_eq!(g.take_scroll(Target::Canvas), Vec2::ZERO, "and doesn't scroll");
    let out = run(&mut g, 0.6, vec![touch(2, TouchPhase::Start, q + egui::vec2(0.0, 90.0)), press(q, true)]);
    assert_eq!(presses(&out), [false], "a second finger releases the selection drag");
    assert!(g.pinching_canvas());
    let out = run(&mut g, 0.7, vec![touch(1, TouchPhase::End, q), press(q, false), touch(2, TouchPhase::End, q), Event::PointerGone]);
    assert!(presses(&out).is_empty());
    assert!(!g.pinching_canvas());
    assert!(!g.engaged());
}

#[test]
fn elsewhere_the_press_goes_through_a_frame_late_and_text_fields_scroll() {
    let p = egui::pos2(10.0, 10.0);
    let mut g = TouchGesture::default();
    let mut raw = egui::RawInput { events: vec![touch(1, TouchPhase::Start, p), press(p, true)], time: Some(0.0), ..Default::default() };
    g.filter(&mut raw, |_| false, || false);
    assert_eq!(raw.events.first(), Some(&Event::PointerMoved(p)), "the pointer goes to the finger");
    assert!(presses(&raw.events).is_empty(), "the press waits for egui to find what is under it");
    let mut raw = egui::RawInput { time: Some(0.02), ..Default::default() };
    g.filter(&mut raw, |_| false, || false);
    assert_eq!(presses(&raw.events), [true], "not a text field: egui's own handling");
    let mut raw = egui::RawInput { events: vec![touch(1, TouchPhase::End, p), press(p, false)], time: Some(0.04), ..Default::default() };
    g.filter(&mut raw, |_| false, || false);
    assert_eq!(presses(&raw.events), [false]);

    let mut g = TouchGesture::default();
    let mut raw = egui::RawInput { events: vec![touch(1, TouchPhase::Start, p), press(p, true)], time: Some(1.0), ..Default::default() };
    g.filter(&mut raw, |_| false, || false);
    let q = p + egui::vec2(0.0, 30.0);
    let mut raw = egui::RawInput { events: vec![touch(1, TouchPhase::Move, q), Event::PointerMoved(q)], time: Some(1.02), ..Default::default() };
    g.filter(&mut raw, |_| false, || true);
    assert!(presses(&raw.events).is_empty());
    assert_eq!(g.take_scroll(Target::Canvas), Vec2::ZERO);
    assert_eq!(g.take_scroll(Target::Field), egui::vec2(0.0, 30.0));
}

/// A multi-line text field in a scroll area, run through the filter as the app runs it
/// (`SheetApp::raw_input_hook`, then `SheetApp::ui` handing the field's scroll to egui): a finger
/// drag scrolls and leaves the field alone, a tap focuses it.
#[test]
fn a_finger_drag_in_a_text_field_scrolls_it_and_a_tap_focuses_it() {
    let ctx = egui::Context::default();
    let field = egui::Id::unique("long field");
    let mut text = "A line of a long note\n".repeat(200);
    let mut t = 0.0;
    let mut frame = |g: &mut TouchGesture, events: Vec<Event>| {
        t += 1.0 / 60.0;
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 300.0));
        let mut raw = egui::RawInput { events, time: Some(t), screen_rect: Some(screen), ..Default::default() };
        g.filter(&mut raw, |_| false, || field_hovered(&ctx));
        let mut offset = 0.0_f32;
        let mut out = ctx.run_ui(raw, |ui| {
            let d = g.take_scroll(Target::Field);
            ui.ctx().input_mut(|i| i.smooth_scroll_delta += d);
            let shown = egui::ScrollArea::vertical().show(ui, |ui| ui.add(egui::TextEdit::multiline(&mut text).id(field)));
            offset = shown.state.clamped_offset().y;
        });
        out.textures_delta.clear();
        offset
    };
    let mut g = TouchGesture::default();
    frame(&mut g, Vec::new());
    let p = egui::pos2(150.0, 250.0);
    frame(&mut g, vec![press(p, true), touch(1, TouchPhase::Start, p)]);
    let mut offset = 0.0_f32;
    for i in 1..=10 {
        let at = p - egui::vec2(0.0, 15.0 * i as f32);
        offset = frame(&mut g, vec![Event::PointerMoved(at), touch(1, TouchPhase::Move, at)]);
    }
    assert!((offset - 150.0).abs() < 1.0, "the field scrolled with the finger: {offset}");
    let end = p - egui::vec2(0.0, 150.0);
    frame(&mut g, vec![press(end, false), Event::PointerGone, touch(1, TouchPhase::End, end)]);
    assert!(!ctx.memory(|m| m.has_focus(field)), "a scroll doesn't focus or select in the field");
    let glide = frame(&mut g, Vec::new());
    assert!(glide > offset, "and glides on: {offset} -> {glide}");
    for _ in 0..300 {
        frame(&mut g, Vec::new());
    }
    assert!(g.repaint_after().is_none());
    frame(&mut g, vec![press(p, true), touch(1, TouchPhase::Start, p)]);
    frame(&mut g, Vec::new());
    frame(&mut g, vec![press(p, false), Event::PointerGone, touch(1, TouchPhase::End, p)]);
    frame(&mut g, Vec::new());
    assert!(ctx.memory(|m| m.has_focus(field)), "a tap focuses the field");
}

#[test]
fn junk_positions_are_ignored() {
    let mut g = TouchGesture::default();
    let out = run(&mut g, 0.0, vec![touch(1, TouchPhase::Start, egui::pos2(f32::NAN, 1.0))]);
    assert!(!out.iter().any(|e| matches!(e, Event::PointerButton { .. })));
    assert_eq!(g.take_scroll(Target::Canvas), Vec2::ZERO);
}

/// The app on a 1200 by 800 phone-in-landscape screen, driven frame by frame through the raw input
/// hook, `logic` and `ui`, 1/60 s apart.
struct Phone {
    ctx: egui::Context,
    app: SheetApp,
    t: f64,
}

impl Phone {
    /// A tablet whose web shell found a coarse pointer (`SheetApp::expect_touch`).
    fn new() -> Self {
        let mut p = Self::desk();
        p.app.expect_touch();
        p
    }

    /// A desktop: no coarse pointer, no finger seen yet.
    fn desk() -> Self {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::pinned_font_definitions());
        crate::theme::apply(&ctx, false);
        let mut session = Session::new();
        session.new_workbook();
        let mut app = SheetApp::new(session, Services::default());
        app.use_prepared_fonts();
        let mut p = Phone { ctx, app, t: 0.0 };
        p.idle(6);
        assert!(p.app.grid.cells_rect.is_some(), "the grid is drawn");
        p
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.t += 1.0 / 60.0;
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut raw = egui::RawInput { events, time: Some(self.t), screen_rect: Some(screen), ..Default::default() };
        let app = &mut self.app;
        app.raw_input_hook(&self.ctx, &mut raw);
        let mut out = self.ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        });
        out.textures_delta.clear();
    }

    fn idle(&mut self, frames: usize) {
        for _ in 0..frames {
            self.frame(Vec::new());
        }
    }

    /// Finger `id` comes down as a browser reports it: its `pointerdown` (a press, and a frame at
    /// once), then its `touchstart` (the emulated press of the first finger with the touch).
    fn down(&mut self, id: u64, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at), press(at, true)]);
        self.frame(vec![press(at, true), touch(id, TouchPhase::Start, at)]);
    }

    fn drag(&mut self, id: u64, from: Pos2, to: Pos2, steps: usize) {
        for i in 1..=steps {
            let at = from + (to - from) * (i as f32 / steps as f32);
            self.frame(vec![Event::PointerMoved(at), touch(id, TouchPhase::Move, at)]);
        }
    }

    /// Its `pointerup` (a release, a frame at once), then its `touchend`.
    fn up(&mut self, id: u64, at: Pos2) {
        self.frame(vec![press(at, false)]);
        self.frame(vec![press(at, false), Event::PointerGone, touch(id, TouchPhase::End, at)]);
    }

    /// Down at `from`, ten moves a frame apart to `to`, up.
    fn swipe(&mut self, from: Pos2, to: Pos2) {
        self.down(1, from);
        self.drag(1, from, to, 10);
        self.up(1, to);
    }

    fn dirty(&self) -> bool {
        self.app.session.active().unwrap().is_dirty()
    }

    fn selection(&self) -> gridcraft_engine::Selection {
        self.app.session.active().unwrap().selection.clone()
    }

    fn scroll(&self) -> Vec2 {
        self.app.view().scroll
    }

    fn zoom(&self) -> u16 {
        self.app.session.active().unwrap().wb.active().unwrap().zoom
    }

    /// The centre of `cell` on screen.
    fn at(&self, cell: CellRef) -> Pos2 {
        let wb = self.app.session.active().unwrap().wb.clone();
        let geo = self.app.grid.geo.as_ref().unwrap();
        geo.screen_cell_rect(wb.active().unwrap(), cell).center()
    }

    /// The cell under `p` on screen.
    fn cell_at(&self, p: Pos2) -> CellRef {
        let wb = self.app.session.active().unwrap().wb.clone();
        let geo = self.app.grid.geo.as_ref().unwrap();
        geo.cell_at(wb.active().unwrap(), geo.mirror_pos(p))
    }

    /// Screen points per sheet unit at the sheet's zoom.
    fn z(&self) -> f32 {
        self.app.grid.geo.as_ref().unwrap().z
    }
}

fn only(cell: CellRef) -> gridcraft_engine::Selection {
    gridcraft_engine::Selection { active: cell, anchor: cell, ranges: vec![RangeRef::new(cell, cell)] }
}

/// A finger dragged up the grid scrolls it down with the finger, 1:1, and on after it lifts;
/// sideways likewise. No cell is selected and the active cell stays.
#[test]
fn a_finger_drag_scrolls_the_grid_both_ways_and_selects_nothing() {
    let mut p = Phone::new();
    let sel = p.selection();
    assert_eq!(sel, only(CellRef::new(0, 0)));
    let z = p.z();
    let from = p.app.grid.cells_rect.unwrap().center() + egui::vec2(0.0, 150.0);
    let to = from - egui::vec2(0.0, 300.0);
    p.down(1, from);
    p.drag(1, from, to, 15);
    assert!((p.scroll().y - 300.0 / z).abs() < 0.5, "follows the finger: {:?}", p.scroll());
    assert_eq!(p.scroll().x, 0.0);
    assert_eq!(p.selection(), sel, "no selection, no active-cell move");
    p.up(1, to);
    let lifted = p.scroll().y;
    p.idle(3);
    assert!(p.scroll().y > lifted + 5.0, "it glides on after the finger lifts: {lifted} -> {}", p.scroll().y);
    p.idle(300);
    let settled = p.scroll();
    p.idle(3);
    assert_eq!(p.scroll(), settled, "and stops");
    assert!(p.app.touch.repaint_after().is_none());

    // Sideways: a finger dragged left scrolls right.
    let from = p.app.grid.cells_rect.unwrap().center() + egui::vec2(200.0, 0.0);
    let to = from - egui::vec2(400.0, 0.0);
    p.down(1, from);
    p.drag(1, from, to, 15);
    assert!((p.scroll().x - 400.0 / z).abs() < 0.5, "follows the finger sideways: {:?}", p.scroll());
    assert_eq!(p.scroll().y, settled.y);
    p.up(1, to);
    p.idle(300);
    assert_eq!(p.selection(), sel);
    assert!(!p.app.grid.drag_select);
}

/// A tap selects the cell under the finger and scrolls nothing.
#[test]
fn a_tap_selects_the_cell() {
    let mut p = Phone::new();
    let target = CellRef::new(6, 3);
    let at = p.at(target);
    assert_eq!(p.cell_at(at), target);
    p.down(1, at);
    p.frame(vec![Event::PointerMoved(at + egui::vec2(2.0, 1.0)), touch(1, TouchPhase::Move, at + egui::vec2(2.0, 1.0))]);
    p.up(1, at + egui::vec2(2.0, 1.0));
    p.idle(2);
    assert_eq!(p.selection(), only(target));
    assert_eq!(p.scroll(), Vec2::ZERO);
}

/// A finger held still for the long press, then dragged, selects the range it covers; the sheet
/// doesn't scroll.
#[test]
fn a_long_press_then_drag_selects_a_range() {
    let mut p = Phone::new();
    let from = p.at(CellRef::new(2, 1));
    let to = p.at(CellRef::new(8, 4));
    p.down(1, from);
    p.idle(32);
    assert!(p.t >= LONG_PRESS);
    p.drag(1, from, to, 10);
    p.up(1, to);
    p.idle(2);
    let sel = p.selection();
    assert_eq!(sel.active, CellRef::new(2, 1), "{sel:?}");
    assert_eq!(sel.ranges, vec![RangeRef::new(CellRef::new(2, 1), CellRef::new(8, 4))], "{sel:?}");
    assert_eq!(p.scroll(), Vec2::ZERO);
}

/// Two fingers spreading apart zoom in about the point between them; nothing is selected.
#[test]
fn a_pinch_zooms_about_its_centre() {
    let mut p = Phone::new();
    let sel = p.selection();
    let centre = p.app.grid.cells_rect.unwrap().center();
    let under = p.cell_at(centre);
    let (a, b) = (centre - egui::vec2(60.0, 0.0), centre + egui::vec2(60.0, 0.0));
    let dirty = p.dirty();
    p.down(1, a);
    // The second finger's `pointerdown`, then its `touchstart`.
    p.frame(vec![press(b, true)]);
    p.frame(vec![press(a, true), touch(2, TouchPhase::Start, b)]);
    for i in 1..=12 {
        let spread = egui::vec2(60.0 + 5.0 * i as f32, 0.0);
        p.frame(vec![Event::PointerMoved(centre - spread), touch(1, TouchPhase::Move, centre - spread), touch(2, TouchPhase::Move, centre + spread)]);
    }
    let zoom = p.zoom();
    assert!(zoom >= 180 && zoom <= 205, "120 pt apart to 240: about twice the zoom, {zoom}%");
    let end = egui::vec2(120.0, 0.0);
    p.frame(vec![press(centre + end, false)]);
    p.frame(vec![touch(2, TouchPhase::End, centre + end)]);
    p.up(1, centre - end);
    p.idle(3);
    assert_eq!(p.zoom(), zoom, "the zoom stays after the fingers lift");
    assert_eq!(p.cell_at(centre), under, "the cell under the pinch stays under it");
    assert_eq!(p.selection(), sel, "a pinch selects nothing");
    assert_eq!(p.dirty(), dirty, "nor marks the workbook modified");
    assert!(p.app.editor.is_none());
}

/// A mouse drag still selects the range it covers, as before.
#[test]
fn a_mouse_drag_still_selects_a_range() {
    let mut p = Phone::desk();
    let from = p.at(CellRef::new(1, 1));
    let to = p.at(CellRef::new(5, 3));
    p.frame(vec![Event::PointerMoved(from)]);
    p.frame(vec![press(from, true)]);
    assert!(p.app.touch.repaint_after().is_none(), "a desktop's press is never held");
    for i in 1..=8 {
        p.frame(vec![Event::PointerMoved(from + (to - from) * (i as f32 / 8.0))]);
    }
    p.frame(vec![press(to, false)]);
    p.idle(2);
    let sel = p.selection();
    assert_eq!(sel.active, CellRef::new(1, 1), "{sel:?}");
    assert_eq!(sel.ranges, vec![RangeRef::new(CellRef::new(1, 1), CellRef::new(5, 3))], "{sel:?}");
    assert_eq!(p.scroll(), Vec2::ZERO);
}

/// On the fill handle a finger drag scrolls and fills nothing; held first, it pulls the handle and
/// fills, as a mouse drag does.
#[test]
fn the_fill_handle_fills_only_after_a_long_press() {
    let mut p = Phone::new();
    p.app.session.run("cell.set", serde_json::json!({"cell": "B2", "input": "7"})).unwrap();
    p.idle(2);
    let value = |p: &Phone, c: CellRef| p.app.session.active().unwrap().wb.active().unwrap().value(c);
    let b2 = CellRef::new(1, 1);
    let at = p.at(b2);
    p.down(1, at);
    p.up(1, at);
    p.idle(2);
    assert_eq!(p.selection(), only(b2));
    let wb = p.app.session.active().unwrap().wb.clone();
    let handle = p.app.grid.geo.as_ref().unwrap().screen_cell_rect(wb.active().unwrap(), b2).max - egui::vec2(1.0, 1.0);
    let down = handle + egui::vec2(0.0, 200.0);
    p.down(1, handle);
    p.drag(1, handle, down, 10);
    p.up(1, down);
    p.idle(300);
    assert!(p.scroll().y < 0.01, "dragging down at the top scrolls into the sheet's start: {:?}", p.scroll());
    assert_eq!(value(&p, CellRef::new(2, 1)), gridcraft_engine::core::Value::Empty, "a finger drag doesn't fill");
    assert_eq!(p.selection(), only(b2));

    let to = p.at(CellRef::new(4, 1));
    p.down(1, handle);
    p.idle(32);
    p.drag(1, handle, to, 10);
    p.up(1, to);
    p.idle(2);
    for row in 2..=4 {
        assert_ne!(value(&p, CellRef::new(row, 1)), gridcraft_engine::core::Value::Empty, "held, the handle fills down to row {}", row + 1);
    }
}

/// Seen in Clawku on a phone: swipes scrolled the sheet but each also selected a range ("32R x
/// 1C" after two swipes up, "75R x 3C" after one sideways, "72R x 15C" after a pinch). The
/// browser's `pointerdown` came a frame before its `touchstart` and went through as a mouse
/// press that stayed down. Swipes (the second one during the first one's glide) select nothing,
/// and a tap later selects just its cell.
#[test]
fn swipes_during_a_glide_select_nothing_and_a_later_tap_selects_one_cell() {
    let mut p = Phone::new();
    let sel = p.selection();
    let dirty = p.dirty();
    let c = p.app.grid.cells_rect.unwrap().center();
    let (from, to) = (c + egui::vec2(0.0, 200.0), c - egui::vec2(0.0, 150.0));
    p.swipe(from, to);
    // 600 ms after the first came down, the second comes down while the sheet still glides.
    let first = p.scroll().y;
    p.idle(36 - 14);
    assert!(p.app.touch.repaint_after().is_some(), "still gliding");
    assert!(p.scroll().y > first);
    p.swipe(from, to);
    p.idle(300);
    assert!(p.scroll().y > 2.0 * 350.0 / p.z(), "both swipes scrolled: {:?}", p.scroll());
    assert_eq!(p.selection(), sel, "two swipes up select nothing");
    // Sideways.
    let x = p.scroll().x;
    p.swipe(c + egui::vec2(250.0, 0.0), c - egui::vec2(250.0, 0.0));
    p.idle(300);
    assert!(p.scroll().x > x, "{:?}", p.scroll());
    assert_eq!(p.selection(), sel, "a sideways swipe selects nothing");
    assert_eq!(p.dirty(), dirty);
    assert!(!p.app.grid.drag_select && p.app.editor.is_none());
    // Two seconds on, a tap selects one cell, and isn't taken for a double-click.
    p.idle(120);
    let target = p.cell_at(c);
    p.down(1, c);
    p.up(1, c);
    p.idle(2);
    assert_eq!(p.selection(), only(target));
    assert!(p.app.editor.is_none(), "a single tap doesn't edit the cell");
}

/// On a device not known to have a touch screen (no coarse pointer) the first finger's early
/// press reaches egui, but it is let go as the finger takes over: the swipe scrolls and selects
/// no range.
#[test]
fn a_swipe_on_an_unknown_device_selects_no_range() {
    let mut p = Phone::desk();
    let c = p.app.grid.cells_rect.unwrap().center();
    p.swipe(c + egui::vec2(0.0, 200.0), c - egui::vec2(0.0, 150.0));
    p.idle(300);
    assert!(p.scroll().y > 0.0);
    let sel = p.selection();
    assert!(sel.ranges.len() == 1 && sel.ranges[0].is_single(), "no range: {sel:?}");
    p.swipe(c + egui::vec2(0.0, 200.0), c - egui::vec2(0.0, 150.0));
    p.idle(300);
    assert_eq!(p.selection(), sel, "a finger is known now: nothing at all");
}
