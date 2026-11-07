//! Clawku live co-editing: the page side of [`gridcraft_engine::live`].
//!
//! Messages with the same-origin parent page (Clawku), on top of the host bridge ([`crate::host`]):
//!
//! - `gridcraft:liveOpen` (from the page): `{ name, bytes, me, canEdit, seq }`. Open this workbook
//!   (as `gridcraft:open` does) and edit it together with others; `seq` is the last change already
//!   in `bytes`. Once it is open the app answers `gridcraft:liveReady` `{ seq }`; send the changes
//!   after `seq` then.
//! - `gridcraft:apply` (from the page): `{ changes: [{ seq, user, change }] }`, in order. A change
//!   that can't be applied is reported with `gridcraft:liveError` `{ seq, error }`: reload then.
//! - `gridcraft:changes` (to the page): `{ changes: [change] }`, this person's new changes.
//! - `gridcraft:liveState` (to the page): `{ seq, pending }` whenever either changes: the last
//!   change applied, and how many of this person's changes are still on their way. A save made
//!   while `pending` is 0 holds exactly the changes up to `seq`.
//! - `gridcraft:saveNow` (from the page): save the workbook (it arrives as `gridcraft:save`).

use std::cell::RefCell;
use std::rc::Rc;

use gridcraft_ui_egui::{Inbox, SheetApp};
use serde_json::{Value as Json, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

/// The largest workbook a `liveOpen` may carry (as for `gridcraft:open`).
const MAX_OPEN_BYTES: f64 = (512u64 << 20) as f64;

/// One message from the page, waiting for the next frame.
enum FromPage {
    Start { name: String, me: String, can_edit: bool, seq: u64 },
    Apply(Vec<(u64, String, Json)>),
    SaveNow,
}

#[derive(Clone, Default)]
pub struct LiveLink {
    queue: Rc<RefCell<Vec<FromPage>>>,
    /// Waiting for the shared workbook to open: (its name, me, can edit, seq).
    starting: Rc<RefCell<Option<(String, String, bool, u64)>>>,
    /// What the page last heard: (seq, pending).
    reported: Rc<RefCell<Option<(u64, usize)>>>,
}

impl LiveLink {
    /// Hear the live messages from the same-origin parent page.
    pub fn listen(&self, inbox: Inbox, ctx: egui::Context) {
        let Some(win) = web_sys::window() else { return };
        let Ok(Some(parent)) = win.parent() else { return };
        let Ok(origin) = win.location().origin() else { return };
        let queue = self.queue.clone();
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            if e.origin() != origin {
                return;
            }
            // Only the parent page talks to us.
            let from_parent = e.source().is_some_and(|s| js_sys::Object::is(&s, parent.as_ref()));
            if !from_parent {
                return;
            }
            let data = e.data();
            let kind = js_sys::Reflect::get(&data, &"type".into()).ok().and_then(|t| t.as_string()).unwrap_or_default();
            match kind.as_str() {
                "gridcraft:liveOpen" => {
                    let Some(name) = js_sys::Reflect::get(&data, &"name".into()).ok().and_then(|v| v.as_string()) else { return };
                    let Some(raw) = js_sys::Reflect::get(&data, &"bytes".into()).ok() else { return };
                    let bytes = if raw.is_instance_of::<js_sys::Uint8Array>() {
                        js_sys::Uint8Array::new(&raw)
                    } else if raw.is_instance_of::<js_sys::ArrayBuffer>() {
                        js_sys::Uint8Array::new(&raw)
                    } else {
                        return;
                    };
                    if f64::from(bytes.length()) > MAX_OPEN_BYTES {
                        log::warn!("live workbook over the open limit; not opened");
                        return;
                    }
                    let me = js_sys::Reflect::get(&data, &"me".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
                    let can_edit = js_sys::Reflect::get(&data, &"canEdit".into()).ok().and_then(|v| v.as_bool()).unwrap_or(false);
                    let seq = js_sys::Reflect::get(&data, &"seq".into()).ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as u64;
                    if me.is_empty() {
                        return;
                    }
                    let name = crate::hostmode::open_name(&name);
                    inbox.lock().unwrap_or_else(|p| p.into_inner()).push((name.clone(), bytes.to_vec()));
                    queue.borrow_mut().push(FromPage::Start { name, me, can_edit, seq });
                }
                "gridcraft:apply" => {
                    let Some(text) = js_sys::JSON::stringify(&data).ok().and_then(|s| s.as_string()) else { return };
                    let Ok(v) = serde_json::from_str::<Json>(&text) else { return };
                    let changes = v["changes"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|c| Some((c["seq"].as_u64()?, c["user"].as_str()?.to_string(), c["change"].clone())))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    queue.borrow_mut().push(FromPage::Apply(changes));
                }
                "gridcraft:saveNow" => queue.borrow_mut().push(FromPage::SaveNow),
                _ => return,
            }
            ctx.request_repaint();
        });
        let _ = win.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref());
        on_message.forget();
    }

    /// Every frame: start once the shared workbook is open, apply the page's changes, send ours.
    pub fn sync(&self, app: &mut SheetApp) {
        let messages = std::mem::take(&mut *self.queue.borrow_mut());
        for m in messages {
            match m {
                FromPage::Start { name, me, can_edit, seq } => {
                    app.session.live = None;
                    *self.starting.borrow_mut() = Some((name, me, can_edit, seq));
                    *self.reported.borrow_mut() = None;
                }
                FromPage::Apply(changes) => {
                    for (seq, user, change) in changes {
                        if app.session.live.is_none() {
                            break;
                        }
                        if let Err(error) = app.session.live_apply(seq, &user, &change) {
                            log::warn!("live change {seq} not applied: {error}");
                            post("gridcraft:liveError", json!({ "seq": seq, "error": error }));
                            app.session.live = None;
                            break;
                        }
                    }
                }
                FromPage::SaveNow => app.run_or_alert("file.save", json!({})),
            }
        }
        // The shared workbook opened (it is the active one, under its name): editing together starts.
        let ready = self.starting.borrow().as_ref().is_some_and(|(name, ..)| app.session.active().is_some_and(|d| d.title == *name));
        if ready && let Some((_, me, can_edit, seq)) = self.starting.borrow_mut().take() {
            match app.session.live_start(&me, can_edit, seq) {
                Ok(()) => post("gridcraft:liveReady", json!({ "seq": seq })),
                Err(e) => post("gridcraft:liveError", json!({ "seq": seq, "error": e.to_string() })),
            }
        }
        let changes = app.session.live_take_outbox();
        if !changes.is_empty() {
            post("gridcraft:changes", json!({ "changes": changes }));
        }
        if let Some(live) = &app.session.live {
            let now = (live.seq, live.pending());
            if *self.reported.borrow() != Some(now) {
                *self.reported.borrow_mut() = Some(now);
                post("gridcraft:liveState", json!({ "seq": now.0, "pending": now.1 }));
            }
        }
    }
}

/// Post `{ type, ...body }` to the same-origin parent page.
fn post(kind: &str, mut body: Json) {
    let Some(win) = web_sys::window() else { return };
    let Ok(Some(parent)) = win.parent() else { return };
    let Ok(origin) = win.location().origin() else { return };
    body["type"] = json!(kind);
    let Ok(value) = js_sys::JSON::parse(&body.to_string()) else { return };
    let _ = parent.post_message(&value, &origin);
}
