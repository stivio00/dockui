# Architecture

```
┌──────────────────────────── main.rs ────────────────────────────┐
│  crossterm input thread ──► input_rx ─┐                         │
│  workers ────────────────►  msg_rx ───┼─► select! event loop    │
│  exec channel ───────────► exec_rx ───┘      │                  │
└──────────────────────────────────────────────┼──────────────────┘
                                               ▼
                                       App (src/app/)
                             all state · keys · mouse · actions
                                               ▲
                                               │ renders from
                                    src/ui/* (pure draw fns)
                                               │
                          workers.rs (tokio tasks) · docker via bollard
```

## Process layout

- **`main.rs`** owns the terminal and three channels:
  - `input_rx` — every crossterm event from a single reader thread. That
    thread is the *only* stdin reader in the process; even interactive
    exec sessions consume this channel (see Terminal handoff).
  - `msg_rx` — `Msg` values from background workers.
  - `exec_rx` — `TerminalRequest`s (interactive sessions) from the UI.
  The loop draws a frame, then `tokio::select!`s on the three channels
  plus an optional `--exit-after` deadline. Note: `select!` evaluates
  every branch expression even when a branch's `if` precondition is
  false, so futures must be built before the macro.
- **`app/`** holds every piece of mutable state (containers, stats,
  logs, events, tree rows, search, popups, toast, `areas`) and all
  behavior, split by concern: `mod.rs` (types, `App` struct, `handle_msg`,
  tree rebuild, view/stream bookkeeping, files-explorer state), `keys.rs`
  (every `handle_*_key` plus the action/edit/exec/ops launchers) and
  `mouse.rs` (mouse dispatch). Widgets own no state; anything that must
  survive a redraw lives here.
- **`files.rs`** parses docker archive (tar) listings into `FsEntry`
  rows — both tar shapes docker produces — and provides the mock
  filesystem for `--mock`.
- **`workers.rs`** spawns tokio tasks (list/stats/logs/events/inspect,
  container actions, recreate, ops, filesystem listings) that talk to the
  daemon through a cloned `Docker` client and report back via `Msg`.
  Worker sets are tracked in `JoinSet` groups so views can abort exactly
  the streams they own (`leave_view_streams`). Filesystem requests carry
  a monotonic `req` counter so a stale `FilesListed` can never overwrite
  a newer listing.
- **`ui/`** modules are pure functions of `App` + a `Rect`. As a side
  effect they record each region's rect into `App.areas`, which the
  mouse handlers read afterwards.
- **`exec.rs`** implements terminal handoff sessions (details below).

## Data flow

1. `workers::spawn_bootstrap` lists containers + volumes and connects a
   bollard client (or `App::mock` builds the demo fleet).
2. `App::handle_msg(Containers)` rebuilds the tree, prunes state for
   vanished containers, and requests details for the selection.
3. Views own their streams: entering Stats spawns per-container stats
   workers, entering Logs spawns log tailers; leaving a view aborts
   them. Log workers buffer partial lines until a newline so
   multiplexed docker frames never render as fragmented lines.
4. Actions (`S/K/R/D`, edit-recreate, ops) are applied either as local
   mock mutations (with a toast) or by spawning an action worker; the
   result comes back as `Msg::ActionDone` → toast (+ auto-open logs).

## Terminal handoff (exec / attach ops)

`t` on a running container, or an op with `attach: true`, sends a
`TerminalRequest` over the exec channel. The event loop then:

1. Disables mouse capture, leaves the alternate screen.
2. Runs the session: `docker exec -it` (create_exec → start_exec) or
   `docker run -it` (create → attach → start).
3. Pumps both directions until the remote side ends. Output frames go
   straight to stdout; input comes from the *same* crossterm channel the
   TUI uses, re-encoded from `KeyEvent`s to terminal byte sequences —
   there is never a second reader on stdin.
4. Re-enters the alternate screen, sends `Clear(ClearType::All)`,
   re-enables mouse capture, rebuilds the `Terminal` (fresh buffers force a
   full repaint without querying the cursor over stdin), and resumes the
   TUI. Session errors surface as a toast, not a crash.

## Threading & async model

One async runtime (multi-thread tokio) for the event loop and workers;
one OS thread blocked in `crossterm::event::read()` feeding an unbounded
channel. `App` is only touched from the event loop, so it needs no
locks. On quit the loop aborts all JoinSets and the process exits
explicitly so the reader thread cannot block shutdown.
