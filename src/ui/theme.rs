use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

pub const ACCENT: Color = Color::Cyan;
pub const DIM: Color = Color::DarkGray;
pub const HEADER_BG: Color = Color::Rgb(28, 34, 48);
pub const SELECT_BG: Color = Color::Rgb(38, 58, 88);
pub const SELECT_FG: Color = Color::White;

pub fn state_color(state: &str) -> Color {
    match state {
        "running" => Color::LightGreen,
        "paused" => Color::Yellow,
        "restarting" => Color::LightYellow,
        "created" => Color::Blue,
        "exited" | "dead" => Color::Red,
        "partial" => Color::LightYellow,
        _ => Color::Gray,
    }
}

pub fn state_icon(state: &str) -> &'static str {
    match state {
        "running" => "●",
        "paused" => "⋈",
        "restarting" => "↻",
        "created" => "○",
        "exited" | "dead" => "✖",
        "partial" => "◐",
        _ => "·",
    }
}

pub fn kv_key(k: &str) -> Span<'static> {
    Span::styled(
        format!("{k:<10}"),
        Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
    )
}

pub fn val(v: impl Into<String>) -> Span<'static> {
    Span::styled(v.into(), Style::new().fg(Color::Reset))
}

pub fn dim(v: impl Into<String>) -> Span<'static> {
    Span::styled(v.into(), Style::new().fg(DIM))
}

pub fn good(v: impl Into<String>) -> Span<'static> {
    Span::styled(v.into(), Style::new().fg(Color::LightGreen))
}
