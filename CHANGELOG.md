# Changelog

All notable changes to `terminale` are documented in this file.

The format is based on [Keep a Changelog 1.1](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning 2.0](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.36]

### Fixed
- **Open tabs survive a crash or power loss.** The last-session snapshot was
  written only on a graceful close (window close / last-tab close) and via a
  non-atomic truncating write, so an unclean exit left either a stale file or —
  as actually happened on a power cut — an all-NUL torn file that failed to
  parse, losing every open tab. Saving is now **crash-safe** (write to a temp
  file, `fsync`, then atomic rename, so an interrupted write can never corrupt
  the session) and runs on a **periodic autosave** while the app is open, not
  only on exit — a crash now loses at most a few seconds of layout. Configurable
  under **Settings → Workspaces** (`window.session_autosave_secs`, default 15s;
  `0` = save on close only).
- **No more freeze while a command floods output.** Draining queued PTY output
  parsed the entire backlog in one uninterruptible pass on the UI thread, so a
  burst (`cat` of a large file, a verbose build, `yes`) froze the window until
  it finished, then recovered. The drain is now bounded by a per-pass time
  budget: it parses a slice, paints a frame, and resumes next tick — the UI
  stays responsive during floods. Configurable under **Settings → Terminal**
  (`terminal.output_drain_budget_ms`, default 8 ms).

### Added
- **Logging settings.** The diagnostic log file and freeze watchdog now have a
  dedicated **Settings → Logging** section: enable/disable the rolling log file
  (`logging.file_enabled`), log level (`logging.file_level`), retention in days
  (`logging.retention_days`), and the slow-frame warning threshold
  (`logging.slow_frame_warn_ms`, applies live). Previously these existed only in
  the config file with no in-app control.
- **Phase-attributed freeze watchdog.** When a frame exceeds the slow-frame
  threshold, the logged warning now breaks the time down by phase —
  `acquire_ms` (surface acquire / compositor), `prepare_ms` (glyph atlas growth
  + text shaping), and `submit_present_ms` (GPU submit + present) — so a
  transient freeze can be attributed to the phase that actually stalled instead
  of just "slow frame".

### Changed
- **Larger initial glyph atlas (256 → 2048 px).** Starting the atlas small
  forced several synchronous grow passes — each re-rasterizing and re-uploading
  every cached glyph — the first time a buffer introduced many new glyphs at
  once (CJK/emoji bursts), a one-off multi-hundred-ms UI-thread hitch. The
  larger initial size avoids that early grow storm at negligible VRAM cost.

## [0.1.35]

### Added
- **Drag & drop file paths.** Dropping one or more files onto the terminal
  window now inserts their paths into the focused pane as a (bracketed) paste,
  so nothing executes on its own — drag an image onto Claude Code and it reads
  it from the path. Previously drops were silently ignored. Configurable under
  **Settings → Terminal**: master toggle (`terminal.drop_paths`), quoting policy
  (`terminal.drop_path_quoting` — `auto`/`always`/`never`, with platform-correct
  quoting that keeps Windows backslashes intact and escapes POSIX shell
  metacharacters), and an optional trailing space (`terminal.drop_path_trailing_space`).
- **Scroll to bottom on input.** Typing or pasting while scrolled up into
  history now snaps the viewport back to the live prompt (the standard
  iTerm2 / Windows Terminal behaviour). Toggle under **Settings → Terminal**
  (`window.scroll_on_input`, on by default).
- **Per-tab "waiting for input" indicator.** When the program in a background
  tab rings the terminal bell (e.g. Claude Code finishing its turn and waiting
  for you), a static amber dot now appears on that tab's chip — distinct from
  the blue unread dot (any output) and the busy spinner (a command is still
  running) — and clears when you focus the tab. It also shows on the active tab
  while the window is unfocused, so a visible-but-unfocused window surfaces it.
  Toggle under **Settings → Appearance** (`appearance.tab_attention_on_bell`,
  on by default).

## [0.1.34]

### Fixed
- **Session restore now keeps the pane you were working in focused.** When a
  restored tab had splits, the rebuilt layout always left keyboard focus on the
  last-spawned pane instead of the one that had it at save time — so reopening
  the app (including with the Quake drop-down restored) silently shifted focus
  to a different pane. The focused pane is now recorded per tab and re-applied
  after the layout is rebuilt. Sessions saved before this change keep their
  previous behaviour.

