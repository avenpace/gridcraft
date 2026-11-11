//! Fingers on a touch screen (phones and tablets, in a browser or natively).
//!
//! A touch arrives from eframe as [`egui::Event::Touch`] plus emulated primary-button pointer
//! events, so left alone a one-finger drag on the grid selects cells and the sheet can't be read
//! past its first screen. [`TouchGesture::filter`] runs in the raw input hook, before egui sees
//! the frame, and holds the emulated press back while it finds out what the finger is doing:
//! - it moves more than [`SLOP`]: the sheet scrolls with it, 1:1 along both axes, and coasts
//!   after it lifts ([`Momentum`]); the press never reaches egui, so no cell is selected, no fill
//!   handle, column border or object is dragged;
//! - it lifts first: a tap, given to egui as a click where the finger came down (selects the
//!   cell, presses a filter button; two taps make a double-click, which edits the cell);
//! - it rests for [`LONG_PRESS`]: the press goes through then, and a drag selects a range (or
//!   pulls the fill handle, moves the selection by its border, resizes a column), as on iOS;
//! - a second finger comes down: a pinch zooms the sheet about the point between the fingers and
//!   the fingers pan it (the grid reads [`egui::InputState::multi_touch`]), and whatever the
//!   first finger started stops.
//!
//! This applies on the grid ([`Target::Canvas`]: cells, headings, rulers and the desk around the
//! pages) and in text fields ([`Target::Field`], whose scroll goes to the scroll area around
//! them). Everything else (buttons, lists, panes, menus, sliders, scroll bars) keeps egui's own
//! touch handling: its scroll areas already scroll when dragged by finger. Mouse, pen-less
//! touchpad and wheel input never contain [`egui::Event::Touch`] and pass through untouched.
//!
//! Adapted from ClaWord's `touch` module (wordcraft 714a8c01), less its Read Mode swipes.

use egui::{Event, Pos2, TouchId, TouchPhase, Vec2};

/// Finger travel (points) that makes a press a scroll rather than a tap or a long press.
pub const SLOP: f32 = 8.0;
/// Seconds a finger rests before its press goes through and a drag selects.
pub const LONG_PRESS: f64 = 0.45;

/// What a finger that came down is acting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The cell grid, with its headings and (in Page Layout) rulers and the desk.
    Canvas,
    /// A text field (its scroll area scrolls).
    Field,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    /// Down, not yet moved past [`SLOP`] or held for [`LONG_PRESS`].
    Pending,
    Scrolling,
    /// Held, then the press went through: egui sees a drag.
    Selecting,
    /// Two fingers or more.
    Multi,
    /// Not the grid or a text field: egui's own handling (the press went through).
    Native,
}

impl Phase {
    /// The emulated pointer press/release of this phase is held back.
    fn owned(self) -> bool {
        matches!(self, Phase::Pending | Phase::Scrolling | Phase::Selecting | Phase::Multi)
    }
}

/// How the fingers on the screen are read. Kept by the app across frames.
#[derive(Debug, Default)]
pub struct TouchGesture {
    phase: Phase,
    /// None while [`Phase::Pending`] waits a frame to see what egui found under the finger.
    target: Option<Target>,
    /// The finger that started the gesture.
    finger: Option<TouchId>,
    down: Vec<TouchId>,
    start: Pos2,
    start_time: f64,
    last: Pos2,
    frame_time: Option<f64>,
    /// This frame's scroll for `target`, in the wheel's convention (positive = content moves
    /// down/right): the finger's own movement.
    scroll: Vec2,
    scroll_target: Option<Target>,
    momentum: Momentum,
    /// The emulated `PointerGone` of a lifted finger waits until a text field's coast ends: the
    /// field's scroll area only scrolls under the pointer.
    gone_pending: bool,
    /// The last primary press egui saw came from a finger.
    press_was_touch: bool,
}

