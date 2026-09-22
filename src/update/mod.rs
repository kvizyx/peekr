//! Automatic updates.
//!
//! At startup the app downloads the manifest of the newest release, compares the SHA-256 of every
//! file it lists against the installed one and fetches only those that differ. A release that
//! changes nothing but the executable therefore costs a single download, while new models come
//! along on their own whenever they do change.
//!
//! Verified files wait in a staging directory until all of them are there; only then are they
//! moved into place, so an interrupted download leaves the installation untouched. Nothing here
//! may keep the app from starting: every failure is logged and the old version runs on.
//!
//! The download runs on its own thread while [`progress`] shows a window with how far along it
//! is, since the app has nothing else to show until it is done.

mod progress;

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use self::progress::Progress;
use crate::platform;

/// Where the releases are. `releases/latest/download/<asset>` redirects to the newest published
/// release's copy of that asset, so finding the latest version needs no GitHub API call, and runs
/// into no API rate limit.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Directory next to the executable where verified files wait to be moved into place.
const STAGING_DIR: &str = ".update";
/// Written once every file is staged, so a half-finished download is never applied.
const READY_FILE: &str = "ready.toml";
/// Appended to the name of a file on its way out. Windows cannot delete the executable of a
/// running process, but it does allow renaming it.
const REPLACED_SUFFIX: &str = ".replaced";
/// Appended to the manifest's name to get the name of its signature.
const SIGNATURE_SUFFIX: &str = ".sig";

/// The keys a release may be signed by, as hexadecimal; `cargo xtask keygen` makes a pair.
///
/// While this is empty the app installs whatever the release page serves, which is how it worked
/// before there was a key. Filling it in turns the check on for good: an update signed by none of
/// these keys is refused from then on.
///
/// More than one key is how a key is retired. A release that adds the new key has to be signed
/// with the old one, since that is all the copies already out there will accept; only once it has
/// had time to reach people may releases be signed with the new key, and only after that may the
/// old one be dropped from this list.
const PUBLIC_KEYS: &[&str] = &["0d6f5937357847b8c4bcb28821971a584cfda6285155b11e02a479a4f985d3e8"];

/// How long the app waits for the manifest before starting without it. Short, because this runs
/// before the tray icon appears, and a machine that just booted may have no network yet.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one attempt at a file may take. Short, because an attempt resumes where the last one
/// stopped: a slow line keeps what it gained, while a line that has gone quiet is noticed soon
/// enough for the cancel button to mean something.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait before trying again, doubling up to the second.
const RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
const CHUNK_SIZE: usize = 1 << 16;

/// What a release consists of, published next to its archives.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Where the file belongs, relative to the directory the executable is in.
    pub path: String,
    /// Of the file itself, not of the compressed download.
    pub sha256: String,
    /// Of the download, so that the progress window knows what it is waiting for.
    pub size: u64,
    /// Whether the file has to be runnable. Nothing carries file modes from the release to here,
    /// and a peekr that is not executable is a peekr that cannot start again.
    #[serde(default)]
    pub executable: bool,
    /// The gzip-compressed file.
    pub url: String,
}

/// Installs an update: one a previous run left staged, or, when `enabled`, a newer release.
/// Returns whether the executable was replaced, which only a restart can bring into use.
pub fn install(enabled: bool) -> bool {
    try_install(enabled).unwrap_or_else(|e| {
        log::warn!("update failed: {e:#}");
        false
    })
}

/// Starts the installed executable again with the same arguments, leaving it to the caller to
/// let this process end.
pub fn restart() -> Result<()> {
    let exe = std::env::current_exe()?;

    Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .spawn()
        .with_context(|| format!("restarting {}", exe.display()))?;

    Ok(())
}

fn try_install(enabled: bool) -> Result<bool> {
    let Some(install) = Install::find() else {
        return Ok(false);
    };

    install.remove_replaced();

    // A staged update is applied even when checking is turned off: it is downloaded and verified
    // already, and leaving it behind would only mean installing it some other day. Moving files
    // into place is over too quickly to be worth a window.
    if let Some(manifest) = install.staged() {
        let replaced = install.apply(&manifest)?;
        platform::record_installed_version(&install.dir, &manifest.version);

        return Ok(replaced);
    }

    if !enabled {
        return Ok(false);
    }

    let manifest = fetch_manifest()?;
    if !is_newer(&manifest.version, VERSION) {
        log::info!("peekr {VERSION} is up to date");
        return Ok(false);
    }

    let outdated = install.outdated(&manifest)?;
    if outdated.is_empty() {
        log::warn!("{} has no file this installation is missing", manifest.version);
        return Ok(false);
    }

    let total: u64 = outdated.iter().map(|&index| manifest.files[index].size).sum();
    log::info!(
        "updating to {}: {} of {} files, {:.1} MB",
        manifest.version,
        outdated.len(),
        manifest.files.len(),
        total as f64 / f64::from(1 << 20)
    );

    download_and_install(install, manifest, outdated, total)
}

