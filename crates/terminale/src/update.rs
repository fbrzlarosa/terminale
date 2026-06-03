//! Self-update from GitHub releases — safe and non-destructive.
//!
//! Safety model (the whole reason this module is careful):
//! - The running process is NEVER killed. We replace the on-disk binary
//!   atomically — `self_replace` handles the Windows running-exe rename dance
//!   and the Unix unlink-and-replace — so the live session keeps running and
//!   the new version applies only on the next launch. No tabs, scrollback or
//!   PTYs are ever lost, and we never force a restart.
//! - Downloads go over HTTPS from the official GitHub release only.
//! - The downloaded archive's SHA-256 is verified against the published
//!   `<asset>.sha256` sidecar before anything touches the installed binary; a
//!   mismatch aborts the update. (Full tamper protection would need
//!   code-signing, which this project does not do yet; the checksum guards
//!   against corrupted or truncated downloads.)
//!
//! All functions here are blocking and do real network I/O, so callers run them
//! off the UI thread (a background `std::thread`) and report back via the event
//! loop — never inline in `about_to_wait`.

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::Path;

const OWNER: &str = "fbrzlarosa";
const REPO: &str = "terminale";

/// The semver this binary was built as.
#[must_use]
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Fetch the latest published release version (semver, no leading `v`).
pub fn latest_version() -> Result<String> {
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(OWNER)
        .repo_name(REPO)
        .build()?
        .fetch()?;
    let latest = releases.first().context("no releases found")?;
    Ok(latest.version.clone())
}

/// `Some(new_version)` if a newer release than the running binary exists,
/// `None` if we're already up to date.
pub fn check_for_update() -> Result<Option<String>> {
    let latest = latest_version()?;
    if self_update::version::bump_is_greater(current_version(), &latest)? {
        Ok(Some(latest))
    } else {
        Ok(None)
    }
}

/// Download the latest release asset for this target, verify its SHA-256, and
/// atomically replace the on-disk binary. The running process is untouched; the
/// new version applies on the next launch. Returns the staged version, or
/// `Ok(None)` when already up to date.
pub fn download_and_stage() -> Result<Option<String>> {
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(OWNER)
        .repo_name(REPO)
        .build()?
        .fetch()?;
    let latest = releases.first().context("no releases found")?;
    if !self_update::version::bump_is_greater(current_version(), &latest.version)? {
        return Ok(None);
    }

    let target = self_update::get_target();
    // cargo-dist asset names are lowercase, so a plain case-sensitive suffix
    // check is correct here.
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let asset = latest
        .assets
        .iter()
        .find(|a| {
            a.name.contains(target) && (a.name.ends_with(".tar.gz") || a.name.ends_with(".zip"))
        })
        .ok_or_else(|| anyhow!("no release asset matching target {target}"))?;
    let sum_name = format!("{}.sha256", asset.name);
    let sum_asset = latest
        .assets
        .iter()
        .find(|a| a.name == sum_name)
        .ok_or_else(|| anyhow!("no {sum_name} checksum published for this release"))?;

    let tmp = tempfile::tempdir().context("create temp dir for download")?;
    let archive = tmp.path().join(&asset.name);

    // Download archive + checksum over HTTPS.
    download_to_file(&asset.download_url, &archive)?;
    let expected = parse_sha256(&download_to_string(&sum_asset.download_url)?);

    // Verify BEFORE touching the installed binary.
    let actual = sha256_of(&archive)?;
    if expected.is_empty() || !actual.eq_ignore_ascii_case(&expected) {
        bail!(
            "checksum mismatch for {} (expected {expected:?}, got {actual}) — refusing to install",
            asset.name
        );
    }

    // Extract the binary and atomically replace ourselves on disk.
    let bin = if cfg!(windows) {
        "terminale.exe"
    } else {
        "terminale"
    };
    let out = tmp.path().join("extracted");
    std::fs::create_dir_all(&out)?;
    self_update::Extract::from_source(&archive)
        .extract_file(&out, bin)
        .with_context(|| format!("extract {bin} from {}", asset.name))?;
    self_replace::self_replace(out.join(bin)).context("atomically replace the running binary")?;

    Ok(Some(latest.version.clone()))
}

fn download_to_file(url: &str, dest: &Path) -> Result<()> {
    let mut file = std::fs::File::create(dest)?;
    self_update::Download::from_url(url)
        .download_to(&mut file)
        .with_context(|| format!("download {url}"))?;
    Ok(())
}

fn download_to_string(url: &str) -> Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    self_update::Download::from_url(url)
        .download_to(&mut buf)
        .with_context(|| format!("download {url}"))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// A cargo-dist `.sha256` file is `"<hex>  <filename>"`; take the first token.
fn parse_sha256(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_owned()
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    // sha2 0.11 (digest 0.11) no longer implements `io::Write` on the hasher
    // and `finalize()` returns a hybrid-array `Array` with no `LowerHex` impl,
    // so we feed it in chunks and hex-encode the digest bytes by hand.
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        write!(hex, "{b:02x}").expect("writing to a String never fails");
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sha256_takes_first_token() {
        assert_eq!(parse_sha256("abc123  terminale.tar.gz\n"), "abc123");
        assert_eq!(parse_sha256("deadbeef"), "deadbeef");
        assert_eq!(parse_sha256(""), "");
    }

    #[test]
    fn current_version_is_set() {
        assert!(!current_version().is_empty());
    }

    #[test]
    fn sha256_of_matches_known_vector() {
        use std::io::Write as _;
        // The canonical SHA-256 test vector: sha256("abc").
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(b"abc").expect("write");
        f.flush().expect("flush");
        assert_eq!(
            sha256_of(f.path()).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
    }
}
