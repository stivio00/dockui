use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph};

use super::theme;
use crate::actions::FormValue;
use crate::app::{App, Popup};
use crate::docker::EndpointKind;

pub fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let v = Layout::vertical([Constraint::Percentage(pct_y)])
        .flex(Flex::Center)
        .split(area);
    let h = Layout::horizontal([Constraint::Percentage(pct_x)])
        .flex(Flex::Center)
        .split(v[0]);
    h[0]
}

pub fn centered_rows(area: Rect, pct_x: u16, rows: u16) -> Rect {
    let rows = rows.min(area.height.saturating_sub(2));
    let v = Layout::vertical([Constraint::Length(rows)])
        .flex(Flex::Center)
        .split(area);
    let h = Layout::horizontal([Constraint::Percentage(pct_x)])
        .flex(Flex::Center)
        .split(v[0]);
    h[0]
}

fn kind_label(kind: &EndpointKind) -> &'static str {
    match kind {
        EndpointKind::Unix => "unix",
        EndpointKind::Tcp => "tcp",
        EndpointKind::Ssh => "ssh",
        EndpointKind::Unknown => "?",
    }
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    match app.popup {
        Popup::Context => draw_context(f, app, area),
        Popup::ContainerSelect => draw_container_select(f, app, area),
        Popup::Help => draw_help(f, app, area),
        Popup::Ops => draw_ops(f, app, area),
        Popup::Confirm => draw_confirm(f, app, area),
        Popup::Edit => draw_edit(f, app, area),
        Popup::Exec => draw_exec(f, app, area),
        Popup::None => {}
    }
}

fn popup_block(title: &str) -> Block<'static> {
    Block::bordered()
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::new().fg(Color::Cyan))
}

fn list_highlight() -> Style {
    Style::new()
        .bg(theme::SELECT_BG)
        .fg(theme::SELECT_FG)
        .add_modifier(Modifier::BOLD)
}

