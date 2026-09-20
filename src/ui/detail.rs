use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::theme;
use crate::app::{App, RowKind};
use crate::model::{Container, Volume, human_bytes};

fn section(title: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("── {title} "),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled("─".repeat(30), Style::new().fg(Color::DarkGray)),
    ])
}

fn kv(k: &str, v: &str) -> Line<'static> {
    Line::from(vec![theme::kv_key(k), theme::val(v)])
}

fn kv_styled(k: &str, v: String, color: Color) -> Line<'static> {
    Line::from(vec![
        theme::kv_key(k),
        Span::styled(v, Style::new().fg(color)),
    ])
}

fn kv_multiline(k: &str, items: &[String]) -> Vec<Line<'static>> {
    if items.is_empty() {
        return vec![kv(k, "—")];
    }
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let prefix = if i == 0 {
            format!("{k:<10}")
        } else {
            " ".repeat(10)
        };
        out.push(Line::from(vec![
            Span::styled(
                prefix,
                Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
            ),
            theme::val(item.clone()),
        ]));
    }
    out
}

fn container_detail(c: &Container, app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            theme::state_icon(&c.state).to_string(),
            Style::new().fg(theme::state_color(&c.state)),
        ),
        Span::raw(" "),
        Span::styled(
            c.name.clone(),
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        theme::dim(c.short_id().to_string()),
    ])];
    lines.push(kv("Image", &c.image));
    lines.push(kv_styled(
        "State",
        format!("{} · {}", c.state, c.status),
        theme::state_color(&c.state),
    ));
    lines.push(kv("Created", &crate::model::fmt_created(c.created)));
    lines.push(kv("Command", &c.command));
    if let Some((p, s)) = c.compose_project().zip(c.compose_service()) {
        lines.push(kv("Compose", &format!("project:{p} service:{s}")));
    }
    if let Some(wd) = c.compose_workdir() {
        lines.push(kv("Dir", wd));
    }

    let d = app.details.get(&c.id);
    match d {
        None => {
            lines.push(section("INSPECT"));
            lines.push(theme::dim("loading details…".to_string()).into());
        }
        Some(d) => {
            // security / capability badges
            let mut badges: Vec<Span<'static>> = Vec::new();
            if d.runs_as_root() {
                badges.push(Span::styled(
                    "root",
                    Style::new()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if d.privileged {
                badges.push(Span::styled(
                    "privileged",
                    Style::new()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if !d.gpus.is_empty() {
                badges.push(Span::styled(
                    format!("gpus:{}", d.gpus.join(",")),
                    Style::new()
                        .fg(Color::LightYellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            for ns in d.host_namespaces() {
                badges.push(Span::styled(ns, Style::new().fg(Color::Magenta)));
            }
            if d.auto_remove {
                badges.push(Span::styled("--rm", Style::new().fg(Color::DarkGray)));
            }
            if !badges.is_empty() {
                let mut row = vec![Span::raw("  ")];
                let n = badges.len();
                for (i, b) in badges.into_iter().enumerate() {
                    row.push(b);
                    if i + 1 < n {
                        row.push(Span::styled(" · ", Style::new().fg(Color::DarkGray)));
                    }
                }
                lines.push(Line::from(row));
            }
            if let Some(ip) = &d.ip {
                lines.push(kv("IP", ip));
            }
            lines.append(&mut kv_multiline("Ports", &d.ports));
            lines.append(&mut kv_multiline("Mounts", &d.mounts));
            lines.append(&mut kv_multiline("Networks", &d.networks));
            if let Some(u) = &d.user {
                lines.push(kv("User", u));
            }
            if let Some(p) = &d.restart_policy {
                lines.push(kv("Restart", p));
            }
            if let Some(shm) = d.shm_size {
                lines.push(kv("ShmSize", &human_bytes(shm.max(0) as u64)));
            }
            if let Some(h) = &d.health {
                lines.push(kv_styled(
                    "Health",
                    h.clone(),
                    if h == "healthy" {
                        Color::LightGreen
                    } else if h == "unhealthy" {
                        Color::LightRed
                    } else {
                        Color::LightYellow
                    },
                ));
            }
            if !d.entrypoint.is_empty() {
                lines.push(kv("Entry", &d.entrypoint));
            }
            if let Some(wd) = &d.working_dir {
                lines.push(kv("WorkDir", wd));
            }
            if let Some(rc) = d.restart_count {
                lines.push(kv("Restarts", &rc.to_string()));
            }
            if let Some(ec) = d.exit_code {
                lines.push(kv("Exit", &ec.to_string()));
            }
            lines.push(section(&format!("ENV VARS ({})", d.env.len())));
            for e in &d.env {
                let (k, v) = e.split_once('=').unwrap_or((e.as_str(), ""));
                lines.push(Line::from(vec![
                    Span::styled(format!("  {k}"), Style::new().fg(Color::Magenta)),
                    Span::styled(format!("={v}"), Style::new().fg(Color::Gray)),
                ]));
            }
            if d.env.is_empty() {
                lines.push(theme::dim("  (none)".to_string()).into());
            }
        }
    }
    lines
}

fn project_detail(name: &str, app: &App) -> Vec<Line<'static>> {
    let cs: Vec<&Container> = app
        .containers
        .iter()
        .filter(|c| c.compose_project() == Some(name))
        .collect();
    let running = cs.iter().filter(|c| c.is_running()).count();
    let mut lines = vec![Line::from(vec![
        Span::styled("◆ ", Style::new().fg(Color::Cyan)),
        Span::styled(
            name.to_string(),
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        theme::good(format!("{}/{} up", running, cs.len())),
    ])];
    if let Some(wd) = cs.first().and_then(|c| c.compose_workdir()) {
        lines.push(kv("Dir", wd));
    }
    if let Some(cf) = cs.first().and_then(|c| c.compose_config_files()) {
        lines.push(kv("Config", cf));
    }
    lines.push(section("SERVICES"));
    let mut services: Vec<String> = cs
        .iter()
        .map(|c| c.compose_service().unwrap_or("—").to_string())
        .collect();
    services.sort();
    services.dedup();
    for s in services {
        let scs: Vec<&Container> = cs
            .iter()
            .copied()
            .filter(|c| c.compose_service() == Some(s.as_str()))
            .collect();
        let up = scs.iter().filter(|c| c.is_running()).count();
        let images: Vec<&str> = scs.iter().map(|c| c.image.as_str()).collect();
        lines.push(Line::from(vec![
            Span::styled(format!("  {s:<12}"), Style::new().fg(Color::Cyan)),
            Span::styled(
                format!("{}/{} ", up, scs.len()),
                Style::new().fg(if up == scs.len() {
                    Color::LightGreen
                } else if up == 0 {
                    Color::LightRed
                } else {
                    Color::LightYellow
                }),
            ),
            theme::dim(images.join(", ")),
        ]));
    }
    let vols: Vec<&Volume> = app
        .volumes
        .iter()
        .filter(|v| v.compose_project() == Some(name))
        .collect();
    if !vols.is_empty() {
        lines.push(section("PROJECT VOLUMES"));
        for v in vols {
            let size = v.size.map(human_bytes).unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<24}", crate::util::truncate(&v.name, 24)),
                    Style::new().fg(Color::Cyan),
                ),
                theme::dim(size),
            ]));
        }
    }
    lines
}

fn service_detail(project: &str, service: &str, app: &App) -> Vec<Line<'static>> {
    let cs: Vec<&Container> = app
        .containers
        .iter()
        .filter(|c| c.compose_project() == Some(project) && c.compose_service() == Some(service))
        .collect();
    let running = cs.iter().filter(|c| c.is_running()).count();
    let mut lines = vec![Line::from(vec![
        Span::styled("◈ ", Style::new().fg(Color::Cyan)),
        Span::styled(
            format!("{project}/{service}"),
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        theme::good(format!("{}/{} up", running, cs.len())),
    ])];
    for c in crate::util::sort_containers(cs) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ", theme::state_icon(&c.state)),
                Style::new().fg(theme::state_color(&c.state)),
            ),
            Span::styled(c.name.clone(), Style::new().fg(Color::White)),
            Span::raw("  "),
            theme::dim(c.image.clone()),
        ]));
    }
    lines
}

