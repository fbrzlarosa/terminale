# Design — persistent sessions (v1.0, `terminale-ipc`)

Status: **proposed**. This document settles the architecture before any of the
PTY path is touched, because the choice below is the expensive one to reverse.

## The goal, stated as a user would

Close the window, or have it crash, or reboot the terminal emulator while a
`cargo build` is running — and find the session still there, with its scrollback,
when terminale comes back. No prefix key, no separate command to learn, no
`tmux attach`: the tabs are simply still the tabs.

This is the feature that makes a terminal hard to leave. It is also the one
people already leave terminale *for*, by running tmux inside it.

## The constraint everything follows from

A PTY's child receives `SIGHUP` when the master side closes. So the master fd
must be held by a process that outlives the window — that is not a design
preference, it is the reason tmux, screen, dtach and abduco all have a daemon.
terminale needs one too.

The only real question is **how much** lives in that daemon.

## Two candidate architectures

### A. Daemon owns the PTY *and* the emulation (the tmux model)

The daemon parses output into a grid and keeps the scrollback; the client asks
for screen state and renders it.

* Reattach is exact, including a half-drawn TUI, because the authoritative grid
  never went anywhere.
* Several clients can attach to one session — the door to `tmux -CC`-style
  sharing and to the v1.5 milestone.
* But: the emulator (`terminale-term`, ~8k lines) has to move out of the GUI
  process, and a grid-diff protocol has to exist between them. That is a
  rewrite of the middle of the application, and every rendering feature —
  Sixel, ligatures, hyperlinks, the OSC 133 marks — has to cross the wire.

### B. Daemon owns the PTY, relays bytes (the dtach model) — **proposed**

The daemon creates the PTY, spawns the shell, keeps a bounded ring of recent
output, and relays bytes to whichever client is attached. On reattach the client
replays the ring through its own emulator and arrives at the same screen.

* The emulator stays where it is. Nothing about rendering crosses the wire, so
  no feature has to be re-implemented in a protocol.
* Correct by construction for the case that matters: the emulator is a pure
  function of (byte stream, resize events), so replaying the same bytes rebuilds
  the same grid.
* Costs: scrollback older than the ring is lost on reattach (bounded, and the
  bound is a setting); a program that redraws only on `SIGWINCH` may need a
  resize nudge after replay, which is exactly what the daemon sends anyway.

**Choosing B.** It buys the whole user-visible promise for a fraction of the
blast radius, and it does not foreclose A: a later daemon-side emulator can be
added behind the same protocol when multi-client attach is actually wanted for
v1.5.

## Shape

```
  terminale (GUI)                       terminaled (session daemon)
  ┌──────────────────────┐              ┌────────────────────────────────┐
  │ tab ── emulator ─────┼── attach ───▶│ session <id>                   │
  │           ▲          │  input       │   pty master (owns it)         │
  │           └──────────┼◀── output ───┤   child shell                  │
  └──────────────────────┘  resize      │   ring buffer (bounded)        │
                                        │   metadata: profile, cwd, title│
   window→session map persisted         └────────────────────────────────┘
   alongside the existing last-session state       one daemon, N sessions
```

* **Transport**: a second Unix socket, `$XDG_RUNTIME_DIR/terminale-sessions.sock`,
  mode 0600 — same trust model and same location as the existing control socket.
* **Daemon lifetime**: started on demand by the first session that needs it,
  double-forked and `setsid`, so it is not in the GUI's process group and
  survives its exit. Exits on its own once the last session's child has exited
  and no client is attached.
* **Reattach**: on launch the client lists sessions, matches them against the
  window→session map it persisted, and attaches. An unmatched live session is
  offered rather than silently adopted or killed.
* **Session end**: when the child exits. A session with no client attached stays
  alive — that is the entire point — bounded by a configurable maximum count so
  a runaway loop of windows cannot fill the machine.
* **Platform**: Unix first. ConPTY handles cannot be handed between processes the
  way a pty master fd can, so Windows needs a different mechanism and is out of
  scope for phase 1 — the same boundary the control socket already has.

## Configuration

Opt-in for its first release, because it changes what closing a window means:

```toml
[sessions]
persistent          = false   # master switch; off until the user asks for it
reattach_on_launch  = true    # attach surviving sessions to the window they were in
replay_bytes        = 2097152 # per-session ring; what reattach can rebuild
max_sessions        = 32      # refuse rather than fill the machine
```

Every one of those gets a control in Settings › Sessions, per the project's
configurability rule.

## Phases

1. **Protocol and replay buffer** (`terminale-ipc`) — wire types and the bounded
   ring, unit-tested, wired into nothing. No behaviour change.
2. **The daemon** — create/list/attach/kill over the socket, plus
   `terminale sessions {list,kill}` to drive it by hand. Still nothing in the GUI.
3. **The client** — a tab may be backed by a daemon session instead of a local
   PTY, behind `[sessions].persistent`.
4. **Reattach on launch** + the Settings section + docs.

Each phase is a PR that leaves `main` shippable, and phase 3 is the only one that
touches the existing PTY path.

## Risks, and what is done about them

* **A wedged daemon holding your shells.** `terminale sessions kill` exists from
  phase 2, and the daemon exits when the last child does.
* **Replay of a screenful of Sixel or a 2 MiB ring being slow.** The replay is
  the same parse path a fast `cat` already takes; it is measured in phase 3
  against the existing parser benchmarks before the flag is offered.
* **Two clients attaching to one session.** Refused in phase 2 (one attach at a
  time) rather than half-supported; architecture A is what makes it correct.
* **A session outliving its usefulness.** `max_sessions`, and the launch flow
  *offers* orphans instead of silently reattaching them.
