# winresources

Embeds an icon and version information into a Windows executable without needing
`rc.exe` or the Windows SDK - resources are written straight into a compiled `.res` file, which
the MSVC linker accepts as an input like any object file.

```rust
winresources::Resources::new()
    .icon(std::fs::read("icon.ico")?)
    .string("ProductName", "Example")
    .link()?;
```

`Resources::new()` fills in the version and a few strings from the crate being built
(`CARGO_PKG_VERSION`, `CARGO_PKG_DESCRIPTION` etc.), `string()` overrides or adds to them.

`link()` does nothing (rather than fail) when the linker is not MSVC, so a `windows-gnu` build
still succeeds, just without an icon or version information.

Used by our own [`build.rs`](../build.rs) for Windows environment.
