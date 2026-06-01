# Security Policy

## Reporting a vulnerability

Please **don't** open a public issue for security problems.

Report privately via **GitHub's private vulnerability reporting**:
[**Report a vulnerability →**](https://github.com/fbrzlarosa/terminale/security/advisories/new)

(Repository → **Security** tab → **Report a vulnerability**.) This keeps the
report visible only to the maintainers until a fix is ready.

Please include: what you observed, steps to reproduce, the affected version /
commit, and your OS. A proof-of-concept (e.g. an escape-sequence payload or a
small repro) helps a lot.

## Supported versions

`terminale` is pre-1.0 and moves fast: security fixes target the **latest
released version**. Older versions are not maintained — please update before
reporting.

## Scope

In scope — defects in `terminale` itself, including:
- Memory-safety issues in the Rust code (especially `unsafe` blocks)
- Crashes or unexpected behaviour triggered by adversarial terminal output
  (escape sequences, OSC payloads, images)
- Escapes from the Lua plugin sandbox
- SSH-client handling flaws

Out of scope:
- Bugs in upstream dependencies — report those to the respective projects
- Issues that require the attacker to already have full local user access
- Use of your own AI-provider API keys (manage your own credentials)

## Disclosure

Coordinated disclosure: you report privately, we confirm and prepare a fix and
an advisory, then we publish it. If you'd like to be credited in the advisory,
say so in your report; otherwise we keep it anonymous.

Thanks for helping keep `terminale` and its users safe.
