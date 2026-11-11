//! Clawku: File › Open asks the host page for a file from its own storage instead of opening the
//! browser's file picker. Opt-in: only a page that loads the app with `?host=parent&open=host`
//! (Clawku) gets `<app>:pick` (`{ type, purpose }`) on the same-origin parent; every other page
//! keeps the browser picker. The host answers with the usual `<app>:open` message, or not at all.
//! The same page also owns Share: the title bar's Share button posts `<app>:share` (`{ type }`).

use wasm_bindgen::JsValue;

/// Whether this page is hosted by Clawku: `?host=parent&open=host`, framed. Clawku then owns the
/// file list, New/Open/Close, sharing and sign-in (`Services::hosted`).
pub fn hosted() -> bool {
    parent().is_some()
}

/// The framing parent of a `?host=parent&open=host` page, with this page's origin.
fn parent() -> Option<(web_sys::Window, String)> {
    let win = web_sys::window()?;
    let search = win.location().search().unwrap_or_default();
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    if params.get("host").as_deref() != Some("parent") || params.get("open").as_deref() != Some("host") {
        return None;
    }
    let parent = win.parent().ok()??;
    if js_sys::Object::is(&parent, &win) {
        return None; // not framed
    }
    let origin = win.location().origin().ok()?;
    Some((parent, origin))
}

/// Post `{ type: "<app>:<kind>", ...fields }` to the host; `false` when this page isn't hosted.
fn post(app: &str, kind: &str, fields: &[(&str, &str)]) -> bool {
    let Some((parent, origin)) = parent() else { return false };
    let msg = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str(&format!("{app}:{kind}")));
    for (key, value) in fields {
        let _ = js_sys::Reflect::set(&msg, &JsValue::from_str(key), &JsValue::from_str(value));
    }
    parent.post_message(&msg, &origin).is_ok()
}

/// Ask the host for a file to open; `false` when this page isn't hosted that way (use the picker).
pub fn host_pick(app: &str, purpose: &str) -> bool {
    post(app, "pick", &[("purpose", purpose)])
}

/// Ask the host to share the open document (its own Share dialog); `false` when not hosted.
pub fn host_share(app: &str) -> bool {
    post(app, "share", &[])
}
