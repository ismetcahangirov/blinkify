//! Window geometry across restarts (#19).
//!
//! # Why this is in Rust and the splitter positions are not
//!
//! The window exists before the renderer does. Nothing in JavaScript can be
//! asked where to put a window that has to be placed in order for JavaScript to
//! run at all, so the size, the position and the maximised state are read from
//! disk in `setup` and applied before the window is shown.
//!
//! The splitters are the opposite: renderer state, read and written only by the
//! renderer, persisted in `localStorage`. The line between the two halves is
//! the operating system's business against the renderer's, which is a real line
//! rather than a convenient one.
//!
//! # The failure this module exists for
//!
//! Restoring a saved position is three lines. Restoring it *safely* is the
//! rest of the file, and the case is not hypothetical: someone docks a laptop,
//! moves Blinkify onto the second monitor, closes it, and comes back on the
//! train. The saved position is now off the edge of every attached display, and
//! an application that restores it faithfully has opened a window the user
//! cannot see, cannot move and cannot close, on a machine that reports it as
//! running.
//!
//! [`place_within`] is the answer and is deliberately a pure function of two
//! rectangles and a fallback: no Tauri, no window, no monitors, so it can be
//! tested exhaustively by `cargo test` without a display attached — which is
//! also the only way the disconnected-monitor case can be tested at all.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, WebviewWindow, Window};

/// The file the geometry lives in, under the app's config directory.
const FILE_NAME: &str = "window-state.json";

/// How much of the window has to be on a monitor for the position to be usable.
///
/// Not "any overlap at all": a window with four pixels on screen is a window
/// the user cannot grab. Not "entirely on screen" either, because a window the
/// user deliberately left hanging a little off the edge should come back where
/// they left it. A third of the window, and enough of the title bar to drag.
const MINIMUM_VISIBLE_FRACTION: f64 = 0.33;

/// The height of the application bar, which is the only part that can be
/// dragged. If none of it is on a monitor, the window cannot be moved by hand.
const DRAGGABLE_STRIP_HEIGHT: f64 = 48.0;

/// A rectangle in logical (device-independent) pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    #[must_use]
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The area shared with `other`, in square logical pixels.
    #[must_use]
    fn intersection_area(&self, other: &Self) -> f64 {
        let width = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let height = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        if width <= 0.0 || height <= 0.0 {
            0.0
        } else {
            width * height
        }
    }

    /// The strip the user can actually grab to move the window.
    #[must_use]
    fn draggable_strip(&self) -> Self {
        Self::new(
            self.x,
            self.y,
            self.width,
            self.height.min(DRAGGABLE_STRIP_HEIGHT),
        )
    }
}

/// What is written to disk.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub bounds: Rect,
    pub maximized: bool,
}

/// Decide where a window should actually open.
///
/// Returns `saved` when enough of it lands on one of `monitors`, and `fallback`
/// when it does not. "Enough" is two conditions and both have to hold:
///
/// 1. at least [`MINIMUM_VISIBLE_FRACTION`] of the window's area is on a single
///    monitor, and
/// 2. some of the draggable strip along its top is on that same monitor.
///
/// Both, because either alone lets a real case through. Area alone accepts a
/// window whose visible third is its bottom edge, which the user can see and
/// cannot move. The strip alone accepts a window showing nothing but a sliver of
/// its own title bar.
///
/// A single monitor rather than the union of all of them. Two displays of
/// different heights leave dead space in the virtual desktop — a 1080p primary
/// beside a 720p secondary leaves a band below the smaller one that belongs to
/// no display at all. A window can have nearly half its area inside that band,
/// a slice on each monitor, and none of it usable. Summing the two slices calls
/// that window visible; asking whether any one display holds enough of it does
/// not.
///
/// With no monitors at all — which a headless session reports — the fallback is
/// returned. There is nowhere to put it and guessing would be worse.
#[must_use]
pub fn place_within(saved: Rect, monitors: &[Rect], fallback: Rect) -> Rect {
    let saved_area = saved.width * saved.height;
    if saved_area <= 0.0 {
        return fallback;
    }

    let strip = saved.draggable_strip();

    let usable = monitors.iter().any(|monitor| {
        let visible_fraction = saved.intersection_area(monitor) / saved_area;
        let strip_visible = strip.intersection_area(monitor) > 0.0;
        visible_fraction >= MINIMUM_VISIBLE_FRACTION && strip_visible
    });

    if usable { saved } else { fallback }
}

