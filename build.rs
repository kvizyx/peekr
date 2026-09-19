//! Embeds the icon and version information into the Windows executable.

use std::fs;

const ICON: &str = "assets/icon.ico";

fn main() -> std::io::Result<()> {
    println!("cargo::rerun-if-changed={ICON}");

    winresources::Resources::new()
        .icon(fs::read(ICON)?)
        // Task Manager and the "Open with" list show the file description as the app's name.
        .string("FileDescription", "Peekr")
        .string("ProductName", "Peekr")
        .link()
}
