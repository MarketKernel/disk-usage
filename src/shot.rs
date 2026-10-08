//! Debug builds only: `DISK_USAGE_SHOT=out.png disk-usage <path>` saves a screenshot of
//! the window once the scan has finished, then exits. Optional:
//! - `DISK_USAGE_SELECT=<n>` selects the n-th largest item,
//! - `DISK_USAGE_ZOOM=<n>` zooms into the n-th largest item,
//! - `DISK_USAGE_QUEUE="<n> <m> …"` adds these largest items to the Trash queue,
//! - `DISK_USAGE_DELAY=<seconds>` waits before the screenshot (default 0.3),
//! - `DISK_USAGE_DURING_SCAN=1` captures the progress screen instead,
//! - `DISK_USAGE_HOVER="x,y x,y …"` moves the mouse through these points (0.15 s each).
//!
//! Without a path argument the start screen is captured.
//!
//! Used to check the UI without a screen recorder.

use std::path::PathBuf;

use eframe::egui;

pub struct Shot {
    path: PathBuf,
    pub select: Option<usize>,
    pub zoom: Option<usize>,
    pub queue: Vec<usize>,
    pub during_scan: bool,
    hover: Vec<egui::Pos2>,
    delay: f64,
    ready_at: Option<f64>,
    requested: bool,
}

fn env<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|s| s.parse().ok())
}

impl Shot {
    pub fn from_env() -> Option<Self> {
        let path = PathBuf::from(std::env::var_os("DISK_USAGE_SHOT")?);
        Some(Self {
            path,
            select: env("DISK_USAGE_SELECT"),
            zoom: env("DISK_USAGE_ZOOM"),
            queue: std::env::var("DISK_USAGE_QUEUE")
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|n| n.parse().ok())
                .collect(),
            during_scan: std::env::var_os("DISK_USAGE_DURING_SCAN").is_some(),
            hover: std::env::var("DISK_USAGE_HOVER")
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|p| {
                    let (x, y) = p.split_once(',')?;
                    Some(egui::pos2(x.parse().ok()?, y.parse().ok()?))
                })
                .collect(),
            delay: env("DISK_USAGE_DELAY").unwrap_or(0.3),
            ready_at: None,
            requested: false,
        })
    }

    /// Feeds simulated mouse moves to egui.
    pub fn input_hook(&self, raw: &mut egui::RawInput) {
        let (Some(start), Some(now)) = (self.ready_at, raw.time) else { return };
        let step = ((now - start) / 0.15) as usize;
        if let Some(&pos) = self.hover.get(step.min(self.hover.len().saturating_sub(1))) {
            raw.events.push(egui::Event::PointerMoved(pos));
        }
    }

    /// Returns true on the first frame in the state to capture.
    pub fn update(&mut self, ctx: &egui::Context, ready: bool) -> bool {
        let now = ctx.input(|i| i.time);
        let first = ready && self.ready_at.is_none();
        if first {
            self.ready_at = Some(now);
        }
        if let Some(t) = self.ready_at {
            if !self.requested && now - t >= self.delay {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                self.requested = true;
            }
            ctx.request_repaint();
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            image::save_buffer(&self.path, &rgba, w as u32, h as u32, image::ColorType::Rgba8)
                .expect("failed to save screenshot");
            std::process::exit(0);
        }
        first
    }
}
