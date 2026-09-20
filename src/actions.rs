use std::collections::HashMap;

use crate::ops::{ContainerSpec, GpuSpec, RecreateRequest, parse_port};
use crate::util::{split_command, split_list};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
    Remove,
}

impl ContainerAction {
    pub fn label(&self) -> &'static str {
        match self {
            ContainerAction::Start => "start",
            ContainerAction::Stop => "stop",
            ContainerAction::Restart => "restart",
            ContainerAction::Remove => "remove",
        }
    }
}

/// An action awaiting confirmation or execution.
#[derive(Clone, Debug)]
pub enum PendingAction {
    Container {
        id: String,
        name: String,
        action: ContainerAction,
    },
    Recreate(Box<RecreateRequest>),
}

impl PendingAction {
    pub fn describe(&self) -> String {
        match self {
            PendingAction::Container { name, action, .. } => {
                format!("{} container \"{name}\"", action.label())
            }
            PendingAction::Recreate(req) => {
                let old = &req.old_name;
                let new = req.spec.name.as_deref().unwrap_or("(unnamed)");
                if old == new {
                    format!("recreate \"{old}\" with changes")
                } else {
                    format!("recreate \"{old}\" as \"{new}\"")
                }
            }
        }
    }
}

pub const GPU_OPTIONS: [&str; 5] = ["none", "all", "1", "2", "3"];

/// A single-line text input with a cursor.
#[derive(Clone, Debug, Default)]
pub struct TextInput {
    pub value: String,
    pub cursor: usize,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.len();
        TextInput { value, cursor }
    }

    fn boundary(&self, mut i: usize) -> usize {
        while i > 0 && !self.value.is_char_boundary(i) {
            i -= 1;
        }
        i.min(self.value.len())
    }

    pub fn insert(&mut self, ch: char) {
        if self.cursor > self.value.len() {
            self.cursor = self.value.len();
        }
        self.value.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.boundary(self.cursor - 1);
        self.value.drain(prev..self.cursor);
        self.cursor = prev;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        let next = self
            .value
            .char_indices()
            .map(|(i, _)| i)
            .find(|&i| i > self.cursor)
            .unwrap_or(self.value.len());
        self.value.drain(self.cursor..next);
    }

    pub fn left(&mut self) {
        self.cursor = self.boundary(self.cursor.saturating_sub(1));
    }

    pub fn right(&mut self) {
        if self.cursor < self.value.len() {
            let next = self
                .value
                .char_indices()
                .map(|(i, _)| i)
                .find(|&i| i > self.cursor)
                .unwrap_or(self.value.len());
            self.cursor = next;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.value.len();
    }
}

#[derive(Clone, Debug)]
pub enum FormValue {
    Text(TextInput),
    Toggle(bool),
    Choice(usize),
}

#[derive(Clone, Debug)]
pub struct FormField {
    pub label: &'static str,
    pub value: FormValue,
}

impl FormField {
    pub fn display(&self) -> String {
        match &self.value {
            FormValue::Text(t) => t.value.clone(),
            FormValue::Toggle(b) => (if *b { "on" } else { "off" }).to_string(),
            FormValue::Choice(i) => GPU_OPTIONS[*i].to_string(),
        }
    }

    fn cycle(&mut self) {
        match &mut self.value {
            FormValue::Toggle(b) => *b = !*b,
            FormValue::Choice(i) => *i = (*i + 1) % GPU_OPTIONS.len(),
            FormValue::Text(_) => {}
        }
    }
}

/// Edit form shown when recreating a container. Non-listed aspects of the
/// original container (binds, restart policy, shm size, pid/ipc, labels,
/// user, workdir, entrypoint, tty) are passed through unchanged.
#[derive(Clone, Debug)]
pub struct EditForm {
    pub id: String,
    pub old_name: String,
    pub was_running: bool,
    pub labels: HashMap<String, String>,
    pub user: Option<String>,
    pub working_dir: Option<String>,
    pub entrypoint: Option<Vec<String>>,
    pub tty: bool,
    pub host: bollard::models::HostConfig,
    pub fields: Vec<FormField>,
    pub sel: usize,
}

fn ports_text(host: &bollard::models::HostConfig) -> String {
    host.port_bindings
        .as_ref()
        .map(|pm| {
            let mut parts: Vec<String> = Vec::new();
            for (cport, bindings) in pm {
                if let Some(bindings) = bindings {
                    for b in bindings {
                        parts.push(match &b.host_ip {
                            Some(ip) if !ip.is_empty() => {
                                format!(
                                    "{}:{}:{}",
                                    ip,
                                    b.host_port.as_deref().unwrap_or("?"),
                                    cport
                                )
                            }
                            _ => format!("{}:{}", b.host_port.as_deref().unwrap_or("?"), cport),
                        });
                    }
                }
            }
            parts.sort();
            parts.join("; ")
        })
        .unwrap_or_default()
}

fn gpu_choice(host: &bollard::models::HostConfig) -> usize {
    let Some(reqs) = host.device_requests.as_ref() else {
        return 0;
    };
    let Some(req) = reqs.first() else {
        return 0;
    };
    match req.count {
        Some(-1) => 1,
        Some(n) if n > 0 => GPU_OPTIONS
            .iter()
            .position(|&o| o == n.to_string())
            .unwrap_or(0),
        _ => 0,
    }
}

