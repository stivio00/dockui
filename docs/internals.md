# Internals

Notes that are easy to get wrong; verified against the crate sources in
`~/.cargo/registry/src/` (bollard 0.18.1, ratatui 0.30.2, crossterm 0.29).

## bollard 0.18 API map

- The Docker client is cheap to clone (`Arc` inside); workers take a clone.
- **Where types live**: `Config` (container create body) is
  `bollard::container::Config<T>` — `bollard::models::Config` is the
  *secret* config, a different, non-generic type. `HostConfig`,
  `PortBinding`, `RestartPolicy`, `DeviceRequest` etc. come from
  `bollard::models`. Option structs (`CreateContainerOptions<T>`,
  `RemoveContainerOptions`, `StopContainerOptions{t:i64}`,
  `RestartContainerOptions{t:isize}`, …) are generic over
  `T: Into<String>` where they carry strings.
- `CreateContainerOptions { name: T, platform: Option<T> }` — the name is
  `T`, not `Option<T>`; pass `""` to let the daemon generate a name.
- Port mapping: `PortMap = HashMap<String, Option<Vec<PortBinding>>>`, so
  pushing a binding needs `or_insert_with(|| Some(Vec::new()))`. Exposed
  ports in `Config` are `HashMap<String, HashMap<(), ()>>`.
- Exec: `create_exec` → `start_exec(Some(StartExecOptions{detach:false,
  tty:true}))` → `StartExecResults::Attached { output, input }` where
  output is a `LogOutput` stream and input an `AsyncWrite`. For
  `docker run -it` use `attach_container` (hijack) **before**
  `start_container`, with `open_stdin`/`attach_*`/`tty` set on the create
  config.
- Errors: connection/HTTP failures arrive as
  `DockerResponseServerError { status_code: u16, message }`; 404 checks
  use that (see `workers::is_not_found`).
- Image pulls stream `CreateImageInfo` with `.error` on failure; dockui
  pulls on `pull: always` or on a 404 at create time.

## ratatui 0.30 / crossterm 0.29

- `ratatui::init()` = raw mode + alternate screen + panic hook; **no mouse
  capture** — enable/disable `EnableMouseCapture` /
  `DisableMouseCapture` manually around the run loop (and drop it during
  terminal handoff).
- `Table::new(rows, widths)` / `List::new(items)` take iterators;
  `.row_highlight_style()` styles the stateful selection.
  `render_stateful_widget` is required for `ListState`/`TableState`, and
  their `.offset()` is what makes click-to-row mapping accurate.
- `Block::bordered().title(..)`, `Sparkline::default().data(..).max(n)`,
  `Clear` before drawing a popup over content.
- crossterm key events: `KeyEventKind::Press` only on Unix, but check
  anyway before acting (Windows sends Release too).

## Worker protocol (`Msg`)

Workers send `Msg` over a bounded channel (1024). Variants: `Containers`,
`Volumes`, `Inspected`, `Stats`,`LogLine`, `Events`, `ActionDone { label,
ok, message, open_logs }`, `ConnErr`… `App::handle_msg` is the only
writer of derived state; on `Containers` it also prunes stats/histories/
details for containers that no longer exist.

Streams are grouped in `JoinSet`s by lifetime (`workers`, `stats_workers`,
`log_workers`); `leave_view_streams` aborts the ones a view owns when
switching away — this is what prevents stats worker leaks and log tailers
outliving their view.

Log tailers buffer bytes until a newline before sending `LogLine`, so
multiplexed docker frames (stdout/stderr interleaved in 16 KB chunks)
render as whole lines.

## Interactive sessions

Both `docker exec -it` and attached ops share `exec::attached_session`:
output frames are written straight to stdout by a spawned pump task; the
session loop `select!`s between that pump finishing (remote end) and key
events arriving on the app's input channel, re-encoded via
`encode_key` (chars, CR, DEL, arrows, F-keys, Ctrl+letter → control
bytes, Alt+letter → ESC-prefixed). Ctrl-C therefore reaches the shell as
`0x03` instead of quitting dockui. Keeping crossterm as the sole stdin
reader is the invariant that makes this robust.

bollard wraps hijacked exec/attach connections in
`NewlineLogOutputDecoder::new(true)`, so TTY streams (raw pty bytes, no
multiplexing header) decode to `LogOutput::Console` frames — never
`StdOut`/`StdErr`. The pump must write `Console` frames too; dropping
them makes every interactive session look dead (blank screen) on any
transport, named pipe or unix socket alike. Non-TTY execs send
multiplexed 8-byte-header frames instead, which decode to
`StdOut`/`StdErr`.

## Mock mode

`App::mock` fabricates a fleet (compose project `webshop`, standalone
containers, volumes); containers whose id starts with `b` get
privileged/GPU/host-namespace demo details for badge testing. Actions are
simulated by mutating mock state + toast; exec reports it needs a real
connection. Tests never touch the real daemon.