impl TouchGesture {
    /// Rewrites one frame's raw input (see the module docs). `on_canvas`: a finger coming down
    /// there is on the grid. `field_hovered`: egui's last pass found a text field under the
    /// pointer (asked the frame after the finger came down, where the pointer was moved to it).
    pub fn filter(&mut self, raw: &mut egui::RawInput, on_canvas: impl Fn(Pos2) -> bool, field_hovered: impl FnOnce() -> bool) {
        let now = raw.time.or(self.frame_time.map(|t| t + 1.0 / 60.0)).unwrap_or(0.0);
        let dt = self.frame_time.map_or(0.0, |t| (now - t).clamp(0.0, 1.0) as f32);
        self.frame_time = Some(now);
        self.scroll = Vec2::ZERO;
        let touched = raw.events.iter().any(|e| matches!(e, Event::Touch { .. }));
        if !touched && self.phase == Phase::Idle && !self.momentum.is_active() && !self.gone_pending {
            // Mouse, touchpad and keyboard frames: as they came, apart from noting their presses.
            if raw.events.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, .. })) {
                self.press_was_touch = false;
            }
            return;
        }
        // The keys held with the finger, as the emulated pointer events carry them.
        let mods =
            raw.events.iter().find_map(|e| if let Event::PointerButton { modifiers, .. } = e { Some(*modifiers) } else { None }).unwrap_or_default();
        let button = |pos: Pos2, pressed: bool| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: mods };
        let mut pre: Vec<Event> = Vec::new();
        let mut post: Vec<Event> = Vec::new();
        // A finger already up: its coast goes on.
        if self.phase == Phase::Idle && self.momentum.is_active() {
            self.scroll += self.momentum.tick(dt);
            if !self.momentum.is_active() && std::mem::take(&mut self.gone_pending) {
                post.push(Event::PointerGone);
            }
        }
        // Last frame moved the pointer under a finger of unknown target: a text field scrolls,
        // anything else gets its press now (a frame late) and egui's own touch handling.
        if self.phase == Phase::Pending && self.target.is_none() {
            if field_hovered() {
                self.target = Some(Target::Field);
            } else {
                self.phase = Phase::Native;
                pre.extend([Event::PointerMoved(self.start), button(self.start, true)]);
                self.press_was_touch = true;
            }
        }
        if self.phase == Phase::Pending && self.target.is_some() && now - self.start_time >= LONG_PRESS {
            self.phase = Phase::Selecting;
            pre.extend([Event::PointerMoved(self.start), button(self.start, true)]);
            self.press_was_touch = true;
        }
        // A finger that came down and lifted within this frame is still this frame's to rewrite.
        let mut owned = self.phase.owned();
        let mut moved = Vec2::ZERO;
        let mut out: Vec<Event> = Vec::with_capacity(raw.events.len() + 4);
        for event in &raw.events {
            let Event::Touch { id, phase, pos, .. } = *event else { continue };
            if !pos.is_finite() {
                continue;
            }
            match phase {
                TouchPhase::Start => {
                    if !self.down.contains(&id) {
                        self.down.push(id);
                    }
                    if self.phase == Phase::Idle {
                        self.begin(id, pos, now, on_canvas(pos));
                        owned = true;
                        out.push(Event::PointerMoved(pos));
                    } else if self.down.len() >= 2 && self.phase.owned() {
                        if self.phase == Phase::Selecting {
                            post.push(button(self.last, false));
                        }
                        self.phase = Phase::Multi;
                        self.momentum.cancel();
                    }
                }
                TouchPhase::Move if Some(id) == self.finger => match self.phase {
                    Phase::Pending if self.target.is_some() && (pos - self.start).length() > SLOP => {
                        self.phase = Phase::Scrolling;
                        moved += pos - self.start;
                        self.last = pos;
                    }
                    Phase::Scrolling => {
                        moved += pos - self.last;
                        self.last = pos;
                    }
                    Phase::Pending => {}
                    _ => self.last = pos,
                },
                TouchPhase::Move => {}
                TouchPhase::End | TouchPhase::Cancel => {
                    self.down.retain(|d| *d != id);
                    if Some(id) == self.finger {
                        self.lift(phase == TouchPhase::End, pos, &mut post, &button);
                    }
                }
            }
        }
        if moved != Vec2::ZERO {
            self.scroll += moved;
            self.momentum.feed(moved, dt);
        }
        self.scroll_target = if self.scroll != Vec2::ZERO { self.target } else { None };
        let multi = self.phase == Phase::Multi;
        let mut gone = false;
        for event in raw.events.drain(..) {
            match &event {
                Event::PointerButton { button: egui::PointerButton::Primary, .. } if owned => continue,
                Event::PointerGone if owned => gone = true,
                Event::PointerMoved(_) if owned && multi => {}
                Event::PointerButton { pressed: true, .. } => {
                    self.press_was_touch = touched || !self.down.is_empty();
                    out.push(event);
                }
                _ => out.push(event),
            }
        }
        if self.down.is_empty() {
            if gone && self.momentum.is_active() && self.target == Some(Target::Field) {
                self.gone_pending = true;
            } else if gone {
                post.push(Event::PointerGone);
            }
            if self.phase != Phase::Idle {
                self.phase = Phase::Idle;
                self.finger = None;
            }
        }
        pre.append(&mut out);
        pre.append(&mut post);
        raw.events = pre;
    }

    fn begin(&mut self, id: TouchId, pos: Pos2, now: f64, on_canvas: bool) {
        self.phase = Phase::Pending;
        self.target = on_canvas.then_some(Target::Canvas);
        self.finger = Some(id);
        self.start = pos;
        self.last = pos;
        self.start_time = now;
        self.gone_pending = false;
        self.momentum.cancel();
    }

    /// The gesture's finger lifted (`ended`; else the system cancelled it) at `pos`.
    fn lift(&mut self, ended: bool, pos: Pos2, post: &mut Vec<Event>, button: &impl Fn(Pos2, bool) -> Event) {
        match self.phase {
            // A tap: a click where the finger came down.
            Phase::Pending if ended => {
                post.extend([Event::PointerMoved(self.start), button(self.start, true), button(self.start, false)]);
                self.press_was_touch = true;
            }
            Phase::Scrolling if ended => self.momentum.release(),
            Phase::Scrolling => self.momentum.cancel(),
            Phase::Selecting => post.push(button(if pos.is_finite() { pos } else { self.last }, false)),
            _ => {}
        }
        self.finger = None;
        if !matches!(self.phase, Phase::Multi | Phase::Native) {
            self.phase = if self.down.is_empty() { Phase::Idle } else { Phase::Multi };
        }
    }

    /// This frame's finger scroll for `target` (the wheel's convention), taken once.
    pub fn take_scroll(&mut self, target: Target) -> Vec2 {
        if self.scroll_target == Some(target) {
            self.scroll_target = None;
            std::mem::take(&mut self.scroll)
        } else {
            Vec2::ZERO
        }
    }

    /// Two fingers or more are on the grid: pinch zooms it and the fingers pan it.
    pub fn pinching_canvas(&self) -> bool {
        self.phase == Phase::Multi && self.target == Some(Target::Canvas)
    }

    /// A finger is down and this filter reads it: what egui sees of it is already decided here.
    pub fn engaged(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// The sheet scrolled into its end: the coast stops.
    pub fn hit_edge(&mut self) {
        self.momentum.cancel();
    }

    /// The last primary press egui saw was a finger's (a touch screen has no hover).
    pub fn press_was_touch(&self) -> bool {
        self.press_was_touch
    }

    /// How soon the app must run again with no new input: a resting finger becomes a long press,
    /// a coast goes on, or a finger's target is decided on the next frame.
    pub fn repaint_after(&self) -> Option<std::time::Duration> {
        match self.phase {
            Phase::Pending if self.target.is_none() => Some(std::time::Duration::ZERO),
            Phase::Pending => {
                let left = LONG_PRESS - (self.frame_time.unwrap_or(self.start_time) - self.start_time);
                Some(std::time::Duration::from_secs_f64(left.clamp(0.0, LONG_PRESS)))
            }
            _ if self.momentum.is_active() || self.gone_pending => Some(std::time::Duration::ZERO),
            _ => None,
        }
    }
}