### Changed
- Dependency updates: `russh` 0.61.1 → 0.61.2 (patch, SSH client) and the dev
  dependency `insta` 1.47.2 → 1.48.0. Dependabot's proposed `swash` 0.1 → 0.2
  and `windows` 0.58 → 0.62 bumps were intentionally skipped: `swash` is not used
  directly (it would only add a duplicate copy alongside cosmic-text's), and the
  `windows` crate is pinned to 0.58 to match `winvd`'s `HWND` type for the Quake
  virtual-desktop pinning.

## [0.1.33] — 2026-06-15

### Changed
- **"Show quake on all desktops" now actually works on Windows.** Previously a
  no-op outside macOS, the option now *pins* the Quake window through the
  Windows virtual-desktop COM API (via `winvd`): switch virtual desktop and the
  drop-down stays on screen — no hide/show round-trip. On Windows builds whose
  COM IIDs winvd doesn't recognise it degrades gracefully to the active desktop
  and logs once. Linux/Wayland is still pending. (Enable in **Settings → Quake →
  Show on all desktops**.)

## [0.1.32] — 2026-06-15

### Added
- **Shift+Enter now works in Claude Code (and other modern TUIs) out of the
  box** — no `/terminal-setup` needed. terminale implements the **kitty keyboard
  protocol** (the `CSI u` progressive enhancement): programs that opt in receive
  unambiguous key events, most importantly `Shift+Enter` (`CSI 13;2u`) for
  multi-line input, plus disambiguated Ctrl/Alt combos, key press/repeat/release
  events, and associated text. The terminal also self-identifies via
  `TERM_PROGRAM`/`TERM_PROGRAM_VERSION` like iTerm2/kitty/WezTerm. Toggle in
  **Settings → Terminal → Kitty keyboard protocol** (default on).
- **The app reopens exactly as you left it.** With session restore enabled, the
  last session now also restores the window's position and size, the monitor it
  was on (matched by the display's friendly name, so it survives reboots and
  rearranged monitors), and — if you closed it while the Quake drop-down was
  showing — reopens in Quake mode on that same monitor. Toggle in
  **Settings → Workspaces → Restore window geometry** and
  **Settings → Quake → Reopen in Quake mode** (both default on).
- **Keep the Quake drop-down on every virtual desktop.** New
  **Settings → Quake → Show on all desktops** (default off): switching virtual
  desktop / workspace still finds the Quake window under its hotkey. Reliable on
  macOS; best-effort on Windows/Linux (it otherwise appears on whichever desktop
  the hotkey is pressed).

### Fixed
- **Closing the window with the X button / Alt+F4 no longer loses your session.**
  Only closing the last tab used to save the last session; the OS close button
  (and the close-confirmation dialog) skipped saving entirely. Both paths now
  persist the session when session restore is enabled.

## [0.1.31] — 2026-06-15

### Fixed
- **The window no longer freezes for a second when you type in a split, then
  recovers on its own.** On every PTY event the autodetect-links pass scanned
  *every* pane in the active tab, calling `Path::exists()` for every path-like
  token on every visible row. On Windows each check is a blocking
  `GetFileAttributesW` syscall (tens-to-hundreds of ms with a cold cache,
  antivirus, or network paths). In a split, typing echoed back as PTY output and
  re-scanned *both* panes — including the non-focused one whose content had not
  changed — while holding each pane's emulator lock across the I/O. Panes are now
  re-scanned only when their emulator content generation actually changes, so an
  unchanged pane issues zero filesystem syscalls instead of dozens.

### Added
- **Freeze diagnostics.** Intermittent "freezes that fix themselves" can also be
  a GPU surface loss (driver reset / TDR, sleep-wake, RDP attach) that the
  renderer silently recovers from — previously logged only at `debug`, so it
  left no trace at the default `info` level. The recovery is now logged at
  `WARN` with a running total, the selected GPU adapter (name / backend / type /
  driver) is logged once at startup, and a new **freeze watchdog**
  (`logging.slow_frame_warn_ms`, default 250 ms, `0` = off) warns whenever a
  single main-window render stalls past the threshold. Configurable live in
  **Settings → About → Diagnostics**.

## [0.1.30]

### Fixed
- **The right-click menu no longer flashes a black block, and stops "stretching"
  when you move between submenus.** The popup is a borderless wgpu window; on
  Windows its surface only offers an opaque alpha mode, so the transparent
  corners around a short flyout composited as solid black. It was also resized
  on every submenu open/switch/close, and each resize recreated the swapchain —
  the compositor scaled the stale frame for one tick, which read as a stretch
  (most visible going from a submenu parent back to a plain row). The window is
  now a fixed size for its whole life and clipped to the L-shape actually
  painted via a window region, so navigating a submenu only changes the clip:
  no swapchain churn, no stretch, and the empty corners show what's behind
  without relying on per-pixel alpha.

## [0.1.29]

### Fixed
- **The app no longer crashes when resuming from standby.** winit's
  `MonitorHandle::name()` / `size()` unwrap a fallible Win32 call
  (`GetMonitorInfoW`) internally; when the OS wakes from sleep it briefly
  invalidates monitor handles (error 1461, *"the screen handle is not
  valid"*), so probing a handle captured *before* standby panicked — taking
  down the whole window with a "fatal error" dialog. Monitor name/size reads
  now go through a panic-safe layer that degrades to a graceful fallback, and
  the fatal-error dialog stays quiet for these recovered probes. The same path
  guards every OS, not just Windows.
- **The Vulkan backend could flood the log file with hundreds of megabytes a
  day.** wgpu's Vulkan present-mode converter WARNs (`Unrecognized present
  mode …`) on every surface reconfigure; a display-induced reconfigure loop
  wrote 390 MB of that single repeated line in one day in the field. That
  module is now capped to errors in both the file and console log layers
  (an explicit user `wgpu_hal` directive still wins). Auto-backend users are
  additionally off Vulkan entirely since 0.1.28's DX12 default.

## [0.1.28]

### Fixed
- **Quake show now returns focus to the window you were last working in.**
  With multiple Quake windows, the show loop focused each window as it
  revealed it, so keyboard focus always landed on the last one in creation
  order — not the one that had focus before the hide. The most recently
  focused terminal window is now re-asserted at the end of the show.
- **Windows could freeze permanently after a monitor powered off** (e.g.
  display sleep overnight). The default GPU backend choice landed on Vulkan,
  whose NVIDIA swapchain is known to block inside frame acquisition when the
  display goes away — hanging the whole event loop. `gpu.backend = "auto"`
  now prefers **Direct3D 12** on Windows: DXGI reports display sleep / GPU
  resets as recoverable errors the renderer already heals from. Vulkan
  remains available via `gpu.backend = "vulkan"` for those who want it.
- **Legacy system-wide installs no longer download an installer that refuses
  to run.** 0.1.27 made the MSI per-user, but a legacy (Program Files)
  install checking for updates would download that new MSI and hit its
  own "can't upgrade a system-wide install" guard — a dead end with a
  confusing (and mojibake-garbled) dialog. The updater now skips the
  installer entirely on legacy installs and points straight at the one-time
  **"Switch to self-updating install"** (Settings → About), which updates
  and migrates in a single step. The installer dialog text is now plain
  ASCII (the `.wxs` is windows-1252 — the arrows became `â†'`) with clear
  instructions for people running the installer by hand: uninstall the old
  copy first, settings are kept.

## [0.1.27]

### Changed
- **No more clicking through installers to update — on any OS.** Three
  changes close the last gap (Windows MSI installs):
  - **The Windows MSI is now a per-user package** (like VS Code's User
    Installer): it installs under `%LOCALAPPDATA%\terminale` with no admin
    rights, so the in-app updater swaps the binary silently in the background
    — the same hands-off updates the PowerShell installer always had. The
    installer refuses to run while a legacy system-wide copy exists (see
    below) so the two can never silently coexist.
  - **Legacy system-wide installs (pre-0.1.27 MSI under Program Files) now
    update silently too**: the verified `.msi` runs with `/passive
    /norestart` — no wizard, just the one unavoidable elevation consent and
    a progress bar.
  - **One-click escape from the legacy install**: **Settings → About →
    "Switch to self-updating install"** downloads and verifies the latest
    portable build, installs it per-user (Start-menu shortcut and user PATH
    included), starts it, and removes the old system-wide copy — one final
    elevation prompt, then silent updates forever. The offer only appears on
    legacy Program Files installs; per-user installs (MSI or PowerShell)
    never see it.

## [0.1.26]

### Added
- **Merge a tab into another tab as a split.** Drag a tab (or a pane header)
  onto a terminal body and a tinted drop zone shows which half
  (left/right/top/bottom) of the pane under the cursor it will occupy;
  release to graft it there, splits and all. Works across windows AND within
  one window thanks to the Chrome-style lift below; panes can also be
  re-arranged within their own tab the same way. There's a menu path too:
  **right-click a tab → "Merge into tab"** picks the destination directly.
  Configurable via `appearance.tab_drop_merge` and **Settings → Appearance →
  "Merge on body drop"** (default: on; off restores the classic body-drop
  tear-out).
- **Chrome-style tab drag.** The moment a dragged tab leaves the tab bar it
  is lifted out of its window: the bar closes the gap and the body shows the
  next tab — so you can drop the dragged tab as a split in its own window
  right away. Cross back into any tab bar and it re-inserts live at the
  hovered slot, exactly like Chrome. (A window's only tab is never lifted.)
- **Break a split pane out: "Move pane to new tab" / "Move pane to new
  window"** in the right-click Split menu — the explicit reverse of merging.
  The same operations remain available by dragging the pane's header strip
  onto a tab bar (→ tab) or outside every window (→ window).
- **The scrollback scrollbar is now interactive.** Grab the thumb and drag to
  pan through history, or click anywhere on the track to jump there. It widens
  on approach, and in the default `auto` mode it appears while scrolled **or
  when the pointer hovers the right edge** — so it can be grabbed even from
  the live bottom. Configurable via `window.scrollbar` (`auto` / `always` /
  `never`) and **Settings → Terminal → Scrollbar** (applies live).

### Fixed
- **Dragging a split pane by its header now actually works.** The header
  press intercept returned before the shared mouse handler ever tracked the
  held button, and the drag promotion gate requires it — so a pane drag
  could never arm. The press now records the held button itself; pane
  headers can be dragged onto a tab bar (→ tab), outside (→ window), or
  onto a pane body (→ split) as designed.
- **The right-click context menu no longer jumps left when hovering a submenu
  on a multi-monitor setup.** The right-edge flip compared absolute screen
  coordinates against the monitor's width instead of its right edge, so on any
  secondary monitor every submenu hover shifted the whole menu. The flip now
  uses real edges and, when genuinely needed, keeps the base column anchored
  and opens the flyout on its left (like native OS menus).
- **The focused-pane border no longer tints the terminal text.** In split
  views the focus border was drawn inset INSIDE the pane, landing right under
  the first and last text rows and columns. Each stroke is now centred on the
  pane boundary — recolouring the divider band and the window padding instead
  (iTerm2-style) — so the content area stays untouched. Same treatment for
  the amber broadcast-input border.
- **The text selection now follows the text.** Selecting and then scrolling
  (or new output arriving) left the highlight glued to fixed screen rows over
  different text — and copying returned whatever was under it now. The
  selection is anchored to the scrollback at creation time: the highlight
  moves with the text and the copied content is what was selected, no matter
  how far you've scrolled since.

## [0.1.25]

### Added
- **Ctrl+C now copies when text is selected** — the smart-copy behaviour of
  Tabby, Windows Terminal, and VS Code. With an active selection, a bare
  Ctrl+C copies it to the clipboard (and clears the selection) instead of
  sending the interrupt to the running program; pressing Ctrl+C again — or
  with nothing selected — sends `^C`/SIGINT exactly as before. Explicit
  Ctrl+C keybindings always take precedence. Configurable via
  `terminal.ctrl_c_copies_selection` and **Settings → Terminal → "Ctrl+C
  copies selection"** (default: on, applies live).

### Security
- **Cleared the last open Dependabot alert** ([GHSA-rhfx-m35p-ff5j](https://github.com/advisories/GHSA-rhfx-m35p-ff5j),
  low): `lru` 0.12.x, pulled in by the pinned `glyphon` 0.6 text renderer, has
  an `IterMut` Stacked-Borrows unsoundness. glyphon never calls `iter_mut`, so
  the unsound path was unreachable — but the vulnerable version sat in
  `Cargo.lock`. glyphon 0.6.0 is now vendored (`vendor/glyphon`, applied via
  `[patch.crates-io]`) with its `lru` requirement bumped to 0.16, moving the
  lockfile to the patched `lru` 0.16.4. The vendor copy goes away with the
  planned egui + wgpu + glyphon render-stack migration.

## [0.1.24]

### Fixed
- **Menu and context-menu actions now land on the window you're working in.**
  With several windows open (e.g. one per monitor), actions chosen from a menu
  — split pane, new tab with profile, SSH picker, AI inject — were applied to
  the most recently created window instead of the one that opened the menu.
  The app now tracks which terminal window the OS last focused and routes all
  window-agnostic actions there.
- **Quake mode now plays the slide-in animation on every window.** With
  multiple Quake windows, only the one that grabbed foreground focus animated
  in — the others popped into place instantly (the slide-out was fine). Each
  animation frame is now painted directly instead of relying on
  `request_redraw()`, which Windows ignores for freshly shown background
  windows.

## [0.1.23]

### Added
- **macOS now updates itself silently too — no paid signing required.** A
  `.app` install previously had no in-app update path (the `.dmg` is a manual
  drag-install). The updater now downloads the new bundle, verifies its
  SHA-256, and swaps `terminale.app` in place — applying on the next launch
  like every other platform, with the running session untouched. There's no
  Gatekeeper prompt: a bundle the app downloads itself carries no quarantine
  flag, and the ad-hoc signature already produced in CI satisfies Apple
  Silicon. Needs the app's folder writable (`/Applications` for admins, always
  for `~/Applications`); otherwise it points you at the `.dmg`. The first
  install from the `.dmg` still shows the one-time unidentified-developer
  prompt, but every update after that is automatic.

### Changed
- **The PowerShell installer now installs per-user, enabling silent background
  updates on Windows.** It installs into `%LOCALAPPDATA%\terminale` (a writable
  location) instead of `~/.cargo`, so the in-app updater replaces the binary in
  place and applies it on the next launch with no UAC prompt and no installer
  to click through — the same hands-off update the portable builds already get.
  The per-machine `.msi` (under `Program Files`) still needs elevation, so it
  keeps the download-and-run-installer update path; the README now recommends
  the PowerShell installer for anyone who wants automatic updates. The Unix
  shell installer likewise moves to `$HOME/.terminale`

## [0.1.22]

### Fixed
- **The in-app updater can now actually install updates.** It asked for an
  archive entry named exactly `terminale`, but the release tarballs nest the
  unix binary under a stem directory (`terminale-x86_64-apple-darwin/terminale`)
  while the Windows zip keeps it flat (`terminale.exe`), so extraction failed
  with *"Could not find the required path in the archive"*. The updater now
  tries both layouts. Note: because the broken updater shipped inside every
  build up to and including 0.1.21, upgrading **to** this release still needs a
  one-time manual download; auto-update works for every release after it.

## [0.1.21]

### Added
- **Restoring a session now reopens each tab in the directory it was in.**
  `window.restore_working_dirs` already existed, but it only worked for shells
  that announce their working directory, which PowerShell does not — and
  PowerShell's `Set-Location` doesn't update the OS process directory either,
  so there was no way to know where it was. A new **shell integration** setting
  (Settings → Terminal → *Report working directory*, on by default) injects a
  tiny prompt hook into PowerShell so it reports its directory via `OSC 9;9`.
  For shells whose `cd` does update the process directory (cmd, bash, zsh) the
  directory is now read from the OS as a fallback at save time. Net effect:
  with *Restore working dirs* on (Settings → Workspaces), reopening terminale
  starts every restored tab back in its folder. The injection is skipped when
  a profile already runs its own `-Command`/`-File`

### Fixed
- **macOS Quake dock no longer leaves an empty strip below the menu bar.**
  AppKit's automatic frame constraining double-counted the menu bar and
  dropped a flush top dock one bar-height lower, leaving a visible gap
  between the menu bar and the window. terminale now overrides
  `-constrainFrameRect:toScreen:` so the docked window stays exactly where it
  is placed, and computes the dock geometry against the screen *work area* —
  so Top/Left/Right docks sit flush under the menu bar and a Bottom dock
  clears the Dock

### Changed
- **macOS Quake dock animations are smooth and show live content.** Slide,
  Bounce and Scale now run through AppKit's native, compositor-driven frame
  animation instead of the per-frame winit pump (which resized the wgpu
  surface every frame and stuttered), pre-rendering one full-size frame so the
  terminal content is visible as the window opens instead of appearing only
  when the animation ends. Fade now works on macOS too, via
  `NSWindow.setAlphaValue:`

## [0.1.20]

### Fixed
- **Quake on a secondary monitor no longer snaps back to the first screen.**
  With the Quake display set to *Window's monitor* (`display = "current"`),
  the monitor the window lives on is now resolved by geometry — the display
  whose rectangle contains the window's centre — instead of trusting the OS
  `current_monitor()` call, which on Windows can report the wrong display (or
  none) right after the window crosses a monitor boundary. The snapshot is
  also never overwritten with an empty result, so a transient probe failure
  can't make the next toggle reappear on the primary monitor. Note: with the
  display set to *Primary* (`display = "primary"`) Quake docks on the primary
  monitor by design — choose *Window's monitor* in Settings → Quake → Display
  for the toggle to follow the window across screens
- **ConPTY console hosts (`OpenConsole.exe`) can no longer outlive a crash.**
  Each terminal pane runs its shell through a pseudo-console backed by an
  `OpenConsole.exe` host. A clean tab close reaps it, but a hard exit — a
  force-kill, a panic-abort, a GPU-driver crash — left the host orphaned, and
  an orphaned host whose pipe peer has vanished busy-loops at ~100% of one
  core, accumulating one per killed instance. The process now confines itself
  to a Windows Job Object with kill-on-close, so the OS terminates every
  console host the instant terminale goes down, no matter how. The MSI updater
  explicitly breaks away from the job so an in-progress upgrade is never killed

## [0.1.19]

### Fixed
- **Restored split panes no longer come up permanently blank.** Terminal
  protocol replies (cursor-position reports for `CSI 6n`, device attributes,
  colour queries) were only ever sent for the focused pane; every other
  pane's replies were silently discarded. The Windows console host sends a
  cursor-position query during startup and blocks on the answer — so when a
  restored split moved focus to the second pane before the first one's query
  was answered, the first shell stalled in its handshake forever and its pane
  stayed empty. Non-focused panes (and shells in background tabs, which could
  stall the same way) now get their protocol queries answered
- **Starting a drag-selection just left of the text now works.** Pressing in
  the thin padding strip before a pane's first column — right next to the
  divider in a split, exactly where drags naturally start — silently failed
  the hit test and the selection never armed. Presses inside a pane's rect
  now clamp to the nearest cell (xterm-style), like trailing-edge clicks
  always did
- **Non-focused split panes no longer freeze visually.** A repaint was only
  requested when the *focused* pane received output, so a sibling pane's
  content was applied to its emulator but never drawn — most visibly, a
  restored split's second pane stayed blank until something else forced a
  frame. Any visible pane's output now schedules a repaint (cheap thanks to
  the per-pane shaped-text cache), and a background tab's first unseen
  output repaints once so the unread badge shows up
- **The selection highlight is actually visible now.** It was painted at a
  hardcoded 55% opacity, which on dark themes (e.g. a `#1a2426` selection
  over a `#0e1415` background) blended down to a ~7-RGB-point difference —
  selections worked but could not be seen. The theme's selection colour is
  now painted opaque, as theme authors design it; a new
  `appearance.selection_opacity` setting (Settings → Appearance) restores
  blending for those who prefer it

### Added
- AI command suggestions are now taught with worked examples (few-shot
  turns) and a compact rule prompt — small local models follow them far
  better than long prose rules

## [0.1.18]

### Fixed
- **In-app updates now work for MSI installs.** The updater tried to replace
  the binary in place, which fails without elevation when terminale lives
  under `Program Files` (the MSI default) — "Check for updates now" just
  errored out. The install location's writability is now probed first: MSI
  installs download the new release's `.msi` (checksum-verified like every
  update) and hand it to the Windows installer, which performs the upgrade
  with the standard elevation prompt. Portable/zip installs keep the silent
  in-place staging. The startup auto-update never launches installer UI —
  on MSI installs it only notifies that a new version is available
- Killed shells are reaped before their pseudo-console is closed, so a torn
  down tab can no longer leave an orphaned console-host process spinning at
  full CPU in the background
- **AI suggestions know where they're running.** SSH sessions (native SSH
  tabs, and `ssh`/`mosh` typed in a local shell and still connected) now tell
  the model the commands execute on a remote host with an unknown OS — it is
  instructed to prefer portable POSIX commands and, when the right command
  depends on the remote OS, to suggest a discovery command first (`uname -a`)
  instead of assuming the local machine. The live OS/shell/cwd line is also
  repeated inside the system prompt, where small local models actually honour
  it — fixing wrong-OS suggestions (Unix commands proposed to PowerShell and
  vice versa)

