use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use super::theme;
use crate::app::{App, Focus, RowKind};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    app.areas.tree = area;
    let rows = &app.tree_rows;
    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| {
            let mut spans: Vec<Span> = Vec::new();
            let indent = "  ".repeat(r.depth);
            let marker = if r.has_children {
                if r.expanded { "▾ " } else { "▸ " }
            } else {
                "  "
            };
            match &r.kind {
                RowKind::Section(title) => {
                    spans.push(Span::styled(
                        format!("{indent}⟐ {title}"),
                        Style::new()
                            .fg(ratatui::style::Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(
                        format!("  {}", r.sub),
                        Style::new().fg(ratatui::style::Color::DarkGray),
                    ));
                }
                RowKind::Project(_) | RowKind::Service(_, _) => {
                    let color = theme::state_color(r.state.as_deref().unwrap_or(""));
                    spans.push(Span::styled(
                        format!("{indent}{marker}◆ "),
                        Style::new().fg(ratatui::style::Color::Cyan),
                    ));
                    spans.push(Span::styled(
                        r.label.clone(),
                        Style::new()
                            .fg(ratatui::style::Color::White)
                            .add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(format!("  {}", r.sub), Style::new().fg(color)));
                }
                RowKind::Container(_) => {
                    let state = r.state.as_deref().unwrap_or("");
                    let color = theme::state_color(state);
                    spans.push(Span::styled(
                        format!("{indent}{marker}{} ", theme::state_icon(state)),
                        Style::new().fg(color),
                    ));
                    spans.push(Span::styled(
                        r.label.clone(),
                        Style::new().fg(ratatui::style::Color::White),
                    ));
                    spans.push(Span::styled(
                        format!("  {}", crate::util::truncate(&r.sub, 44)),
                        Style::new().fg(ratatui::style::Color::DarkGray),
                    ));
                }
                RowKind::Volume(_) => {
                    spans.push(Span::styled(
                        format!("{indent}{marker}▤ "),
                        Style::new().fg(ratatui::style::Color::Cyan),
                    ));
                    spans.push(Span::styled(
                        r.label.clone(),
                        Style::new().fg(ratatui::style::Color::White),
                    ));
                    spans.push(Span::styled(
                        format!("  {}", r.sub),
                        Style::new().fg(ratatui::style::Color::DarkGray),
                    ));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let filter = match &app.search.active {
        Some(re) => format!(" │ filter: {}", re.as_str()),
        None => String::new(),
    };
    let title = format!(
        " TREE │ {} containers · {} volumes{filter} ",
        app.containers.len(),
        app.volumes.len()
    );
    let block = ratatui::widgets::Block::bordered()
        .title(Span::styled(
            title,
            Style::new()
                .fg(ratatui::style::Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(if app.focus == Focus::Tree {
            Style::new().fg(ratatui::style::Color::Cyan)
        } else {
            Style::new().fg(ratatui::style::Color::DarkGray)
        });

    app.tree_state
        .select((!rows.is_empty()).then_some(app.tree_sel));
    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(
                Style::new()
                    .bg(theme::SELECT_BG)
                    .fg(theme::SELECT_FG)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ "),
        area,
        &mut app.tree_state,
    );

    if app.tree_rows.is_empty() {
        let msg = if app.search_active() {
            "no matches (esc clears the filter)"
        } else {
            match &app.conn {
                crate::app::ConnState::Connecting => "connecting to docker…",
                crate::app::ConnState::Mock => "loading mock data…",
                _ => "no containers",
            }
        };
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 2,
            width: area.width.saturating_sub(4),
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                msg,
                Style::new().fg(ratatui::style::Color::DarkGray),
            )),
            inner,
        );
    }
}
