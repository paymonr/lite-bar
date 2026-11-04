//! Embeds `assets/lite-bar.ico` as the exe's icon (resource group 1, `tray::APP_ICON_ID`).
//!
//! The icon is turned into a compiled resource (`.res`) here and passed to the linker, which
//! accepts `.res` files directly (MSVC `link.exe` and `lld-link` alike), so no resource compiler
//! is needed. To change the icon, edit and run `scripts/make-icon.py`.

use std::path::PathBuf;

fn main() {
    const ICON: &str = "assets/lite-bar.ico";
    println!("cargo:rerun-if-changed={ICON}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ico = std::fs::read(ICON).unwrap_or_else(|e| panic!("can't read {ICON}: {e}"));
    let res = lite_bar_core::icon_res::icon_res(&ico, 1)
        .unwrap_or_else(|e| panic!("{ICON} is not a usable icon: {e}"));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("lite-bar.res");
    std::fs::write(&out, res).unwrap_or_else(|e| panic!("can't write {}: {e}", out.display()));
    // Every linked target (the exe and its unit tests) gets the icon.
    println!("cargo:rustc-link-arg={}", out.display());
}