/// egui's last pass found a text field under the pointer: one of the widgets it marked hovered
/// keeps a text edit's state.
pub fn field_hovered(ctx: &egui::Context) -> bool {
    let hovered: Vec<egui::Id> = ctx.interaction_snapshot(|s| s.hovered.iter().copied().collect());
    hovered.into_iter().any(|id| egui::TextEdit::load_state(ctx, id).is_some())
}

/// Longest frame time trusted for velocity and decay (a stall shouldn't fling the sheet).
const MAX_DT: f32 = 0.1;
/// Shortest event spacing trusted for velocity (two events in one burst aren't infinitely fast).
const MIN_DT: f32 = 1.0 / 500.0;
/// Velocity is averaged over roughly this many seconds of input.
const VELOCITY_WINDOW: f32 = 0.04;
/// A finger that rested this long before lifting doesn't fling.
const STALE: f32 = 0.1;
/// Slower flicks than this (points/s) don't coast…
const MIN_FLING: f32 = 150.0;
/// …and coasting stops below this speed.
const STOP_SPEED: f32 = 20.0;
/// Fastest coast (points/s).
const MAX_SPEED: f32 = 8000.0;
/// Exponential decay rate of the coast (1/s): speed falls to 1/e every ~0.33 s.
const DECAY: f32 = 3.0;

