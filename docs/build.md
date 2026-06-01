# Building from source

`terminale` is a Rust workspace. You need a recent stable toolchain and the
native graphics/PTY prerequisites for your OS.

## Prerequisites

- **Rust 1.88+** (install via [rustup](https://rustup.rs)).
- A GPU/driver with Vulkan, Metal, DX12, or OpenGL — `wgpu` picks the best
  available backend automatically.

### Linux

Install the development headers for the windowing and font stacks. On
Debian/Ubuntu:

```bash
sudo apt install build-essential pkg-config cmake \
  libfontconfig1-dev libxkbcommon-dev \
  libwayland-dev libxcb1-dev
```

Vulkan or an up-to-date Mesa GL driver is recommended. On Fedora/Arch use the
equivalent `fontconfig`, `libxkbcommon`, `wayland`, and `libxcb` `-devel`
packages.

### macOS

Install the Xcode command-line tools:

```bash
xcode-select --install
```

Nothing else is needed — Metal ships with the OS.

### Windows

Install the **Visual Studio Build Tools** with the *Desktop development with
C++* workload (provides the MSVC toolchain and the Windows SDK). The default
backend is DX12.

## Build & run

```bash
git clone https://github.com/fbrzlarosa/terminale
cd terminale
cargo build --release
./target/release/terminale
```

For an optimized local install on your `PATH`:

```bash
cargo install --path crates/terminale
```

## Useful commands

```bash
cargo run -p terminale            # debug run
cargo test --workspace            # run the full test suite
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all                   # format
```

## Workspace layout

| Crate | Responsibility |
|---|---|
| `terminale` | The app: window, event loop, tabs, palette, settings UI, suggestion bar |
| `terminale-core` | Shared domain types and glue |
| `terminale-term` | Terminal grid + ANSI engine |
| `terminale-render` | GPU rendering: glyph atlas, background pipeline, pixel font |
| `terminale-ui` | Reusable UI widgets |
| `terminale-config` | TOML schema, defaults, validation, keybinds |
| `terminale-ai` | AI providers behind one trait |
| `terminale-ssh` | SSH client |
| `terminale-plugin` | Lua plugin host |

## Packaging helpers

A couple of `xtask` commands help produce platform assets:

```bash
# Regenerate the Windows .ico and macOS .icns from assets/icons/icon.svg
cargo run -p xtask -- gen-icons

# macOS: wrap a built binary into a proper terminale.app bundle (run on macOS)
cargo build --release
cargo run -p xtask -- bundle-macos          # → target/terminale.app
open target/terminale.app                   # launch it
# or drag target/terminale.app into /Applications to get it in Launchpad

# macOS: build the installer .pkg that drops terminale.app into /Applications
# (this is what the release pipeline ships; needs the system `pkgbuild`)
cargo run -p xtask -- pkg-macos             # → target/terminale-v<ver>-<triple>.pkg
```

A bare Unix binary in `/Applications` doesn't show up in Launchpad/Spotlight and
opens inside your terminal when double-clicked; the `.app` bundle makes macOS
treat terminale as a real GUI application with its icon. `pkg-macos` wraps that
same bundle in an installer package — the macOS release artifact is built this
way (see the `macos-app-pkg` job in `.github/workflows/release.yml`), not by
cargo-dist's bare-binary pkg.

## Troubleshooting

- **Blank window / GPU errors on Linux:** ensure a working Vulkan or GL driver;
  set `WGPU_BACKEND=gl` to force the OpenGL backend as a fallback.
- **Font not found:** the requested `font.family` isn't installed; `terminale`
  falls back to a bundled monospace family. Install the font or pick another in
  Settings.
