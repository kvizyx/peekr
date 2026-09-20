fn main() {
    // Without this, Cargo reruns the script whenever any file in the package changes.
    println!("cargo::rerun-if-changed=build.rs");

    // Embed app information into the Windows executable.
    #[cfg(windows)]
    embed_executable_resources().expect("windows resources are embedded");
}

#[cfg(windows)]
fn embed_executable_resources() -> std::io::Result<()> {
    const ICON: &str = "assets/icon.ico";

    println!("cargo::rerun-if-changed={ICON}");

    winresources::Resources::new()
        .icon(std::fs::read(ICON)?)
        // Task manager and apps list show the file description as the app's name.
        .string("FileDescription", "Peekr")
        .string("ProductName", "Peekr")
        .link()
}
