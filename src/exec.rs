use bollard::Docker;
use bollard::container::{Config, LogOutput};
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use futures_util::StreamExt;
use ratatui::crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::pin::Pin;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedReceiver;

/// A request to open an interactive terminal on a container.
/// `user`: None = the container's configured user, Some("root"), or a custom
/// user name.
#[derive(Clone, Debug)]
pub struct ExecRequest {
    pub id: String,
    pub name: String,
    pub user: Option<String>,
}

/// Anything that needs the real terminal handed over to it. Sent over the
/// exec channel from the UI; the main loop tears the TUI down, runs the
/// session against the real terminal, then restores the TUI.
#[derive(Debug)]
pub enum TerminalRequest {
    /// `docker exec -it` into a running container
    Exec(ExecRequest),
    /// `docker run -it`: create, attach, start
    AttachRun {
        name: String,
        config: Box<Config<String>>,
    },
}

/// Preferred shell bootstrap: use bash when the image has it, else sh.
const SHELL_CMD: &str = "command -v bash >/dev/null 2>&1 && exec bash || exec sh";

type LogStream =
    Pin<Box<dyn futures_util::Stream<Item = Result<LogOutput, bollard::errors::Error>> + Send>>;
type ByteWriter = Pin<Box<dyn AsyncWrite + Send>>;

/// Run an interactive session against the current terminal.
///
/// The caller must have already left the TUI (alternate screen off, mouse
/// capture off). Terminal input is NOT read directly: the app's crossterm
/// reader thread stays the single stdin owner and forwards key events over
/// `input`; they are re-encoded to terminal byte sequences here. This keeps
/// exactly one reader on stdin at all times.
pub async fn run_session(
    docker: &Docker,
    req: TerminalRequest,
    input: &mut UnboundedReceiver<CEvent>,
) -> anyhow::Result<()> {
    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    enable_raw_mode()?;
    let res = match req {
        TerminalRequest::Exec(r) => run_exec(docker, r, input).await,
        TerminalRequest::AttachRun { name, config } => {
            run_attach(docker, name, config, input).await
        }
    };
    let _ = disable_raw_mode();
    println!();
    res
}

/// `docker exec -it` flow: create the exec instance, attach, pump.
async fn run_exec(
    docker: &Docker,
    req: ExecRequest,
    input: &mut UnboundedReceiver<CEvent>,
) -> anyhow::Result<()> {
    let exec = docker
        .create_exec(
            &req.id,
            CreateExecOptions::<String> {
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(true),
                cmd: Some(vec!["sh".into(), "-c".into(), SHELL_CMD.into()]),
                user: req.user.clone(),
                ..Default::default()
            },
        )
        .await?;
    let result = docker
        .start_exec(
            &exec.id,
            Some(StartExecOptions {
                detach: false,
                tty: true,
                ..Default::default()
            }),
        )
        .await?;
    let StartExecResults::Attached {
        output,
        input: stream_in,
    } = result
    else {
        anyhow::bail!("exec detached immediately");
    };
    attached_session(output, stream_in, input).await
}

/// `docker run -it` flow: create, attach (hijacked connection), start, pump.
/// On error after the container exists, the leftover container is removed.
async fn run_attach(
    docker: &Docker,
    name: String,
    config: Box<Config<String>>,
    input: &mut UnboundedReceiver<CEvent>,
) -> anyhow::Result<()> {
    use bollard::container::{
        AttachContainerOptions, CreateContainerOptions, RemoveContainerOptions,
    };
    let created = docker
        .create_container(
            Some(CreateContainerOptions {
                name: name.clone(),
                platform: None,
            }),
            *config,
        )
        .await?;
    let attached = docker
        .attach_container(
            &created.id,
            Some(AttachContainerOptions::<String> {
                stdin: Some(true),
                stdout: Some(true),
                stderr: Some(true),
                stream: Some(true),
                logs: Some(false),
                detach_keys: None,
            }),
        )
        .await?;
    if let Err(e) = docker
        .start_container(
            &created.id,
            None::<bollard::container::StartContainerOptions<String>>,
        )
        .await
    {
        let _ = docker
            .remove_container(
                &created.id,
                Some(RemoveContainerOptions {
                    v: false,
                    force: true,
                    link: false,
                }),
            )
            .await;
        return Err(e.into());
    }
    attached_session(attached.output, attached.input, input).await
}