/// Downloads the files and puts them in place on a thread of its own, while this one shows how
/// far along it is: winit only opens windows on the thread the process started on.
fn download_and_install(install: Install, manifest: Manifest, outdated: Vec<usize>, total: u64) -> Result<bool> {
    let progress = Arc::new(Progress::new(total));
    let version = manifest.version.clone();

    let worker = std::thread::spawn({
        let progress = Arc::clone(&progress);

        move || {
            let installed = install
                .stage(&manifest, &outdated, &progress)
                .and_then(|()| install.apply(&manifest))
                .inspect(|_| platform::record_installed_version(&install.dir, &manifest.version));

            // Whatever happened, the window has nothing left to wait for.
            progress.finish();
            installed
        }
    });

    let shown = progress::show(&progress, &version);
    let installed = worker.join().map_err(|_| anyhow!("the update thread panicked"))?;

    // Giving up is the user's call to make, not a failure to report. A cancel that arrived while
    // the last file was already going into place is too late to undo, though, and an update that
    // is installed has to be reported as installed, or the app would run the version it replaced.
    if installed.is_err() && progress.is_cancelled() {
        log::info!("the update was cancelled");
        return Ok(false);
    }

    // The update itself matters more than the window that was meant to show it.
    installed.inspect(|_| {
        if let Err(e) = shown {
            log::warn!("the update window failed: {e:#}");
        }
    })
}

/// The directory the app runs from, and the executable inside it.
struct Install {
    dir: PathBuf,
    executable: String,
}

impl Install {
    /// The installation the running executable belongs to, or `None` when its files are not the
    /// app's to replace: a development build, or an installation a package manager owns.
    fn find() -> Option<Self> {
        if cfg!(debug_assertions) {
            return None;
        }

        let exe = std::env::current_exe()
            .inspect_err(|e| log::warn!("cannot locate the executable: {e}"))
            .ok()?;
        let dir = exe.parent()?.to_path_buf();
        let executable = exe.file_name()?.to_str()?.to_owned();

        if !is_writable(&dir) {
            log::info!(
                "{} is read-only; leaving updates to whoever installed peekr",
                dir.display()
            );
            return None;
        }

        Some(Self { dir, executable })
    }

    fn staging(&self) -> PathBuf {
        self.dir.join(STAGING_DIR)
    }

    /// The update a previous run downloaded but did not finish installing.
    fn staged(&self) -> Option<Manifest> {
        let text = fs::read_to_string(self.staging().join(READY_FILE)).ok()?;

        toml::from_str(&text)
            .inspect_err(|e| log::warn!("ignoring a staged update: {e}"))
            .ok()
    }

    /// Which of the manifest's files differ from the installed ones, as indices into it.
    fn outdated(&self, manifest: &Manifest) -> Result<Vec<usize>> {
        let mut outdated = Vec::new();

        for (index, file) in manifest.files.iter().enumerate() {
            let installed = sha256_of(&self.dir.join(safe_path(&file.path)?));

            if !installed.is_ok_and(|installed| installed == file.sha256) {
                outdated.push(index);
            }
        }

        Ok(outdated)
    }

    /// Downloads the given files of the manifest, and marks the update ready once every one of
    /// them is in the staging directory.
    fn stage(&self, manifest: &Manifest, outdated: &[usize], progress: &Progress) -> Result<()> {
        let staging = self.staging();
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;

        let agent = agent(Some(ATTEMPT_TIMEOUT));

        for file in outdated.iter().filter_map(|&index| manifest.files.get(index)) {
            log::info!("downloading {}", file.path);
            progress.downloading(&file.path);

            let destination = staging.join(safe_path(&file.path)?);
            download(&agent, file, &destination, progress).with_context(|| format!("downloading {}", file.path))?;

            progress.completed(file.size);
        }

        progress.installing();
        fs::write(staging.join(READY_FILE), toml::to_string(manifest)?)?;

        Ok(())
    }

