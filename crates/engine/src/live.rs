//! Live co-editing (Clawku): several people edit one workbook at once.
//!
//! The host (Clawku) puts every change into one order and sends each change, numbered, to
//! everyone who has the workbook open, the person who made it included. Every copy applies the
//! changes in that order, so every copy ends up the same.
//!
//! A change is a top-level command that changed the workbook, with the sheet and selection it ran
//! in: the same command runs in the same place on every copy. Your own changes show at once
//! (optimistic): each one keeps a snapshot of the workbook from just before it. When someone
//! else's change arrives while some of yours are still on their way, yours are put back to that
//! snapshot, theirs runs, and yours run again on top, in the order the host will give them.
//! Undo and Redo work on the shared history (the last change, whoever made it), so they wait for
//! their turn instead of showing at once.
//!
//! Clipboards stay per person: Copy and Cut are shared too, so every copy keeps each person's
//! copied cells, and a Paste pastes what that person copied, everywhere.
//!
//! Commands that run scripts or macros are refused while editing together, and a change from the
//! host is checked against the same rules before it runs, so a change can only ever be one of the
//! workbook's own editing commands.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value as Json, json};

use crate::cmd::{CommandSpec, find_command};
use crate::{Clipboard, EngineError, HistoryEntry, Result, Selection, Session};
use gridcraft_model::Workbook;

/// The most of your own changes that may wait for the host at once; past it, editing pauses
/// until the host catches up (a slow or lost connection).
pub const MAX_PENDING: usize = 200;

/// Shared besides the commands that change the workbook: the shared history and the clipboards.
const SHARED_EXTRA: [&str; 4] = ["edit.undo", "edit.redo", "edit.copy", "edit.cut"];

/// Never run while editing together: scripts and macros (they would run on everyone's copy).
fn refused(id: &str) -> bool {
    id.starts_with("macro.") || id.starts_with("automate.") || id.starts_with("file.") || id.starts_with("window.")
}

/// Each person's own view, though it is kept in the workbook (saved with the file): zoom. It never
/// goes to the host, so zooming (a pinch, Ctrl+wheel, the zoom slider) changes nobody else's copy
/// and leaves nothing waiting to be stored; a viewer may zoom too.
const VIEW_ONLY: [&str; 3] = ["view.zoom", "view.zoom100", "view.zoomToSelection"];

/// Whether a command is part of the shared workbook: anything that can change it, and the shared
/// history and clipboards.
fn shared(spec: &CommandSpec) -> bool {
    (spec.undoable || SHARED_EXTRA.contains(&spec.id)) && !VIEW_ONLY.contains(&spec.id)
}

/// Undo and Redo: they wait for their turn in the shared order.
fn ordered_only(id: &str) -> bool {
    matches!(id, "edit.undo" | "edit.redo")
}

/// Copy and Cut: they run here at once and are shared for the other copies' clipboards.
fn clipboard_only(id: &str) -> bool {
    matches!(id, "edit.copy" | "edit.cut")
}

/// The workbook and its history just before one of your changes.
#[derive(Clone, Debug)]
struct Base {
    wb: Arc<Workbook>,
    selection: Selection,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
}

/// One of your changes the host has not numbered yet.
#[derive(Clone, Debug)]
struct Pending {
    op: Json,
    /// Your clipboard when you made it (a Paste runs again with it).
    clip: Option<Clipboard>,
    base: Base,
}

/// The live state of the shared workbook in this copy.
#[derive(Clone, Debug)]
pub struct Live {
    /// The shared workbook (its document uid): other workbooks open here stay private.
    pub doc: u64,
    /// This person (the host's id for them).
    pub me: String,
    /// Viewers see every change but make none.
    pub can_edit: bool,
    /// Changes to send to the host, oldest first ([`Session::live_take_outbox`]).
    outbox: Vec<Json>,
    pending: Vec<Pending>,
    /// The other people's clipboards.
    clips: HashMap<String, Option<Clipboard>>,
    /// The last change number applied.
    pub seq: u64,
    applying: bool,
    /// A command being watched: (the change, your clipboard, the state before it).
    watching: Option<(Json, Option<Clipboard>, Base)>,
}