impl EditForm {
    pub fn from_spec(id: &str, spec: &ContainerSpec, was_running: bool) -> Self {
        let fields = vec![
            FormField {
                label: "Name",
                value: FormValue::Text(TextInput::new(spec.name.clone().unwrap_or_default())),
            },
            FormField {
                label: "Image",
                value: FormValue::Text(TextInput::new(spec.image.clone().unwrap_or_default())),
            },
            FormField {
                label: "Command",
                value: FormValue::Text(TextInput::new(
                    spec.cmd.clone().map(|c| c.join(" ")).unwrap_or_default(),
                )),
            },
            FormField {
                label: "Env",
                value: FormValue::Text(TextInput::new(spec.env.join("; "))),
            },
            FormField {
                label: "Ports",
                value: FormValue::Text(TextInput::new(ports_text(&spec.host))),
            },
            FormField {
                label: "Privileged",
                value: FormValue::Toggle(spec.host.privileged.unwrap_or(false)),
            },
            FormField {
                label: "GPUs",
                value: FormValue::Choice(gpu_choice(&spec.host)),
            },
            FormField {
                label: "Network",
                value: FormValue::Text(TextInput::new(
                    spec.host.network_mode.clone().unwrap_or_default(),
                )),
            },
        ];
        EditForm {
            id: id.to_string(),
            old_name: spec.name.clone().unwrap_or_default(),
            was_running,
            labels: spec.labels.clone(),
            user: spec.user.clone(),
            working_dir: spec.working_dir.clone(),
            entrypoint: spec.entrypoint.clone(),
            tty: spec.tty,
            host: spec.host.clone(),
            fields,
            sel: 0,
        }
    }

    pub fn field(&self, label: &str) -> Option<&FormField> {
        self.fields.iter().find(|f| f.label == label)
    }

    pub fn text(&self, label: &str) -> &str {
        match self.field(label).map(|f| &f.value) {
            Some(FormValue::Text(t)) => &t.value,
            _ => "",
        }
    }

    pub fn toggled(&self, label: &str) -> bool {
        match self.field(label).map(|f| &f.value) {
            Some(FormValue::Toggle(b)) => *b,
            _ => false,
        }
    }

    pub fn choice(&self, label: &str) -> usize {
        match self.field(label).map(|f| &f.value) {
            Some(FormValue::Choice(i)) => *i,
            _ => 0,
        }
    }

    /// Toggle / cycle the selected field (space or enter on non-text fields).
    pub fn cycle_selected(&mut self) {
        if let Some(f) = self.fields.get_mut(self.sel) {
            f.cycle();
        }
    }

    pub fn to_request(&self) -> RecreateRequest {
        let mut host = self.host.clone();
        host.privileged = Some(self.toggled("Privileged"));
        let network = self.text("Network").trim().to_string();
        host.network_mode = (!network.is_empty()).then_some(network);
        let gpu = GPU_OPTIONS[self.choice("GPUs")];
        host.device_requests = match gpu {
            "none" => None,
            "all" => Some(vec![GpuSpec::Spec("all".into()).to_request()]),
            n => Some(vec![GpuSpec::Count(n.parse().unwrap_or(1)).to_request()]),
        };
        let mut bindings: bollard::models::PortMap = HashMap::new();
        for spec in split_list(self.text("Ports")) {
            if let Some((key, binding)) = parse_port(&spec)
                && let Some(v) = bindings.entry(key).or_insert_with(|| Some(Vec::new()))
            {
                v.push(binding);
            }
        }
        host.port_bindings = (!bindings.is_empty()).then_some(bindings);

        let cmd = split_command(self.text("Command"));
        let spec = ContainerSpec {
            name: Some(self.text("Name").trim().to_string()).filter(|n| !n.is_empty()),
            image: Some(self.text("Image").trim().to_string()).filter(|i| !i.is_empty()),
            cmd: (!cmd.is_empty()).then_some(cmd),
            entrypoint: self.entrypoint.clone(),
            env: split_list(self.text("Env")),
            labels: self.labels.clone(),
            user: self.user.clone(),
            working_dir: self.working_dir.clone(),
            tty: self.tty,
            host,
        };
        RecreateRequest {
            old_id: self.id.clone(),
            old_name: self.old_name.clone(),
            was_running: self.was_running,
            spec,
        }
    }
}

/// Human-readable GPU summary for badges, from inspect host config.
pub fn gpu_summary(host: &bollard::models::HostConfig) -> Vec<String> {
    host.device_requests
        .as_ref()
        .map(|reqs| {
            reqs.iter()
                .map(|r| match r.count {
                    Some(-1) => "all".to_string(),
                    Some(n) if n > 0 => n.to_string(),
                    _ => r
                        .device_ids
                        .as_ref()
                        .map(|ids| ids.join(","))
                        .unwrap_or_else(|| "?".into()),
                })
                .collect()
        })
        .unwrap_or_default()
}
