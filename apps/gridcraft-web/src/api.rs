//! `window.gridcraft`: the page command API (`craft-web-api`, storytold/craft-libs#413).
//!
//! The crate installs `request`, `execute`, `commands`, `docInspect`, `uiInspect` and `screenshot`
//! over the same JSON methods as the desktop control server (`docs/control-protocol.md`), as
//! promises with a timeout, so an agent driving GridCraft in a browser tab (or an `<iframe>` in
//! another app) uses the same commands as the CLI and the MCP server:
//!
//! ```js
//! await gridcraft.execute("cell.set", {cell: "B2", input: "=SUM(A1:A9)"})  // engine command → its result
//! await gridcraft.request("document.inspect")                             // control method → its result
//! await gridcraft.commands()                                              // command list
//! const {base64} = await gridcraft.screenshot()                           // the sheet as PNG (base64)
//! ```
//!
//! A request goes onto the channel whose receiver is `SheetApp::control_rx`; the app's own
//! per-frame drain of it (`control::poll`, the same one the desktop control server feeds) runs the
//! request and answers. Each call polls that answer without blocking until it arrives: results
//! resolve with the method's `result`, errors reject with an `Error`.
//!
//! Installed only when the page loads the app with `?control` (crate::web).

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use craft_web_api::methods::Methods;
use craft_web_api::page::{PageApi, Request};
use gridcraft_ui_egui::{ControlRequest, ControlResponse};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// The app id in `window.<app>`.
const APP: &str = "gridcraft";

/// How often a waiting call looks for the app's reply, in milliseconds (about a frame).
const POLL_MS: i32 = 16;

thread_local! {
    /// The app's control channel, whose receiver is `SheetApp::control_rx`.
    static TX: RefCell<Option<Sender<ControlRequest>>> = const { RefCell::new(None) };
    static CTX: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// Give the API the app's control channel (`rx`, the same channel's receiver, goes to
/// `SheetApp::control_rx`) and context: the page calls between frames, so each request asks for
/// one. Call before [`install`], which uses what it sent.
pub fn connect(ctx: egui::Context, tx: Sender<ControlRequest>) {
    CTX.with(|c| *c.borrow_mut() = Some(ctx));
    TX.with(|t| *t.borrow_mut() = Some(tx));
}

/// Install `window.gridcraft`: the crate's `request`, `execute`, `commands`, `docInspect`,
/// `uiInspect` and `screenshot` over this app's control channel.
pub fn install() -> Result<(), JsValue> {
    let request: Request = Rc::new(|method, params| send(&method, params));
    Rc::new(PageApi::new(APP, Methods::default(), request)).install()
}

/// A queued control request whose reply has not arrived. Dropping it before the reply (the page's
/// timeout fired) cancels the request if the app has not started it: the page was told it failed,
/// so the app must not run it late (#1914).
struct Waiting {
    reply: Receiver<ControlResponse>,
    gate: Arc<AtomicU8>,
    answered: bool,
}

impl Waiting {
    /// The app's reply once it has answered or gone away; `None` while the request is still
    /// waiting. The control channel replies with the protocol's whole `{ok, result}` /
    /// `{ok: false, error}` object, so it needs no re-wrapping.
    fn poll(&mut self) -> Option<Value> {
        match self.reply.try_recv() {
            Ok(reply) => {
                self.answered = true;
                Some(reply)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.answered = true;
                Some(json!({"ok": false, "error": "request dropped"}))
            }
        }
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        if !self.answered {
            ControlRequest::abandon(&self.gate);
        }
    }
}

/// Queue a control request on the app's control channel and wait for its reply, as the protocol's
/// `{ok, result}` / `{ok: false, error}` object.
fn send(method: &str, params: Value) -> Pin<Box<dyn Future<Output = Value>>> {
    let (req, rx) = ControlRequest::new(method, params);
    let gate = Arc::clone(&req.state);
    // A send that fails (the app is gone) drops the request with its reply sender, so the wait
    // below answers "request dropped" on its first poll instead of hanging.
    if TX.with(|t| t.borrow().as_ref().is_some_and(|tx| tx.send(req).is_ok())) {
        CTX.with(|c| {
            if let Some(ctx) = c.borrow().as_ref() {
                ctx.request_repaint();
            }
        });
    }
    let mut waiting = Waiting { reply: rx, gate, answered: false };
    Box::pin(async move {
        loop {
            if let Some(reply) = waiting.poll() {
                return reply;
            }
            sleep(POLL_MS).await;
        }
    })
}

/// Yield to the page's event loop for `ms` milliseconds: the wait must not block the app's frame.
async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = JsFuture::from(promise).await;
}