    /// Moves every staged file into place and clears the staging directory. Reports whether the
    /// executable was among them, the one change a restart is needed for.
    ///
    /// Either all of the files are installed or none of them are: a failure part way through puts
    /// the ones already moved back, so that the installation is never a mix of two versions.
    /// The files of a manifest, with the running executable last.
    ///
    /// Between moving a file aside and moving its replacement in there is an instant where it is
    /// at neither name. Losing power in that instant is survivable for every file but the one the
    /// app is started from, so that one is left until everything else is safely in place.
    fn install_order<'a>(&self, manifest: &'a Manifest) -> Vec<&'a ManifestFile> {
        let running = self.dir.join(&self.executable);
        let mut files: Vec<&ManifestFile> = manifest.files.iter().collect();

        files.sort_by_key(|file| safe_path(&file.path).is_ok_and(|path| self.dir.join(path) == running));
        files
    }

    fn apply(&self, manifest: &Manifest) -> Result<bool> {
        let staging = self.staging();
        let mut installed: Vec<Installed> = Vec::new();

        for file in self.install_order(manifest) {
            let path = safe_path(&file.path)?;
            let source = staging.join(&path);
            if !source.is_file() {
                continue;
            }

            match install_file(&source, &self.dir.join(&path)) {
                Ok(file) => installed.push(file),
                Err(e) => {
                    roll_back(installed);
                    return Err(e).with_context(|| format!("installing {}", file.path));
                }
            }
        }

        // By the whole path: a file that merely shares its name, somewhere under the models, is
        // not the one this process is running.
        let running = self.dir.join(&self.executable);
        let replaced_executable = installed.iter().any(|file| file.destination == running);

        // The files that were replaced are of no use now. This process is running one of them,
        // which Windows will not let go of until it ends, so that one waits for the next start.
        for file in &installed {
            if let Some(stashed) = &file.stashed {
                let _ = fs::remove_file(stashed);
            }
        }

        fs::remove_dir_all(&staging).with_context(|| format!("clearing {}", staging.display()))?;

        log::info!("installed peekr {}", manifest.version);
        Ok(replaced_executable)
    }

    /// Deletes what an earlier update renamed out of the way. Only the executable ever needs it,
    /// so the directory it lives in is the only one worth looking through.
    fn remove_replaced(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };

        for entry in entries.flatten() {
            if entry
                .file_name()
                .as_encoded_bytes()
                .ends_with(REPLACED_SUFFIX.as_bytes())
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// One file moved into the installation, and what it takes to undo that.
struct Installed {
    /// Where it came from, which is where it goes back to.
    source: PathBuf,
    destination: PathBuf,
    /// Where the file it replaced was moved, when there was one.
    stashed: Option<PathBuf>,
}

/// Puts a downloaded file in place of the installed one.
///
/// The file being replaced is moved aside rather than overwritten, for two reasons: a rename
/// leaves nothing to put back if a later file fails, and the executable of the running process
/// cannot be overwritten on Windows at all, only renamed away.
fn install_file(source: &Path, destination: &Path) -> Result<Installed> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    let stashed = destination.exists().then(|| with_suffix(destination, REPLACED_SUFFIX));

    if let Some(stashed) = &stashed {
        let _ = fs::remove_file(stashed);
        fs::rename(destination, stashed)?;
    }

    fs::rename(source, destination).inspect_err(|_| {
        // Put the old file back rather than leave the installation without one.
        if let Some(stashed) = &stashed {
            let _ = fs::rename(stashed, destination);
        }
    })?;

    Ok(Installed {
        source: source.to_owned(),
        destination: destination.to_owned(),
        stashed,
    })
}

/// Undoes the moves an interrupted update managed to make, newest first. Every step is best
/// effort: there is nothing left to do about a failure here but leave the rest as it is.
fn roll_back(installed: Vec<Installed>) {
    log::warn!(
        "the update could not be installed; putting {} files back",
        installed.len()
    );

    for file in installed.into_iter().rev() {
        // Back to the staging directory, so that the next attempt can still make use of it.
        let _ = fs::rename(&file.destination, &file.source);

        if let Some(stashed) = &file.stashed {
            let _ = fs::rename(stashed, &file.destination);
        }
    }
}

/// Downloads the manifest of the newest release for this platform, and checks who signed it
/// before anything in it is believed.
fn fetch_manifest() -> Result<Manifest> {
    let url = format!(
        "{REPOSITORY}/releases/latest/download/update-{}-{}.toml",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let agent = agent(Some(CHECK_TIMEOUT));
    let text = fetch_text(&agent, &url).context("reading the release manifest")?;
    verify_signature(&agent, &url, text.as_bytes())?;

    toml::from_str(&text).context("the release manifest is not readable")
}

/// Checks that the manifest was signed by whoever holds the private half of [`PUBLIC_KEY`].
///
/// One signature covers the whole release: the manifest carries a SHA-256 for every file, and
/// each download is checked against its own before being installed.
fn verify_signature(agent: &ureq::Agent, url: &str, manifest: &[u8]) -> Result<()> {
    verify_against(PUBLIC_KEYS, agent, url, manifest)
}

fn verify_against(public_keys: &[&str], agent: &ureq::Agent, url: &str, manifest: &[u8]) -> Result<()> {
    if public_keys.is_empty() {
        log::warn!("no release key is compiled in; the update is trusted on its https alone");
        return Ok(());
    }

    let url = format!("{url}{SIGNATURE_SUFFIX}");
    let signature = fetch_text(agent, &url).context("reading the signature of the release")?;
    let signature = hex::decode(signature.trim()).context("the signature is not hexadecimal")?;

    for key in public_keys {
        let key = hex::decode(key).context("a key in PUBLIC_KEYS is not hexadecimal")?;
        let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key);

        if key.verify(manifest, &signature).is_ok() {
            return Ok(());
        }
    }

    bail!("the release is signed by no key this version of peekr accepts")
}

