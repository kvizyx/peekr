//! Signing releases, so that the app installs only what this project published.
//!
//! The signature covers the update manifest, and the manifest carries an SHA-256 for every file
//! of the release, so one signature vouches for all of them.
//!
//! `cargo xtask keygen <file>` writes a key file; the public half it prints goes into
//! `PUBLIC_KEYS` in `src/update/mod.rs`. `PEEKR_SIGNING_KEY` then points at that file, or holds
//! its contents, and `PEEKR_SIGNING_KEY_PASSWORD` unlocks it when it has a password.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use ring::rand::SecureRandom as _;
use ring::signature::{Ed25519KeyPair, KeyPair as _};

/// The key file, or its contents. A file keeps the key out of the shell's history and out of the
/// process list, which is why it is what `keygen` writes; the contents are what a build server
/// has to be handed instead.
const KEY_VARIABLE: &str = "PEEKR_SIGNING_KEY";
/// The password the key file was written with, when it has one.
const PASSWORD_VARIABLE: &str = "PEEKR_SIGNING_KEY_PASSWORD";
/// Appended to the manifest's name to get the name of its signature.
const SIGNATURE_SUFFIX: &str = ".sig";

/// Marks a key file that holds the key as it is.
const PLAIN: u8 = 1;
/// Marks a key file whose key is sealed with a password.
const SEALED: u8 = 2;

const SALT: usize = 16;
const NONCE: usize = 12;
const SEED: usize = 32;
/// Deliberately slow, so that guessing the password of a stolen key file is expensive. A release
/// pays this once, which is nothing next to building one.
const ROUNDS: u32 = 600_000;

/// Writes a new key file, and prints the public half to put in `PUBLIC_KEYS`.
///
/// The key is sealed with `PEEKR_SIGNING_KEY_PASSWORD` when that is set, which is the difference
/// between a stolen laptop costing a key and costing nothing.
#[expect(clippy::print_stdout, reason = "the public key is the output of this task")]
pub fn keygen(path: &Path) -> Result<()> {
    if path.exists() {
        bail!(
            "{} already exists; a signing key is not worth overwriting by accident",
            path.display()
        );
    }

    let mut seed = [0u8; SEED];
    random(&mut seed)?;

    let pair = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|e| anyhow!("unusable key: {e}"))?;
    let password = std::env::var(PASSWORD_VARIABLE).ok();

    std::fs::write(path, hex::encode(seal(&seed, password.as_deref())?))
        .with_context(|| format!("writing {}", path.display()))?;

    eprintln!("wrote the signing key to {}", path.display());
    if password.is_none() {
        eprintln!("without a password: set {PASSWORD_VARIABLE} before generating one to seal it");
    }

    eprintln!("\nAdd this to PUBLIC_KEYS in src/update/mod.rs:\n");
    println!("{}", hex::encode(pair.public_key().as_ref()));

    eprintln!(
        "\nKeep the key file. Losing it means never being able to update the copies already out \
         there; letting it go means someone else can update them."
    );

    Ok(())
}

/// Signs the given manifests, and checks each signature against the keys the app is built with.
///
/// Signing with a key the app does not accept is the one mistake that costs nothing to make and
/// everything to discover: it goes unnoticed until an installed copy quietly refuses the update.
pub fn sign_all(root: &Path, manifests: &[&str]) -> Result<()> {
    if std::env::var_os(KEY_VARIABLE).is_none() {
        bail!("{KEY_VARIABLE} is not set; there is nothing to sign with");
    }

    let accepted = public_keys(root)?;
    if accepted.is_empty() {
        bail!("PUBLIC_KEYS in src/update/mod.rs is empty; the app would accept any release");
    }

    for manifest in manifests {
        let manifest = Path::new(manifest);
        sign(manifest)?;

        let signature = std::fs::read_to_string(with_suffix(manifest, SIGNATURE_SUFFIX))?;
        let signature = hex::decode(signature.trim()).context("the signature is not hexadecimal")?;
        let contents = std::fs::read(manifest)?;

        let signed_by = accepted.iter().position(|key| {
            let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key);
            key.verify(&contents, &signature).is_ok()
        });

        match signed_by {
            Some(index) => eprintln!("  checked against key {} of {}", index + 1, accepted.len()),
            None => bail!(
                "{} was signed with a key the app does not accept; an update signed with it would \
                 be refused by every copy already installed",
                manifest.display()
            ),
        }
    }

    Ok(())
}

