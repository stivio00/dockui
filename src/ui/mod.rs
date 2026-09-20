pub mod detail;
pub mod events;
pub mod logs;
pub mod popup;
pub mod stats;
pub mod theme;
pub mod tree;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, ConnState, Focus, View};

fn header_line(app: &App) -> Line<'static> {
    let ctx = app.current_context();
    let status_span = match &app.conn {
        ConnState::Connected => Span::styled(" ● connected ", Style::new().fg(Color::LightGreen)),
        ConnState::Mock => Span::styled(" ● mock ", Style::new().fg(Color::Magenta)),
        ConnState::Connecting => {
            Span::styled(" ◌ connecting ", Style::new().fg(Color::LightYellow))
        }
        ConnState::Error(_) => Span::styled(" ✖ disconnected ", Style::new().fg(Color::LightRed)),
    };
    let running = app.containers.iter().filter(|c| c.is_running()).count();
    let projects = app
        .containers
        .iter()
        .filter_map(|c| c.compose_project())
        .collect::<std::collections::HashSet<&str>>()
        .len();

    let sep = Span::styled(" │ ", Style::new().fg(Color::DarkGray));
    Line::from(vec![
        Span::styled(
            " dockui ",
            Style::new()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("ctx:", Style::new().fg(Color::DarkGray)),
        Span::styled(
            ctx.name.clone(),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        status_span,
        sep.clone(),
        Span::styled("projects ", Style::new().fg(Color::DarkGray)),
        Span::styled(projects.to_string(), Style::new().fg(Color::White)),
        sep.clone(),
        Span::styled("containers ", Style::new().fg(Color::DarkGray)),
        Span::styled(
            format!("{}/{}", running, app.containers.len()),
            Style::new().fg(Color::White),
        ),
        sep.clone(),
        Span::styled("volumes ", Style::new().fg(Color::DarkGray)),
        Span::styled(app.volumes.len().to_string(), Style::new().fg(Color::White)),
        sep.clone(),
        Span::styled("view ", Style::new().fg(Color::DarkGray)),
        Span::styled(app.view.title().to_string(), Style::new().fg(Color::Yellow)),
    ])
}

fn footer_line(app: &App) -> Line<'static> {
    let hints: &[(&str, &str)] = match app.view {
        View::Tree => &[
            ("j/k", "nav"),
            ("enter", "open"),
            ("tab", "pane"),
            ("L", "logs"),
            ("t", "sh"),
            ("E", "edit"),
            ("S/K/R/D", "ctl"),
            ("/", "filter"),
            ("o", "ops"),
            ("?", "help"),
            ("q", "quit"),
        ],
        View::Stats => &[
            ("j/k", "select"),
            ("L", "logs"),
            ("/", "filter"),
            ("1", "tree"),
            ("c", "context"),
            ("q", "quit"),
        ],
        View::Events => &[
            ("j/k", "scroll"),
            ("f", "live"),
            ("/", "filter"),
            ("1", "tree"),
            ("c", "context"),
            ("q", "quit"),
        ],
        View::Logs => &[
            ("j/k", "scroll"),
            ("PgUp/Dn", "page"),
            ("f", "follow"),
            ("s", "switch"),
            ("/", "filter"),
            ("esc", "back"),
            ("q", "quit"),
        ],
    };
    let mut spans = Vec::new();
    for (i, (k, d)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!("[{k}]"),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {d}"),
            Style::new().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

fn search_prompt_line(app: &App) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "/",
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )];
    let v = &app.search.input.value;
    let cur = app.search.input.cursor.min(v.len());
    let shown: String = format!("{}▏{}", &v[..cur], &v[cur..]);
    spans.push(Span::styled(shown, Style::new().fg(Color::White)));
    if let Some(err) = &app.search.error {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("invalid regex: {err}"),
            Style::new().fg(Color::LightRed),
        ));
    } else {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "enter apply · esc cancel · ctrl-u clear",
            Style::new().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

fn toast_line(app: &App) -> Line<'static> {
    let t = app.toast.as_ref().expect("toast checked by caller");
    let (icon, color) = if t.ok {
        (" ✓ ", Color::LightGreen)
    } else {
        (" ✖ ", Color::LightRed)
    };
    Line::from(vec![
        Span::styled(icon, Style::new().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(t.text.clone(), Style::new().fg(color)),
    ])
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    let header_style = Style::new().bg(theme::HEADER_BG);
    f.render_widget(
        Paragraph::new(header_line(app)).style(header_style),
        chunks[0],
    );

    // body
    match app.view {
        View::Tree => {
            let body = Layout::horizontal([
                Constraint::Percentage(app.split_pct),
                Constraint::Percentage(100 - app.split_pct),
            ])
            .split(chunks[1]);
            tree::draw(f, app, body[0]);
            app.areas.detail = body[1];
            let lines = detail::build_detail_lines(app);
            let title = if app.focus == Focus::Detail {
                " DETAILS (focused) "
            } else {
                " DETAILS "
            };
            let block = Block::bordered()
                .title(Span::styled(
                    title,
                    Style::new()
                        .fg(if app.focus == Focus::Detail {
                            Color::Cyan
                        } else {
                            Color::DarkGray
                        })
                        .add_modifier(Modifier::BOLD),
                ))
                .border_style(if app.focus == Focus::Detail {
                    Style::new().fg(Color::Cyan)
                } else {
                    Style::new().fg(Color::DarkGray)
                });
            let mut p = Paragraph::new(lines)
                .block(block)
                .style(Style::new().fg(Color::Reset));
            if app.detail_scroll > 0 {
                p = p.scroll((app.detail_scroll, 0));
            }
            f.render_widget(p, body[1]);
        }
        View::Stats => stats::draw(f, app, chunks[1]),
        View::Logs => logs::draw(f, app, chunks[1]),
        View::Events => events::draw(f, app, chunks[1]),
    }

    // footer / search prompt
    if app.search.prompting {
        f.render_widget(
            Paragraph::new(search_prompt_line(app)).style(Style::new().bg(theme::HEADER_BG)),
            chunks[2],
        );
    } else {
        f.render_widget(
            Paragraph::new(footer_line(app)).style(Style::new().bg(theme::HEADER_BG)),
            chunks[2],
        );
    }

    // expire toasts
    if app.toast_expired() {
        app.toast = None;
    }

    // toast / error banner (line above the footer)
    if app.toast.is_some() || matches!(app.conn, ConnState::Error(_)) {
        let line = if let Some(_t) = &app.toast {
            toast_line(app)
        } else {
            let err = match &app.conn {
                ConnState::Error(e) => e.clone(),
                _ => String::new(),
            };
            Line::from(vec![
                Span::styled(" ✖ ", Style::new().fg(Color::LightRed)),
                Span::styled(
                    crate::util::truncate(&err, (area.width as usize).saturating_sub(6)),
                    Style::new().fg(Color::LightRed),
                ),
                Span::styled(
                    "  press [c] to switch context",
                    Style::new().fg(Color::DarkGray),
                ),
            ])
        };
        let banner_area = Rect {
            x: 0,
            y: area.height.saturating_sub(2),
            width: area.width,
            height: 1,
        };
        let bg = if app.toast.as_ref().is_some_and(|t| t.ok) {
            Color::Rgb(20, 48, 32)
        } else {
            Color::Rgb(60, 20, 24)
        };
        f.render_widget(Paragraph::new(line).style(Style::new().bg(bg)), banner_area);
    }

    if app.popup != crate::app::Popup::None {
        popup::draw(f, app, chunks[1]);
    }
}
