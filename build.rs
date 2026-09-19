fn main() {
    // Without this, Cargo reruns the script whenever any file in the package changes.
    println!("cargo::rerun-if-changed=build.rs");

    // Embed the icon and version information into the Windows executable.
    #[cfg(windows)]
    embed_resources().expect("windows resources are embedded");
}

#[cfg(windows)]
fn embed_resources() -> std::io::Result<()> {
    const ICON: &str = "assets/icon.ico";

    println!("cargo::rerun-if-changed={ICON}");

    winresources::Resources::new()
        .icon(std::fs::read(ICON)?)
        // Task Manager and the "Open with" list show the file description as the app's name.
        .string("FileDescription", "Peekr")
        .string("ProductName", "Peekr")
        .link()
}