impl Live {
    pub fn new(doc: u64, me: impl Into<String>, can_edit: bool, seq: u64) -> Self {
        Live {
            doc,
            me: me.into(),
            can_edit,
            outbox: Vec::new(),
            pending: Vec::new(),
            clips: HashMap::new(),
            seq,
            applying: false,
            watching: None,
        }
    }

    /// How many of your changes are still on their way.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

impl Session {
    /// Start editing the active workbook together with others (`me`: the host's id for this person).
    pub fn live_start(&mut self, me: &str, can_edit: bool, seq: u64) -> Result<()> {
        let doc = self.active().ok_or(EngineError::NoDocument)?.uid;
        self.live = Some(Live::new(doc, me, can_edit, seq));
        Ok(())
    }

    /// Changes made here since the last call, to send to the host.
    pub fn live_take_outbox(&mut self) -> Vec<Json> {
        self.live.as_mut().map(|l| std::mem::take(&mut l.outbox)).unwrap_or_default()
    }

    /// Before a top-level command: `Some` answers it without running it here (refused, view only,
    /// or Undo/Redo, which wait for the host); `None` runs it.
    pub(crate) fn live_before(&mut self, spec: &CommandSpec, id: &str, params: &Json) -> Option<Result<Json>> {
        let doc = self.active()?.uid;
        let live = self.live.as_ref()?;
        if live.applying || live.doc != doc || !shared(spec) {
            return None;
        }
        if refused(id) {
            return Some(Err(EngineError::Disabled(id.into(), "not available while editing together".into())));
        }
        if !live.can_edit {
            // A viewer copies for themselves; nothing else changes their copy.
            return if id == "edit.copy" { None } else { Some(Err(EngineError::Disabled(id.into(), "view only".into()))) };
        }
        if live.pending.len() >= MAX_PENDING {
            return Some(Err(EngineError::Disabled(id.into(), "waiting for the connection to catch up".into())));
        }
        let d = self.active()?;
        let op = json!({ "id": id, "params": params, "sheet": d.wb.active_sheet(), "selection": d.selection });
        if ordered_only(id) {
            self.live.as_mut()?.outbox.push(op);
            return Some(Ok(json!({ "live": "sent" })));
        }
        if clipboard_only(id) {
            self.live.as_mut()?.outbox.push(op);
            return None;
        }
        let base = Base { wb: d.wb.clone(), selection: d.selection.clone(), undo: d.undo.clone(), redo: d.redo.clone() };
        let clip = self.clipboard.clone();
        self.live.as_mut()?.watching = Some((op, clip, base));
        None
    }

    /// After a top-level command: a change to the shared workbook goes to the host.
    pub(crate) fn live_after(&mut self) {
        let Some((op, clip, base)) = self.live.as_mut().and_then(|l| l.watching.take()) else { return };
        let changed = self.active().is_some_and(|d| !Arc::ptr_eq(&d.wb, &base.wb));
        if changed && let Some(live) = self.live.as_mut() {
            live.outbox.push(op.clone());
            live.pending.push(Pending { op, clip, base });
        }
    }

