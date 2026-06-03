# Changelog

All notable changes to `terminale` are documented in this file.

The format is based on [Keep a Changelog 1.1](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning 2.0](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.13]

### Added
- Bottom-right SSH quick-connect button: appears whenever at least one SSH host is configured;
  clicking it opens a searchable dropdown scoped to your hosts and connects the chosen one in a new tab
- Detect `ssh …` commands you type and offer a one-click "Save this SSH host?" prompt (Save / Dismiss
  + a default-checked "don't ask again"); saved hosts (metadata only — the secret stays in the OS
  keychain) show up in the quick-connect dropdown and Settings → SSH hosts. New
  `terminal.offer_save_ssh_hosts` config toggle (default `true`) controls the prompt

### Fixed
- No more double cursor under full-screen TUIs: an app that hides the real
  cursor with `ESC[?25l` (DECTCEM) and paints its own — vim, fzf, AI CLIs —
  used to get terminale's cursor drawn on top of it. The hide request is now
  honoured
- Settings → About → "Check for updates now" reports its outcome in the
  status bar ("up to date", "downloaded — restart to apply", or the error)
  and disables itself while the check runs; before, it only wrote to the log
  and read as a dead button

### Performance
- Scrolling the Settings window no longer deep-clones the entire configuration
  on every repaint frame; the live-apply diff now runs against a borrow and
  clones only when a control actually changed something

### Changed
- Dependency refresh (all suites green): portable-pty 0.9, rfd 0.16,
  global-hotkey 0.8, sha2 0.11, self_update 0.44, resvg/usvg 0.47,
  tiny-skia 0.12, schemars 1.2, CodeQL action v4
- `terminale --schema` now emits JSON Schema 2020-12 (was draft-07), following
  the schemars 1.x upgrade

## [0.1.12]

### Added
- Real close-confirmation dialog: with `window.confirm_close` on, closing a
  tab or window now opens a Confirm/Cancel dialog (Esc/click-outside cancels)
  instead of the old "flash + press close again within 1.5 s" mechanism that
  read as "nothing happened"
- `Fade` Quake animation — whole-window opacity fade on Windows (layered
  window); a legacy `animation = "fade"` config now gets a real fade instead
  of silently degrading to Slide
- `ai.offer_fix_on_failure` is now implemented (the toggle existed but did
  nothing): after a command exits non-zero, an unobtrusive amber hint with a
  [Fix] button appears in the suggestion bar and routes to the AI assistant
- Settings → Appearance → "Focus border opacity": the focused-pane border is
  now translucent by default (0.35) — it was a hard fully-opaque ring that
  crowded the first row of text
- Profile editor: environment-variables editor (KEY=VALUE per line) — the
  `env` field was config-file-only; quoted arguments with spaces now survive
  the Arguments field round-trip
- Status bar segment editor: ↑/↓ reorder buttons (was delete + re-add)

### Fixed
- Settings changes are no longer silently dropped: the live-apply gate
  compared ~150 hand-picked fields and missed entire sections (profiles, AI
  provider keys/models, GPU, updates, integration, directory jump, resource
  indicators, the [ssh] block, two dozen shortcut bindings, …) — editing only
  one of those in Settings neither applied nor saved it. The gate now derives
  full structural equality over the whole config, so every field present and
  future is covered
- Bottom rows no longer render under the AI suggestion bar: its 30 px band is
  reserved in the grid's bottom budget while open (stacked above the CPU/RAM
  strip) and the PTY reflows on open/close
- Quake: an explicit position→top/bottom/left/right snap now clears the
  remembered floating geometry, so hide/show re-docks full-width instead of
  replaying the size from a previous title-bar un-dock
- Quake animations stay inside the monitor: show/hide is an edge-pinned
  reveal (the old slide travelled fully past the edge — visible on a stacked
  monitor above); Scale is now a real zoom from the dock edge (it was
  accidentally identical to Slide); the PTY grid is not resized mid-animation
