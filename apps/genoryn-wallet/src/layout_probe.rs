//! Opt-in report of where named controls are drawn (`--layout-report FILE`).
//!
//! The native UI test reads it to click real controls instead of guessed
//! pixels. It holds control names and rectangles in logical points only: no
//! wallet data, addresses, amounts or text typed by the user.
use eframe::egui;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

struct Probe {
    path: PathBuf,
    frame: BTreeMap<String, [f32; 4]>,
    written: BTreeMap<String, [f32; 4]>,
    page: String,
    written_page: String,
}

static PROBE: OnceLock<Mutex<Probe>> = OnceLock::new();

/// Enables the report; later calls are ignored.
pub fn enable(path: PathBuf) {
    let _ = PROBE.set(Mutex::new(Probe {
        path,
        frame: BTreeMap::new(),
        written: BTreeMap::new(),
        page: String::new(),
        written_page: String::new(),
    }));
}

/// Records `name` at `rect` for this frame. Free when the report is off.
pub fn record(name: &str, rect: egui::Rect) {
    if let Some(probe) = PROBE.get()
        && let Ok(mut probe) = probe.lock()
    {
        probe.frame.insert(
            name.to_owned(),
            [rect.min.x, rect.min.y, rect.width(), rect.height()],
        );
    }
}

/// Records the response's rectangle and passes the response on.
pub fn tag(name: &str, response: egui::Response) -> egui::Response {
    record(name, response.rect);
    response
}

/// The page shown this frame, so the test can wait for navigation.
pub fn page(name: &str) {
    if let Some(probe) = PROBE.get()
        && let Ok(mut probe) = probe.lock()
    {
        name.clone_into(&mut probe.page);
    }
}

/// Writes the frame's controls when they changed, atomically (write, then rename).
pub fn end_frame(pixels_per_point: f32) {
    let Some(probe) = PROBE.get() else {
        return;
    };
    let Ok(mut probe) = probe.lock() else {
        return;
    };
    let frame = std::mem::take(&mut probe.frame);
    if frame == probe.written && probe.page == probe.written_page {
        return;
    }
    let controls: serde_json::Map<String, serde_json::Value> = frame
        .iter()
        .map(|(name, rect)| (name.clone(), serde_json::json!(rect)))
        .collect();
    let report = serde_json::json!({
        "schema": 1,
        "units": "logical points from the window's client-area origin",
        "pixels_per_point": pixels_per_point,
        "page": probe.page,
        "controls": controls,
    });
    let temporary = probe.path.with_extension("tmp");
    if std::fs::write(&temporary, report.to_string()).is_ok()
        && std::fs::rename(&temporary, &probe.path).is_ok()
    {
        probe.written = frame;
        probe.written_page = probe.page.clone();
    }
}