fn draw_context(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = app.contexts.len().clamp(3, 12) as u16 + 2;
    let popup = centered_rows(area, 70, rows);
    app.areas.popup = popup;
    app.areas.popup_list = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    f.render_widget(Clear, popup);

    let items: Vec<ListItem> = app
        .contexts
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let marker = if i == app.current_ctx {
                Span::styled("● ", Style::new().fg(Color::LightGreen))
            } else {
                Span::raw("  ")
            };
            let name = if i == app.current_ctx {
                Span::styled(
                    c.name.clone(),
                    Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(c.name.clone(), Style::new().fg(Color::Gray))
            };
            ListItem::new(Line::from(vec![
                marker,
                name,
                Span::raw("  "),
                Span::styled(kind_label(&c.kind), Style::new().fg(Color::Magenta)),
                Span::raw("  "),
                Span::styled(
                    crate::util::truncate(&c.endpoint, popup.width.saturating_sub(18) as usize),
                    Style::new().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    app.popup_state
        .select((!app.contexts.is_empty()).then_some(app.popup_sel));
    f.render_stateful_widget(
        List::new(items)
            .block(popup_block(
                "DOCKER CONTEXTS │ ↑/↓ select │ enter switch │ esc cancel",
            ))
            .highlight_style(list_highlight())
            .highlight_symbol("▶ "),
        popup,
        &mut app.popup_state,
    );
}

fn draw_container_select(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = app.containers.len().clamp(3, 14) as u16 + 2;
    let popup = centered_rows(area, 65, rows);
    app.areas.popup = popup;
    app.areas.popup_list = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    f.render_widget(Clear, popup);

    let items: Vec<ListItem> = app
        .containers
        .iter()
        .map(|c| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", theme::state_icon(&c.state)),
                    Style::new().fg(theme::state_color(&c.state)),
                ),
                Span::styled(c.name.clone(), Style::new().fg(Color::White)),
                Span::raw("  "),
                Span::styled(
                    crate::util::truncate(&c.image, 30),
                    Style::new().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    app.popup_state
        .select((!app.containers.is_empty()).then_some(app.popup_sel));
    f.render_stateful_widget(
        List::new(items)
            .block(popup_block(
                "FOLLOW LOGS OF │ ↑/↓ select │ enter open │ esc cancel",
            ))
            .highlight_style(list_highlight())
            .highlight_symbol("▶ "),
        popup,
        &mut app.popup_state,
    );
}

fn draw_ops(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = app.ops.len().clamp(3, 10) as u16 + 2;
    let popup = centered_rows(area, 72, rows);
    app.areas.popup = popup;
    app.areas.popup_list = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    f.render_widget(Clear, popup);

    if app.ops.is_empty() {
        let msg = match &app.ops_error {
            Some(e) => format!("ops.yml error: {e}"),
            None => format!(
                "no ops in {} — see ops.example.yml",
                crate::ops::ops_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/.dockui/ops.yml".into())
            ),
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                crate::util::truncate(&msg, popup.width.saturating_sub(4) as usize),
                Style::new().fg(Color::DarkGray),
            )))
            .block(popup_block(" OPS │ esc close ")),
            popup,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .ops
        .iter()
        .map(|(name, op)| {
            let mut spans = vec![
                Span::styled(
                    name.clone(),
                    Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
            ];
            if let Some(d) = &op.description {
                spans.push(Span::styled(
                    crate::util::truncate(d, 34),
                    Style::new().fg(Color::Gray),
                ));
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                crate::util::truncate(&op.summary(), popup.width.saturating_sub(24) as usize),
                Style::new().fg(Color::DarkGray),
            ));
            ListItem::new(Line::from(spans))
        })
        .collect();

    app.popup_state.select(Some(app.popup_sel));
    f.render_stateful_widget(
        List::new(items)
            .block(popup_block(
                " OPS (~/.dockui/ops.yml) │ ↑/↓ select │ enter run │ esc cancel",
            ))
            .highlight_style(list_highlight())
            .highlight_symbol("▶ "),
        popup,
        &mut app.popup_state,
    );
}

fn draw_confirm(f: &mut Frame, app: &mut App, area: Rect) {
    let popup = centered_rows(area, 56, 5);
    app.areas.popup = popup;
    app.areas.popup_list = Rect::default();
    f.render_widget(Clear, popup);
    let what = app
        .pending
        .as_ref()
        .map(|p| p.describe())
        .unwrap_or_else(|| "nothing".into());
    let lines = vec![
        Line::from(Span::styled(
            "Confirm",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(what, Style::new().fg(Color::White))),
        Line::from(vec![
            Span::styled(
                "[y] ",
                Style::new()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("confirm   ", Style::new().fg(Color::Gray)),
            Span::styled(
                "[n/esc] ",
                Style::new()
                    .fg(Color::LightRed)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("cancel", Style::new().fg(Color::Gray)),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).block(popup_block(" CONFIRM ")), popup);
}

fn draw_edit(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(form) = &app.edit else {
        return;
    };
    let rows = (form.fields.len() as u16) + 5; // header + hint + fields + blank + apply
    let popup = centered_rows(area, 68, rows);
    app.areas.popup = popup;
    app.areas.popup_list = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    f.render_widget(Clear, popup);

    let value_width = popup.width.saturating_sub(16) as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(form.fields.len() + 4);
    let arrow = if form.text("Name") == form.old_name {
        form.old_name.clone()
    } else {
        format!("{}  →  {}", form.old_name, form.text("Name"))
    };
    lines.push(Line::from(Span::styled(
        arrow,
        Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "type to edit · ↑/↓/tab switch · space toggle · enter next",
        Style::new().fg(Color::DarkGray),
    )));

    for (i, field) in form.fields.iter().enumerate() {
        let selected = i == form.sel;
        let marker = if selected { "▶ " } else { "  " };
        let label_style = if selected {
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::Blue)
        };
        let value = match (&field.value, selected) {
            (FormValue::Text(t), true) => {
                let v = &t.value;
                let cur = t.cursor.min(v.len());
                let shown = format!("{}▏{}", &v[..cur], &v[cur..]);
                Span::styled(
                    crate::util::truncate(&shown, value_width),
                    Style::new().fg(Color::White),
                )
            }
            (FormValue::Text(t), false) => Span::styled(
                crate::util::truncate(&t.value, value_width),
                Style::new().fg(Color::Gray),
            ),
            (FormValue::Toggle(b), _) => Span::styled(
                if *b { "on" } else { "off" }.to_string(),
                if *b {
                    Style::new().fg(Color::LightGreen)
                } else {
                    Style::new().fg(Color::DarkGray)
                },
            ),
            (FormValue::Choice(c), _) => Span::styled(
                crate::actions::GPU_OPTIONS[*c].to_string(),
                Style::new().fg(Color::LightYellow),
            ),
        };
        lines.push(Line::from(vec![
            Span::styled(marker, Style::new().fg(Color::Cyan)),
            Span::styled(format!("{:<10}", field.label), label_style),
            value,
        ]));
    }
    lines.push(Line::raw(""));
    let apply_sel = form.sel == form.fields.len();
    lines.push(Line::from(vec![
        Span::styled(
            if apply_sel { "▶ " } else { "  " },
            Style::new().fg(Color::Cyan),
        ),
        Span::styled(
            "APPLY (recreate)",
            if apply_sel {
                Style::new()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::Green)
            },
        ),
    ]));

    f.render_widget(
        Paragraph::new(lines).block(popup_block(
            " RECREATE CONTAINER │ enter on APPLY recreates │ esc cancel",
        )),
        popup,
    );
}

fn draw_exec(f: &mut Frame, app: &mut App, area: Rect) {
    let popup = centered_rows(area, 56, 5);
    app.areas.popup = popup;
    f.render_widget(Clear, popup);
    if let Some(input) = &app.exec_user_input {
        app.areas.popup_list = Rect::default();
        let v = &input.value;
        let cur = input.cursor.min(v.len());
        let shown = format!("{}▏{}", &v[..cur], &v[cur..]);
        let lines = vec![
            Line::from(Span::styled(
                "user name (empty = as configured)",
                Style::new().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(shown, Style::new().fg(Color::White))),
        ];
        f.render_widget(
            Paragraph::new(lines).block(popup_block(" EXEC AS USER │ enter go │ esc back")),
            popup,
        );
        return;
    }
    app.areas.popup_list = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let items: Vec<ListItem> = ["as configured user", "as root", "custom user…"]
        .into_iter()
        .map(|s| {
            ListItem::new(Line::from(vec![
                Span::styled("  ", Style::new().fg(Color::Cyan)),
                Span::styled(s, Style::new().fg(Color::White)),
            ]))
        })
        .collect();
    let name = app
        .selected_container()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    app.popup_state.select(Some(app.popup_sel));
    f.render_stateful_widget(
        List::new(items)
            .block(popup_block(&format!(
                " TERMINAL ON {name} │ enter open │ esc cancel"
            )))
            .highlight_style(list_highlight())
            .highlight_symbol("▶ "),
        popup,
        &mut app.popup_state,
    );
}

fn draw_help(f: &mut Frame, app: &mut App, area: Rect) {
    let popup = centered(area, 62, 78);
    app.areas.popup = popup;
    app.areas.popup_list = Rect::default();
    f.render_widget(Clear, popup);
    let key = |k: &str| {
        Span::styled(
            format!("{k:<8}"),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )
    };
    let desc = |d: &str| Span::styled(d.to_string(), Style::new().fg(Color::Gray));
    let lines = vec![
        Line::from(vec![
            key("1/2/3/4"),
            desc("tree / stats / events / logs view"),
        ]),
        Line::from(vec![
            key("j/k"),
            desc("move down / up (scroll in detail pane)"),
        ]),
        Line::from(vec![key("g/G"), desc("top / bottom")]),
        Line::from(vec![key("enter"), desc("expand node / focus detail pane")]),
        Line::from(vec![key("←/→"), desc("collapse / expand")]),
        Line::from(vec![key("tab"), desc("toggle tree <-> detail focus")]),
        Line::from(vec![key("/"), desc("regex filter (esc clears)")]),
        Line::from(vec![
            key("L"),
            desc("follow logs (container, service or project)"),
        ]),
        Line::from(vec![key("t"), desc("open terminal on running container")]),
        Line::from(vec![
            key("E"),
            desc("edit + recreate container (name/image/ports/env/gpus)"),
        ]),
        Line::from(vec![
            key("S/K/R/D"),
            desc("start / stop / restart / delete container"),
        ]),
        Line::from(vec![key("o"), desc("run ops from ~/.dockui/ops.yml")]),
        Line::from(vec![key("f"), desc("toggle follow (logs / events)")]),
        Line::from(vec![key("s"), desc("switch log container (in logs view)")]),
        Line::from(vec![key("c"), desc("docker context selector")]),
        Line::from(vec![key("r"), desc("refresh")]),
        Line::from(vec![
            key("mouse"),
            desc("click selects · wheel scrolls · click again opens"),
        ]),
        Line::from(vec![key("esc"), desc("back / clear filter / close popup")]),
        Line::from(vec![key("q"), desc("quit")]),
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::styled("compose tree: ", Style::new().fg(Color::Magenta)),
            desc("projects → services → containers"),
        ]),
    ];
    let _ = app;
    f.render_widget(
        Paragraph::new(lines).block(popup_block("HELP │ esc close")),
        popup,
    );
}