/// Where the state file lives. `None` when the platform has no config directory.
fn state_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|directory| directory.join(FILE_NAME))
}

/// Read the saved state, or `None` if there is not a usable one.
///
/// Every failure returns `None` rather than an error. A missing file is a first
/// launch, and a corrupt one is a preference that will be overwritten the next
/// time the window closes — neither is a reason to fail to open.
#[must_use]
pub fn load(app: &AppHandle) -> Option<WindowState> {
    let path = state_path(app)?;
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Write the window's current geometry.
///
/// Errors are swallowed for the same reason they are on read: losing a window
/// position is not worth interrupting somebody who is closing the application.
/// Takes a [`Window`] rather than a [`WebviewWindow`] because that is what
/// `on_window_event` hands over, and every measurement it needs — the maximised
/// flag, the scale factor, the outer position and the inner size — lives on the
/// window rather than on the webview inside it.
pub fn save(app: &AppHandle, window: &Window) {
    let Some(path) = state_path(app) else { return };

    let maximized = window.is_maximized().unwrap_or(false);

    /* The *restored* geometry, never the maximised one. Saving the maximised
    bounds means un-maximising on the next launch gives a window the size of
    the screen with no way back to the size the user chose. Tauri reports the
    outer position and inner size of the maximised window, so the restored
    size has to be read while it is still restored — which is why this reads
    the size before touching the maximised flag. */
    let scale = window.scale_factor().unwrap_or(1.0);
    let Ok(position) = window.outer_position() else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };

    let logical_position = position.to_logical::<f64>(scale);
    let logical_size = size.to_logical::<f64>(scale);

    let state = WindowState {
        bounds: Rect::new(
            logical_position.x,
            logical_position.y,
            logical_size.width,
            logical_size.height,
        ),
        maximized,
    };

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(encoded) = serde_json::to_string_pretty(&state) {
        let _ = fs::write(path, encoded);
    }
}

/// Apply the saved geometry to a window, validating it against the monitors
/// that are actually attached right now.
pub fn restore(app: &AppHandle, window: &WebviewWindow) {
    let Some(state) = load(app) else { return };

    let monitors: Vec<Rect> = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| {
            let scale = monitor.scale_factor();
            let position = monitor.position().to_logical::<f64>(scale);
            let size = monitor.size().to_logical::<f64>(scale);
            Rect::new(position.x, position.y, size.width, size.height)
        })
        .collect();

    /* The fallback is the window as Tauri has just created it from
    `tauri.conf.json` — centred, at the configured size. Falling back to
    "where the configuration says" rather than to a hard-coded rectangle
    means there is one place that decides what a default window looks like. */
    let fallback = current_bounds(window).unwrap_or(state.bounds);
    let bounds = place_within(state.bounds, &monitors, fallback);

    let _ = window.set_size(LogicalSize::new(bounds.width, bounds.height));
    let _ = window.set_position(LogicalPosition::new(bounds.x, bounds.y));

    /* Maximise last. Maximising first and then setting a size produces a window
    that reports itself maximised while occupying the restored bounds, which
    is a state Windows has no way to get out of except by maximising again. */
    if state.maximized {
        let _ = window.maximize();
    }
}

fn current_bounds(window: &WebviewWindow) -> Option<Rect> {
    let scale = window.scale_factor().ok()?;
    let position = window.outer_position().ok()?.to_logical::<f64>(scale);
    let size = window.inner_size().ok()?.to_logical::<f64>(scale);
    Some(Rect::new(position.x, position.y, size.width, size.height))
}

#[cfg(test)]
mod tests {
    use super::{Rect, place_within};

