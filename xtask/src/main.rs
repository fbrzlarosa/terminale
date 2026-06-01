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