fn volume_detail(v: &Volume) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled("▤ ", Style::new().fg(Color::Cyan)),
        Span::styled(
            crate::util::truncate(&v.name, 60).to_string(),
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
    ])];
    lines.push(kv("Driver", &v.driver));
    lines.push(kv("Scope", &v.scope));
    if let Some(s) = v.size {
        lines.push(kv("Size", &human_bytes(s)));
    }
    if let Some(rc) = v.ref_count {
        lines.push(kv("RefCount", &rc.to_string()));
    }
    if let Some(p) = v.compose_project() {
        lines.push(kv("Compose", p));
    }
    lines.push(kv("Mount", &v.mountpoint));
    if !v.labels.is_empty() {
        lines.push(section("LABELS"));
        for (k, val) in v.labels.iter().take(12) {
            lines.push(Line::from(vec![
                Span::styled(format!("  {k}"), Style::new().fg(Color::Magenta)),
                Span::styled(format!("={val}"), Style::new().fg(Color::Gray)),
            ]));
        }
    }
    lines
}

pub fn build_detail_lines(app: &App) -> Vec<Line<'static>> {
    match app.selected_row().map(|r| r.kind.clone()) {
        Some(RowKind::Container(id)) => app
            .container_by_id(&id)
            .map(|c| container_detail(c, app))
            .unwrap_or_default(),
        Some(RowKind::Project(p)) => project_detail(&p, app),
        Some(RowKind::Service(p, s)) => service_detail(&p, &s, app),
        Some(RowKind::Volume(name)) => app
            .volumes
            .iter()
            .find(|v| v.name == name)
            .map(volume_detail)
            .unwrap_or_default(),
        Some(RowKind::Section(_)) | None => vec![
            theme::dim("select a container, service, project or volume".to_string()).into(),
            theme::dim("press ? for help".to_string()).into(),
        ],
    }
}

pub fn detail_line_count(app: &App) -> usize {
    build_detail_lines(app).len()
}
