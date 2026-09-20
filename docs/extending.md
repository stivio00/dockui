# Contributing & extending dockui

## Ground rules

- `cargo clippy --all-targets` zero warnings, `cargo test` green, `cargo
  fmt` run — before every commit.
- Comments only where the code is genuinely non-obvious; none in tests.
- Tests must not launch or mutate containers on a real daemon; use
  `--mock` / `App::mock`.

## Adding a UI feature

The pattern, end to end (search `/` is the reference implementation):

1. **State** goes on `App` (`src/app.rs`) — anything that must survive a
   redraw. If it's a popup payload, keep `Popup` itself `Copy`/data-free
   and hold the data in a dedicated `Option<…>` field.
2. **Keys**: add a branch to the matching `handle_*_key` (or
   `handle_global_key` if it works everywhere). Escape ordering matters:
   filter-clear comes first, then popups, then view escape.
3. **Mouse**: if the feature is clickable, record its `Rect` in
   `App.areas` during draw and handle it in `handle_mouse` /
   `mouse_scroll` / `mouse_left`. Use the persistent `ListState`'s
   `offset()` when mapping a click to a row index.
4. **Rendering** is a pure function under `src/ui/` taking `&mut App` (it
   may advance its own state, e.g. `ListState`). No business logic.
5. **Workers**: background work goes in `src/workers.rs` as a spawn
   function returning a `JoinHandle` that reports via `Msg`; add the
   `Msg` variant in the same file and handle it in `App::handle_msg`.
   Add the handle to the right `JoinSet` so `leave_view_streams` can
   abort it.
6. **Tests**: full-UI render tests run against a `TestBackend` with the
   mock fleet — see `tests/render.rs`; drive keys with the `press` helper
   and assert on buffer text. Pure logic (parsing, mapping, expansion)
   gets unit tests (`tests/ops.rs` style).
7. `cargo fmt && cargo clippy --all-targets && cargo test`.

## Adding an op field

1. Add the field to `Op` in `src/ops.rs` (`#[serde(default)]`, so old
   files keep parsing; `deny_unknown_fields` is on, so typos fail loudly).
2. Map it in `Op::to_create` (and `summary()` if it should show in the
   ops popup). Follow the docker CLI mapping — the field should behave
   like the corresponding `docker run` flag.
3. Document it in `ops.example.yml`.
4. Cover parsing + mapping in `tests/ops.rs`.

## Adding a detail field / badge

`ContainerDetails` (in `model.rs`) is filled by `convert_inspect` in
`workers.rs` from a `ContainerInspectResponse`; render it in
`ui/detail.rs` (badges first line, then kv sections). Note
`ContainerDetails` cannot derive `PartialEq` (bollard types), so tests
assert on rendered text.

## Adding a docker API call

Check the exact shapes in
`~/.cargo/registry/src/index.crates.io-*/bollard-0.18.1/src/` first (see
`docs/internals.md` for the traps: generic option structs, models vs
container module, PortMap's `Option<Vec<_>>`). Wrap it in a worker; map
failures to `Msg::ActionDone` or `ConnErr`, never panic in a worker.
