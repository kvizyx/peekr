//! Running a subprocess and turning a non-zero exit into an error.

use std::process::Command;

use anyhow::{Context, Result, bail};

/// Runs a command, failing with what it was and how it failed if it does not succeed.
pub fn run(command: &mut Command) -> Result<()> {
    let status = command.status().with_context(|| format!("starting {command:?}"))?;

    if !status.success() {
        bail!("{command:?} failed with {status}");
    }

    Ok(())
}

/// Runs a command and returns what it printed to standard output.
pub fn capture(command: &mut Command) -> Result<String> {
    let output = command.output().with_context(|| format!("starting {command:?}"))?;

    if !output.status.success() {
        bail!(
            "{command:?} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout).context("the command's output is not utf-8")
}
