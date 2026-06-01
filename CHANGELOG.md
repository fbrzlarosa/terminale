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
- Production release pipeline via `cargo-dist`: `.msi` (Windows), `.dmg` (macOS), tarballs (Linux),
  Homebrew formula, `install.sh` / `install.ps1` one-liner installers (unsigned binaries)
- Plan for tmux compatibility (Tier 1 in v0.5.0, full tmux Control Mode in v1.5.0)

### Fixed
- macOS: the Settings window no longer pegs a CPU core while open. The custom
  title bar called `is_maximized()` every frame (which on macOS rebuilds the
  AppKit theme frame) and the content scroll area requested a repaint forever;
  both are fixed (~105% → ~1% CPU at idle)
- macOS trackpad: plain cursor motion is no longer misread as a text selection.
  A dropped trackpad button-release could leave a stale "left held" flag; we now
  confirm against the OS's real button state and clear it on focus loss
- Trackpad scrolling is no longer ~3× too fast and chunky: the per-notch
  rows-per-wheel-click multiplier is applied only to mouse-wheel notches, while a
  trackpad's continuous pixel deltas map smoothly to rows
- macOS: the bottom CPU/RAM/GPU resource strip rendered ~2× too large on
  Retina/HiDPI (its label font was pre-scaled and then scaled again by the text
  renderer); it now uses the same logical-size convention as the other bars

## [0.1.6]

### Changed
- macOS GUI download is now a `.dmg` (was a `.pkg`): open the image and drag
  **terminale.app** onto the bundled `/Applications` shortcut — the standard Mac
  install flow. Built by the `macos-app-dmg` release job via the new
  `xtask dmg-macos`. The `…-apple-darwin.tar.gz` stays a bare command-line binary
  on purpose (it backs the `install.sh` and Homebrew paths), so it is not the GUI
  app.

### Removed
- `xtask pkg-macos` / the `.pkg` installer, superseded by the `.dmg` above.

## [0.1.5]

### Added
- Pixel-art resource-indicator strip at the bottom of the window: segmented CPU%
  and RAM% meters (coloured by load level) plus the GPU adapter name/backend.
  Lives in a reserved band so the grid shrinks to fit and it never overlaps
  terminal content; a bottom suggestion/status bar floats above it. New
  `[resource_indicators]` config (default on) and a toggle in
  Settings → Appearance. GPU utilisation% isn't available cross-platform, so the
  GPU slot is a label rather than a meter.

## [0.1.4]

### Added
- `xtask pkg-macos` wraps the `terminale.app` bundle in an installer `.pkg`
  (via the system `pkgbuild`) — the single source of truth used by both local
  builds and the release pipeline

### Changed
- macOS `.pkg` now installs a real **terminale.app** into `/Applications` (shown
  in Launchpad/Spotlight, launches as a GUI app) instead of a bare Unix binary
  that opened the user's terminal. Built by a dedicated `macos-app-pkg` release
  job; cargo-dist's bare-binary `pkg` installer is no longer used on macOS
- macOS app icon (`.icns`) now embeds the full set of sizes with `@2x` retina
  variants (16–1024 px) so it renders crisply in the Dock, Finder and Launchpad

## [0.1.3]

### Fixed
- Selection no longer triggers from plain cursor motion: drag-select now requires
  the left button to actually be held, fixing a macOS trackpad case where a
  stale press turned movement into a runaway text selection

## [0.1.2]

### Fixed
- macOS: the right-click context menu no longer closes a few milliseconds after a
  trackpad two-finger tap (macOS handed focus back to the parent immediately; the
  popup now re-grabs focus during a short grace window)

### Added
- `xtask bundle-macos` assembles a proper macOS `terminale.app` bundle (Info.plist +
  icon + binary) so the app appears in Launchpad/Spotlight and launches directly
  instead of opening inside the user's terminal

### Documentation
- How to open the unsigned macOS/Windows builds (Gatekeeper/SmartScreen)
- Packaging-helper commands (`xtask gen-icons`, `xtask bundle-macos`) in `docs/build.md`

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

[Unreleased]: https://github.com/fbrzlarosa/terminale/compare/v0.1.6...HEAD
[0.1.6]: https://github.com/fbrzlarosa/terminale/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/fbrzlarosa/terminale/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/fbrzlarosa/terminale/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/fbrzlarosa/terminale/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/fbrzlarosa/terminale/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/fbrzlarosa/terminale/releases/tag/v0.1.1
