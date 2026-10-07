#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod colors;
mod format;
mod icon;
mod platform;
mod scan;
#[cfg(debug_assertions)]
mod shot;
mod sunburst;
mod tree;

use std::path::PathBuf;

fn main() -> eframe::Result {
    // Old macOS versions pass a `-psn_…` process serial number to apps started from Finder.
    let path = std::env::args_os()
        .nth(1)
        .filter(|p| !p.is_empty() && !p.to_string_lossy().starts_with("-psn_"))
        .map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Disk Usage")
            .with_app_id("disk-usage")
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size([720.0, 480.0])
            .with_drag_and_drop(true)
            .with_icon(icon::window_icon()),
        ..Default::default()
    };

    eframe::run_native("Disk Usage", options, Box::new(|cc| Ok(Box::new(app::App::new(cc, path)))))
}