/// The keys the app is built to accept, read out of `PUBLIC_KEYS` in its source. Parsed by hand,
/// as the app is a binary crate and has nothing xtask could ask.
fn public_keys(root: &Path) -> Result<Vec<Vec<u8>>> {
    let source = root.join("src").join("update").join("mod.rs");
    let source = std::fs::read_to_string(&source).with_context(|| format!("reading {}", source.display()))?;

    let list = source
        .split_once("const PUBLIC_KEYS: &[&str] = &[")
        .and_then(|(_, rest)| rest.split_once("];"))
        .context("no PUBLIC_KEYS in src/update/mod.rs")?
        .0;

    list.split('"')
        .filter(|part| part.chars().all(|c| c.is_ascii_hexdigit()) && !part.is_empty())
        .map(|key| hex::decode(key).context("a key in PUBLIC_KEYS is not hexadecimal"))
        .collect()
}

fn with_suffix(path: &Path, suffix: &str) -> std::ffi::OsString {
    let mut path = std::ffi::OsString::from(path);
    path.push(suffix);

    path
}

/// Writes `<manifest>.sig` next to the manifest, if there is a key to sign with.
///
/// Without one the release goes out unsigned, which is what a local build wants. A published
/// release must not; see RELEASE.md.
pub fn sign(manifest: &Path) -> Result<()> {
    let Some(key) = std::env::var_os(KEY_VARIABLE) else {
        eprintln!("{KEY_VARIABLE} is not set: the manifest is left unsigned");
        return Ok(());
    };

    let key = key.to_str().context("the signing key is not text")?.trim();
    // A path when it names a file, and the key itself otherwise: a build server has nowhere to
    // keep a file, and a person has no business pasting a key onto a command line.
    let key = std::fs::read_to_string(key).map_or_else(|_| key.to_owned(), |file| file.trim().to_owned());

    let sealed = hex::decode(&key).with_context(|| format!("{KEY_VARIABLE} is not hexadecimal"))?;
    let seed = unseal(&sealed, std::env::var(PASSWORD_VARIABLE).ok().as_deref())?;

    let pair = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|e| anyhow!("unusable signing key: {e}"))?;
    let contents = std::fs::read(manifest).with_context(|| format!("reading {}", manifest.display()))?;

    let path = with_suffix(manifest, SIGNATURE_SUFFIX);
    std::fs::write(&path, hex::encode(pair.sign(&contents).as_ref()))?;

    eprintln!("signed {}", manifest.display());
    Ok(())
}

/// The bytes of a key file: a marker, then either the key or a password-sealed copy of it.
fn seal(seed: &[u8; SEED], password: Option<&str>) -> Result<Vec<u8>> {
    let Some(password) = password else {
        return Ok([&[PLAIN][..], seed].concat());
    };

    let mut salt = [0u8; SALT];
    let mut nonce = [0u8; NONCE];
    random(&mut salt)?;
    random(&mut nonce)?;

    let mut sealed = seed.to_vec();
    encryption_key(password, &salt)?
        .seal_in_place_append_tag(
            ring::aead::Nonce::assume_unique_for_key(nonce),
            ring::aead::Aad::empty(),
            &mut sealed,
        )
        .map_err(|_| anyhow!("the key could not be sealed"))?;

    Ok([&[SEALED][..], &salt, &nonce, &sealed].concat())
}

