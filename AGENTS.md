# AGENTS.md — working on dockui

`dockui` is a ratatui + bollard Docker TUI. Read `docs/architecture.md` first;
this file is the quick operating manual for agents and contributors.

## Commands

```sh
cargo build                        # build debug
cargo run                          # run against the real docker daemon
cargo run -- --mock                # demo fleet, no daemon needed
cargo run -- --exit-after 3000     # self-quit after N ms (smoke tests)
cargo test                         # unit tests + TestBackend render tests
cargo clippy --all-targets         # must be warning-free
cargo fmt                          # always run before committing
```

Smoke-test the TUI without a terminal of your own:

```sh
script -q /tmp/dockui.log sh -c 'stty rows 40 cols 120; exec cargo run -- --exit-after 5000'
```

On Windows (pwsh) just run `cargo run -- --exit-after 5000` — the TUI
renders ANSI even without a pty.

## Non-negotiables

- `cargo clippy --all-targets` reports zero warnings; `cargo test` is green
  before any commit.
- No comments unless the code is genuinely non-obvious; none in tests.
- Never launch privileged/GPU containers against the user's real daemon
  from tests — use `--mock` (App::mock) for anything that mutates state.
- bollard 0.18 API notes live in `docs/internals.md`; verify field names
  against the crate source in `~/.cargo/registry/src/` before adding new
  API calls — many option structs are generic over `T: Into<String>` and
  several types exist under both `bollard::models` and other modules.
- ratatui 0.30: `ratatui::init()` enables raw mode + alt screen but NOT
  mouse capture — dockui enables/disables it manually in `src/main.rs`.
- NEVER call `Terminal::clear()` (ratatui): it reads the cursor position
  over stdin (`\x1b[6n`), which races with the app's input thread and hangs
  without a terminal emulator. To force a full repaint, recreate the
  `Terminal` and send `Clear(ClearType::All)` after `EnterAlternateScreen`
  (see `exec_session` in `src/main.rs`).
- State that must survive redraws lives on `App` (src/app.rs), never in
  widgets. `App.areas` is written during draw and read by mouse handlers.
  Popup-layer state that rides on another popup (e.g. `env_editor` over
  `Popup::Edit`) and layout state (`split_pct`, `dragging_split`) also
  live on `App`.

## Structure

- `src/main.rs` — event loop, input thread, interactive terminal handoff
- `src/app.rs` — all state, key/mouse handling, actions, popups
- `src/workers.rs` — background tasks and the `Msg` protocol
- `src/ui/` — pure rendering from `App`; no logic
- `src/ops.rs`, `src/actions.rs`, `src/exec.rs` — ops schema, edit form +
  container actions, attached terminal sessions
- `src/mock.rs` — demo fleet; containers whose id starts with `b` get
  privileged/GPU/host-namespace demo details
- `tests/render.rs` — full-UI tests on a 130x42 TestBackend against mock
  data; `tests/ops.rs` — ops parsing/mapping unit tests;
  `tests/exec.rs` — attached-session pump/key-encoding tests

## Adding a feature

1. Put state and behavior in `App` (or a new module with pure functions).
2. Render it under `src/ui/`, set its `App.areas` rect if it needs clicks.
3. Wire keys in the matching `handle_*_key`, mouse in `handle_mouse`.
4. Add a render test in `tests/render.rs` asserting on the buffer text.
5. `cargo fmt && cargo clippy --all-targets && cargo test`.
