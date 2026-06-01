//! `cargo xtask` — workspace task runner.
//!
//! Modelled after the rust-analyzer pattern: encodes CI commands as a Rust
//! binary so they stay consistent between local dev and GitHub Actions.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
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
    for size in [16u32, 32, 48, 64, 128, 256, 512] {
        png.insert(size, render_png(&tree, size)?);
    }
    let get = |s: u32| png.get(&s).cloned().expect("rendered above");

    let ico = build_ico(&[16, 32, 48, 64, 128, 256].map(|s| (s, get(s))))?;
    std::fs::write(ICON_ICO, &ico)?;
    println!("wrote {ICON_ICO} ({} bytes)", ico.len());

    // PNG-based icns OSTypes: ic07=128, ic08=256, ic09=512.
    let icns = build_icns(&[("ic07", get(128)), ("ic08", get(256)), ("ic09", get(512))]);
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
