use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

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

fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
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

#[test]
fn env_editor_opens_adds_and_commits() {
    let mut app = test_app();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Container(id) if id.starts_with("a1b2")))
        .unwrap();
    app.tree_sel = idx;
    press(&mut app, 'E');
    assert_eq!(app.popup, Popup::Edit);
    // Name -> Image -> Command -> Env
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter));
    assert!(app.env_editor.is_some(), "enter on Env opens the table");

    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("EDIT ENV"), "editor popup missing\n{text}");
    assert!(text.contains("NGINX_VERSION"));
    assert!(text.contains("1.27.0"));

    // add a row, type key, tab to value, type value, leave cell, close
    press(&mut app, 'a');
    for c in "FOO".chars() {
        press(&mut app, c);
    }
    app.handle_key(key(KeyCode::Tab));
    for c in "bar".chars() {
        press(&mut app, c);
    }
    app.handle_key(key(KeyCode::Esc)); // leave cell
    app.handle_key(key(KeyCode::Esc)); // close editor
    assert!(app.env_editor.is_none());
    let env = app.edit.as_ref().unwrap().text("Env").to_string();
    assert!(env.contains("FOO=bar"), "new row missing in {env}");
    assert!(
        env.contains("NGINX_VERSION=1.27.0"),
        "old rows lost in {env}"
    );
    // editor stays over the form, which is still open
    assert_eq!(app.popup, Popup::Edit);
}

#[test]
fn env_editor_click_opens_and_edits_cell() {
    let mut app = test_app();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Container(id) if id.starts_with("a1b2")))
        .unwrap();
    app.tree_sel = idx;
    press(&mut app, 'E');
    let _ = render(&mut app, 130, 42);
    // click the Env row of the form: header(1) + hint(1) + field index 3
    let a = app.areas.popup_list;
    let row = a.y + 2 + 3;
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), a.x + 4, row));
    assert!(app.env_editor.is_some(), "click on Env row opens table");

    render(&mut app, 130, 42);
    let a = app.areas.popup_list;
    // click the VALUE cell of the first row
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        app.areas.env_split_x + 5,
        a.y,
    ));
    assert_eq!(
        app.env_editor.as_ref().unwrap().editing,
        Some(dockui::actions::EnvCol::Value)
    );
}

#[test]
fn split_resize_keys_and_drag_move_divider() {
    let mut app = test_app();
    let lines = render(&mut app, 130, 42);
    assert!(lines.iter().any(|l| l.contains("webshop-web-1")));
    let default_w = app.areas.tree.width;

    press(&mut app, '>');
    let _ = render(&mut app, 130, 42);
    let grown_w = app.areas.tree.width;
    assert!(grown_w > default_w, "{grown_w} vs {default_w}");

    press(&mut app, '<');
    press(&mut app, '<');
    let _ = render(&mut app, 130, 42);
    let shrunk_w = app.areas.tree.width;
    assert!(shrunk_w < default_w, "{shrunk_w} vs {default_w}");

    // drag the divider: mousedown on it, drag left, release
    let t = app.areas.tree;
    let div = t.x + t.width;
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), div, t.y + 5));
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        t.x + 35,
        t.y + 5,
    ));
    app.handle_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        t.x + 35,
        t.y + 5,
    ));
    let _ = render(&mut app, 130, 42);
    let dragged_w = app.areas.tree.width;
    assert!(dragged_w < default_w, "drag did not shrink tree pane");
}

#[test]
fn files_view_opens_navigates_and_returns() {
    let mut app = test_app();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Container(id) if id.starts_with("a1b2")))
        .unwrap();
    app.tree_sel = idx;
    press(&mut app, 'f');
    assert_eq!(app.view, View::Files);
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("FILES: webshop-web-1"));
    assert!(text.contains("bin/"));
    assert!(text.contains("etc/"));
    assert!(text.contains(".dockerenv"));
    assert!(text.contains("6 items"));

    press(&mut app, 'j');
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.files.as_ref().unwrap().path, "/etc");
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("hostname"));
    assert!(text.contains("resolv.conf"));
    assert!(text.contains("ssl/"));

    app.handle_key(key(KeyCode::Backspace));
    assert_eq!(app.files.as_ref().unwrap().path, "/");
    press(&mut app, 'r');
    assert_eq!(app.files.as_ref().unwrap().entries.len(), 6);

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.view, View::Tree);
}

#[test]
fn files_view_volume_row_toast() {
    let mut app = test_app();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Volume(v) if v == "registry_data"))
        .unwrap();
    app.tree_sel = idx;
    press(&mut app, 'f');
    assert_eq!(app.view, View::Tree);
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("only containers can be browsed"));
}

#[test]
fn files_view_mouse_selects_and_opens() {
    let mut app = test_app();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Container(id) if id.starts_with("a1b2")))
        .unwrap();
    app.tree_sel = idx;
    press(&mut app, 'f');
    let _ = render(&mut app, 130, 42);
    let a = app.areas.files;
    assert!(a.width > 0);

    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        a.x + 4,
        a.y + 4,
    ));
    assert_eq!(app.files.as_ref().unwrap().sel, 2);

    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        a.x + 4,
        a.y + 4,
    ));
    assert_eq!(app.files.as_ref().unwrap().path, "/home");
}

#[test]
fn files_view_empty_listing_does_not_panic() {
    let mut app = test_app();
    let idx = app
        .tree_rows
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Container(id) if id.starts_with("a1b2")))
        .unwrap();
    app.tree_sel = idx;
    press(&mut app, 'f');
    app.files.as_mut().unwrap().entries.clear();
    let lines = render(&mut app, 130, 42);
    let text = joined(&lines);
    assert!(text.contains("FILES: webshop-web-1"));
}
