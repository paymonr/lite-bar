//! Lite Bar: Windows tray app with an FPS overlay.

fn main() {}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn runs_on_windows() {
        assert!(cfg!(windows));
    }
}
