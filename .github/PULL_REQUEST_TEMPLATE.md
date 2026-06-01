<!-- Thank you for contributing to terminale! Fill out the sections below. -->

## Summary

<!-- Briefly describe what this PR changes and why. Link the issue it addresses. -->

Closes #

## Type of change

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Breaking change (fix or feature that changes existing behavior)
- [ ] Performance improvement
- [ ] Documentation only
- [ ] Refactor / internal cleanup
- [ ] Test infrastructure

## Cross-platform check

<!-- terminale targets Linux, macOS, and Windows equally. -->

- [ ] Tested on Linux
- [ ] Tested on macOS
- [ ] Tested on Windows
- [ ] CI matrix passes on all three (verified in PR checks)
- [ ] No new OS-specific code, or new code is gated with `#[cfg(target_os = "…")]`

## Tests

- [ ] Unit tests added/updated
- [ ] Integration tests added/updated (if applicable)
- [ ] Snapshot tests updated (if renderer touched)
- [ ] Bench unchanged or improved (if hot path touched)
- [ ] Coverage stays above 75% for affected crates

## Quality gates

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- [ ] `cargo deny check` passes (new deps audited)
- [ ] CHANGELOG.md updated under `## [Unreleased]`
- [ ] Docs updated (`docs/`, rustdoc, README if user-facing)

## Screenshots / demos

<!-- For UI changes, include a screenshot or short GIF. -->

## Additional context

<!-- Anything reviewers should know: tradeoffs, follow-up issues, etc. -->
