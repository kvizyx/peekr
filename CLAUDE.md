# CLAUDE.md

<!-- rtk-instructions v2 -->
## Command Output

Command output here is condensed to save tokens, keeping every signal and
dropping costly noise. Treat it as the complete result: run commands
normally, and batch related commands into one call to avoid extra turns.
Truncated results state their recovery path in their own output. Re-run a
command as `rtk proxy <cmd>` only when its result is unusable: empty when
output was clearly expected, contradicting its exit code, or garbled.
<!-- /rtk-instructions -->

## Checks

Run formatting, linting and tests before considering a change done:

```bash
cargo fmt --check
cargo clippy --all-targets
cargo test
```

Clippy runs with `pedantic` plus a few restriction lints (see `[lints]` in `Cargo.toml`) and must
report no issues. When touching the OCR pipeline, also compare `cargo run --release -- --image <png>`
output before and after the change.

## Rust conventions

- **Readability.** Separate logical steps inside a function with blank lines (setup, main work,
  result). Keep functions short; extract a helper once a block needs its own comment.
- **Language.** Code, comments, UI strings and docs are in English. Cyrillic is only allowed in OCR
  data and tests (e.g. the homoglyph table in `src/ocr/text.rs`).
- **Naming.** Descriptive names over abbreviations; single letters only for coordinates (`x`, `y`),
  colour channels and short closures.
- **Constants.** No magic numbers: tunable thresholds are named constants with a doc comment
  explaining what they control.
- **Errors.** Return `anyhow::Result` and add context with `.context()` / `.with_context()`.
  No `unwrap()` outside tests; `expect()` only for invariants, with a message saying why it holds.
- **Unsafe.** Only in `src/platform/`. Every `unsafe` block gets a `// SAFETY:` comment.
- **Lints.** Fix warnings instead of silencing them. If an exception is justified, use
  `#[expect(lint, reason = "...")]` on the smallest possible item, never a crate-wide `allow`.
- **Docs.** Every module starts with a `//!` summary. Public items and non-obvious private ones get
  `///` docs that explain *why*, not just *what*.
- **Tests.** Pure logic (geometry, decoding, text post-processing) gets unit tests in a `tests`
  module at the bottom of the file, structured as arrange / act / assert separated by blank lines.
- **Dependencies.** Prefer the standard library; add a crate only when it saves real work, with
  `default-features = false` when the defaults are not needed.
