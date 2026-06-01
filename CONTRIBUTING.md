# Contributing to `terminale`

Thanks for your interest in contributing! This document covers everything you
need to know to make your first PR a success.

## Code of Conduct

This project follows the [Contributor Covenant 2.1](CODE_OF_CONDUCT.md).
By participating you agree to abide by it.

## Quick start

```bash
git clone https://github.com/fbrzlarosa/terminale
cd terminale

# Install required toolchain (pinned in rust-toolchain.toml)
rustup show

# Install lefthook (pre-commit hooks) — optional but recommended
cargo install lefthook
lefthook install

# Install dev tools
cargo install cargo-llvm-cov cargo-deny cargo-audit typos-cli

# Verify everything works
cargo xtask ci
```

## Project layout

This is a Cargo workspace. Each subsystem lives in its own crate:

| Crate | Responsibility |
|---|---|
| `terminale-core` | PTY spawn, session lifecycle, event bus |
| `terminale-term` | Terminal engine wrapping `alacritty_terminal` |
| `terminale-render` | wgpu pipelines, glyph atlas, shaders |
| `terminale-ui` | winit windows, tabs, input handling |
| `terminale-config` | TOML config + schema + figment layering |
| `terminale` | Binary entry point |
| `xtask` | Workspace task runner for CI commands |

See [`docs/architecture.md`](docs/architecture.md) for the full picture.

## Workflow

### 1. Pick an issue

- Look for issues labeled `good-first-issue` or `help-wanted`
- Comment on the issue to claim it before starting work
- For larger changes, open an RFC discussion first

### 2. Branch naming

```
feat/<short-description>      # new feature
fix/<short-description>       # bug fix
docs/<short-description>      # documentation only
refactor/<short-description>  # internal restructuring
perf/<short-description>      # performance improvement
test/<short-description>      # test-only changes
chore/<short-description>     # tooling / build
```

### 3. Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>

[optional body]

[optional footer]
```

Examples:

```
feat(ui): add drag-out tab to new window
fix(render): handle GPU lost event without crash
docs(config): document keybinds section
perf(term): avoid alloc in escape parser hot path
```

Allowed types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`,
`ci`, `chore`.

### 4. Tests are mandatory

Every code change requires tests. See [`docs/testing.md`](docs/testing.md) for
patterns. Minimum standards:

- New function → at least one unit test
- New module → integration test demonstrating it
- Bug fix → regression test that fails without the fix
- Renderer change → snapshot test via `insta`
- Parser change → property test via `proptest`

Coverage threshold (enforced in CI): **≥ 75% per non-binary crate**.

### 5. Run CI locally before pushing

```bash
cargo xtask ci    # runs: fmt --check, clippy -D warnings, test, deny
```

If lefthook is installed, this runs automatically on `git push`.

### 6. Open a PR

- Fill in the PR template
- Link the issue it closes (`Closes #123`)
- Mark as draft if work is in progress
- Request review when ready

## Cross-platform requirements

`terminale` targets Linux, macOS, and Windows equally. Every PR must:

1. Pass CI on all three OS matrix entries
2. Not regress behavior on any OS
3. Use cross-platform APIs (`portable-pty`, `directories`, `global-hotkey`)
   rather than OS-specific calls where possible
4. Document any OS-specific behavior in `docs/platform.md`

If your change needs OS-specific code, gate it with `#[cfg(target_os = "…")]`
and write a test matrix entry.

## Adding a dependency

New dependencies require:

1. A commit message that explains *why* the dep was added
2. Compatible license (MIT, Apache-2.0, BSD-2/3, ISC, MPL-2.0, Zlib, Unicode)
   — verified by `cargo deny check`
3. No open advisories (`cargo audit`)
4. Update to `crates/<crate>/Cargo.toml` AND `Cargo.toml` workspace block
5. Update `docs/dependencies.md`

Forbidden (`cargo deny` enforced):
- Anything Electron-like (`tao`, `wry`, etc.) — see project philosophy in README
- GPL/AGPL crates (incompatible with our MIT/Apache dual license)

## Performance regressions

Performance-critical code (renderer, parser) is covered by `criterion`
benchmarks in `benches/`. CI runs benchmarks on demand and posts a regression
report. If your PR slows a benchmark by >5%, justify it or revisit.

## Documentation

User-facing docs live in `docs/`. Rustdoc lives in source. Update both:

- New public API → rustdoc with example
- New config option → `docs/config.md`
- New keybind → `docs/keybindings.md`
- Breaking change → `CHANGELOG.md` under `## [Unreleased]`

## Releasing

Maintainers only. See [`docs/release.md`](docs/release.md).

## Questions?

- General questions → [Discussions](https://github.com/fbrzlarosa/terminale/discussions)
- Bug? → [Issue](https://github.com/fbrzlarosa/terminale/issues/new/choose)
- Security? → [`SECURITY.md`](SECURITY.md)

Welcome aboard 🚀
