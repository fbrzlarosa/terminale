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
use std::path::{Path, PathBuf};

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

/// What an update attempt actually did. Callers turn this into user-facing
/// copy — the three success shapes need very different follow-up actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// Already running the latest published version.
    UpToDate,
    /// The new binary was verified and swapped on disk; it applies on the
    /// next launch. The running session is untouched.
    Staged(String),
    /// The install location is not writable from this process (typically an
    /// MSI install under `Program Files`), so the platform installer was
    /// downloaded, verified, and launched — the user finishes the update in
    /// its UI (Windows handles the elevation prompt).
    InstallerLaunched(String),
    /// Non-interactive contexts only (startup auto-update): a newer version
    /// exists but applying it needs the platform installer, which would pop
    /// UI/elevation prompts unprompted — so nothing was launched.
    InstallerRequired(String),
}

/// Download the latest release for this target, verify its SHA-256, and apply
/// it. Two strategies:
///
/// * **Writable install** (zip/tarball/portable/dev): extract the binary and
///   atomically replace the on-disk image — `self_replace` handles the
///   Windows running-exe rename dance — returning [`UpdateOutcome::Staged`].
///   The running process is untouched; the new version applies on the next
///   launch.
/// * **Non-writable install** (MSI under `Program Files`): in-place
///   replacement is impossible without elevation, and silently rewriting a
///   Windows-Installer-managed tree would desync the MSI database anyway.
///   With `interactive = true` the `.msi` for the new version is downloaded,
///   checksum-verified, and handed to `msiexec` ([`UpdateOutcome::
///   InstallerLaunched`]); with `interactive = false` nothing is launched and
///   [`UpdateOutcome::InstallerRequired`] is returned so the caller can
///   notify instead.
pub fn download_and_apply(interactive: bool) -> Result<UpdateOutcome> {
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(OWNER)
        .repo_name(REPO)
        .build()?
        .fetch()?;
    let latest = releases.first().context("no releases found")?;
    if !self_update::version::bump_is_greater(current_version(), &latest.version)? {
        return Ok(UpdateOutcome::UpToDate);
    }

    // macOS: when we're running from a `.app` bundle, update by swapping the
    // WHOLE bundle, not the inner binary. Replacing just the Mach-O would
    // invalidate the bundle's (ad-hoc) code signature and Gatekeeper would
    // reject the app as "damaged". This path is taken before the generic
    // writable-dir check below because the inner `Contents/MacOS` dir is itself
    // writable, which would otherwise route us into the wrong (binary-swap)
    // strategy.
    #[cfg(target_os = "macos")]
    {
        if let Some(bundle) = macos_app_bundle_path() {
            return apply_macos_bundle_swap(latest, &bundle, interactive);
        }
    }

    if !install_dir_is_writable() {
        return apply_via_installer(latest, interactive);
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

    let tmp = tempfile::tempdir().context("create temp dir for download")?;
    let archive = tmp.path().join(&asset.name);
    download_and_verify(&latest.version, &asset.name, &latest.assets, &archive)?;

    // Extract the binary and atomically replace ourselves on disk.
    let bin = if cfg!(windows) {
        "terminale.exe"
    } else {
        "terminale"
    };
    let out = tmp.path().join("extracted");
    std::fs::create_dir_all(&out)?;

    let candidates = archive_bin_candidates(&asset.name, bin);

    // Whichever candidate matches, the file lands at `out.join(candidate)`:
    // both the tar (`unpack_in`) and zip backends preserve the entry's full
    // path relative to the output dir.
    let mut extracted: Option<std::path::PathBuf> = None;
    let mut last_err = None;
    for cand in &candidates {
        match self_update::Extract::from_source(&archive).extract_file(&out, cand) {
            Ok(()) => {
                extracted = Some(out.join(cand));
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    let extracted = extracted.ok_or_else(|| {
        let tried = candidates.join(", ");
        match last_err {
            Some(e) => anyhow!("extract {bin} from {} (tried: {tried}): {e}", asset.name),
            None => anyhow!("extract {bin} from {} (tried: {tried})", asset.name),
        }
    })?;
    self_replace::self_replace(extracted).context("atomically replace the running binary")?;

    Ok(UpdateOutcome::Staged(latest.version.clone()))
}

/// Candidate archive-internal paths for the binary, in priority order.
///
/// cargo-dist's layout is not uniform: the unix `.tar.gz` nests the binary
/// under a top-level directory named after the archive stem
/// (`terminale-x86_64-apple-darwin/terminale`), while the Windows `.zip` keeps
/// it flat at the root (`terminale.exe`). `self_update`'s `extract_file`
/// matches the entry path *exactly*, so we try the nested path first and fall
/// back to the bare name — a single code path covering both layouts (and a
/// future cargo-dist change in either direction) without guessing.
fn archive_bin_candidates(asset_name: &str, bin: &str) -> [String; 2] {
    let stem = asset_name
        .strip_suffix(".tar.gz")
        .or_else(|| asset_name.strip_suffix(".zip"))
        .unwrap_or(asset_name);
    [format!("{stem}/{bin}"), bin.to_string()]
}

/// Can this process create files in the directory the running binary lives
/// in? Probed with a real `create_new` + delete, which is the only reliable
/// answer on Windows (ACLs) and Unix (mount flags, ownership) alike.
fn install_dir_is_writable() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    dir_is_writable(dir)
}

/// Can this process create (and remove) a file in `dir`? Real `create_new`
/// probe — the only reliable test across Windows ACLs and Unix ownership/mount
/// flags. Used both for the install dir and, on macOS, for the `.app` bundle's
/// parent (e.g. `/Applications`, which admin users can write but standard users
/// cannot).
fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".terminale-update-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// If `exe` lives inside a macOS application bundle
/// (`…/Foo.app/Contents/MacOS/foo`), return the bundle directory
/// (`…/Foo.app`). `None` for a bare binary anywhere else. Pure path logic —
/// unit-testable on any OS (only wired into the updater on macOS).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn app_bundle_from_exe(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?; // …/Contents/MacOS
    if macos.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?; // …/Contents
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let app = contents.parent()?; // …/Foo.app
    if app.extension()?.to_str()? != "app" {
        return None;
    }
    Some(app.to_path_buf())
}

/// Release asset name for the zipped `.app` bundle of `target` (e.g.
/// `terminale-aarch64-apple-darwin-app.zip`). Mirrors the name the release
/// workflow uploads.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_app_asset_name(target: &str) -> String {
    format!("terminale-{target}-app.zip")
}

/// The `.app` bundle we're running from, if any.
#[cfg(target_os = "macos")]
fn macos_app_bundle_path() -> Option<PathBuf> {
    app_bundle_from_exe(&std::env::current_exe().ok()?)
}

/// Update a macOS `.app` install by downloading the new bundle and swapping it
/// in place — no installer, no Gatekeeper prompt (the bundle we download
/// ourselves carries no quarantine flag), and the running session is left
/// untouched (the new bundle applies on the next launch, like every other
/// platform's staged update).
///
/// Requires the bundle's parent directory to be writable (true for `/Applications`
/// on an admin account, and always for `~/Applications`). When it isn't, there
/// is no silent path, so we point the user at the `.dmg`.
#[cfg(target_os = "macos")]
fn apply_macos_bundle_swap(
    latest: &self_update::update::Release,
    bundle: &Path,
    interactive: bool,
) -> Result<UpdateOutcome> {
    let parent = bundle
        .parent()
        .context("app bundle has no parent directory")?;
    if !dir_is_writable(parent) {
        if interactive {
            bail!(
                "terminale.app is in a location this account can't modify ({}). \
                 Update by downloading the latest .dmg and dragging it over, or move \
                 terminale.app into ~/Applications for silent auto-updates.",
                parent.display()
            );
        }
        return Ok(UpdateOutcome::InstallerRequired(latest.version.clone()));
    }

    let target = self_update::get_target();
    let asset_name = macos_app_asset_name(target);
    if !latest.assets.iter().any(|a| a.name == asset_name) {
        bail!("no {asset_name} published for this release");
    }

    let tmp = tempfile::tempdir().context("create temp dir for download")?;
    let zip = tmp.path().join(&asset_name);
    download_and_verify(&latest.version, &asset_name, &latest.assets, &zip)?;

    // Extract with `ditto`, which preserves the bundle's symlinks, permissions,
    // and code signature (plain unzip can mangle all three).
    let extracted = tmp.path().join("extracted");
    std::fs::create_dir_all(&extracted)?;
    ditto(&[
        "-x",
        "-k",
        &zip.to_string_lossy(),
        &extracted.to_string_lossy(),
    ])
    .context("extract the .app archive")?;
    let new_app = extracted.join("terminale.app");
    if !new_app.exists() {
        bail!("archive {asset_name} did not contain terminale.app");
    }
    // We downloaded the bundle ourselves so it carries no quarantine flag, but
    // strip it defensively in case a future download path is quarantine-aware.
    let _ = std::process::Command::new("xattr")
        .args(["-dr", "com.apple.quarantine", &new_app.to_string_lossy()])
        .status();

    // Stage the new bundle on the SAME volume as the target (the temp dir is on
    // a different volume, so a cross-volume rename would fail). `ditto` keeps
    // the signature intact on copy.
    let pid = std::process::id();
    let staged = parent.join(format!(".terminale-new-{pid}.app"));
    let old = parent.join(format!(".terminale-old-{pid}.app"));
    for p in [&staged, &old] {
        if p.exists() {
            std::fs::remove_dir_all(p).ok();
        }
    }
    ditto(&[&new_app.to_string_lossy(), &staged.to_string_lossy()])
        .context("stage the new bundle next to the install location")?;

    // Swap: move the live bundle aside, move the new one in, drop the old.
    // Same-volume renames are atomic and safe while the app runs (the live
    // process keeps its already-mapped image). Roll back on any failure so the
    // user is never left without an app.
    std::fs::rename(bundle, &old).context("move the current app bundle aside")?;
    match std::fs::rename(&staged, bundle) {
        Ok(()) => {
            std::fs::remove_dir_all(&old).ok();
            Ok(UpdateOutcome::Staged(latest.version.clone()))
        }
        Err(e) => {
            std::fs::rename(&old, bundle).ok();
            std::fs::remove_dir_all(&staged).ok();
            Err(e).context("install the new app bundle")
        }
    }
}

/// Run macOS `ditto` with the given args, failing on a non-zero exit.
#[cfg(target_os = "macos")]
fn ditto(args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("ditto")
        .args(args)
        .status()
        .context("run ditto (macOS)")?;
    if !status.success() {
        bail!("ditto failed (args: {args:?})");
    }
    Ok(())
}

/// Non-writable install: hand the update to the platform installer.
///
/// Windows: download + verify the release `.msi` and launch `msiexec /i` —
/// Windows Installer performs the upgrade (and shows the standard elevation
/// prompt). Elsewhere a read-only install means a package manager owns the
/// binary, so we bail with a pointer to it rather than fight the ownership.
fn apply_via_installer(
    latest: &self_update::update::Release,
    interactive: bool,
) -> Result<UpdateOutcome> {
    if !cfg!(windows) {
        bail!(
            "terminale is installed in a read-only location; update it with the package \
             manager that installed it (e.g. `brew upgrade terminale` or your distro's tool)"
        );
    }
    if !interactive {
        return Ok(UpdateOutcome::InstallerRequired(latest.version.clone()));
    }

    let target = self_update::get_target();
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let asset = latest
        .assets
        .iter()
        .find(|a| a.name.contains(target) && a.name.ends_with(".msi"))
        .ok_or_else(|| anyhow!("no .msi release asset matching target {target}"))?;

    // Persistent temp location — msiexec reads the file AFTER this function
    // returns, so a self-deleting tempdir would yank it away mid-install.
    let dir = std::env::temp_dir().join("terminale-update");
    std::fs::create_dir_all(&dir).context("create download dir for the installer")?;
    let msi = dir.join(&asset.name);
    download_and_verify(&latest.version, &asset.name, &latest.assets, &msi)?;

    // Hand off to Windows Installer: it upgrades the managed install,
    // prompts for elevation itself, and asks to close the running app.
    //
    // CREATE_BREAKAWAY_FROM_JOB (0x0100_0000): terminale confines itself to a
    // kill-on-close Job Object so its ConPTY hosts can't outlive a crash (see
    // `process_job`). msiexec MUST escape that job — otherwise quitting
    // terminale to let the upgrade proceed would kill the installer mid-flight.
    // The job is created with BREAKAWAY_OK, so this succeeds.
    //
    // `#[cfg(windows)]`-gated because `CommandExt::creation_flags` lives in
    // `std::os::windows` and would not compile elsewhere. The non-Windows path
    // never reaches here — the `!cfg!(windows)` bail above returns first — but
    // the body is still type-checked on every target, so the gate is required.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        std::process::Command::new("msiexec")
            .arg("/i")
            .arg(&msi)
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .context("launch msiexec for the downloaded installer")?;
    }
    Ok(UpdateOutcome::InstallerLaunched(latest.version.clone()))
}

