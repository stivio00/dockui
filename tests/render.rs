use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use dockui::app::{App, Popup, RowKind, View};
use dockui::mock;
use dockui::workers::{LogTarget, Msg};

fn test_app() -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel(1024);
    let mut app = App::new(tx, true, None).expect("app");
    app.handle_msg(Msg::ConnMock);
    app.handle_msg(Msg::Containers(mock::sample_containers()));
    app.handle_msg(Msg::Volumes(mock::sample_volumes()));
    for c in mock::sample_containers() {
        app.handle_msg(Msg::Inspected {
            id: c.id.clone(),
            details: mock::sample_details(&c.id),
        });
    }
    app.handle_msg(Msg::Event(dockui::model::DockerEvent {
        time: 1761300012,
        typ: "container".into(),
        action: "start".into(),
        actor_id: "a1b2c3d4e5f6".into(),
        actor_name: "webshop-web-1".into(),
        scope: "local".into(),
    }));
    app
}

fn render(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let frame = terminal.draw(|f| dockui::ui::draw(f, app)).unwrap();
    buffer_lines(frame.buffer)
}

fn buffer_lines(buf: &Buffer) -> Vec<String> {
    let w = buf.area.width as usize;
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, cell) in buf.content.iter().enumerate() {
        cur.push_str(cell.symbol());
        if (i + 1) % w == 0 {
            lines.push(cur.trim_end().to_string());
            cur = String::new();
        }
    }
    lines
}

fn joined(lines: &[String]) -> String {
    lines.join("\n")
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(app: &mut App, c: char) {
    app.handle_key(key(KeyCode::Char(c)));
}

#[test]
fn tree_view_renders_sections_and_rows() {
    let mut app = test_app();
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    for expected in [
        "PROJECTS (COMPOSE)",
        "webshop",
        "observability",
        "CONTAINERS (STANDALONE)",
        "registry",
        "sandbox",
        "VOLUMES",
        "webshop_pgdata",
    ] {
        assert!(
            text.contains(expected),
            "tree view missing {expected:?}\n{text}"
        );
    }
    // header
    assert!(text.contains("ctx:mock-local"));
    assert!(text.contains("projects 2"));
    assert!(text.contains("containers 6/8"));
    assert!(text.contains("volumes 5"));
}

#[test]
fn tree_view_shows_services_and_containers() {
    let mut app = test_app();
    // projects and services are auto-expanded on first load
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("webshop-web-1"));
    assert!(text.contains("nginx:1.27"));
    assert!(text.contains("ghcr.io/webshop/api"));
    assert!(text.contains("postgres:16.2"));

    // collapsing a service hides its own containers but not sibling services'
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Service(p, s) if p == "webshop" && s == "web"))
        .unwrap();
    app.tree_sel = idx;
    app.handle_key(key(KeyCode::Enter));
    assert!(
        !app.tree_rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Container(id) if id.starts_with("a1b2")))
    );
    // ...other services keep their containers
    assert!(
        app.tree_rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Container(id) if id.starts_with("b2c3")))
    );
}

#[test]
fn detail_pane_shows_env_vars_and_ports() {
    let mut app = test_app();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Container(id) if id.starts_with("a1b2")))
        .unwrap();
    app.tree_sel = idx;
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("ENV VARS"));
    assert!(text.contains("NGINX_VERSION"));
    assert!(text.contains("0.0.0.0:8080 -> 80/tcp"));
    assert!(text.contains("webshop_site"));
    assert!(text.contains("IP"));
}

#[test]
fn detail_pane_shows_project_services_and_volumes() {
    let mut app = test_app();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Project(p) if p == "webshop"))
        .unwrap();
    app.tree_sel = idx;
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("SERVICES"));
    assert!(text.contains("PROJECT VOLUMES"));
    assert!(text.contains("webshop_pgdata"));
    assert!(text.contains("Config"));
}

#[test]
fn stats_view_renders_table_and_sparklines() {
    let mut app = test_app();
    // feed a few stats samples
    let ids: Vec<String> = app.containers.iter().map(|c| c.id.clone()).collect();
    for tick in 0..10 {
        for id in &ids {
            app.handle_msg(Msg::Stats {
                id: id.clone(),
                sample: dockui::model::StatsSample {
                    cpu_pct: 12.5,
                    mem: 100_000_000,
                    mem_limit: 4_294_967_296,
                    mem_pct: 2.3,
                    net_rx: 1000,
                    net_tx: 500,
                    blk_read: 10,
                    blk_write: 5,
                    pids: Some(9),
                },
            });
        }
        let _ = tick;
    }
    app.enter_view(View::Stats);
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("CONTAINER STATS"));
    assert!(text.contains("CPU%"));
    assert!(text.contains("MEM%"));
    assert!(text.contains("NET I/O"));
    assert!(text.contains("BLOCK I/O"));
    assert!(text.contains("12.50%"));
    assert!(text.contains("CPU 12.50%"));
    assert!(text.contains("95.4MiB"));
}