## [0.1.17]

### Fixed
- **Closing a tab no longer freezes (and gets killed) the whole app** when the
  tab's shell still has child processes attached. PTY teardown — including the
  Windows pseudo-console close, which blocks until the console host exits —
  now runs on a background reaper thread instead of the UI event loop
- **Text selection, link detection, and mouse reporting now work in split
  panes.** Mouse hit-testing was window-global and knew nothing about the pane
  layout, so in any split the clicked cell matched no pane's grid: drag /
  double-click / triple-click selection silently failed and URLs under the
  pointer were never recognised. Hit-testing is now pane-aware (pane-local
  cells), and the URL/path scanner runs per pane — links resolve in every
  pane of a split, not just the focused one
- Holding **Shift** now bypasses app mouse reporting (xterm convention), so
  text can be selected and the wheel scrolls history even inside full-screen
  apps that capture the mouse
- The renderer now recovers from lost/outdated GPU surfaces (driver reset,
  sleep/wake, RDP, monitor hot-plug) by reconfiguring and retrying instead of
  freezing the window; probed cell sizes are clamped (zero-advance fonts
  produced degenerate 65535-column grids) and renderer init fails gracefully
  on adapters with empty surface capabilities (virtual/RDP displays)
- Config saves are atomic (write-to-temp + rename): a crash mid-save can no
  longer leave a torn `config.toml`