/// Pump a hijacked docker stream to/from the real terminal until either the
/// remote side ends or the input channel closes.
async fn attached_session(
    output: LogStream,
    stream_in: ByteWriter,
    input: &mut UnboundedReceiver<CEvent>,
) -> anyhow::Result<()> {
    let mut stream_in = stream_in;
    let mut pump = tokio::spawn(async move {
        let mut output = output;
        let mut out = tokio::io::stdout();
        while let Some(item) = output.next().await {
            match item {
                Ok(LogOutput::StdOut { message }) | Ok(LogOutput::StdErr { message }) => {
                    let _ = out.write_all(&message).await;
                    let _ = out.flush().await;
                }
                Ok(_) => {}
                Err(e) => {
                    // the stream ends with an error once the session is
                    // done; only surface real failures
                    let msg = e.to_string();
                    if !msg.contains("No such container") && !msg.contains("No such exec instance")
                    {
                        eprintln!("\r\ndui: session ended: {msg}");
                    }
                    break;
                }
            }
        }
    });

    loop {
        tokio::select! {
            done = &mut pump => {
                let _ = done;
                break;
            }
            ev = input.recv() => {
                match ev {
                    Some(CEvent::Key(key)) => {
                        let bytes = encode_key(key);
                        if !bytes.is_empty() {
                            stream_in.write_all(&bytes).await?;
                            stream_in.flush().await?;
                        }
                    }
                    Some(_) => {}
                    None => {
                        pump.abort();
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Re-encode a crossterm key event into the raw byte sequence a terminal
/// application expects to receive (xterm-style). Ctrl+keys map to their
/// control bytes (Ctrl-C -> 0x03), Alt+ keys get an ESC prefix.
pub fn encode_key(key: KeyEvent) -> Vec<u8> {
    let KeyEvent {
        code,
        modifiers,
        kind,
        ..
    } = key;
    if !matches!(kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return Vec::new();
    }
    let mut out = Vec::new();
    if modifiers.contains(KeyModifiers::ALT) {
        out.push(0x1b);
    }
    match code {
        KeyCode::Char(c) => {
            if modifiers.contains(KeyModifiers::CONTROL) {
                out.clear();
                let c = c.to_ascii_lowercase();
                if c.is_ascii_lowercase() {
                    out.push(c as u8 - 0x60);
                } else if c == ' ' || c == '@' {
                    out.push(0);
                }
                return out;
            }
            let mut b = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
        }
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => out.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => out.extend_from_slice(b"\x1b[B"),
        KeyCode::Right => out.extend_from_slice(b"\x1b[C"),
        KeyCode::Left => out.extend_from_slice(b"\x1b[D"),
        KeyCode::Home => out.extend_from_slice(b"\x1b[H"),
        KeyCode::End => out.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => out.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => out.extend_from_slice(b"\x1b[6~"),
        KeyCode::Insert => out.extend_from_slice(b"\x1b[2~"),
        KeyCode::Delete => out.extend_from_slice(b"\x1b[3~"),
        KeyCode::F(n) => {
            let seq = match n {
                1 => "\x1bOP",
                2 => "\x1bOQ",
                3 => "\x1bOR",
                4 => "\x1bOS",
                5 => "\x1b[15~",
                6 => "\x1b[17~",
                7 => "\x1b[18~",
                8 => "\x1b[19~",
                9 => "\x1b[20~",
                10 => "\x1b[21~",
                11 => "\x1b[23~",
                12 => "\x1b[24~",
                _ => "",
            };
            out.extend_from_slice(seq.as_bytes());
        }
        _ => return Vec::new(),
    }
    out
}
