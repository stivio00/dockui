# UI reference

The screen is a header line, a body, a status line (toast / error) and a
footer of key hints. The body depends on the active view:

- **Tree view** (`1`): left = tree (58% default, resizable), right = detail pane.
- **Stats** (`2`), **Events** (`3`), **Logs** (`4`): single full-width
  panel; Logs/Events target names are shown in the panel title.
- **Files** (`f` on a container or volume row): filesystem explorer.
- **Detail focus**: inside Tree view, focus can move between the tree and
  the detail pane (Tab / Enter).

## Views

### Tree

Rows are, in order: compose projects → services → containers, then
standalone containers, then volumes. Rows carry an expand caret; `expanded`
is a set of row keys on `App`, so tree state survives redraws. With a
search filter active the tree keeps matched rows, their ancestors, and the
full subtrees of matched parents, and auto-expands everything so matches
are visible; the selection snaps to the first matching container.

The tree/detail split is `App.split_pct` (58% default, clamped 20–80):
`<` / `>` nudge it in both focus modes, and dragging the divider between
the panes with the mouse resizes it live (drag state: `App.dragging_split`).

### Detail pane

Sections: identity (name, id, image, command), badges — `root`,
`privileged`, `gpus:N`, `net:host` / `pid:host` / `ipc:host`, `--rm` —
then status/health/exit, networks, mounts, ports, entrypoint/workdir,
restart policy, shm size, and env vars (the env list is scrollable via the
detail focus keys).

### Stats

A table of running containers (CPU%, MEM, MEM%, NET I/O, BLOCK I/O, PIDS)
and, below, CPU and memory sparklines for the selected container. Values
come from 1 Hz stats workers; history is a fixed-width ring per container
for the sparkline. When a filter is active only matching containers are
listed.

### Logs / Events

Log lines are tagged `name │ message` when multiple containers are
targeted; the title shows `N/M lines` (filtered/total) and the filter
itself. Follow mode keeps the view pinned to the newest line; any scroll
up turns follow off, returning to the bottom re-enables it. Clicking a
line pins it (`off = len - 1 - abs(y)`), like k9s.

### Files

`f` on a container (or volume) row opens a filesystem explorer for it.
The title shows the target, the first line the current path plus status
(`…` while loading, the error in red, else `N items`). Directories render
cyan with a `/` suffix and a `▸` caret, files show their size
(`13B`, `1.1M`). `j/k/g/G` navigate, Enter/`l` (or a second click on the
selected row) enters a directory, `←`/`h`/Backspace goes to the parent,
`r` reloads, `/` filters by name, Esc returns to the tree.

Container listings come from the docker archive API (works on stopped
containers too). Volume listings are read through a throwaway
never-started `busybox:latest` container that bind-mounts the volume at
`/mnt/dockui` and is always removed afterwards — if `busybox` is not
pulled the error line says so.

## Popups

All popups are centered boxes rendered last; their rect + inner list rect
are recorded in `App.areas` for mouse support. Clicking outside closes.

- **Contexts** (`c`) — docker context switcher.
- **Help** (`?`) — key reference.
- **Ops** (`o`) — entries from `~/.dockui/ops.yml` with description + a
  flag summary (image, --privileged, --gpus, --net …, -it).
- **Confirm** — shown for remove; `y`/Enter proceeds, `n`/Esc cancels.
- **Edit** (`E`) — 8 fields (Name, Image, Command, Env `;`-separated,
  Ports `;`-separated, Privileged, GPUs cycle, Network) + an APPLY row.
  ↑/↓/Tab move, letters type into text fields, space/Enter cycles
  toggles/choices, Enter on APPLY recreates the container with all other
  host settings passed through unchanged.
- **Env editor** — Enter (or a click) on the Edit form's Env field opens a
  KEY/VALUE table over it. ↑/↓ or j/k pick a row, Enter/Tab start editing
  the key (then the value), `a` appends a row, `d`/Delete removes it, and
  Esc applies the table back into the Env field. Clicking a cell edits it
  directly; clicking outside the table applies and closes it. Rows with an
  empty key are dropped; values may contain `=` but not `;`.
- **Exec** (`t`) — "as configured user" / "as root" / "custom user…" →
  mini text input; Enter launches the terminal handoff.

## Toast & search prompt

Transient messages (action results, errors) render on the line above the
footer for 4 s — green for success, red for failure. While typing a
filter (`/`) the footer is replaced by the prompt: the input with a
block-cursor, plus Enter-apply / Esc-cancel / Ctrl-U-clear hints, and the
regex compile error if the pattern is invalid (the prompt stays open so
it can be fixed).

## Mouse

- Wheel scrolls the popup list if one is open, else the focused area of
  the active view (tree rows, detail, stats rows, logs/events lines,
  files rows).
- Click selects: tree rows (second click on the selected row acts as
  Enter), stats table rows, popup entries (including edit-form fields and
  APPLY), context list, files rows (second click on a directory opens it).
- Click in the detail pane moves focus there; clicking outside a popup
  closes it; clicking a log/event line pins the view to it.