/// Download release asset `name` to `dest` and verify it against its
/// published `.sha256` sidecar. Fails (and removes nothing) on any mismatch —
/// the caller's `dest` must be treated as poisoned in that case.
fn download_and_verify(
    version: &str,
    name: &str,
    assets: &[self_update::update::ReleaseAsset],
    dest: &Path,
) -> Result<()> {
    let sum_name = format!("{name}.sha256");
    if !assets.iter().any(|a| a.name == sum_name) {
        bail!("no {sum_name} checksum published for this release");
    }

    // Download archive + checksum over HTTPS — via the BROWSER download URL,
    // not the `api.github.com` asset endpoint that `asset.download_url`
    // carries. API downloads count against GitHub's unauthenticated rate
    // limit (60 requests/hour per IP) and fail with 403 once exhausted;
    // `github.com/<owner>/<repo>/releases/download/…` is the CDN path with no
    // API rate limit. Only the small release-list metadata call still touches
    // the API.
    download_to_file(&browser_download_url(version, name), dest)?;
    let expected = parse_sha256(&download_to_string(&browser_download_url(
        version, &sum_name,
    ))?);

    // Verify BEFORE anything acts on the downloaded file.
    let actual = sha256_of(dest)?;
    if expected.is_empty() || !actual.eq_ignore_ascii_case(&expected) {
        bail!(
            "checksum mismatch for {name} (expected {expected:?}, got {actual}) — refusing to \
             install. If the release was published minutes ago its assets may still be \
             uploading; retry shortly"
        );
    }
    Ok(())
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

/// Rate-limit-free download URL for a release asset. Our tags are always
/// `v{semver}` (cargo-dist), and `self_update` strips the leading `v` from
/// `Release::version`, so the tag is reconstructed here.
fn browser_download_url(version: &str, asset_name: &str) -> String {
    format!("https://github.com/{OWNER}/{REPO}/releases/download/v{version}/{asset_name}")
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

    /// Dev/test binaries live in `target/…`, which is always writable — the
    /// probe must say so (and clean up after itself; the probe file must not
    /// survive the call).
    #[test]
    fn install_dir_writable_probe_is_clean() {
        assert!(install_dir_is_writable());
        let dir = std::env::current_exe()
            .expect("current_exe")
            .parent()
            .expect("exe parent")
            .to_path_buf();
        let leftover = std::fs::read_dir(dir)
            .expect("read exe dir")
            .filter_map(Result::ok)
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".terminale-update-probe-")
            });
        assert!(!leftover, "writability probe file must be removed");
    }

    #[test]
    fn browser_download_url_uses_cdn_not_api() {
        let url = browser_download_url("0.1.14", "terminale-x86_64-pc-windows-msvc.zip");
        assert_eq!(
            url,
            "https://github.com/fbrzlarosa/terminale/releases/download/v0.1.14/terminale-x86_64-pc-windows-msvc.zip",
        );
        assert!(
            !url.contains("api.github.com"),
            "asset downloads must never go through the rate-limited API host"
        );
    }

    #[test]
    fn archive_bin_candidates_cover_both_cargo_dist_layouts() {
        // unix `.tar.gz`: the binary is nested under the archive stem, so the
        // first (nested) candidate is the one that matches.
        assert_eq!(
            archive_bin_candidates("terminale-x86_64-apple-darwin.tar.gz", "terminale"),
            [
                "terminale-x86_64-apple-darwin/terminale".to_string(),
                "terminale".to_string(),
            ],
        );
        // Windows `.zip`: the binary sits flat at the root, so the second
        // (bare-name) candidate is the one that matches.
        assert_eq!(
            archive_bin_candidates("terminale-x86_64-pc-windows-msvc.zip", "terminale.exe"),
            [
                "terminale-x86_64-pc-windows-msvc/terminale.exe".to_string(),
                "terminale.exe".to_string(),
            ],
        );
    }

    #[test]
    fn app_bundle_from_exe_detects_bundle() {
        // Inside a bundle → returns the .app dir.
        assert_eq!(
            app_bundle_from_exe(Path::new(
                "/Applications/terminale.app/Contents/MacOS/terminale"
            )),
            Some(PathBuf::from("/Applications/terminale.app"))
        );
        assert_eq!(
            app_bundle_from_exe(Path::new(
                "/Users/me/Applications/terminale.app/Contents/MacOS/terminale"
            )),
            Some(PathBuf::from("/Users/me/Applications/terminale.app"))
        );
        // Bare binaries / wrong layout → None.
        assert_eq!(
            app_bundle_from_exe(Path::new("/usr/local/bin/terminale")),
            None
        );
        assert_eq!(
            app_bundle_from_exe(Path::new("/home/me/.terminale/terminale")),
            None
        );
        // A binary in a `MacOS` dir that isn't under `Contents/*.app` → None.
        assert_eq!(app_bundle_from_exe(Path::new("/tmp/MacOS/terminale")), None);
    }

    #[test]
    fn macos_app_asset_name_matches_release_naming() {
        assert_eq!(
            macos_app_asset_name("aarch64-apple-darwin"),
            "terminale-aarch64-apple-darwin-app.zip"
        );
        assert_eq!(
            macos_app_asset_name("x86_64-apple-darwin"),
            "terminale-x86_64-apple-darwin-app.zip"
        );
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
