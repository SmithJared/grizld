// Grizld - Vim-based video editor
// Main entry point

mod app;
mod audio;
mod buffer_manager;
mod command;
mod renderer;

#[cfg(target_os = "macos")]
mod metal;

use app::EditorApp;

fn main() -> Result<(), eframe::Error> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    tracing::info!("Starting Grizld video editor...");

    // Configure eframe options
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title("Grizld - Vim Video Editor"),
        ..Default::default()
    };

    // Run the application
    eframe::run_native(
        "Grizld",
        options,
        Box::new(|cc| Ok(Box::new(EditorApp::new(cc)))),
    )
}