- Linux: every window now sets the Wayland `app_id` / X11 `WM_CLASS`
  (`terminale`), so GNOME/KDE match the desktop entry and show the brand icon
  instead of a generic gear; all sub-windows group under the same dock entry
- macOS: changing the Quake hotkey binding in Settings re-registers the global
  hotkey live (previously required a restart), and docked Quake windows no
  longer leave an empty strip under the menu bar when shown before the window
  server has flushed
- The log file no longer fills with third-party GPU noise (`wgpu` logged every
  device poll at INFO — hundreds of MB per day); chatty crates are capped at
  WARN unless explicitly re-enabled via `logging.file_level`

### Changed
- The pointer now shows the standard **I-beam** cursor over the terminal text
  grid (and the hand cursor over Ctrl+clickable links), instead of the default
  arrow everywhere
- **Quake mode is anchored to its own monitor**: the toggle always shows the
  window on the monitor it was last visible on, and dragging it to another
  monitor re-anchors it there. The previous behaviour ("follow the mouse
  cursor at hotkey time") proved unreliable and surprising; the `display`
  setting's `current` option now means "window's monitor" (`primary` and
  fixed-index pinning are unchanged)
- New minimal app icon: a bare `>_` prompt glyph with a full-spectrum diagonal
  gradient on a transparent background