    /// Apply the host's next change (`seq`, made by `user`). Changes arrive in order; one already
    /// applied is skipped.
    pub fn live_apply(&mut self, seq: u64, user: &str, op: &Json) -> std::result::Result<(), String> {
        let Some(live) = self.live.as_ref() else { return Err("not editing together".into()) };
        if seq <= live.seq {
            return Ok(());
        }
        let doc = live.doc;
        let mine = user == live.me;
        let id = op.get("id").and_then(Json::as_str).ok_or("a change without a command")?.to_string();
        let spec = find_command(&id).ok_or_else(|| format!("unknown command {id}"))?;
        if !shared(spec) || refused(&id) {
            return Err(format!("{id} is not a change to the shared workbook"));
        }
        if self.active().map(|d| d.uid) != Some(doc) {
            return Err("the shared workbook is not the active one".into());
        }
        // Your own change, numbered: it already shows here (Copy and Cut ran here too).
        if mine && !ordered_only(&id) {
            if let Some(live) = self.live.as_mut() {
                if !clipboard_only(&id) && !live.pending.is_empty() {
                    live.pending.remove(0);
                }
                live.seq = seq;
            }
            return Ok(());
        }
        let requests = self.ui_requests.len();
        let place = self.active().map(|d| (d.wb.active_sheet(), d.selection.clone()));
        // This person's zoom per sheet: putting the workbook back below must not undo it.
        let zooms: Vec<u16> = self.active().map(|d| d.wb.sheets.iter().map(|sh| sh.zoom).collect()).unwrap_or_default();
        let pending = self.live.as_mut().map(|l| std::mem::take(&mut l.pending)).unwrap_or_default();
        // Yours go back to before the first of them…
        if let Some(first) = pending.first() {
            self.live_restore(&first.base);
        }
        // …theirs runs where it was made, with their clipboard…
        self.live_run(&id, op, if mine { None } else { Some(user) });
        // …and yours run again on top.
        let mut again = Vec::with_capacity(pending.len());
        for p in pending {
            let Some(d) = self.active() else { break };
            let base = Base { wb: d.wb.clone(), selection: d.selection.clone(), undo: d.undo.clone(), redo: d.redo.clone() };
            let own = std::mem::replace(&mut self.clipboard, p.clip.clone());
            let pid = p.op.get("id").and_then(Json::as_str).unwrap_or_default().to_string();
            self.live_run(&pid, &p.op, None);
            self.clipboard = own;
            again.push(Pending { op: p.op, clip: p.clip, base });
        }
        // Back to where this person was; nobody else's change pops a question here.
        if let Some((sheet, selection)) = place
            && let Some(d) = self.active_mut()
        {
            if sheet < d.wb.sheets.len() {
                d.wb.set_active_sheet(sheet);
            }
            d.selection = selection;
            if d.wb.sheets.iter().zip(&zooms).any(|(sh, z)| sh.zoom != *z) {
                let wb = Arc::make_mut(&mut d.wb);
                for (i, z) in zooms.iter().enumerate() {
                    if let Some(sh) = wb.sheet_mut(i)
                        && sh.zoom != *z
                    {
                        sh.zoom = *z;
                    }
                }
            }
        }
        self.ui_requests.truncate(requests);
        if let Some(live) = self.live.as_mut() {
            live.pending = again;
            live.seq = seq;
        }
        Ok(())
    }

    /// Put the shared workbook back to `base` (before your waiting changes).
    fn live_restore(&mut self, base: &Base) {
        if let Some(d) = self.active_mut() {
            if !Arc::ptr_eq(&d.wb, &base.wb) {
                d.wb = base.wb.clone();
                d.calc.rebuild(&d.wb);
            }
            d.selection = base.selection.clone();
            d.undo = base.undo.clone();
            d.redo = base.redo.clone();
            d.revision += 1;
        }
    }

