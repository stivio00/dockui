use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Cell, Row as TRow, Table};

use crate::app::App;
use crate::model::fmt_hms;

fn type_color(typ: &str) -> Color {
    match typ {
        "container" => Color::LightBlue,
        "image" => Color::LightMagenta,
        "volume" => Color::Cyan,
        "network" => Color::LightYellow,
        "daemon" => Color::LightGreen,
        _ => Color::Gray,
    }
}

fn action_color(action: &str) -> Color {
    if action.starts_with("die")
        || action.starts_with("kill")
        || action.starts_with("destroy")
        || action.starts_with("remove")
        || action.starts_with("oom")
        || action.starts_with("unhealthy")
    {
        Color::LightRed
    } else if action.starts_with("start")
        || action.starts_with("create")
        || action.starts_with("attach")
        || action.starts_with("healthy")
    {
        Color::LightGreen
    } else {
        Color::Gray
    }
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    app.areas.events = area;
    let events = app.visible_events();
    let len = events.len();
    let header = TRow::new(["TIME", "TYPE", "ACTION", "ACTOR", "ID", "SCOPE"].map(|h| {
        Cell::from(Span::styled(
            h,
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
    }));

    let height = area.height.saturating_sub(2) as usize;
    let visible = height.min(len);
    let end = len.saturating_sub(app.events_off);
    let start = end.saturating_sub(visible);

    let rows: Vec<TRow> = events
        .iter()
        .skip(start)
        .take(visible)
        .map(|ev| {
            TRow::new(vec![
                Cell::from(Span::styled(
                    fmt_hms(ev.time),
                    Style::new().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(
                    ev.typ.clone(),
                    Style::new()
                        .fg(type_color(&ev.typ))
                        .add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    ev.action.clone(),
                    Style::new().fg(action_color(&ev.action)),
                )),
                Cell::from(Span::styled(
                    crate::util::truncate(&ev.actor_name, 32),
                    Style::new().fg(Color::White),
                )),
                Cell::from(Span::styled(
                    ev.actor_id.chars().take(12).collect::<String>(),
                    Style::new().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(
                    ev.scope.clone(),
                    Style::new().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();

    let follow = if app.events_follow { " ● live" } else { "" };
    let filter = match &app.search.active {
        Some(re) => format!(" │ filter: {}", re.as_str()),
        None => String::new(),
    };
    let title = format!(" DOCKER EVENTS │ {len}{filter}{follow} ");
    let widths = [
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(24),
        Constraint::Min(20),
        Constraint::Length(13),
        Constraint::Length(7),
    ];
    f.render_widget(
        Table::new(rows, widths).header(header).block(
            Block::bordered()
                .title(Span::styled(
                    title,
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::new().fg(Color::Cyan)),
        ),
        area,
    );
}
