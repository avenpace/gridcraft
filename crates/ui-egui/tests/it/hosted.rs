//! Hosted by Clawku (`Services::hosted`): the app shows the one workbook the page sent. Save and
//! Save As hand it to the page under its own name, copies go to the page under other names, Share
//! is the page's, and nothing makes or shows another workbook.

use std::sync::{Arc, Mutex};

use egui::{Event, Key, Modifiers};
use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_ui_egui::backstage::Page;
use gridcraft_ui_egui::{Inbox, Services, SheetApp};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;
type Files = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
/// Saves the page was handed: (name, bytes, saveAs).
type Saves = Arc<Mutex<Vec<(String, Vec<u8>, bool)>>>;

/// What the hosting page was handed.
#[derive(Default, Clone)]
struct HostPage {
    /// Save / Save As: (name, bytes, saveAs).
    saves: Saves,
    /// Copies and exports.
    copies: Files,
    shares: Arc<Mutex<usize>>,
    /// Data › Get Data picks (the browser's picker), by the inbox they deliver to.
    imports: Arc<Mutex<Vec<Inbox>>>,
}

const CTRL: Modifiers = Modifiers { alt: false, ctrl: true, shift: false, mac_cmd: false, command: true };

/// A hosted app showing "Budget.xlsx" (A1 = 1), as the page opened it.
fn hosted() -> (HostPage, Harness) {
    let page = HostPage::default();
    let (saves, copies, shares, imports) = (page.saves.clone(), page.copies.clone(), page.shares.clone(), page.imports.clone());
    let inbox = Inbox::default();
    let services = Services {
        hosted: true,
        host_save: Some(Box::new(move |name, bytes, save_as| saves.lock().unwrap().push((name.to_string(), bytes.to_vec(), save_as)))),
        download: Some(Box::new(move |name, bytes| copies.lock().unwrap().push((name.to_string(), bytes.to_vec())))),
        share: Some(Box::new(move || *shares.lock().unwrap() += 1)),
        pick_import_async: Some(Box::new(move |inbox| imports.lock().unwrap().push(inbox))),
        inbox: Some(inbox.clone()),
        ..Default::default()
    };
    let mut h = crate::support::services_harness(services, 4);
    inbox.lock().unwrap().push(("Budget.xlsx".into(), workbook_bytes(1.0)));
    h.run_steps(3);
    (page, h)
}

fn workbook_bytes(a1: f64) -> Vec<u8> {
    let mut s = gridcraft_engine::Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": a1.to_string()})).unwrap();
    gridcraft_engine::io::save_bytes(&s.active().unwrap().wb, "Budget.xlsx").unwrap()
}

fn edit(h: &mut Harness, input: &str) {
    h.state_mut().run("cell.set", json!({"cell": "A1", "input": input})).unwrap();
    h.run_steps(2);
}

fn key(h: &mut Harness, k: Key, m: Modifiers) {
    h.input_mut().events.push(Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: m });
    h.input_mut().events.push(Event::Key { key: k, physical_key: Some(k), pressed: false, repeat: false, modifiers: m });
    h.run_steps(2);
}

fn button(h: &mut Harness, label: &str) {
    h.get_by_role_and_label(egui::accesskit::Role::Button, label).click();
    h.run_steps(3);
}

fn has_button(h: &Harness, label: &str) -> bool {
    h.query_all_by_role_and_label(egui::accesskit::Role::Button, label).next().is_some()
}

fn a1_of(bytes: &[u8]) -> Value {
    let (wb, _) = gridcraft_engine::io::open_bytes("Budget.xlsx", bytes).unwrap();
    wb.active().unwrap().value(CellRef::new(0, 0))
}

#[test]
fn the_page_s_workbook_is_the_only_one_and_another_open_replaces_it() {
    let (_, mut h) = hosted();
    assert_eq!(h.state().session.documents().len(), 1, "the page's workbook replaced the blank one");
    assert_eq!(h.state().session.active().unwrap().title, "Budget.xlsx");
    h.state_mut().services.inbox.as_ref().unwrap().lock().unwrap().push(("Other.xlsx".into(), workbook_bytes(2.0)));
    h.run_steps(3);
    let titles: Vec<String> = h.state().session.documents().iter().map(|d| d.title.clone()).collect();
    assert_eq!(titles, ["Other.xlsx"], "the page shows one workbook at a time");
}

#[test]
fn save_overwrites_the_page_s_file_under_its_own_name() {
    let (page, mut h) = hosted();
    // File › Save: no Save As page, no download.
    edit(&mut h, "2");
    h.state_mut().open_backstage(Page::Info);
    h.run_steps(2);
    button(&mut h, "Save");
    assert!(h.state().backstage.is_none(), "Save goes straight back to the sheet");
    // The title bar's Save button (it runs `file.save`) and Ctrl+S.
    edit(&mut h, "3");
    h.state_mut().run_or_alert("file.save", json!({}));
    edit(&mut h, "4");
    key(&mut h, Key::S, CTRL);
    let saves = page.saves.lock().unwrap();
    assert_eq!(saves.len(), 3, "{:?}", saves.iter().map(|s| (&s.0, s.2)).collect::<Vec<_>>());
    for (i, (name, bytes, save_as)) in saves.iter().enumerate() {
        assert_eq!(name, "Budget.xlsx", "the name the page opened it with");
        assert!(!save_as, "Save overwrites the page's file");
        assert_eq!(a1_of(bytes), Value::Number(i as f64 + 2.0));
    }
    assert!(page.copies.lock().unwrap().is_empty(), "nothing was downloaded");
    assert!(!h.state().session.active().unwrap().is_dirty(), "a saved workbook is clean");
}