### Security
- **AI API keys moved out of `config.toml` and into the OS keychain**
  (Windows Credential Manager / macOS Keychain / Secret Service). Existing
  plaintext keys migrate automatically on the next save; encrypted backups
  carry them through the credentials channel; `config.toml` is `0600` on Unix
- Lua plugin hooks run under a wall-clock execution budget
  (`plugins.hook_budget_ms`, default 100 ms, configurable in Settings): a
  runaway or malicious hook is aborted instead of hanging the UI
- OS notifications triggered by terminal output (OSC 9/777) are rate-limited
  per rolling 10 s window (`terminal.os_notification_rate_limit`, configurable
  in Settings), deduplicated, and dispatched off the UI thread
- `OSC 1337 SetUserVar` names are capped per pane so untrusted output cannot
  grow the variable map without bound

### Performance
- Split panes no longer re-shape every visible row of every pane on every
  frame: non-focused panes' shaped text is cached per pane (with correct
  invalidation via an emulator content-generation counter) — the dominant
  steady-state render cost in split layouts is gone
- The PTY hot path drops two per-chunk heap copies, coalesces redundant
  event-loop wakeups during output floods, and skips idle panes without
  touching emulator locks
- The tab bar rebuild and the plugin-host snapshot are skipped when nothing
  observable changed

