//! Automatic updates, by Velopack.
//!
//! Velopack installs the app, keeps the packages of its releases and swaps one version for
//! another. What is left here is deciding when: at startup, before the tray icon appears, the app
//! asks the latest release whether there is anything newer, downloads it while a window shows how
//! far along it is, and restarts into it.
//!
//! Only an installation Velopack made can update itself. A development build, or a copy unpacked
//! from the .tar.gz, runs as it is.

mod progress;

use std::sync::Arc;
use std::sync::mpsc;

use anyhow::{Result, anyhow};
use velopack::sources::HttpSource;
use velopack::{HttpOptions, UpdateCheck, UpdateInfo, UpdateManager, VelopackApp};

use self::progress::Progress;

/// Where the releases are. `releases/latest/download/<asset>` redirects to the newest published
/// release's copy of that asset, so Velopack finds its feed and packages without a GitHub API
/// call, and runs into no API rate limit.
const RELEASES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases/latest/download");

/// How long the app waits to hear whether there is an update before starting without one.
/// Short, because this runs before the tray icon appears, and a machine that just booted may
/// have no network yet.
const CHECK_TIMEOUT_MS: u64 = 5_000;

/// Velopack's part of startup. An installer, an update or an uninstall starts the app with
/// arguments of its own, which this handles and then ends the process; it has to come before
/// anything else looks at the arguments.
pub fn init() {
    // A downloaded update is installed by `install`, which only the tray app runs: a capture
    // started from a system shortcut has no business restarting the app from under it.
    VelopackApp::build().set_auto_apply_on_startup(false).run();
}

/// Installs a newer release, if there is one, and restarts into it: when this returns, the app
/// goes on with the version it has.
///
/// An update a previous run downloaded is installed even when checking is turned off: it is on
/// disk already, and leaving it would only mean installing it some other day.
pub fn install(enabled: bool) {
    if let Err(e) = try_install(enabled) {
        log::warn!("update failed: {e:#}");
    }
}

fn try_install(enabled: bool) -> Result<()> {
    let Some(checker) = manager(CHECK_TIMEOUT_MS) else {
        return Ok(());
    };

    if let Some(pending) = checker.get_update_pending_restart() {
        log::info!("installing {}, downloaded earlier", pending.Version);
        return Ok(checker.apply_updates_and_restart(&pending)?);
    }

    if !enabled {
        return Ok(());
    }

    let update = match checker.check_for_updates()? {
        UpdateCheck::UpdateAvailable(update) => update,
        UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => {
            log::info!("peekr {} is up to date", checker.get_current_version());
            return Ok(());
        }
    };

    // No time limit here: Velopack's limit covers the whole transfer, and a slow line is still
    // worth waiting for while the window says how far along it is.
    let downloader = manager(0).ok_or_else(|| anyhow!("the installation went away"))?;
    if !download(downloader.clone(), &update)? {
        return Ok(());
    }

    Ok(downloader.apply_updates_and_restart(&*update)?)
}

/// The update manager of this installation, or `None` when Velopack did not install it.
fn manager(timeout_ms: u64) -> Option<UpdateManager> {
    let options = HttpOptions {
        Headers: Vec::new(),
        TimeoutMilliseconds: timeout_ms,
    };

    UpdateManager::new(HttpSource::new_with_options(RELEASES, options), None, None)
        .inspect_err(|e| log::info!("not updating: {e}"))
        .ok()
}

/// Downloads the update on a thread of its own while this one shows how far along it is, since
/// winit only opens windows on the thread the process started on. Reports whether the update is
/// ready to install.
///
/// Putting it off leaves the download running: once it is done it waits on disk, and the next
/// start installs it.
fn download(manager: UpdateManager, update: &UpdateInfo) -> Result<bool> {
    let version = update.TargetFullRelease.Version.clone();
    log::info!(
        "downloading {version}, {:.1} MB",
        update.TargetFullRelease.Size as f64 / f64::from(1 << 20)
    );

    let progress = Arc::new(Progress::default());
    let (sender, percents) = mpsc::channel::<i16>();

    std::thread::spawn({
        let progress = Arc::clone(&progress);
        move || {
            for percent in percents {
                progress.set_percent(percent);
            }
        }
    });

    let worker = std::thread::spawn({
        let progress = Arc::clone(&progress);
        let update = update.clone();

        move || {
            let downloaded = manager.download_updates(&update, Some(sender));
            progress.finish();
            downloaded
        }
    });

    if let Err(e) = progress::show(&progress, &version) {
        log::warn!("the update window failed: {e:#}");
    }

    if progress.is_put_off() {
        log::info!("{version} goes on downloading, and is installed on the next start");
        return Ok(false);
    }

    worker.join().map_err(|_| anyhow!("the update thread panicked"))??;

    Ok(true)
}
