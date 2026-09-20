use std::time::Duration;

use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event as CEvent};
use ratatui::crossterm::execute;
use tokio::sync::mpsc;

use dui::app::App;
use dui::exec::{TerminalRequest, run_session};
use dui::ui;
use dui::workers::Msg;

fn parse_args() -> (bool, Option<u64>) {
    let args: Vec<String> = std::env::args().collect();
    let mock = args.iter().any(|a| a == "--mock" || a == "--demo");
    let exit_after = args
        .iter()
        .position(|a| a == "--exit-after")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<u64>().ok());
    (mock, exit_after)
}

/// The app's single stdin reader. It stays alive for the whole process —
/// also during interactive exec sessions, which consume the same event
/// channel and forward re-encoded keys to the container.
fn spawn_input_thread(tx: mpsc::UnboundedSender<CEvent>) {
    std::thread::spawn(move || {
        loop {
            match ratatui::crossterm::event::read() {
                Ok(ev) => {
                    if tx.send(ev).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (mock, exit_after) = parse_args();

    let (msg_tx, msg_rx) = mpsc::channel::<Msg>(1024);
    let (input_tx, input_rx) = mpsc::unbounded_channel::<CEvent>();
    let (exec_tx, exec_rx) = mpsc::unbounded_channel::<TerminalRequest>();
    spawn_input_thread(input_tx);

    let mut rt = Runtime {
        msg_rx,
        input_rx,
        exec_rx,
        msg_tx,
        exec_tx: Some(exec_tx),
    };

    let terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = run(terminal, &mut rt, mock, exit_after).await;
    execute!(std::io::stdout(), DisableMouseCapture)?;
    ratatui::restore();

    if let Err(e) = result {
        eprintln!("dui: {e:#}");
        std::process::exit(1);
    }
    // force exit so the crossterm reader thread cannot block shutdown
    std::process::exit(0);
}

/// All channels shared between the event loop, the app and the workers.
struct Runtime {
    msg_rx: mpsc::Receiver<Msg>,
    input_rx: mpsc::UnboundedReceiver<CEvent>,
    exec_rx: mpsc::UnboundedReceiver<TerminalRequest>,
    msg_tx: mpsc::Sender<Msg>,
    exec_tx: Option<mpsc::UnboundedSender<TerminalRequest>>,
}

/// Hand the terminal to an interactive session: leave the TUI, run the
/// session attached to the real terminal, then restore the TUI. Never
/// fails the whole app — errors are reported as a toast by the caller.
async fn exec_session(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    req: TerminalRequest,
    input_rx: &mut mpsc::UnboundedReceiver<CEvent>,
) -> anyhow::Result<()> {
    use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};

    execute!(std::io::stdout(), DisableMouseCapture)?;
    ratatui::restore();

    let res = match app.docker.as_ref() {
        Some(docker) => run_session(docker, req, input_rx).await,
        None => Ok(()),
    };

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    terminal.clear()?;
    res
}

async fn run(
    mut terminal: ratatui::DefaultTerminal,
    rt: &mut Runtime,
    mock: bool,
    exit_after: Option<u64>,
) -> anyhow::Result<()> {
    let Runtime {
        msg_rx,
        input_rx,
        exec_rx,
        msg_tx,
        exec_tx,
    } = rt;

    let mut app = App::new(msg_tx.clone(), mock, exec_tx.clone())?;
    app.bootstrap();

    let exit_timer = exit_after.map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));

    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        // tokio::select! evaluates every branch expression, even disabled
        // ones, so the timeout future must be built up-front instead of
        // unwrapping `exit_timer` inside a conditional branch.
        let wakeup = match exit_timer {
            Some(deadline) => tokio::time::sleep_until(deadline),
            None => tokio::time::sleep(Duration::from_millis(250)),
        };

        tokio::select! {
            maybe_msg = msg_rx.recv() => {
                match maybe_msg {
                    Some(msg) => app.handle_msg(msg),
                    None => break,
                }
            }
            maybe_exec = exec_rx.recv() => {
                if let Some(req) = maybe_exec {
                    if let Err(e) = exec_session(&mut terminal, &mut app, req, input_rx).await {
                        app.toast(false, format!("session failed: {e:#}"));
                    }
                } else {
                    continue;
                }
            }
            maybe_ev = input_rx.recv() => {
                match maybe_ev {
                    Some(CEvent::Key(key)) => {
                        if !app.handle_key(key) {
                            break;
                        }
                    }
                    Some(CEvent::Mouse(me)) => app.handle_mouse(me),
                    Some(CEvent::Resize(_, _)) | Some(_) => {}
                    None => break,
                }
            }
            _ = wakeup => {
                if exit_timer.is_some() {
                    break;
                }
            }
        }
    }

    app.workers.abort_all();
    app.stats_workers.abort_all();
    app.log_workers.abort_all();
    Ok(())
}
