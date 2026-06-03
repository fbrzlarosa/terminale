//! `cargo xtask` — workspace task runner.
//!
//! Modelled after the rust-analyzer pattern: encodes CI commands as a Rust
//! binary so they stay consistent between local dev and GitHub Actions.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

#[derive(Parser)]
#[command(name = "xtask", about = "Workspace task runner for terminale")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the full CI suite (fmt + clippy + test + deny).
    Ci,
    /// Run only the formatter check.
    Fmt,
    /// Run only the clippy lints.
    Clippy,
    /// Run only the test suite.
    Test,
    /// Run only the cargo-deny policy check.
    Deny,
    /// Regenerate the Windows `.ico` and macOS `.icns` icons from the source
    /// `assets/icons/icon.svg`. Run this whenever the SVG changes, then commit
    /// the results.
    GenIcons,
    /// Assemble a macOS `terminale.app` bundle around an already-built binary
    /// (run `cargo build --release` first). Produces `target/terminale.app`,
    /// which appears in Launchpad/Spotlight with the brand icon and launches as
    /// a GUI app (instead of a bare executable that opens a terminal). Run on
    /// macOS.
    BundleMacos {
        /// Path to the built `terminale` binary. Defaults to
        /// `target/release/terminale`.
        #[arg(long)]
        bin: Option<PathBuf>,
    },
    /// Assemble the `.app` (via `bundle-macos`) and wrap it in a `.dmg` disk
    /// image — the standard macOS GUI download: open it and drag `terminale.app`
    /// onto the bundled `/Applications` shortcut. Produces
    /// `target/terminale-v<version>-<target>.dmg`. Run on macOS (needs the
    /// system `hdiutil`).
    DmgMacos {
        /// Path to the built `terminale` binary. Defaults to
        /// `target/release/terminale` (or the `--target`-specific path when
        /// `--target` is given and the default is absent).
        #[arg(long)]
        bin: Option<PathBuf>,
        /// Target triple used only for the output filename (e.g.
        /// `aarch64-apple-darwin`). Defaults to the host triple so the file
        /// matches the README/dist naming on each release runner.
        #[arg(long)]
        target: Option<String>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Ci => {
            fmt()?;
            clippy()?;
            test()?;
            deny()?;
        }
        Cmd::Fmt => fmt()?,
        Cmd::Clippy => clippy()?,
        Cmd::Test => test()?,
        Cmd::Deny => deny()?,
        Cmd::GenIcons => gen_icons()?,
        Cmd::BundleMacos { bin } => {
            bundle_macos(bin)?;
        }
        Cmd::DmgMacos { bin, target } => dmg_macos(bin, target)?,
    }
    Ok(())
}

fn fmt() -> Result<()> {
    run("cargo", &["fmt", "--all", "--", "--check"])
}

fn clippy() -> Result<()> {
    run(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )
}

fn test() -> Result<()> {
    run(
        "cargo",
        &["test", "--workspace", "--all-features", "--no-fail-fast"],
    )
}

fn deny() -> Result<()> {
    match run("cargo", &["deny", "check"]) {
        Ok(()) => Ok(()),
        Err(e) => {
            // cargo-deny is optional locally; warn but do not fail.
            eprintln!("warning: skipping cargo-deny ({e}). Install with `cargo install --locked cargo-deny`.");
            Ok(())
        }
    }
}

/// Source SVG and output paths, all relative to the workspace root.
const ICON_SVG: &str = "assets/icons/icon.svg";
const ICON_ICO: &str = "assets/icons/terminale.ico";
const ICON_ICNS: &str = "assets/icons/terminale.icns";

/// Rasterise `icon.svg` and pack the bitmaps into a Windows `.ico` and a macOS
/// `.icns`. Both container formats embed PNG payloads directly, so we only need
/// the same SVG renderer the app uses at runtime — no external image tooling.
fn gen_icons() -> Result<()> {
    let svg = std::fs::read(ICON_SVG)?;
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default())?;

    // Render each size once; both containers draw from this set.
    let mut png = std::collections::BTreeMap::<u32, Vec<u8>>::new();
    for size in [16u32, 32, 48, 64, 128, 256, 512, 1024] {
        png.insert(size, render_png(&tree, size)?);
    }
    let get = |s: u32| png.get(&s).cloned().expect("rendered above");

    let ico = build_ico(&[16, 32, 48, 64, 128, 256].map(|s| (s, get(s))))?;
    std::fs::write(ICON_ICO, &ico)?;
    println!("wrote {ICON_ICO} ({} bytes)", ico.len());

    // PNG-based icns OSTypes, covering both the 1x sizes and their @2x retina
    // variants so Finder/Launchpad/the Dock render crisply at every slot:
    //   icp4=16  icp5=32  ic07=128  ic08=256  ic09=512
    //   ic11=16@2x(32)  ic12=32@2x(64)  ic13=128@2x(256)  ic14=256@2x(512)  ic10=512@2x(1024)
    let icns = build_icns(&[
        ("icp4", get(16)),
        ("icp5", get(32)),
        ("ic11", get(32)),
        ("ic12", get(64)),
        ("ic07", get(128)),
        ("ic13", get(256)),
        ("ic08", get(256)),
        ("ic14", get(512)),
        ("ic09", get(512)),
        ("ic10", get(1024)),
    ]);
    std::fs::write(ICON_ICNS, &icns)?;
    println!("wrote {ICON_ICNS} ({} bytes)", icns.len());
    Ok(())
}

