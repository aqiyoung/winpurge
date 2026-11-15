mod app;
mod engine;
mod model;
mod rules;
mod scan;
mod treemap;
mod util;
mod winapi;

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([980.0, 640.0])
            .with_title("WinPurge"),
        ..Default::default()
    };

    eframe::run_native(
        "WinPurge",
        options,
        Box::new(
            |cc| -> Result<Box<dyn eframe::App>, Box<dyn std::error::Error + Send + Sync>> {
                Ok(Box::new(app::WinPurge::new(cc)))
            },
        ),
    )
}
