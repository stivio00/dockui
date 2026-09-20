use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use dui::app::{App, Popup, RowKind, View};
use dui::mock;
use dui::workers::Msg;

fn setup_app() -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel(1024);
    let mut app = App::new(tx, true, None).expect("app");
    app.handle_msg(Msg::ConnMock);
    let containers = mock::sample_containers();
    app.handle_msg(Msg::Containers(containers.clone()));
    app.handle_msg(Msg::Volumes(mock::sample_volumes()));
    for c in &containers {
        app.handle_msg(Msg::Inspected {
            id: c.id.clone(),
            details: mock::sample_details(&c.id),
        });
    }
    for tick in 0..30 {
        for c in &containers {
            if !c.is_running() {
                continue;
            }
            let phase = tick as f64 / 8.0 + (c.id.len() % 7) as f64;
            let base = if c.name.contains("db") {
                22.0
            } else if c.name.contains("api") {
                38.0
            } else {
                4.0
            };
            app.handle_msg(Msg::Stats {
                id: c.id.clone(),
                sample: dui::model::StatsSample {
                    cpu_pct: (base + phase.sin() * 14.0).max(0.2),
                    mem: 412_000_000,
                    mem_limit: 4_294_967_296,
                    mem_pct: 9.6,
                    net_rx: 12_000_000,
                    net_tx: 3_000_000,
                    blk_read: 9_000_000,
                    blk_write: 2_000_000,
                    pids: Some(12),
                },
            });
        }
    }
    app.handle_msg(Msg::Event(dui::model::DockerEvent {
        time: 1761300012,
        typ: "container".into(),
        action: "start".into(),
        actor_id: "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0".into(),
        actor_name: "webshop-web-1".into(),
        scope: "local".into(),
    }));
    app.handle_msg(Msg::Event(dui::model::DockerEvent {
        time: 1761300025,
        typ: "image".into(),
        action: "pull".into(),
        actor_id: "sha256:9c2f6b1e".into(),
        actor_name: "nginx:1.27".into(),
        scope: "local".into(),
    }));
    app
}

fn feed_logs(app: &mut App, containers: &[dui::model::Container]) {
    for i in 0..24 {
        for c in containers {
            if c.is_running() {
                app.handle_msg(Msg::LogLine {
                    name: c.name.clone(),
                    line: format!(
                        "2026-09-20T08:{:02}:{:02}.123Z GET /api/products 200 {}ms",
                        i / 2,
                        i,
                        5 + i % 20
                    ),
                });
            }
        }
    }
}

fn dump(app: &mut App, title: &str) -> anyhow::Result<()> {
    let backend = TestBackend::new(118, 34);
    let mut terminal = Terminal::new(backend)?;
    let frame = terminal.draw(|f| dui::ui::draw(f, app))?;
    println!("=== {title} ===");
    print_buffer(frame.buffer);
    Ok(())
}

fn print_buffer(buf: &Buffer) {
    let w = buf.area.width as usize;
    for (i, cell) in buf.content.iter().enumerate() {
        print!("{}", cell.symbol());
        if (i + 1) % w == 0 {
            println!();
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut app = setup_app();
    app.tree_sel = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Container(id) if id.starts_with("a1b2")))
        .unwrap();
    dump(&mut app, "TREE + DETAIL (container selected, env vars)")?;

    let mut app = setup_app();
    app.tree_sel = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Project(p) if p == "webshop"))
        .unwrap();
    dump(&mut app, "TREE + DETAIL (project selected)")?;

    let mut app = setup_app();
    app.enter_view(View::Stats);
    dump(&mut app, "STATS")?;

    let mut app = setup_app();
    let containers = mock::sample_containers();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Project(p) if p == "webshop"))
        .unwrap();
    app.tree_sel = idx;
    app.open_logs_for_row();
    feed_logs(&mut app, &containers);
    dump(&mut app, "LOGS (project aggregate)")?;

    let mut app = setup_app();
    app.view = View::Events;
    dump(&mut app, "EVENTS")?;

    let mut app = setup_app();
    app.popup = Popup::Context;
    dump(&mut app, "CONTEXT SELECTOR")?;

    let mut app = setup_app();
    app.popup = Popup::Help;
    dump(&mut app, "HELP")?;
    Ok(())
}