fn fetch_text(agent: &ureq::Agent, url: &str) -> Result<String> {
    Ok(agent
        .get(url)
        .call()
        .with_context(|| format!("requesting {url}"))?
        .into_body()
        .read_to_string()?)
}

/// Downloads one gzip-compressed file, checking its SHA-256 before putting it in place, so that
/// an interrupted or corrupted download never looks complete.
fn download(agent: &ureq::Agent, file: &ManifestFile, destination: &Path, progress: &Progress) -> Result<()> {
    fs::create_dir_all(destination.parent().context("destination has no parent directory")?)?;

    let compressed = with_suffix(destination, ".part");
    fetch(agent, file, &compressed, progress)?;

    let part = with_suffix(destination, ".unpacked");
    let mut body = GzDecoder::new(File::open(&compressed)?);
    let mut out = BufWriter::new(File::create(&part)?);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; CHUNK_SIZE];

    loop {
        let read = body.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        out.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    out.flush()?;
    drop(out);

    let _ = fs::remove_file(&compressed);

    let actual = hex::encode(hasher.finalize());
    if actual != file.sha256 {
        let _ = fs::remove_file(&part);
        bail!("SHA-256 mismatch: expected {}, got {actual}", file.sha256);
    }

    // Nothing else carries a file mode from the release to here, and the one file that needs one
    // is the executable the app restarts into.
    #[cfg(unix)]
    if file.executable {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(&part, fs::Permissions::from_mode(0o755))?;
    }

    fs::rename(&part, destination)?;
    Ok(())
}

/// Fetches the compressed file, picking up where an earlier attempt left off and starting over as
/// often as it takes. Only the user calls this off.
///
/// A download that resumes is what makes both halves of that possible: an attempt can be given a
/// short deadline, so that a connection which has gone quiet is noticed and the cancel is acted
/// on, without a slow line ever losing the ground it gained.
fn fetch(agent: &ureq::Agent, file: &ManifestFile, part: &Path, progress: &Progress) -> Result<()> {
    let mut delay = RETRY_DELAY;
    let size = |part: &Path| fs::metadata(part).map_or(0, |part| part.len());

    loop {
        let have = size(part);

        // The bar counts from what is really on disk, however many attempts that took.
        progress.restart();
        progress.downloaded(have);

        if have >= file.size {
            return Ok(());
        }

        let outcome = attempt(agent, file, part, have, progress);
        if progress.is_cancelled() {
            bail!("cancelled");
        }

        // Ground gained is ground gained, whether the attempt ran out of time or ran out of file:
        // go straight round again and let the size decide whether that was the last of it.
        if size(part) > have {
            delay = RETRY_DELAY;
            continue;
        }

        match outcome {
            Ok(()) => log::warn!("{}: the server sent nothing", file.path),
            Err(e) => log::warn!("{}: {e:#}", file.path),
        }

        if !progress.waiting(delay) {
            bail!("cancelled");
        }

        delay = (delay * 2).min(MAX_RETRY_DELAY);
    }
}

/// One go at the rest of the file, appended to what is already there.
fn attempt(agent: &ureq::Agent, file: &ManifestFile, part: &Path, have: u64, progress: &Progress) -> Result<()> {
    let request = agent.get(&file.url);
    let response = if have > 0 {
        request.header("Range", &format!("bytes={have}-")).call()?
    } else {
        request.call()?
    };

    // A server that ignores the range hands back the whole file; appending that to what is
    // already there would make nonsense of it, so the file starts again instead.
    let resuming = response.status().as_u16() == 206;
    let mut out = BufWriter::new(
        fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resuming)
            .truncate(!resuming)
            .open(part)?,
    );
    if !resuming && have > 0 {
        progress.restart();
    }

    let mut body = response.into_body().into_reader();
    let mut buffer = vec![0; CHUNK_SIZE];

    loop {
        let read = body.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        out.write_all(&buffer[..read])?;
        progress.downloaded(read as u64);

        if progress.is_cancelled() {
            bail!("cancelled");
        }
    }

    out.flush()?;
    Ok(())
}

fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(timeout)
        .user_agent(concat!("peekr/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Whether the app may replace its own files. An installation a package manager owns belongs to
/// the system, and writing into it would need privileges the app does not (and should not) have.
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".peekr-writable");
    let writable = File::create(&probe).is_ok();

    let _ = fs::remove_file(&probe);
    writable
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; CHUNK_SIZE];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        hasher.update(&buffer[..read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path);
    name.push(suffix);

    PathBuf::from(name)
}

/// Turns a path from the manifest into a relative path inside the installation. The manifest
/// comes off the network, so it does not get to name a file outside of it.
fn safe_path(path: &str) -> Result<PathBuf> {
    let parts: Vec<&str> = path.split('/').collect();

    let escapes = |part: &&str| matches!(*part, "" | "." | "..") || part.contains(['\\', ':']);

    if parts.iter().any(escapes) {
        bail!("{path:?} does not name a file inside the installation");
    }

    Ok(parts.iter().collect())
}