## [0.1.16]

Supersedes v0.1.15, whose release pipeline failed mid-publish and left a
partial release (no Windows/Linux downloads). All v0.1.15 changes below are
included.

### Fixed
- In-app updates no longer fail with `403` once GitHub's unauthenticated API
  rate limit (60 requests/hour per IP) is exhausted: the archive and its
  `.sha256` sidecar now download from the un-metered
  `github.com/<owner>/<repo>/releases/download/` CDN path instead of the
  `api.github.com` asset endpoint. Only the small release-metadata lookup
  still touches the API
- The release pipeline now publishes a release only after **every** asset is
  attached (created as a draft, flipped public at the very end): an update
  check that fired while assets were still uploading used to download a
  partial archive and fail checksum verification — or see a release with no
  matching asset at all

## [0.1.15]

### Fixed
- macOS: the downloaded `.dmg` no longer opens as **"damaged"** on Apple
  Silicon. The `.app` bundle is now ad-hoc code-signed during packaging
  (`codesign --force --deep --sign -`): the linker already signs the inner
  binary, but with no bundle-level signature Gatekeeper saw a mismatched
  (tampered-looking) signature and blocked the app outright — fatal on arm64,
  which requires signed code to run. This is not notarization (first launch
  still needs right-click → Open), but it removes the dead-end "damaged" error
  on both the aarch64 and x86_64 downloads