/// Render the SVG into a `size`×`size` PNG, centred and aspect-preserved.
fn render_png(tree: &usvg::Tree, size: u32) -> Result<Vec<u8>> {
    let mut pixmap =
        tiny_skia::Pixmap::new(size, size).ok_or_else(|| anyhow::anyhow!("pixmap alloc failed"))?;
    let svg = tree.size();
    let scale = (size as f32 / svg.width()).min(size as f32 / svg.height());
    let tx = (size as f32 - svg.width() * scale) * 0.5;
    let ty = (size as f32 - svg.height() * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    Ok(pixmap.encode_png()?)
}

/// Assemble a PNG-payload `.ico` (Vista+). A 256-px image stores 0 in the
/// single-byte width/height fields, per the ICO spec.
fn build_ico(images: &[(u32, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&u16::try_from(images.len())?.to_le_bytes());

    let mut offset = 6 + 16 * images.len();
    for (size, data) in images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.push(dim); // width
        out.push(dim); // height
        out.push(0); // palette colours
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&u32::try_from(data.len())?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(offset)?.to_le_bytes());
        offset += data.len();
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    Ok(out)
}

/// Assemble a PNG-payload `.icns`: an `icns` magic + total length, then one
/// `OSType`/length/PNG block per image (lengths are big-endian and include the
/// 8-byte block header).
fn build_icns(images: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (ostype, data) in images {
        body.extend_from_slice(ostype.as_bytes());
        body.extend_from_slice(&((8 + data.len()) as u32).to_be_bytes());
        body.extend_from_slice(data);
    }
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// Bundle identifier — keep in sync with `[workspace.metadata.dist.mac-pkg-config]`.
const MAC_BUNDLE_ID: &str = "dev.stackbyte.terminale";

/// Assemble `target/terminale.app` around a built binary so macOS treats it as a
/// real GUI application: it shows up in Launchpad/Spotlight with the brand icon
/// and launches directly (a bare Unix binary in /Applications instead opens the
/// user's terminal and runs inside it).
fn bundle_macos(bin: Option<PathBuf>) -> Result<PathBuf> {
    let version = env!("CARGO_PKG_VERSION");
    let bin = bin.unwrap_or_else(|| PathBuf::from("target/release/terminale"));
    if !bin.exists() {
        bail!(
            "binary not found at {} — run `cargo build --release` first (or pass --bin)",
            bin.display()
        );
    }

    let app = PathBuf::from("target/terminale.app");
    let macos = app.join("Contents/MacOS");
    let resources = app.join("Contents/Resources");
    // Start clean so stale files don't linger between runs.
    if app.exists() {
        std::fs::remove_dir_all(&app).ok();
    }
    std::fs::create_dir_all(&macos)?;
    std::fs::create_dir_all(&resources)?;

    std::fs::copy(&bin, macos.join("terminale")).context("copy binary into bundle")?;
    std::fs::copy(
        "assets/icons/terminale.icns",
        resources.join("terminale.icns"),
    )
    .context("copy terminale.icns into bundle (run `xtask gen-icons` if missing)")?;

    std::fs::write(app.join("Contents/Info.plist"), info_plist(version))?;
    // PkgInfo is optional but conventional for an APPL bundle.
    std::fs::write(app.join("Contents/PkgInfo"), "APPL????")?;

    // Mark the inner binary executable (copy can drop the bit on some setups).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = macos.join("terminale");
        let mut perm = std::fs::metadata(&p)?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&p, perm)?;
    }

    // Strip extended attributes (com.apple.provenance / quarantine that macOS
    // stamps on copied binaries) so the packaged bundle is clean and doesn't
    // carry the build machine's provenance into the shipped artifact. `xattr`
    // on recent macOS dropped its `-r` flag, so recurse via `find`. Best-effort:
    // a clean tree is nice-to-have, not load-bearing.
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("find")
            .args([&app.to_string_lossy(), "-exec", "xattr", "-c", "{}", ";"])
            .status();
    }

    // Ad-hoc sign the *bundle*. The linker already ad-hoc-signs the inner
    // Mach-O on Apple Silicon, but that signature expects a bundle-level
    // `Contents/_CodeSignature/CodeResources` that doesn't exist until the
    // bundle itself is signed. Without this step `codesign --verify` reports
    // "code has no resources but signature indicates they must be present" and
    // Gatekeeper rejects the app as **damaged** — a hard, fatal block on Apple
    // Silicon (where arm64 code must be signed to run at all). Ad-hoc signing
    // is NOT notarization: the user still right-clicks → Open the first time,
    // but it turns the dead-end "damaged" error into the normal
    // unidentified-developer prompt. Done last so the earlier `xattr -c` sweep
    // can't strip the freshly written signature metadata.
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("codesign")
            .args(["--force", "--deep", "--sign", "-", &app.to_string_lossy()])
            .status()
            .context("run codesign (macOS) to ad-hoc sign the bundle")?;
        if !status.success() {
            bail!(
                "codesign --sign - failed for {} (the .app would be rejected as 'damaged' on Apple Silicon)",
                app.display()
            );
        }
    }

    println!("built {} (v{version})", app.display());
    println!(
        "test it: open {}  — or drag it into /Applications",
        app.display()
    );
    Ok(app)
}