/// Whether `candidate` is a later release than `current`. Both are `major.minor.patch`; anything
/// after the patch number marks a pre-release, which comes before the release of that version.
fn is_newer(candidate: &str, current: &str) -> bool {
    match (version_key(candidate), version_key(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        // A version neither side can read is no reason to replace anything.
        _ => false,
    }
}

/// The three numbers of a version and whether it is a final release, in an order that compares
/// the way versions are meant to.
fn version_key(version: &str) -> Option<(u32, u32, u32, bool)> {
    let (numbers, release) = match version.split_once('-') {
        Some((numbers, _)) => (numbers, false),
        None => (version, true),
    };

    let mut parts = numbers.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;

    parts.next().is_none().then_some((major, minor, patch, release))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::BufRead;
    use std::net::TcpListener;

    use flate2::write::GzEncoder;

    use super::*;

    /// A directory of its own for a test that touches the file system, removed when it is done.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("peekr-update-{name}"));

            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a temporary directory");

            Self(path)
        }

        fn write(&self, path: &str, contents: &str) -> PathBuf {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().expect("a parent directory")).expect("a directory");
            fs::write(&path, contents).expect("a written file");

            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_later_release_is_an_update() {
        assert!(is_newer("0.4.0", "0.3.0"));
        assert!(is_newer("0.3.1", "0.3.0"));
        // Not what comparing the versions as text would say.
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn the_same_or_an_earlier_release_is_not() {
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.3.0", "0.4.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
    }

    #[test]
    fn a_pre_release_comes_before_the_release_it_leads_to() {
        assert!(!is_newer("0.4.0-rc.1", "0.4.0"));
        assert!(is_newer("0.4.0", "0.4.0-rc.1"));
        assert!(is_newer("0.4.0-rc.1", "0.3.9"));
    }

    #[test]
    fn an_unreadable_version_is_never_an_update() {
        assert!(!is_newer("latest", "0.3.0"));
        assert!(!is_newer("0.4", "0.3.0"));
        assert!(!is_newer("0.4.0.1", "0.3.0"));
        assert!(!is_newer("0.4.0", "nightly"));
    }

    #[test]
    fn a_manifest_path_stays_inside_the_installation() {
        assert_eq!(safe_path("peekr.exe").expect("a file"), Path::new("peekr.exe"));
        assert_eq!(
            safe_path("models/det/model.onnx").expect("a file"),
            Path::new("models").join("det").join("model.onnx")
        );

        for path in [
            "",
            "/etc/passwd",
            "../peekr.exe",
            "models/../../x",
            "./x",
            "C:/x",
            "a\\b",
        ] {
            assert!(safe_path(path).is_err(), "{path:?} should be rejected");
        }
    }

    #[test]
    fn applying_an_update_moves_the_staged_files_into_place() {
        let temp = TempDir::new("apply");
        temp.write("peekr", "old executable");
        temp.write("models/det/model.onnx", "old model");
        temp.write(".update/peekr", "new executable");
        temp.write(".update/models/det/model.onnx", "new model");

        let install = Install {
            dir: temp.0.clone(),
            executable: "peekr".to_owned(),
        };
        let manifest = Manifest {
            version: "9.9.9".to_owned(),
            files: vec![entry("peekr"), entry("models/det/model.onnx")],
        };

        let restart = install.apply(&manifest).expect("the update is applied");

        assert!(restart, "replacing the executable calls for a restart");
        assert_eq!(
            fs::read_to_string(temp.0.join("peekr")).expect("the file"),
            "new executable"
        );
        assert_eq!(
            fs::read_to_string(temp.0.join("models/det/model.onnx")).expect("the file"),
            "new model"
        );
        assert!(!install.staging().exists(), "the staging directory is cleared");
    }

    #[test]
    fn a_file_missing_from_the_staging_directory_is_left_alone() {
        let temp = TempDir::new("partial");
        temp.write("peekr", "old executable");
        temp.write("README.md", "old docs");
        temp.write(".update/README.md", "new docs");

        let install = Install {
            dir: temp.0.clone(),
            executable: "peekr".to_owned(),
        };
        let manifest = Manifest {
            version: "9.9.9".to_owned(),
            files: vec![entry("peekr"), entry("README.md")],
        };

        let restart = install.apply(&manifest).expect("the update is applied");

        assert!(
            !restart,
            "the executable did not change, so there is nothing to restart into"
        );
        assert_eq!(
            fs::read_to_string(temp.0.join("peekr")).expect("the file"),
            "old executable"
        );
        assert_eq!(
            fs::read_to_string(temp.0.join("README.md")).expect("the file"),
            "new docs"
        );
    }

    #[test]
    fn a_failure_part_way_through_puts_the_installation_back() {
        let temp = TempDir::new("rollback");
        temp.write("peekr", "the old executable");
        // A file where the next entry needs a directory, so installing it cannot work.
        temp.write("licenses", "not a directory");
        temp.write(".update/peekr", "the new executable");
        temp.write(".update/licenses/third-party.html", "new licenses");

        let install = Install {
            dir: temp.0.clone(),
            executable: "peekr".to_owned(),
        };
        let manifest = Manifest {
            version: "9.9.9".to_owned(),
            files: vec![entry("peekr"), entry("licenses/third-party.html")],
        };

        let applied = install.apply(&manifest);

        assert!(applied.is_err(), "the update could not be installed");
        assert_eq!(
            fs::read_to_string(temp.0.join("peekr")).expect("the file"),
            "the old executable",
            "the file that was already installed went back"
        );
        assert_eq!(
            fs::read_to_string(install.staging().join("peekr")).expect("the file"),
            "the new executable",
            "and is staged again, ready for another attempt"
        );
    }

    #[test]
    fn installing_a_file_puts_it_in_place() {
        let temp = TempDir::new("replace");
        let destination = temp.write("peekr", "old executable");
        let source = temp.write("new-peekr", "new executable");

        install_file(&source, &destination).expect("the file is installed");

        assert_eq!(fs::read_to_string(&destination).expect("the file"), "new executable");
        assert!(!source.exists(), "the staged file was moved, not copied");
    }

    /// The case the whole updater rests on: Windows will not let a running executable be written
    /// over, so it is renamed aside first. Run for real, with a copy of this test binary as the
    /// running executable, because nothing short of a live process locks a file this way.
    #[cfg(windows)]
    #[test]
    fn a_running_executable_is_replaced() {
        let temp = TempDir::new("running");
        let destination = temp.0.join("peekr.exe");
        fs::copy(std::env::current_exe().expect("the test binary"), &destination).expect("a copy of it");
        let source = temp.write("staged.exe", "new executable");

        let mut child = Command::new(&destination)
            .args(["--exact", "--ignored", "update::tests::keeps_this_binary_running"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the copy runs");

        // A running executable cannot be opened for writing, which says the image is mapped.
        let locked = (0..100).any(|_| {
            let unlocked = fs::OpenOptions::new().write(true).open(&destination).is_ok();
            if unlocked {
                std::thread::sleep(Duration::from_millis(20));
            }

            !unlocked
        });

        let replaced = locked.then(|| install_file(&source, &destination));

        let _ = child.kill();
        let _ = child.wait();

        assert!(locked, "the copy did not start, so there was nothing to replace");
        replaced
            .expect("a running process")
            .expect("its executable is replaced");
        assert_eq!(fs::read_to_string(&destination).expect("the file"), "new executable");
        assert!(
            with_suffix(&destination, REPLACED_SUFFIX).exists(),
            "the old executable waits to be deleted on the next start"
        );
    }

    /// Not a test: the process `a_running_executable_is_replaced` replaces the executable of.
    /// Ignored, so that it only runs when that test asks for it by name.
    #[cfg(windows)]
    #[test]
    #[ignore = "helper process for a_running_executable_is_replaced"]
    fn keeps_this_binary_running() {
        std::thread::sleep(Duration::from_secs(5));
    }

    /// Serves the files of a made-up release over HTTP, for as long as the tests keep asking.
    fn serve(files: HashMap<String, Vec<u8>>) -> String {
        serve_counting(files).0
    }

    /// The same, with a count of the bytes actually sent: what tells a download that resumed from
    /// one that quietly started over.
    fn serve_counting(files: HashMap<String, Vec<u8>>) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&sent);

        let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
        let address = format!("http://{}", listener.local_addr().expect("a local address"));

        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = std::io::BufReader::new(&stream);

                let mut request = String::new();
                reader.read_line(&mut request).expect("a request line");

                // Only the range matters here; a release server honours it, so this one has to
                // as well, or a download could never be shown to resume.
                let mut from = 0;
                let mut header = String::new();
                while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
                    if let Some(range) = header.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        from = range.trim_end().trim_end_matches('-').parse().unwrap_or(0);
                    }
                    header.clear();
                }

                let path = request.split(' ').nth(1).unwrap_or_default();
                let body = files.get(path);

                let mut stream = &stream;
                let status = match (body.is_some(), from) {
                    (false, _) => "404 Not Found",
                    (true, 0) => "200 OK",
                    (true, _) => "206 Partial Content",
                };
                let body = body.map(Vec::as_slice).unwrap_or_default();
                let body = body.get(from.min(body.len())..).unwrap_or_default();

                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
                counter.fetch_add(body.len(), std::sync::atomic::Ordering::Relaxed);
            }
        });

        (address, sent)
    }

    fn gzip(contents: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(contents.as_bytes()).expect("a compressed copy");

        encoder.finish().expect("a compressed copy")
    }

    fn digest(contents: &str) -> String {
        hex::encode(Sha256::digest(contents.as_bytes()))
    }

    /// A manifest entry for a file that is only ever moved around, never downloaded.
    fn entry(path: &str) -> ManifestFile {
        ManifestFile {
            path: path.to_owned(),
            sha256: String::new(),
            size: 0,
            executable: false,
            url: String::new(),
        }
    }

    #[test]
    fn staging_downloads_only_the_files_that_differ() {
        let temp = TempDir::new("stage");
        temp.write("peekr", "the old executable");
        temp.write("models/det/model.onnx", "a model nobody touched");

        let (executable, model) = (gzip("the new executable"), gzip("a model nobody touched"));
        let (executable_size, model_size) = (executable.len() as u64, model.len() as u64);
        let address = serve(HashMap::from([
            ("/peekr.gz".to_owned(), executable),
            ("/model.onnx.gz".to_owned(), model),
        ]));

        let install = Install {
            dir: temp.0.clone(),
            executable: "peekr".to_owned(),
        };
        let manifest = Manifest {
            version: "9.9.9".to_owned(),
            files: vec![
                ManifestFile {
                    path: "peekr".to_owned(),
                    sha256: digest("the new executable"),
                    size: executable_size,
                    executable: false,
                    url: format!("{address}/peekr.gz"),
                },
                ManifestFile {
                    path: "models/det/model.onnx".to_owned(),
                    sha256: digest("a model nobody touched"),
                    size: model_size,
                    executable: false,
                    url: format!("{address}/model.onnx.gz"),
                },
            ],
        };

        let outdated = install.outdated(&manifest).expect("the installed files are read");
        assert_eq!(outdated, [0], "the unchanged model is not downloaded again");

        let progress = Progress::new(0);
        install
            .stage(&manifest, &outdated, &progress)
            .expect("the update is staged");

        assert_eq!(
            fs::read_to_string(install.staging().join("peekr")).expect("the staged file"),
            "the new executable"
        );
        assert_eq!(
            install.staged().as_ref(),
            Some(&manifest),
            "the update is ready to apply"
        );
    }

    #[test]
    fn a_download_picks_up_where_the_last_attempt_stopped() {
        let temp = TempDir::new("resume");
        let body = gzip("the new executable");
        let (half, size) = (body.len() / 2, body.len() as u64);

        // The server hands back only what was asked for, so the half already on disk is the half
        // that is not sent again.
        let (address, sent) = serve_counting(HashMap::from([("/peekr.gz".to_owned(), body.clone())]));
        let part = temp.0.join("peekr.part");
        fs::write(&part, &body[..half]).expect("half a download");

        let file = ManifestFile {
            path: "peekr".to_owned(),
            sha256: digest("the new executable"),
            size,
            executable: false,
            url: format!("{address}/peekr.gz"),
        };

        let progress = Progress::new(size);
        download(&agent(None), &file, &temp.0.join("peekr"), &progress).expect("the rest arrives");

        assert_eq!(
            fs::read_to_string(temp.0.join("peekr")).expect("the file"),
            "the new executable"
        );
        assert_eq!(
            sent.load(std::sync::atomic::Ordering::Relaxed),
            body.len() - half,
            "only the part that was missing came over the wire"
        );
        assert!(!part.exists(), "the partial download is cleared away");
    }

    #[test]
    fn the_executable_is_the_last_file_installed() {
        let temp = TempDir::new("order");
        let install = Install {
            dir: temp.0.clone(),
            executable: "peekr".to_owned(),
        };

        // Listed first, where it would otherwise be installed first.
        let manifest = Manifest {
            version: "9.9.9".to_owned(),
            files: vec![entry("peekr"), entry("models/det/model.onnx"), entry("README.md")],
        };

        let order: Vec<&str> = install
            .install_order(&manifest)
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert_eq!(order, ["models/det/model.onnx", "README.md", "peekr"]);
    }

    #[test]
    fn a_download_gives_up_only_when_the_user_says_so() {
        let temp = TempDir::new("cancel");
        // Nothing is served here, so every attempt fails and the retry would go on for ever.
        let address = serve(HashMap::new());

        let file = ManifestFile {
            path: "peekr".to_owned(),
            sha256: digest("never arrives"),
            size: 1024,
            executable: false,
            url: format!("{address}/peekr.gz"),
        };

        let progress = Arc::new(Progress::new(file.size));
        std::thread::spawn({
            let progress = Arc::clone(&progress);
            move || {
                std::thread::sleep(Duration::from_millis(300));
                progress.cancel();
            }
        });

        let failed = download(&agent(None), &file, &temp.0.join("peekr"), &progress).expect_err("no file arrives");

        assert!(
            format!("{failed:#}").contains("cancelled"),
            "it stopped because it was told to"
        );
    }

    /// A downloaded file is written by this process, so it gets this process's default mode, and
    /// a peekr that is not executable cannot start again.
    #[cfg(unix)]
    #[test]
    fn the_executable_arrives_runnable() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = TempDir::new("mode");
        let bodies = [("peekr", "the new executable"), ("model.onnx", "a model")];
        let sizes: HashMap<&str, u64> = bodies
            .iter()
            .map(|(name, body)| (*name, gzip(body).len() as u64))
            .collect();
        let address = serve(
            bodies
                .iter()
                .map(|(name, body)| (format!("/{name}.gz"), gzip(body)))
                .collect(),
        );

        let agent = agent(None);
        let progress = Progress::new(0);
        let mode = |name: &str, contents: &str, executable: bool| {
            let file = ManifestFile {
                path: name.to_owned(),
                sha256: digest(contents),
                size: sizes[name],
                executable,
                url: format!("{address}/{name}.gz"),
            };
            let destination = temp.0.join(name);

            download(&agent, &file, &destination, &progress).expect("the file is downloaded");
            fs::metadata(&destination).expect("the file").permissions().mode() & 0o111
        };

        assert_ne!(
            mode("peekr", "the new executable", true),
            0,
            "the executable can be run"
        );
        assert_eq!(mode("model.onnx", "a model", false), 0, "and nothing else is");
    }

    #[test]
    fn a_download_that_does_not_match_its_checksum_is_thrown_away() {
        let temp = TempDir::new("corrupt");
        temp.write("peekr", "the old executable");

        let body = gzip("something else entirely");
        let size = body.len() as u64;
        let address = serve(HashMap::from([("/peekr.gz".to_owned(), body)]));

        let install = Install {
            dir: temp.0.clone(),
            executable: "peekr".to_owned(),
        };
        let manifest = Manifest {
            version: "9.9.9".to_owned(),
            files: vec![ManifestFile {
                path: "peekr".to_owned(),
                sha256: digest("the new executable"),
                size,
                executable: false,
                url: format!("{address}/peekr.gz"),
            }],
        };

        let progress = Progress::new(0);
        let staged = install.stage(&manifest, &[0], &progress);

        assert!(staged.is_err(), "the download is rejected");
        assert!(install.staged().is_none(), "and nothing is left to apply");
        assert_eq!(
            fs::read_to_string(temp.0.join("peekr")).expect("the file"),
            "the old executable"
        );
    }

    #[test]
    fn a_release_is_taken_only_from_a_key_this_version_accepts() {
        let signer = |seed: [u8; 32]| ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed).expect("a key");
        let (old, new) = (signer([7u8; 32]), signer([8u8; 32]));
        let manifest = b"version = \"9.9.9\"\n";

        let address = serve(HashMap::from([
            (
                "/by-old.toml.sig".to_owned(),
                hex::encode(old.sign(manifest).as_ref()).into_bytes(),
            ),
            (
                "/by-new.toml.sig".to_owned(),
                hex::encode(new.sign(manifest).as_ref()).into_bytes(),
            ),
            ("/forged.toml.sig".to_owned(), hex::encode([0u8; 64]).into_bytes()),
        ]));

        let public =
            |pair: &ring::signature::Ed25519KeyPair| hex::encode(ring::signature::KeyPair::public_key(pair).as_ref());
        let (old, new) = (public(&old), public(&new));

        let agent = agent(None);
        let check = |keys: &[&str], path: &str, manifest: &[u8]| {
            verify_against(keys, &agent, &format!("{address}{path}"), manifest).is_ok()
        };

        assert!(check(&[&old], "/by-old.toml", manifest), "the release is ours");
        assert!(
            !check(&[&old], "/by-old.toml", b"version = \"0.0.1\"\n"),
            "a manifest that was tampered with is not"
        );
        assert!(
            !check(&[&old], "/forged.toml", manifest),
            "and neither is a forged signature"
        );

        // Retiring a key: both are accepted while the new one is finding its way to people, and
        // either one may sign meanwhile.
        assert!(!check(&[&old], "/by-new.toml", manifest), "a key not yet trusted");
        assert!(check(&[&old, &new], "/by-new.toml", manifest), "once it is listed");
        assert!(
            check(&[&old, &new], "/by-old.toml", manifest),
            "and the old one still signs"
        );
        assert!(
            !check(&[&new], "/by-old.toml", manifest),
            "until the old one is dropped"
        );

        assert!(
            check(&[], "/forged.toml", b"anything at all"),
            "without a key compiled in there is nothing to check against"
        );
    }

    #[test]
    fn a_manifest_round_trips_through_toml() {
        let manifest = Manifest {
            version: "0.4.0".to_owned(),
            files: vec![ManifestFile {
                path: "models/rec/pp-ocrv6-small/model.onnx".to_owned(),
                sha256: "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634".to_owned(),
                size: 18_874_368,
                executable: false,
                url: "https://example.invalid/model.invalid.gz".to_owned(),
            }],
        };

        let text = toml::to_string(&manifest).expect("serializable");
        let parsed: Manifest = toml::from_str(&text).expect("deserializable");

        assert_eq!(parsed, manifest);
    }
}