- AI suggestions stopped re-proposing the command that just failed: both the
  suggestion bar and Ask AI now receive structured context (recent commands
  with exit codes via OSC 133, the failed command's own output, cwd, shell)
  instead of a raw 200-line scrollback dump, plus a verbatim-repeat guard;
  Ask AI also gets the terminal context on plain opens and answers at low
  temperature
- Settings live-apply gaps: scrollback now applies to every split pane (was
  focused-pane-only); zen_hide edits apply while zen is active; vertical
  tab-bar width applies from the in-app save; profile edits refresh the
  default-profile cache and picker entries; snippet renames refresh the
  palette on external config reloads
- Eased cursor blink actually animates (repaints at `cursor.animation_fps`,
  which was a dead setting; the smooth fade rendered as a hard step)
- Settings description text bumped +1 pt (12.6 → 13.6) and the four
  description paragraphs that bypassed the shared helper now use it
- Settings slider ranges now reach the full validated range (font size,
  scroll steps, scrollback, blink rate, cell tint, tab widths, status-bar
  interval); background-FX "max bands" capped at the real GPU limit (48);
  the line-height and clear-screen Reset buttons restore the true defaults

## [0.1.11]

### Fixed
- macOS: a docked Quake window no longer leaves an empty strip below the
  menu bar and no longer stutters while sliding. Dock modes are positioned
  natively (`NSWindow.setFrame` against the screen's visible frame, with
  AppKit's built-in frame animation) instead of through winit, which
  double-counts the menu bar and repositions frame-by-frame. The app is
  also activated on show, so the hotkey takes keyboard focus from any app

## [0.1.10]

### Added
- "Restart session" in the right-click menu (and Ctrl+Shift+R, upgraded
  from the old crashed-tab-only restart): kills and respawns the focused
  pane's session in place — split layout preserved, profile command
  honoured, current directory inherited. Disabled for SSH panes
- Dragging a docked Quake window (top/bottom/left/right) by its title bar
  now un-docks it Chrome-style: the window shrinks back to the floating
  size it had before docking, and keeps that geometry across hide/show

### Fixed
- Empty band at the bottom of the terminal after resizing: the resize
  path double-counted the tab/status-bar chrome and sized the grid a few
  rows short (opening a new tab "fixed" it; the next resize broke it
  again). The grid now always fills down to the bottom padding
- Fresh-launch glitch where the first prompt rendered one row lower: the
  shell booted at a pre-tab-bar grid size and was shrunk after it had
  already printed, displacing the prompt via a ConPTY reflow. The PTY now
  gets its final size before the window is revealed
- Status bar: the right-aligned text (e.g. the clock) was clipped at the
  window edge — its position came from a width estimate ~30% too small;
  it now uses the real measured text width at any DPI
- Quake position memory: toggling hide while the show animation was
  still in flight saved the mid-slide position (and treated it as a user
  adjustment), so the window reappeared half-way. The resting geometry
  is saved instead; rapid toggles also animate from the live position
  rather than teleporting off-screen

### Changed
- Rendering: the focused pane's shaped text is cached across frames and
  rebuilt only when content or font/geometry actually change — cursor
  blink, background FX and bell redraws no longer re-shape every visible
  row (the dominant render cost). The GPU label in the resource strip
  and the Settings live-apply diff are similarly gated

## [0.1.9]

### Security
- SSH library bumped (russh 0.45 → 0.61.1), closing five advisories: three
  high-severity remote DoS vectors (unbounded post-decompression packet
  size, unchecked CryptoVec allocation growth, pre-auth allocation in the
  keyboard-interactive handler) and two moderate ones (channel-window
  adjust overflow, server userauth state reuse)

### Added
- SSH agent authentication now works on Windows: terminale talks to the
  OpenSSH agent service named pipe, with Pageant as fallback (previously
  agent auth was Unix-only)

### Changed
- RSA keys now sign with the strongest SHA-2 hash the server advertises —
  plain ssh-rsa/SHA-1 is refused by modern OpenSSH servers

## [0.1.8]

### Fixed
- Tab busy spinner no longer lights up while you type. Output that closely
  follows a keystroke or paste (echo / prompt repaint — syntax-highlighting
  shells redraw the whole line on every key) no longer counts as command
  activity; real commands (OSC 133 or sustained output) still drive the
  spinner, even mid-typing
- egui sub-windows (Settings, AI assistant, context menu, paste guard,
  password prompt) no longer peg a CPU core while idle — a self-sustaining
  `RedrawRequested` repaint loop is broken (~42% → ~0% CPU at idle);
  hover fades, combo animations and AI streaming still repaint on demand

### Changed
- Idle CPU and per-frame disk I/O cut across the board: the Settings window
  caches its theme and saved-workspace lists instead of re-scanning the disk
  on every repaint; background FX, spinner, bell and jump-highlight redraws
  are skipped while the window is minimized or fully covered; the tab
  activity spinner only animates while the window is focused and visible;
  background FX pause while unfocused (new
  `background_fx.pause_when_unfocused` toggle, default on); CPU / memory are
  only sampled while the resource strip is enabled

## [0.1.7]

### Added
- Built-in self-update from GitHub releases. `--check-update` / `--update` CLI
  flags, an `[updates]` config (`check_on_startup`, `auto_install`) and a
  Settings → About panel. Downloads are HTTPS-only from the official release and
  the archive is verified against its published SHA-256; the on-disk binary is
  then replaced atomically. The running session is never interrupted and the new
  version applies on the next launch — never a forced restart.

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
- Initial workspace scaffold (Cargo workspace, 6 starter crates, CI/release/audit GitHub Actions workflows)
- Community standards (README, CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, dual MIT/Apache license)
- `cargo-deny` policy banning webview wrappers (`tao`, `wry`) — project is native-only by design
- Production release pipeline via `cargo-dist`: `.msi` (Windows), `.dmg` (macOS), tarballs (Linux),
  Homebrew formula, `install.sh` / `install.ps1` one-liner installers (unsigned binaries)
- Plan for tmux compatibility (Tier 1 in v0.5.0, full tmux Control Mode in v1.5.0)

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

[Unreleased]: https://github.com/fbrzlarosa/terminale/compare/v0.1.13...HEAD
[0.1.13]: https://github.com/fbrzlarosa/terminale/compare/v0.1.12...v0.1.13
[0.1.12]: https://github.com/fbrzlarosa/terminale/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/fbrzlarosa/terminale/compare/v0.1.10...v0.1.11
[0.1.10]: https://github.com/fbrzlarosa/terminale/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/fbrzlarosa/terminale/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/fbrzlarosa/terminale/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/fbrzlarosa/terminale/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/fbrzlarosa/terminale/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/fbrzlarosa/terminale/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/fbrzlarosa/terminale/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/fbrzlarosa/terminale/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/fbrzlarosa/terminale/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/fbrzlarosa/terminale/releases/tag/v0.1.1
