//! Fleet page — apps / build / deploy. Stub during the port; the cockpit calls (suite apps,
//! build options, deploy strategy) wire in the same direct-call way the Network page shows.
use crate::app::App;

pub fn ui(_app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Fleet");
    ui.separator();
    ui.label("apps · build · deploy — port in progress");
    ui.label(
        egui::RichText::new("will call cockpit::flow::store_groups / install_suite_app / deploy_strategy")
            .weak()
            .small(),
    );
}