#[test]
fn save_as_lets_the_page_ask_for_the_new_name() {
    let (page, mut h) = hosted();
    edit(&mut h, "5");
    key(&mut h, Key::F12, Modifiers::NONE);
    h.state_mut().open_backstage(Page::SaveAs);
    h.run_steps(2);
    assert!(h.query_by_label("Download (.xlsx)").is_none(), "no download wording");
    button(&mut h, "Save As (.xlsx)");
    let saves = page.saves.lock().unwrap();
    assert_eq!(saves.len(), 2);
    for (name, bytes, save_as) in saves.iter() {
        assert_eq!(name, "Budget.xlsx");
        assert!(*save_as, "Save As");
        assert_eq!(a1_of(bytes), Value::Number(5.0));
    }
    assert!(page.copies.lock().unwrap().is_empty());
}

#[test]
fn copies_and_exports_go_to_the_page_under_other_names() {
    let (page, mut h) = hosted();
    edit(&mut h, "6");
    h.state_mut().open_dialog("saveCopy", json!({"format": "xlsx"}));
    h.state_mut().open_dialog("saveCopy", json!({"format": "csv"}));
    h.state_mut().run_or_alert("file.exportPdf", json!({}));
    h.run_steps(2);
    let copies = page.copies.lock().unwrap();
    let names: Vec<&str> = copies.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(names, ["Budget (copy).xlsx", "Budget.csv", "Budget.pdf"]);
    assert_eq!(a1_of(&copies[0].1), Value::Number(6.0));
    assert!(copies[2].1.starts_with(b"%PDF-"));
    assert!(page.saves.lock().unwrap().is_empty(), "a copy is not a Save");
    let d = h.state().session.active().unwrap();
    assert!(d.is_dirty() && d.title == "Budget.xlsx", "the workbook keeps its name and unsaved changes");
}

#[test]
fn share_is_the_page_s() {
    let (page, mut h) = hosted();
    button(&mut h, "Share");
    assert_eq!(*page.shares.lock().unwrap(), 1, "Share asks the page");
    assert!(h.query_by_label("Save a Copy as Excel Workbook (.xlsx)…").is_none(), "no export menu in its place");
}

#[test]
fn nothing_makes_or_shows_another_workbook() {
    let (_, mut h) = hosted();
    h.state_mut().ui.ribbon_tab = "View".into();
    h.run_steps(2);
    assert!(has_button(&h, "Freeze Panes"), "the View tab is showing");
    assert!(!has_button(&h, "New Window") && !has_button(&h, "Arrange All"), "no other windows");
    for (k, m) in [(Key::N, CTRL), (Key::W, CTRL), (Key::F4, CTRL)] {
        key(&mut h, k, m);
    }
    h.state_mut().open_dialog("start", json!({}));
    h.run_steps(2);
    assert_eq!(h.state().session.documents().len(), 1, "Ctrl+N, Ctrl+W and Ctrl+F4 do nothing");
    assert!(h.state().dialog.is_none(), "no start screen and no Save Changes question");
    // File opens on Info: no Home, New, Close, Account or recent workbooks.
    h.state_mut().ui.recent = vec!["/tmp/Old.xlsx".into()];
    h.state_mut().open_backstage(Page::Home);
    h.run_steps(3);
    assert_eq!(h.state().backstage.as_ref().map(|b| b.page), Some(Page::Info));
    for hidden in ["Home", "New", "Close", "Account", "Old.xlsx", "Not saved yet", "Location"] {
        assert!(h.query_by_label(hidden).is_none(), "{hidden} is not offered when hosted");
    }
    button(&mut h, "Open");
    assert!(h.query_by_label("Old.xlsx").is_none(), "Open lists no recent workbooks");
}

#[test]
fn get_data_imports_into_this_workbook_with_the_browser_picker() {
    let (page, mut h) = hosted();
    h.state_mut().ui.ribbon_tab = "Data".into();
    h.run_steps(2);
    button(&mut h, "Get Data (Text/CSV)");
    let inbox = page.imports.lock().unwrap().pop().expect("the browser's picker was asked");
    inbox.lock().unwrap().push(("Sales.csv".into(), b"Region,Total\nNorth,10\n".to_vec()));
    h.run_steps(3);
    let d = h.state().session.active().unwrap();
    assert_eq!(h.state().session.documents().len(), 1, "no workbook of its own");
    assert_eq!(d.title, "Budget.xlsx");
    assert!(d.wb.sheets.iter().any(|s| s.name == "Sales"), "a new sheet named after the file");
}
