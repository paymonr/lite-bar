//! Lite Bar: Windows tray app with an FPS overlay.

// These helpers are wired into `main` by a later task.
#[cfg(windows)]
#[allow(dead_code)]
mod elevation;
#[cfg(windows)]
#[allow(dead_code)]
mod etw;
#[cfg(windows)]
#[allow(dead_code)]
mod paths;
#[cfg(windows)]
#[allow(dead_code)]
mod single_instance;

fn main() {}