/// Kinetic scrolling after a finger flick: tracks the velocity while the finger moves and keeps
/// scrolling with exponentially decaying speed after it lifts. Time-based, so frame-rate
/// independent. Deltas are in screen points, positive = content moves down/right. (ClaWord's
/// `scroll::Momentum`, which also serves its touchpads; here a finger always reports its lift.)
#[derive(Clone, Debug, Default)]
struct Momentum {
    velocity: Vec2,
    /// Seconds since the last fed delta.
    since_input: f32,
    /// The finger is moving (deltas arrived and no lift yet).
    streaming: bool,
    coasting: bool,
}

impl Momentum {
    /// A frame's finger delta, `dt` seconds after the previous frame. Non-finite input is ignored.
    fn feed(&mut self, delta: Vec2, dt: f32) {
        if !delta.is_finite() || !dt.is_finite() {
            return;
        }
        let dt = dt.clamp(0.0, MAX_DT);
        let fresh = !self.streaming || self.since_input + dt > STALE;
        let span = if fresh { dt } else { self.since_input + dt }.clamp(MIN_DT, MAX_DT);
        let sample = delta / span;
        self.velocity = if fresh {
            sample
        } else {
            let a = 1.0 - (-span / VELOCITY_WINDOW).exp();
            self.velocity + (sample - self.velocity) * a
        };
        let speed = self.velocity.length();
        if speed > MAX_SPEED {
            self.velocity *= MAX_SPEED / speed;
        }
        self.since_input = 0.0;
        self.streaming = true;
        self.coasting = false;
    }

    /// The finger lifted: coast if the flick was fast and recent enough.
    fn release(&mut self) {
        let fling = self.streaming && self.since_input <= STALE && self.velocity.length() >= MIN_FLING;
        self.streaming = false;
        if fling {
            self.coasting = true;
        } else {
            self.cancel();
        }
    }

    /// Stop dead (a new finger, a cancelled touch, the end of the sheet).
    fn cancel(&mut self) {
        self.velocity = Vec2::ZERO;
        self.coasting = false;
        self.streaming = false;
    }

    /// Advance a frame after the lift; returns how far to coast this frame.
    fn tick(&mut self, dt: f32) -> Vec2 {
        if !dt.is_finite() || dt <= 0.0 {
            return Vec2::ZERO;
        }
        let dt = dt.min(MAX_DT);
        self.since_input += dt;
        if !self.coasting {
            return Vec2::ZERO;
        }
        // Exact integral of v·e^(−kt) over the frame, so the distance doesn't depend on frame rate.
        let keep = (-DECAY * dt).exp();
        let step = self.velocity * ((1.0 - keep) / DECAY);
        self.velocity *= keep;
        if self.velocity.length() < STOP_SPEED {
            self.cancel();
        }
        step
    }

    /// Still coasting: keep repainting.
    fn is_active(&self) -> bool {
        self.coasting
    }
}

#[cfg(test)]
mod tests;