    /// A 1920×1080 monitor at the origin, which is every single-display machine.
    const PRIMARY: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    /// A second display to the right of it — the one that gets unplugged.
    const SECONDARY: Rect = Rect {
        x: 1920.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    const FALLBACK: Rect = Rect {
        x: 240.0,
        y: 90.0,
        width: 1440.0,
        height: 900.0,
    };

    #[test]
    fn keeps_a_window_that_is_fully_on_a_monitor() {
        let saved = Rect::new(100.0, 100.0, 1440.0, 900.0);
        assert_eq!(place_within(saved, &[PRIMARY], FALLBACK), saved);
    }

    #[test]
    fn keeps_a_window_on_a_second_monitor_while_it_is_attached() {
        let saved = Rect::new(2000.0, 120.0, 1440.0, 900.0);
        assert_eq!(place_within(saved, &[PRIMARY, SECONDARY], FALLBACK), saved);
    }

    /// The failure case #19 names. The window was left on a display that is no
    /// longer there, so restoring it faithfully would open it where nobody can
    /// see it.
    #[test]
    fn falls_back_when_the_saved_monitor_is_gone() {
        let saved = Rect::new(2000.0, 120.0, 1440.0, 900.0);
        assert_eq!(place_within(saved, &[PRIMARY], FALLBACK), FALLBACK);
    }

    #[test]
    fn falls_back_when_no_monitor_is_attached_at_all() {
        let saved = Rect::new(100.0, 100.0, 1440.0, 900.0);
        assert_eq!(place_within(saved, &[], FALLBACK), FALLBACK);
    }

    /// Left hanging a little off the right edge on purpose. That is a window
    /// the user arranged, and it comes back as they left it.
    #[test]
    fn keeps_a_window_deliberately_hanging_off_an_edge() {
        let saved = Rect::new(1500.0, 100.0, 800.0, 600.0);
        assert_eq!(place_within(saved, &[PRIMARY], FALLBACK), saved);
    }

    /// Only a sliver showing. Enough to see, not enough to grab.
    #[test]
    fn falls_back_when_barely_any_of_the_window_is_visible() {
        let saved = Rect::new(1850.0, 100.0, 800.0, 600.0);
        assert_eq!(place_within(saved, &[PRIMARY], FALLBACK), FALLBACK);
    }

    /// The case an area-only check lets through: plenty of the window on
    /// screen, but the bar the user drags it by is above the top edge.
    #[test]
    fn falls_back_when_the_draggable_strip_is_off_the_top() {
        let saved = Rect::new(200.0, -200.0, 1200.0, 900.0);
        let area_visible = saved.intersection_area(&PRIMARY) / (saved.width * saved.height);
        assert!(
            area_visible > 0.33,
            "the case is only interesting if most of the window is on screen"
        );
        assert_eq!(place_within(saved, &[PRIMARY], FALLBACK), FALLBACK);
    }

    /// The case a union-of-monitors check lets through.
    ///
    /// A 1080p primary beside a 720p secondary leaves a 360px band of dead
    /// space below the smaller display. This window has a quarter of itself on
    /// one monitor, a fifth on the other and the rest in the band — 46% of it
    /// is on *a* display, which a union test accepts, and no single display
    /// holds enough of it to be usable.
    #[test]
    fn falls_back_when_most_of_the_window_is_in_dead_space_between_displays() {
        let smaller = Rect::new(1920.0, 0.0, 1280.0, 720.0);
        let saved = Rect::new(1720.0, 400.0, 500.0, 1000.0);

        let area = saved.width * saved.height;
        let on_primary = saved.intersection_area(&PRIMARY) / area;
        let on_smaller = saved.intersection_area(&smaller) / area;

        assert!(
            on_primary < 0.33 && on_smaller < 0.33,
            "neither display holds enough alone: {on_primary} and {on_smaller}"
        );
        assert!(
            on_primary + on_smaller >= 0.33,
            "but the union does, which is the check this case exists to reject"
        );

        assert_eq!(place_within(saved, &[PRIMARY, smaller], FALLBACK), FALLBACK);
    }

    #[test]
    fn falls_back_on_a_degenerate_rectangle() {
        // A zero-sized window has been reported by a driver change mid-session.
        let saved = Rect::new(0.0, 0.0, 0.0, 0.0);
        assert_eq!(place_within(saved, &[PRIMARY], FALLBACK), FALLBACK);
    }
}