## [0.1.14]

### Fixed
- Native file dialogs (theme/background/backup/export pickers) are now owned
  by the window that opened them: a parentless modal dialog could open
  *behind* the app, which then looked frozen and was reported by Windows as
  an application hang
- Quake Fade can no longer strand the window layered and semi-transparent
  when the animation is interrupted (rapid toggle, switching the animation
  off, config reload mid-fade): every instant show/hide path now restores
  full opacity, matching what the animation-completion path already did
- Quake auto-hide on focus loss no longer fires when focus moves to one of
  terminale's own windows (Settings, AI assistant) — configuring Quake from
  Settings used to fade the terminal away mid-edit

### Added
- Diagnostic file logging: terminale now writes a rolling daily log file to
  `<config dir>/logs/terminale.log.<date>` (GUI launches have no console, so
  freezes/crashes previously left nothing to inspect). New `[logging]` config
  section — `file_enabled` (default on), `file_level` (default `info`),
  `retention_days` (default 7) — plus a Diagnostics card in Settings → About
  with an "Open logs folder" button
- Expanded Lua plugin API: `get_selection()`, `get_scrollback(n)`,
  `get_visible_text()` (synchronous, copy-based snapshot reads) and
  `register_keybinding(combo, fn)`. Content reads are a privacy opt-in
  (`plugins.allow_scrollback_read`, default off) capped at
  `plugins.scrollback_read_cap` lines; plugin keybindings are gated by
  `plugins.allow_keybindings` and can never shadow the user's own keybinds
  or shortcuts. All three settings live in Settings → Plugins and apply
  without a restart

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

