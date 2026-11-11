use super::Dialog;

/// Help ▸ About ClawSheet: the app, its contributors and the AI models that helped. The credits
/// are compiled in (`crate::credits`, docs/contributors.md). Hosted (Clawku), it is the product
/// name and version only: the contributors' GitHub pages and the community link are the
/// standalone app's.
pub(super) fn about(ui: &mut egui::Ui, d: &mut Dialog, hosted: bool) {
    let tabs: &[&str] = if hosted { &["About", "Models"] } else { &["About", "Contributors", "Models"] };
    ui.horizontal(|ui| {
        for tab in tabs {
            if ui.selectable_label(d.tab == *tab, *tab).clicked() {
                d.tab = tab.to_string();
            }
        }
    });
    ui.separator();
    match d.tab.as_str() {
        "Contributors" if !hosted => crate::credits::contributors_ui(ui),
        "Models" => crate::credits::models_ui(ui),
        _ => {
            ui.heading("ClawSheet");
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.add_space(4.0);
            ui.label("A clean-room, open-source, Rust-native spreadsheet. Based on the ArtCraft Crafting Apps (MIT OR Apache-2.0).");
            if !hosted {
                ui.hyperlink_to("clawku.id", "https://clawku.id/");
                ui.hyperlink_to("Join the Clawku community", "https://clawku.id/");
            }
        }
    }
}
