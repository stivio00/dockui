use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::theme;
use crate::app::App;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    app.areas.logs = area;
    let target = match app.log_targets.len() {
        0 => "—".to_string(),
        1 => app.log_targets[0].name.clone(),
        n => format!("{n} containers"),
    };
    let filter = match &app.search.active {
        Some(re) => format!(" │ filter: {}", re.as_str()),
        None => String::new(),
    };
    let lines_all = app.visible_log_lines();
    let follow = if app.log_follow { " ● following" } else { "" };
    let title = format!(
        " LOGS │ {target} │ {}/{} lines{filter}{follow} ",
        lines_all.len(),
        app.log_lines.len()
    );

    let block = Block::bordered()
        .title(Span::styled(
            title,
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::new().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let height = inner.height as usize;
    let len = lines_all.len();
    let visible = height.min(len);
    let end = len.saturating_sub(app.log_off);
    let start = end.saturating_sub(visible);
    let slice: Vec<Line> = lines_all
        .iter()
        .skip(start)
        .take(visible)
        .map(|l| {
            // dim the timestamp prefix, keep the message bright
            let (ts, msg) = match l.find(' ') {
                Some(i) if l.len() > 20 && l.as_bytes()[4] == b'-' => (&l[..i], &l[i + 1..]),
                _ => ("", l.as_str()),
            };
            if ts.is_empty() {
                Line::from(Span::styled((*l).clone(), Style::new().fg(Color::Gray)))
            } else {
                Line::from(vec![
                    Span::styled(format!("{ts} "), Style::new().fg(Color::DarkGray)),
                    Span::styled(msg.to_string(), Style::new().fg(Color::Gray)),
                ])
            }
        })
        .collect();
    f.render_widget(Paragraph::new(slice), inner);

    if len > height {
        let mut sb = ScrollbarState::new(len.saturating_sub(height))
            .position(start)
            .viewport_content_length(height);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_style(Style::new().fg(theme::ACCENT)),
            area,
            &mut sb,
        );
    }
}