[Unreleased]: https://github.com/fbrzlarosa/terminale/compare/v0.1.36...HEAD
[0.1.36]: https://github.com/fbrzlarosa/terminale/compare/v0.1.35...v0.1.36
[0.1.35]: https://github.com/fbrzlarosa/terminale/compare/v0.1.34...v0.1.35
[0.1.34]: https://github.com/fbrzlarosa/terminale/compare/v0.1.33...v0.1.34
[0.1.33]: https://github.com/fbrzlarosa/terminale/compare/v0.1.32...v0.1.33
[0.1.32]: https://github.com/fbrzlarosa/terminale/compare/v0.1.31...v0.1.32
[0.1.31]: https://github.com/fbrzlarosa/terminale/compare/v0.1.30...v0.1.31
[0.1.30]: https://github.com/fbrzlarosa/terminale/compare/v0.1.29...v0.1.30
[0.1.29]: https://github.com/fbrzlarosa/terminale/compare/v0.1.28...v0.1.29
[0.1.28]: https://github.com/fbrzlarosa/terminale/compare/v0.1.27...v0.1.28
[0.1.27]: https://github.com/fbrzlarosa/terminale/compare/v0.1.26...v0.1.27
[0.1.26]: https://github.com/fbrzlarosa/terminale/compare/v0.1.25...v0.1.26
[0.1.25]: https://github.com/fbrzlarosa/terminale/compare/v0.1.24...v0.1.25
[0.1.24]: https://github.com/fbrzlarosa/terminale/compare/v0.1.23...v0.1.24
[0.1.23]: https://github.com/fbrzlarosa/terminale/compare/v0.1.22...v0.1.23
[0.1.22]: https://github.com/fbrzlarosa/terminale/compare/v0.1.21...v0.1.22
[0.1.21]: https://github.com/fbrzlarosa/terminale/compare/v0.1.20...v0.1.21
[0.1.20]: https://github.com/fbrzlarosa/terminale/compare/v0.1.19...v0.1.20
[0.1.19]: https://github.com/fbrzlarosa/terminale/compare/v0.1.18...v0.1.19
[0.1.18]: https://github.com/fbrzlarosa/terminale/compare/v0.1.17...v0.1.18
[0.1.17]: https://github.com/fbrzlarosa/terminale/compare/v0.1.16...v0.1.17
[0.1.16]: https://github.com/fbrzlarosa/terminale/compare/v0.1.15...v0.1.16
[0.1.15]: https://github.com/fbrzlarosa/terminale/compare/v0.1.14...v0.1.15
[0.1.14]: https://github.com/fbrzlarosa/terminale/compare/v0.1.13...v0.1.14
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
