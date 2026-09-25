# Release

An application is installed and updated by [Velopack](https://velopack.io). When the tray app starts, it
reads the Velopack feed of the newest published release. If there is a newer version, it downloads
it (a delta when it is one version behind, the full package otherwise) and restarts into it.

What a release carries, for each platform (`windows-x86_64`, `linux-x86_64`, `linux-aarch64`,
`macos-aarch64`):

- `releases.<platform>.json`: the feed installed copies read;
- `peekr-<version>-<platform>-full.nupkg`, and `-delta.nupkg` when there was a release before;
- for people: `peekr-windows-x86_64-Setup.exe` and `-Portable.zip` on Windows, and on Linux
  `peekr-<platform>.AppImage` and `peekr-<version>-<platform>.tar.gz`, and on macOS
  `peekr-macos-aarch64-Setup.pkg` and `-Portable.zip`, both holding the `Peekr.app` bundle.

The `.tar.gz` is for systems that cannot run an AppImage. It does not update itself, and neither
does a development build.

## Workflow

### 1. Raise the version

Edit `version` in [`Cargo.toml`](Cargo.toml). The tag has to match it, and the workflow checks.

### 2. Tag and push

```bash
git tag vX.Y.Z
git push origin main vX.Y.Z
```

The workflow runs the same checks as CI (formatting, clippy and the tests) and builds nothing
until they pass. It then builds every platform with `cargo xtask dist` and opens a **draft**
release with the files above.

The delta is made from the latest *published* release, which `vpk` downloads during the build.
Publish one release before building the next: otherwise the delta is made from the one before, and
copies of the unpublished release download the whole package.

### 3. Publish

Look the draft over, and publish it. Nothing reaches users before that, since `releases/latest`,
which the app follows, skips drafts. The next time an installed copy starts, it finds the release.

### 4. Update WinGet

```bash
cargo xtask winget X.Y.Z
wingetcreate submit --token <token> target/winget/manifests/k/kvizyx/Peekr/X.Y.Z
```

Velopack keeps the version in "Apps & features" current as it updates, so `winget list` tells the
truth in between.

## What not to do

- Do not replace or re-upload assets of a release that is already published. Installed copies
  check every package against the SHA-256 in the feed, and one that changed under them fails.
- Do not change the package id (`peekr`) or the channel names. Installed copies look for the
  feed of the channel they were installed from, and Windows knows the installation by the id.
