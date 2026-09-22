# Release

The application updates itself. When it starts, it reads the manifest of the newest release, downloads only
the files whose SHA-256 differs from the ones it has, and restarts into the new version. It
installs a release only if the manifest carries a signature made with a key the app was built to
accept.

That makes the signing key the single most important thing in this document, so losing it or letting it slip away will cause problems.

The key is never given to the build server. GitHub Actions builds the release - you sign it on your
own machine and then publish it.

## Making the key (one-time)

```bash
# Optional, but recommended (the key file is encrypted with this).
export PEEKR_SIGNING_KEY_PASSWORD='...'

cargo xtask keygen ~/.peekr/signing.key
```

The command prints the public half to standard output. Put it in `PUBLIC_KEYS` in
[`src/update/mod.rs`](src/update/mod.rs). Back up `~/.peekr/signing.key` somewhere that is not
this machine.

`keygen` refuses to overwrite an existing key file.

## Per-release

### 1. Raise the version

Edit `version` in [`Cargo.toml`](Cargo.toml). The tag has to match it, and the workflow checks.

### 2. Tag and push

```bash
git tag vX.Y.Z
git push origin main vX.Y.Z
```

The workflow runs the same checks as CI — formatting, clippy and the tests — and builds nothing
until they pass, so a tag cannot turn into a release the tests would have stopped. It then builds
Windows, Linux x86-64 and Linux aarch64 and opens a draft release with the archives, the
Windows installer and the `update-*` files.

Nothing reaches users yet: `releases/latest`, which the updater follows, skips drafts.

### 3. Sign and check

```bash
export PEEKR_SIGNING_KEY=~/.peekr/signing.key
export PEEKR_SIGNING_KEY_PASSWORD='...'   # Only if the key file has one

cargo xtask release vX.Y.Z
```

This downloads each platform's manifest, signs it, checks every signature against `PUBLIC_KEYS`,
and uploads the `.sig` files. It then confirms the draft actually has, for each of
`windows-x86_64`, `linux-x86_64` and `linux-aarch64`: the manifest, its signature, the
`update-<platform>-*.gz` payload files, and the archive (plus the Windows installer). It fails
loudly and names what is missing, instead of leaving a draft partly signed.

**A published release without its `.sig` files stops updates for everyone.** The app refuses what
it cannot verify, and it says so only in its log — this step exists so that is caught here instead.

### 4. Publish

Publishing the draft! The next time an installed copy starts, it
finds it.

## Retiring a key

A copy of peekr accepts the keys that were in `PUBLIC_KEYS` when it was built, and nothing else.
A new key therefore has to arrive in a release signed with the old one:

1. Make the new key. **Add** it to `PUBLIC_KEYS`, leaving the old one first.
2. Release, signing with the **old** key. Copies out there accept it and learn the new key.
3. Wait for that release to reach people.
4. Release again, signing with the **new** key.
5. Once nothing worth supporting is still on an older version, drop the old key from the list.

Skipping step 2 strands every copy that has not updated yet. Losing the old key before step 4
leaves no way to do this at all.

## What not to do

- Do not publish a release whose `.sig` files are missing.
- Do not replace or re-upload assets of a release that is already published. Someone may be part
  way through downloading them, and a download that resumes into a file whose contents changed
  fails its checksum.
- Do not replace a key in `PUBLIC_KEYS`; add to the list and retire as above.
- Do not put the signing key on the build server. The signature is worth having because the key
  is somewhere a compromise of this repository cannot reach.
