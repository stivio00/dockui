# dockui — Docker, in your terminal

`dockui` is a fast, mouse-aware terminal UI for Docker: browse containers,
projects, services and volumes; watch live CPU/memory sparklines; tail logs
and events; start/stop/restart/remove containers; edit-and-recreate them
with changed ports, env, GPUs or privileges; open an interactive shell on
any running container; and launch your own one-key "ops" (e.g. a
privileged `top`, a GPU inventory, or a disposable toolbox shell).

Built with [ratatui](https://ratatui.rs) + [bollard](https://github.com/fussybeaver/bollard-rust),
no docker CLI required.

```
cargo run --release
```

## Features

- **Tree view** of compose projects → services → containers, standalone
  containers and volumes, with state icons and live status.
- **Detail pane** for the selection: image, command, networks, mounts,
  ports, health, restart policy — plus root / privileged / GPU /
  host-namespace badges.
- **Stats view** with per-container CPU%, memory, net/block I/O and PIDs,
  and CPU/memory sparklines for the selected container (1s cadence).
- **Logs & events views**, follow mode, scroll, and pin-to-line by mouse
  click.
- **Actions**: `S/K/R/D` start/stop/restart/remove (delete asks for
  confirmation), `E` edit-and-recreate (name, image, command, env, ports,
  privileged, GPUs, network), `t` interactive shell (`docker exec -it`,
  bash-or-sh, as configured user / root / custom).
- **Ops** (`o`): user-defined one-key container launches from
  `~/.dockui/ops.yml` — privileged/GPU/host-namespace configs, port and
  volume mappings, `attach: true` for `docker run -it` style ops.
- **Search/filter** (`/`) with regex, applied to the tree, stats, logs and
  events. `Esc` clears.
- **Mouse**: click to select, wheel to scroll, click logs/events to pin,
  second click on a tree row to expand.
- **Docker contexts** (`c`) and `--mock` demo mode with a sample fleet.

## Install

From source (Rust 1.85+):

```sh
cargo install --git https://github.com/stivio00/dockui
# or from a checkout:
cargo install --path .
```

`dockui` talks to the daemon via `DOCKER_HOST` or the docker context store,
just like the docker CLI.

## Keys

| Key                     | Action                                            |
|-------------------------|---------------------------------------------------|
| `j` / `k` / arrows      | Navigate (also scrolls detail, logs, events)      |
| `g` / `G`               | Top / bottom                                      |
| Enter / Space / Right   | Open selection (expand groups, focus detail)      |
| `h` / Left / Esc        | Collapse / go up / clear filter / leave view      |
| `1` `2` `3` `4`         | Tree / stats / events / logs view                 |
| `L`                     | Logs for selection                                |
| `t`                     | Interactive shell on running container            |
| `E`                     | Edit & recreate container                         |
| `S` `K` `R` `D`         | Start / stop / restart / remove container         |
| `o`                     | Ops launcher (from `~/.dockui/ops.yml`)              |
| `/`                     | Regex filter (Enter applies, Esc cancels)         |
| `c`                     | Switch docker context                             |
| `r`                     | Refresh                                           |
| `q` / Ctrl-C            | Quit                                              |
| `?`                     | Help                                              |

## Ops — `~/.dockui/ops.yml`

Define one-key launches; see [`ops.example.yml`](ops.example.yml) for all
fields. Highlights:

```yaml
top:                       # privileged host monitor with all GPUs
  image: ubuntu:24.04
  privileged: true
  gpus: all
  network: host
  pid: host
  volumes: ["/proc:/host/proc:ro", "/sys:/host/sys:ro"]
  command: top

shell:                     # docker run -it toolbox
  image: alpine:3.20
  attach: true             # hand the terminal to the container
  command: sh -c "apk add --no-cache curl jq; exec sh"
```

Names and env values expand `${VAR}` / `$VAR` from your environment.

## Development

```sh
cargo run -- --mock        # demo fleet, no docker needed
cargo test                 # unit + render tests (TestBackend)
cargo clippy --all-targets # zero-warning policy
cargo fmt
```

Further docs: [docs/architecture.md](docs/architecture.md) ·
[docs/ui.md](docs/ui.md) · [docs/internals.md](docs/internals.md) ·
[docs/extending.md](docs/extending.md)

## License

Apache-2.0 — see [LICENSE](LICENSE).