#[test]
fn logs_view_renders_prefixed_lines() {
    let mut app = test_app();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Project(p) if p == "webshop"))
        .unwrap();
    app.tree_sel = idx;
    app.open_logs_for_row();
    assert_eq!(app.view, View::Logs);
    app.handle_msg(Msg::LogLine {
        name: "webshop-web-1".into(),
        line: "2026-09-20T08:00:00.000Z GET /api/products 200".into(),
    });
    app.handle_msg(Msg::LogLine {
        name: "webshop-db-1".into(),
        line: "2026-09-20T08:00:01.000Z slow query took 212ms".into(),
    });
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("LOGS"));
    assert!(text.contains("[webshop-web-1]"));
    assert!(text.contains("GET /api/products 200"));
    assert!(text.contains("[webshop-db-1]"));
    assert!(text.contains("following"));
}

#[test]
fn events_view_renders_table() {
    let mut app = test_app();
    app.view = View::Events;
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("DOCKER EVENTS"));
    assert!(text.contains("ACTION"));
    assert!(text.contains("start"));
    assert!(text.contains("webshop-web-1"));
    assert!(text.contains("container"));
}

#[test]
fn context_popup_renders_contexts() {
    let mut app = test_app();
    app.popup = Popup::Context;
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("DOCKER CONTEXTS"));
    assert!(text.contains("mock-local"));
    assert!(text.contains("desktop-linux"));
    assert!(text.contains("t480"));
}

#[test]
fn help_popup_renders_keybindings() {
    let mut app = test_app();
    app.popup = Popup::Help;
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("HELP"));
    assert!(text.contains("docker context selector"));
}

#[test]
fn key_navigation_works() {
    let mut app = test_app();
    assert_eq!(app.tree_sel, 0);
    press(&mut app, 'j');
    assert_eq!(app.tree_sel, 1);
    press(&mut app, 'j');
    assert_eq!(app.tree_sel, 2);
    press(&mut app, 'k');
    assert_eq!(app.tree_sel, 1);
    press(&mut app, 'G');
    let last = app.tree_rows.len() - 1;
    assert_eq!(app.tree_sel, last);
    press(&mut app, 'g');
    assert_eq!(app.tree_sel, 0);

    // collapse the projects section
    press(&mut app, ' '); // toggle section
    assert!(!app.expanded.contains("sec:projects"));
    let has_webshop = app
        .tree_rows
        .iter()
        .any(|r| matches!(&r.kind, RowKind::Project(p) if p == "webshop"));
    assert!(!has_webshop, "project rows should be hidden after collapse");
    press(&mut app, ' ');
    assert!(app.expanded.contains("sec:projects"));

    // views
    press(&mut app, '2');
    assert_eq!(app.view, View::Stats);
    press(&mut app, '3');
    assert_eq!(app.view, View::Events);
    press(&mut app, '1');
    assert_eq!(app.view, View::Tree);
    press(&mut app, '4');
    assert_eq!(app.view, View::Logs);
    // esc returns to tree
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.view, View::Tree);

    // popup open/close
    press(&mut app, 'c');
    assert_eq!(app.popup, Popup::Context);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.popup, Popup::None);

    // q quits
    assert!(!press_and_check(&mut app, 'q'));
}

fn press_and_check(app: &mut App, c: char) -> bool {
    app.handle_key(key(KeyCode::Char(c)))
}

#[test]
fn logs_key_scrolling_and_follow() {
    let mut app = test_app();
    let targets: Vec<LogTarget> = app
        .containers
        .iter()
        .take(1)
        .map(|c| LogTarget {
            id: c.id.clone(),
            name: c.name.clone(),
        })
        .collect();
    app.open_logs(targets);
    for i in 0..100 {
        app.handle_msg(Msg::LogLine {
            name: "webshop-web-1".into(),
            line: format!("2026-09-20T08:00:00.000Z line {i}"),
        });
    }
    assert!(app.log_follow);
    press(&mut app, 'k');
    assert!(!app.log_follow);
    assert_eq!(app.log_off, 1);
    press(&mut app, 'j');
    assert!(app.log_follow);
    assert_eq!(app.log_off, 0);
    press(&mut app, 'G');
    press(&mut app, 'f');
    assert!(!app.log_follow);
    press(&mut app, 'f');
    assert!(app.log_follow);
}

#[test]
fn release_events_are_ignored() {
    let mut app = test_app();
    let ev = KeyEvent {
        code: KeyCode::Char('q'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Release,
        state: ratatui::crossterm::event::KeyEventState::NONE,
    };
    assert!(app.handle_key(ev), "release q must not quit");
}

#[test]
fn volume_detail_renders() {
    let mut app = test_app();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Volume(v) if v == "registry_data"))
        .unwrap();
    app.tree_sel = idx;
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("Driver"));
    assert!(text.contains("7.0GiB"));
    assert!(text.contains("RefCount"));
}
