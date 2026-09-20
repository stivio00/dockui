use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph};

use super::theme;
use crate::app::App;
use crate::util::human_bytes;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(fs) = app.files.as_ref() else {
        return;
    };
    app.areas.files = area;

    let title = format!(" FILES: {} ", crate::util::truncate(&fs.name, 40));
    let block = Block::bordered()
        .title(Span::styled(
            title,
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::new().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);

    let mut status = vec![Span::styled(
        fs.path.clone(),
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )];
    if fs.loading {
        status.push(Span::styled("  …", Style::new().fg(Color::DarkGray)));
    } else if let Some(err) = &fs.error {
        status.push(Span::styled(
            format!(
                "  ✖ {}",
                crate::util::truncate(err, (area.width as usize).saturating_sub(6))
            ),
            Style::new().fg(Color::LightRed),
        ));
    } else {
        status.push(Span::styled(
            format!("  {} items", fs.entries.len()),
            Style::new().fg(Color::DarkGray),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(status)), chunks[0]);

    let (items, count) = {
        let visible = app.visible_files();
        let items: Vec<ListItem> = visible
            .iter()
            .map(|e| {
                if e.dir {
                    ListItem::new(Line::from(vec![
                        Span::styled("▸ ", Style::new().fg(Color::Cyan)),
                        Span::styled(
                            format!("{}/", e.name),
                            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                        ),
                    ]))
                } else {
                    ListItem::new(Line::from(vec![
                        Span::styled("  ", Style::new().fg(Color::DarkGray)),
                        Span::styled(e.name.clone(), Style::new().fg(Color::White)),
                        Span::styled(
                            format!("  {}", human_bytes(e.size)),
                            Style::new().fg(Color::DarkGray),
                        ),
                    ]))
                }
            })
            .collect();
        (items, visible.len())
    };

    let sel = fs.sel;
    if let Some(st) = app.files.as_mut() {
        st.state.select((count > 0).then(|| sel.min(count - 1)));
        f.render_stateful_widget(
            List::new(items)
                .highlight_style(
                    Style::new()
                        .bg(theme::SELECT_BG)
                        .fg(theme::SELECT_FG)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("▶ "),
            chunks[1],
            &mut st.state,
        );
    }
}
