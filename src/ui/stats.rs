use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row as TRow, Sparkline, Table};

use super::theme;
use crate::app::App;
use crate::model::{fmt_percent, human_bytes};

fn cpu_color(pct: f64) -> Color {
    if pct >= 200.0 {
        Color::LightRed
    } else if pct >= 100.0 {
        Color::LightYellow
    } else {
        Color::LightGreen
    }
}

fn mem_color(pct: f64) -> Color {
    if pct >= 80.0 {
        Color::LightRed
    } else if pct >= 50.0 {
        Color::LightYellow
    } else {
        Color::LightGreen
    }
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let rows_area =
        Layout::vertical([Constraint::Percentage(58), Constraint::Percentage(42)]).split(area);
    app.areas.stats_table = area;
    app.areas.stats_charts = rows_area[1];

    // build all display data while borrowing app, then render with the
    // persistent TableState afterwards
    let (table_rows, sel_data, running_len) = {
        let running = app.stats_rows();
        let table_rows: Vec<TRow> = running
            .iter()
            .map(|c| {
                let s = app.stats.get(&c.id);
                let cells: Vec<Cell> = vec![
                    Cell::from(Line::from(vec![
                        Span::styled(
                            format!("{} ", theme::state_icon(&c.state)),
                            Style::new().fg(theme::state_color(&c.state)),
                        ),
                        Span::styled(c.name.clone(), Style::new().fg(Color::White)),
                    ])),
                    Cell::from(match s {
                        None => Line::from(theme::dim("…")),
                        Some(s) => Line::from(Span::styled(
                            fmt_percent(s.cpu_pct),
                            Style::new().fg(cpu_color(s.cpu_pct)),
                        )),
                    }),
                    Cell::from(Line::from(theme::dim(
                        s.map(|x| human_bytes(x.mem)).unwrap_or_default(),
                    ))),
                    Cell::from(Line::from(Span::styled(
                        s.map(|x| fmt_percent(x.mem_pct)).unwrap_or_default(),
                        Style::new().fg(s.map(|x| mem_color(x.mem_pct)).unwrap_or(Color::Gray)),
                    ))),
                    Cell::from(Line::from(theme::dim(
                        s.map(|x| format!("{} / {}", human_bytes(x.net_rx), human_bytes(x.net_tx)))
                            .unwrap_or_default(),
                    ))),
                    Cell::from(Line::from(theme::dim(
                        s.map(|x| {
                            format!("{} / {}", human_bytes(x.blk_read), human_bytes(x.blk_write))
                        })
                        .unwrap_or_default(),
                    ))),
                    Cell::from(Line::from(theme::dim(
                        s.and_then(|x| x.pids)
                            .map(|p| p.to_string())
                            .unwrap_or_default(),
                    ))),
                ];
                TRow::new(cells)
            })
            .collect();
        let sel = running.get(app.stats_sel.min(running.len().saturating_sub(1)));
        let sel_data = sel.map(|c| {
            let sample = app.stats.get(&c.id).cloned();
            let cpu_data: Vec<u64> = app
                .cpu_hist
                .get(&c.id)
                .map(|h| h.iter().copied().collect())
                .unwrap_or_default();
            let mem_data: Vec<u64> = app
                .mem_hist
                .get(&c.id)
                .map(|h| h.iter().copied().collect())
                .unwrap_or_default();
            (c.name.clone(), sample, cpu_data, mem_data)
        });
        (table_rows, sel_data, running.len())
    };

    let header = TRow::new(
        [
            "NAME",
            "CPU%",
            "MEM",
            "MEM%",
            "NET I/O",
            "BLOCK I/O",
            "PIDS",
        ]
        .map(|h| {
            Cell::from(Span::styled(
                h,
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))
        }),
    );

    let title = format!(" CONTAINER STATS │ {running_len} running ");
    let widths = [
        Constraint::Percentage(26),
        Constraint::Percentage(9),
        Constraint::Percentage(13),
        Constraint::Percentage(9),
        Constraint::Percentage(19),
        Constraint::Percentage(19),
        Constraint::Percentage(5),
    ];
    if running_len == 0 {
        app.stats_state.select(None);
    } else {
        app.stats_state
            .select(Some(app.stats_sel.min(running_len - 1)));
    }
    f.render_stateful_widget(
        Table::new(table_rows, widths)
            .header(header)
            .block(
                Block::bordered()
                    .title(Span::styled(
                        title,
                        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ))
                    .border_style(Style::new().fg(Color::Cyan)),
            )
            .row_highlight_style(
                Style::new()
                    .bg(theme::SELECT_BG)
                    .fg(theme::SELECT_FG)
                    .add_modifier(Modifier::BOLD),
            ),
        rows_area[0],
        &mut app.stats_state,
    );

    // sparklines for the selected container
    let charts = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows_area[1]);
    match sel_data {
        None => {
            let b = Block::bordered()
                .title(if app.search_active() {
                    " NO MATCHES "
                } else {
                    " NO RUNNING CONTAINERS "
                })
                .border_style(Style::new().fg(Color::DarkGray));
            f.render_widget(Paragraph::new("").block(b), rows_area[1]);
        }
        Some((name, sample, cpu_data, mem_data)) => {
            let cpu_title = format!(
                " {name} · CPU {} ",
                sample
                    .as_ref()
                    .map(|s| fmt_percent(s.cpu_pct))
                    .unwrap_or_else(|| "—".into())
            );
            let mem_limit = sample.as_ref().map(|s| s.mem_limit).unwrap_or(0);
            let mem_title = format!(
                " MEM {} / {} ({}) ",
                sample
                    .as_ref()
                    .map(|s| human_bytes(s.mem))
                    .unwrap_or_else(|| "—".into()),
                if mem_limit > 0 {
                    human_bytes(mem_limit)
                } else {
                    "?".into()
                },
                sample
                    .as_ref()
                    .map(|s| fmt_percent(s.mem_pct))
                    .unwrap_or_else(|| "—".into())
            );
            let cpu_chart = Sparkline::default()
                .block(
                    Block::bordered()
                        .title(Span::styled(
                            cpu_title,
                            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                        ))
                        .border_style(Style::new().fg(Color::DarkGray)),
                )
                .data(cpu_data)
                .max(100)
                .style(Style::new().fg(Color::LightGreen));
            let mem_chart = Sparkline::default()
                .block(
                    Block::bordered()
                        .title(Span::styled(
                            mem_title,
                            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                        ))
                        .border_style(Style::new().fg(Color::DarkGray)),
                )
                .data(mem_data)
                .max(mem_limit.max(1))
                .style(Style::new().fg(Color::LightMagenta));
            f.render_widget(cpu_chart, charts[0]);
            f.render_widget(mem_chart, charts[1]);
        }
    }
}