fn unseal(contents: &[u8], password: Option<&str>) -> Result<Vec<u8>> {
    match contents.split_first() {
        Some((&PLAIN, seed)) if seed.len() == SEED => Ok(seed.to_vec()),
        Some((&SEALED, rest)) if rest.len() > SALT + NONCE => {
            let password = password.with_context(|| format!("the key has a password; set {PASSWORD_VARIABLE}"))?;

            let (salt, rest) = rest.split_at(SALT);
            let (nonce, sealed) = rest.split_at(NONCE);
            let nonce =
                ring::aead::Nonce::try_assume_unique_for_key(nonce).map_err(|_| anyhow!("unusable key file"))?;

            let mut sealed = sealed.to_vec();
            let seed = encryption_key(password, salt)?
                .open_in_place(nonce, ring::aead::Aad::empty(), &mut sealed)
                .map_err(|_| anyhow!("wrong password for the signing key"))?;

            Ok(seed.to_vec())
        }
        _ => bail!("this does not look like a peekr signing key"),
    }
}

/// The encryption key a password stands for. Slow on purpose; see [`ROUNDS`].
fn encryption_key(password: &str, salt: &[u8]) -> Result<ring::aead::LessSafeKey> {
    let rounds = std::num::NonZeroU32::new(ROUNDS).context("a key needs at least one round")?;
    let mut derived = [0u8; 32];

    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        salt,
        password.as_bytes(),
        &mut derived,
    );

    let key = ring::aead::UnboundKey::new(&ring::aead::CHACHA20_POLY1305, &derived)
        .map_err(|_| anyhow!("unusable encryption key"))?;

    Ok(ring::aead::LessSafeKey::new(key))
}

fn random(bytes: &mut [u8]) -> Result<()> {
    ring::rand::SystemRandom::new()
        .fill(bytes)
        .map_err(|_| anyhow!("no secure randomness available"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_verifies_against_the_public_key() {
        let seed = [7u8; SEED];
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed).expect("a key");
        let manifest = b"version = \"0.4.0\"\n";

        let signature = pair.sign(manifest);
        let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, pair.public_key().as_ref());

        assert!(key.verify(manifest, signature.as_ref()).is_ok(), "the release is ours");
        assert!(
            key.verify(b"version = \"9.9.9\"\n", signature.as_ref()).is_err(),
            "a manifest that was tampered with is not"
        );
    }

    #[test]
    fn the_keys_the_app_accepts_are_read_from_its_source() {
        let keys = public_keys(crate::project_root()).expect("PUBLIC_KEYS is readable");

        assert!(
            !keys.is_empty(),
            "releases are signed, so the app has a key to check them with"
        );
        assert!(
            keys.iter().all(|key| key.len() == 32),
            "an ed25519 public key is 32 bytes"
        );
    }

    #[test]
    fn a_key_without_a_password_is_stored_as_it_is() {
        let seed = [3u8; SEED];

        let file = seal(&seed, None).expect("a key file");

        assert_eq!(unseal(&file, None).expect("the key back"), seed);
    }

    #[test]
    fn a_sealed_key_needs_its_password() {
        let seed = [9u8; SEED];

        let file = seal(&seed, Some("open sesame")).expect("a key file");

        assert!(
            !file.windows(SEED).any(|window| window == seed),
            "the key itself is nowhere in the file"
        );
        assert_eq!(unseal(&file, Some("open sesame")).expect("the key back"), seed);
        assert!(unseal(&file, Some("open barley")).is_err(), "the wrong password");
        assert!(unseal(&file, None).is_err(), "no password at all");
    }

    #[test]
    fn rejects_anything_that_is_not_a_key_file() {
        assert!(unseal(&[], None).is_err(), "nothing");
        assert!(unseal(&[PLAIN, 1, 2, 3], None).is_err(), "too short for a key");
        assert!(unseal(&[99, 1, 2, 3], None).is_err(), "an unknown marker");
    }
}