    /// Run one change in the place it was made: its sheet and selection, and (for another
    /// person's change) their clipboard. Errors are the same on every copy, so they are dropped.
    fn live_run(&mut self, id: &str, op: &Json, clipboard_of: Option<&str>) {
        if let Some(d) = self.active_mut() {
            if let Some(sheet) = op.get("sheet").and_then(Json::as_u64).map(|s| s as usize)
                && sheet < d.wb.sheets.len()
            {
                d.wb.set_active_sheet(sheet);
            }
            if let Some(sel) = op.get("selection").and_then(|s| serde_json::from_value::<Selection>(s.clone()).ok()) {
                d.selection = sel;
            }
        }
        let theirs = clipboard_of.map(|u| {
            let clip = self.live.as_mut().and_then(|l| l.clips.remove(u)).flatten();
            (u.to_string(), std::mem::replace(&mut self.clipboard, clip))
        });
        if let Some(l) = self.live.as_mut() {
            l.applying = true;
        }
        let params = op.get("params").cloned().unwrap_or(Json::Null);
        let _ = self.execute(id, params);
        if let Some(l) = self.live.as_mut() {
            l.applying = false;
        }
        if let Some((user, own)) = theirs {
            let clip = std::mem::replace(&mut self.clipboard, own);
            if let Some(l) = self.live.as_mut() {
                l.clips.insert(user, clip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A copy of the shared workbook for `me`.
    fn copy(me: &str, can_edit: bool) -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({})).unwrap();
        s.live_start(me, can_edit, 0).unwrap();
        s
    }

    /// The host: number every waiting change in the order given and send it to everyone.
    fn host(seq: &mut u64, copies: &mut [(&str, &mut Session)], order: &[usize]) {
        let mut sent: Vec<(String, Json)> = Vec::new();
        for &i in order {
            let (name, s) = &mut copies[i];
            for op in s.live_take_outbox() {
                sent.push((name.to_string(), op));
            }
        }
        for (user, op) in sent {
            *seq += 1;
            for (_, s) in copies.iter_mut() {
                s.live_apply(*seq, &user, &op).unwrap();
            }
        }
    }

    fn value(s: &mut Session, cell: &str) -> Json {
        s.execute("cell.get", json!({ "cell": cell })).unwrap()["value"].clone()
    }

    fn set(s: &mut Session, cell: &str, input: &str) {
        s.execute("cell.set", json!({ "cell": cell, "input": input })).unwrap();
    }

    fn select(s: &mut Session, range: &str) {
        s.execute("selection.set", json!({ "range": range })).unwrap();
    }

    #[test]
    fn changes_show_at_once_and_end_up_the_same_everywhere() {
        let (mut a, mut b) = (copy("a", true), copy("b", true));
        set(&mut a, "A1", "1");
        set(&mut b, "A1", "2");
        set(&mut b, "B1", "=A1*10");
        assert_eq!(value(&mut a, "A1"), json!(1.0), "your own change shows at once");
        let mut seq = 0;
        // The host takes a's change first: b's two go back, a's runs, b's run again on top.
        host(&mut seq, &mut [("a", &mut a), ("b", &mut b)], &[0, 1]);
        for s in [&mut a, &mut b] {
            assert_eq!(value(s, "A1"), json!(2.0));
            assert_eq!(value(s, "B1"), json!(20.0));
            assert_eq!(s.live.as_ref().unwrap().pending(), 0);
            assert_eq!(s.live.as_ref().unwrap().seq, 3);
        }
    }

    #[test]
    fn a_paste_pastes_what_that_person_copied_everywhere() {
        let (mut a, mut b) = (copy("a", true), copy("b", true));
        set(&mut a, "A1", "hello");
        select(&mut a, "A1");
        a.execute("edit.copy", json!({})).unwrap();
        // b copies something else meanwhile: the paste below still pastes a's cells everywhere
        set(&mut b, "D4", "mine");
        select(&mut b, "D4");
        b.execute("edit.copy", json!({})).unwrap();
        select(&mut a, "C3");
        a.execute("edit.paste", json!({})).unwrap();
        let mut seq = 0;
        host(&mut seq, &mut [("a", &mut a), ("b", &mut b)], &[1, 0]);
        for s in [&mut a, &mut b] {
            assert_eq!(value(s, "C3"), json!("hello"));
            assert_eq!(value(s, "D4"), json!("mine"));
        }
        // b's own clipboard is still b's
        select(&mut b, "E5");
        b.execute("edit.paste", json!({})).unwrap();
        host(&mut seq, &mut [("a", &mut a), ("b", &mut b)], &[1]);
        assert_eq!(value(&mut a, "E5"), json!("mine"));
    }

    #[test]
    fn undo_is_shared_and_waits_for_its_turn() {
        let (mut a, mut b) = (copy("a", true), copy("b", true));
        let mut seq = 0;
        set(&mut a, "A1", "1");
        host(&mut seq, &mut [("a", &mut a), ("b", &mut b)], &[0]);
        set(&mut b, "A2", "2");
        host(&mut seq, &mut [("a", &mut a), ("b", &mut b)], &[1]);
        // a undoes: the last change in the shared workbook is b's
        a.execute("edit.undo", json!({})).unwrap();
        assert_eq!(value(&mut a, "A2"), json!(2.0), "undo waits for the host");
        host(&mut seq, &mut [("a", &mut a), ("b", &mut b)], &[0]);
        for s in [&mut a, &mut b] {
            assert_eq!(value(s, "A2"), Json::Null);
            assert_eq!(value(s, "A1"), json!(1.0));
        }
    }

    #[test]
    fn people_stay_where_they_were() {
        let (mut a, mut b) = (copy("a", true), copy("b", true));
        select(&mut b, "Z9");
        set(&mut a, "A1", "x");
        let mut seq = 0;
        host(&mut seq, &mut [("a", &mut a), ("b", &mut b)], &[0]);
        assert_eq!(b.active().unwrap().selection.a1(), "Z9");
    }

    #[test]
    fn zoom_is_each_persons_own_and_never_a_change() {
        let (mut a, mut b) = (copy("a", true), copy("b", true));
        // A pinch, Ctrl+wheel and the zoom slider all run view.zoom.
        for id in ["view.zoom", "view.zoom100", "view.zoomToSelection"] {
            a.execute(id, json!({ "percent": 150 })).unwrap();
        }
        a.execute("view.zoom", json!({ "percent": 175 })).unwrap();
        assert!(a.live_take_outbox().is_empty(), "zoom sends nothing to the host");
        assert_eq!(a.live.as_ref().unwrap().pending(), 0, "zoom leaves nothing waiting");
        let mut v = copy("v", false);
        assert!(v.execute("view.zoom", json!({ "percent": 50 })).is_ok(), "a viewer may zoom");
        assert!(v.live_take_outbox().is_empty());
        // The host never runs a zoom either.
        let op = json!({ "id": "view.zoom", "params": { "percent": 300 }, "sheet": 0, "selection": b.active().unwrap().selection });
        assert!(b.live_apply(1, "a", &op).is_err());
        // A real edit is still a change, and someone else's change (taken first, so a's edit is put
        // back and run again) keeps a's zoom.
        set(&mut a, "A1", "1");
        assert_eq!(a.live.as_ref().unwrap().pending(), 1, "an edit goes to the host");
        set(&mut b, "B1", "2");
        let mut seq = 0;
        host(&mut seq, &mut [("a", &mut a), ("b", &mut b)], &[1, 0]);
        assert_eq!(seq, 2, "the two edits, and no zoom");
        assert_eq!(value(&mut a, "B1"), json!(2.0));
        assert_eq!(value(&mut a, "A1"), json!(1.0));
        assert_eq!(a.active().unwrap().wb.sheets[0].zoom, 175, "someone else's change keeps your zoom");
        assert_eq!(b.active().unwrap().wb.sheets[0].zoom, 100, "and nobody else's zoom changed");
    }

    #[test]
    fn viewers_and_scripts_change_nothing() {
        let mut v = copy("v", false);
        assert!(v.execute("cell.set", json!({ "cell": "A1", "input": "1" })).is_err());
        assert!(v.execute("edit.copy", json!({})).is_ok(), "a viewer may copy for themselves");
        assert!(v.live_take_outbox().is_empty());
        let mut a = copy("a", true);
        let op = json!({ "id": "file.close", "params": {}, "sheet": 0, "selection": a.active().unwrap().selection });
        assert!(a.live_apply(1, "x", &op).is_err(), "only editing commands come from the host");
    }
}