/// Wrap the assembled `terminale.app` in a compressed `.dmg` disk image — the
/// standard macOS GUI download. Opening the image shows `terminale.app` next to
/// an `/Applications` shortcut, so the user just drags the app across to
/// install. Shells out to the macOS-native `hdiutil`.
fn dmg_macos(bin: Option<PathBuf>, target: Option<String>) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let app = bundle_macos(bin)?;

    // Filename target triple: explicit flag wins, else the host triple so a
    // local run on Apple Silicon / Intel labels itself correctly.
    let target = target.unwrap_or_else(host_target_triple);
    let dmg = PathBuf::from(format!("target/terminale-v{version}-{target}.dmg"));
    if dmg.exists() {
        std::fs::remove_file(&dmg).ok();
    }

    // Stage the image contents: a fresh copy of the .app plus a symlink to
    // /Applications so the mounted volume offers the familiar drag-to-install
    // layout. Build it under target/ and clean any leftovers first.
    let stage = PathBuf::from("target/dmg-stage");
    if stage.exists() {
        std::fs::remove_dir_all(&stage).ok();
    }
    std::fs::create_dir_all(&stage)?;
    // `cp -R` preserves the bundle (incl. the executable bit) faithfully.
    run(
        "cp",
        &["-R", &app.to_string_lossy(), &stage.to_string_lossy()],
    )
    .context("copy terminale.app into the dmg staging dir")?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("/Applications", stage.join("Applications"))
        .context("create /Applications shortcut in the dmg")?;

    // UDZO = zlib-compressed read-only image (the conventional distribution
    // format). `-ov` overwrites a stale image; the volume name is what Finder
    // shows in the title bar when the image is mounted.
    run(
        "hdiutil",
        &[
            "create",
            "-volname",
            "terminale",
            "-srcfolder",
            &stage.to_string_lossy(),
            "-ov",
            "-format",
            "UDZO",
            &dmg.to_string_lossy(),
        ],
    )
    .context("hdiutil failed (run on macOS)")?;
    std::fs::remove_dir_all(&stage).ok();

    println!("built {} (v{version}, {target})", dmg.display());
    Ok(())
}

/// Best-effort host target triple for the dmg filename. We only care about the
/// arch on macOS; the OS/abi suffix is fixed for an Apple build.
fn host_target_triple() -> String {
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    format!("{arch}-apple-darwin")
}

fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>terminale</string>
    <key>CFBundleDisplayName</key><string>terminale</string>
    <key>CFBundleIdentifier</key><string>{MAC_BUNDLE_ID}</string>
    <key>CFBundleVersion</key><string>{version}</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
    <key>CFBundleExecutable</key><string>terminale</string>
    <key>CFBundleIconFile</key><string>terminale.icns</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
"#
    )
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    println!("\x1b[1m> {} {}\x1b[0m", program, args.join(" "));
    let status: ExitStatus = Command::new(program).args(args).status()?;
    if !status.success() {
        bail!("{program} {} failed with {status}", args.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Smoke-test the CLI parser so the binary stays callable when commands
    // are renamed.
    #[test]
    fn cli_parses_ci() {
        let cli = Cli::try_parse_from(["xtask", "ci"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Ci));
    }

    #[test]
    fn missing_subcommand_errors() {
        assert!(Cli::try_parse_from(["xtask"]).is_err());
    }
}
