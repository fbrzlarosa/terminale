# Changelog

All notable changes to `terminale` are documented in this file.

The format is based on [Keep a Changelog 1.1](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning 2.0](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Bottom-right SSH quick-connect button: appears whenever at least one SSH host is configured;
  clicking it opens a searchable dropdown scoped to your hosts and connects the chosen one in a new tab
- Detect `ssh …` commands you type and offer a one-click "Save this SSH host?" prompt (Save / Dismiss
  + a default-checked "don't ask again"); saved hosts (metadata only — the secret stays in the OS
  keychain) show up in the quick-connect dropdown and Settings → SSH hosts. New
  `terminal.offer_save_ssh_hosts` config toggle (default `true`) controls the prompt
- Initial workspace scaffold (Cargo workspace, 6 starter crates, CI/release/audit GitHub Actions workflows)
- Community standards (README, CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, dual MIT/Apache license)
- `cargo-deny` policy banning webview wrappers (`tao`, `wry`) — project is native-only by design
- Production release pipeline via `cargo-dist`: `.msi` (Windows), `.pkg` (macOS), tarballs (Linux),
  Homebrew formula, `install.sh` / `install.ps1` one-liner installers (unsigned binaries)
- Plan for tmux compatibility (Tier 1 in v0.5.0, full tmux Control Mode in v1.5.0)

## [0.1.1]

### Added
- Windows installer registers a Start-Menu shortcut (so terminale is searchable from the
  Start menu) and an optional, on-by-default Desktop shortcut
- Application icon is embedded in `terminale.exe`, so the taskbar, Alt-Tab, and the
  MSI/Start-Menu/Desktop shortcuts show the brand glyph
- Linux: on launch, terminale registers a freedesktop `.desktop` entry and icon under
  `$XDG_DATA_HOME` so it appears in the application menu and launcher search. New
  `integration.desktop_entry` setting (Settings → Desktop integration) plus
  `--install-desktop-entry` / `--uninstall-desktop-entry` CLI flags
- `xtask gen-icons` regenerates the `.ico`/`.icns` from the source `icon.svg`

### Changed
- Slimmer release pipeline: dropped artifact code-signing/notarization and build
  attestations (unsigned open-source binaries); build runners pinned to current images
- CI/release JS actions opted into Node 24 ahead of the Node 20 runner deprecation

## [0.1.0]

First public pre-alpha. See README for status.

<!--
Sections in each release (only include those with entries):
- Added       — new features
- Changed     — changes in existing functionality
- Deprecated  — soon-to-be removed features
- Removed     — features removed in this release
- Fixed       — bug fixes
- Performance — speedups and resource savings
- Security    — vulnerability fixes
- Tests       — significant test infra changes
-->

[Unreleased]: https://github.com/fbrzlarosa/terminale/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/fbrzlarosa/terminale/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/fbrzlarosa/terminale/releases/tag/v0.1.0
