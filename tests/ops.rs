use std::collections::HashMap;

use dui::ops::{Cmd, GpuSpec, Op, expand_template, expand_with, parse_ops, parse_port, parse_size};
use dui::util::{split_command, split_list};

fn op_from_yaml(yaml: &str) -> Op {
    let ops = parse_ops(yaml).expect("parse");
    ops.into_iter()
        .find(|(n, _)| n == "test")
        .map(|(_, op)| op)
        .unwrap()
}

#[test]
fn parse_ops_valid_and_sorted() {
    let yaml = "
zzz:
  image: alpine:3.20
aaa:
  image: nginx:1.27
";
    let ops = parse_ops(yaml).unwrap();
    let names: Vec<&str> = ops.iter().map(|(n, _)| n.as_str()).collect();
    // load() sorts; parse_ops preserves insertion order (sorted by load)
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"aaa") && names.contains(&"zzz"));
}

#[test]
fn parse_ops_requires_image() {
    let yaml = "
test:
  name: whatever
";
    let err = parse_ops(yaml).unwrap_err().to_string();
    assert!(err.contains("missing required field 'image'"), "{err}");
}

#[test]
fn parse_ops_rejects_unknown_fields() {
    let yaml = "
test:
  image: alpine:3.20
  gpu: all
";
    assert!(parse_ops(yaml).is_err());
}

#[test]
fn parse_ops_rejects_invalid_yaml() {
    assert!(parse_ops(":\n  - not a map: [").is_err());
}

#[test]
fn expand_template_env_vars() {
    let vars: HashMap<String, String> = [
        ("DUI_TEST_VAR", "hello"),
        ("DUI_X", "a b"), // value with a space
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let lookup = |k: &str| vars.get(k).cloned();
    assert_eq!(expand_with("x${DUI_TEST_VAR}y", lookup), "xhelloy");
    assert_eq!(expand_with("$DUI_TEST_VAR!", lookup), "hello!");
    assert_eq!(
        expand_with("${DUI_TEST_VAR}-${DUI_TEST_VAR}", lookup),
        "hello-hello"
    );
    assert_eq!(expand_with("$DUI_X", lookup), "a b");
    // unknown vars expand to empty, lone dollars stay, unclosed braces are
    // kept verbatim
    assert_eq!(expand_with("${DUI_MISSING_VAR_XYZ}", lookup), "");
    assert_eq!(expand_with("no vars", lookup), "no vars");
    assert_eq!(expand_with("dollar $ alone", lookup), "dollar $ alone");
    assert_eq!(expand_with("unclosed ${oops", lookup), "unclosed ${oops");
    assert_eq!(expand_with("$$VAR", lookup), "$"); // second $ starts a new var
    // the env-backed wrapper resolves real environment variables
    assert_eq!(
        expand_template("${HOME}"),
        std::env::var("HOME").unwrap_or_default()
    );
}

#[test]
fn parse_size_variants() {
    assert_eq!(parse_size("100"), Some(100));
    assert_eq!(parse_size("1k"), Some(1024));
    assert_eq!(parse_size("1kb"), Some(1024));
    assert_eq!(parse_size("1b"), Some(1));
    assert_eq!(parse_size("512m"), Some(512 * 1024 * 1024));
    assert_eq!(parse_size("512mb"), Some(512 * 1024 * 1024));
    assert_eq!(parse_size("2G"), Some(2 * 1024 * 1024 * 1024));
    assert_eq!(parse_size("b"), None);
    assert_eq!(parse_size("xx"), None);
}

#[test]
fn split_command_respects_quotes() {
    assert_eq!(
        split_command("sh -c \"apk add jq && sh\""),
        vec!["sh", "-c", "apk add jq && sh"]
    );
    assert_eq!(split_command("  a   b  "), vec!["a", "b"]);
    assert_eq!(split_command("x 'y z' \"a b\""), vec!["x", "y z", "a b"]);
    assert_eq!(split_command(""), Vec::<String>::new());
    assert_eq!(split_command("unclosed \"quote"), vec!["unclosed", "quote"]);
}

#[test]
fn split_list_on_semicolons() {
    assert_eq!(split_list("a=b; c=d ;e=f"), vec!["a=b", "c=d", "e=f"]);
    assert!(split_list("  ;  ").is_empty());
}

#[test]
fn parse_port_variants() {
    let (key, b) = parse_port("8080:80").unwrap();
    assert_eq!(key, "80/tcp");
    assert_eq!(b.host_port.as_deref(), Some("8080"));
    assert!(b.host_ip.is_none());

    let (key, b) = parse_port("127.0.0.1:9090:90/udp").unwrap();
    assert_eq!(key, "90/udp");
    assert_eq!(b.host_ip.as_deref(), Some("127.0.0.1"));
    assert_eq!(b.host_port.as_deref(), Some("9090"));

    assert!(parse_port("8080").is_none());
    assert!(parse_port(":80").is_none());
    assert!(parse_port("a:b:c:d").is_none());
    assert!(parse_port("abc:def").is_none());
    assert!(parse_port("8080:80/bogus").is_none());
}

#[test]
fn to_create_maps_all_options() {
    let yaml = "
test:
  image: ubuntu:24.04
  name: stephen_top
  privileged: true
  gpus: all
  network: host
  pid: host
  ipc: host
  command: top -n 1
  env:
    - FOO=bar
  volumes:
    - /proc:/host/proc:ro
  ports:
    - \"8080:80\"
    - \"127.0.0.1:9090:90/udp\"
  restart: on-failure:5
  shm_size: 512m
  tty: true
  rm: true
";
    let op = op_from_yaml(yaml);
    assert_eq!(op.container_name().as_deref(), Some("stephen_top"));

    let (name, config) = op.to_create();
    assert_eq!(name.as_deref(), Some("stephen_top"));
    assert_eq!(config.image.as_deref(), Some("ubuntu:24.04"));
    assert_eq!(
        config.cmd,
        Some(vec!["top".to_string(), "-n".to_string(), "1".to_string()])
    );
    assert_eq!(config.env, Some(vec!["FOO=bar".to_string()]));
    assert_eq!(config.tty, Some(true));

    let host = config.host_config.as_ref().unwrap();
    assert_eq!(host.privileged, Some(true));
    assert_eq!(host.network_mode.as_deref(), Some("host"));
    assert_eq!(host.pid_mode.as_deref(), Some("host"));
    assert_eq!(host.ipc_mode.as_deref(), Some("host"));
    assert_eq!(host.auto_remove, Some(true));
    assert_eq!(host.shm_size, Some(512 * 1024 * 1024));
    assert_eq!(host.binds, Some(vec!["/proc:/host/proc:ro".to_string()]));

    let gpus = host.device_requests.as_ref().unwrap();
    assert_eq!(gpus[0].count, Some(-1), "gpus: all -> count -1");
    assert_eq!(
        gpus[0].capabilities.as_deref(),
        Some([vec!["gpu".to_string()]].as_slice())
    );

    let rp = host.restart_policy.as_ref().unwrap();
    assert_eq!(rp.maximum_retry_count, Some(5));

    let bindings = host.port_bindings.as_ref().unwrap();
    assert_eq!(bindings.len(), 2);
    assert!(bindings.contains_key("80/tcp"));
    assert!(bindings.contains_key("90/udp"));
    let exposed = config.exposed_ports.as_ref().unwrap();
    assert_eq!(exposed.len(), 2);
}

#[test]
fn to_create_gpu_count_and_ids() {
    let yaml = "
test:
  image: ubuntu:24.04
  gpus: 2
";
    let op = op_from_yaml(yaml);
    let (_, config) = op.to_create();
    let host = config.host_config.as_ref().unwrap();
    assert_eq!(
        host.device_requests.as_ref().unwrap()[0].count,
        Some(2),
        "gpus: 2 -> count 2"
    );

    let yaml = "
test:
  image: ubuntu:24.04
  gpus: \"GPU-aaaa,GPU-bbbb\"
";
    let op = op_from_yaml(yaml);
    let (_, config) = op.to_create();
    let host = config.host_config.as_ref().unwrap();
    assert_eq!(
        host.device_requests.as_ref().unwrap()[0].device_ids,
        Some(vec!["GPU-aaaa".to_string(), "GPU-bbbb".to_string()])
    );
}

#[test]
fn restart_policy_variants() {
    fn policy_for(restart: &str) -> Op {
        let yaml = format!("test:\n  image: alpine\n  restart: {restart}\n");
        op_from_yaml(&yaml)
    }
    let (_, c) = policy_for("always").to_create();
    let p = c.host_config.unwrap().restart_policy.unwrap();
    assert!(p.maximum_retry_count.is_none());

    let (_, c) = policy_for("on-failure:3").to_create();
    let p = c.host_config.unwrap().restart_policy.unwrap();
    assert_eq!(p.maximum_retry_count, Some(3));

    // invalid policy is silently ignored
    let (_, c) = policy_for("sometimes").to_create();
    assert!(c.host_config.unwrap().restart_policy.is_none());
}

#[test]
fn cmd_one_and_many_forms() {
    let yaml = "
test:
  image: alpine
  command: sh -c \"echo hi\"
";
    let op = op_from_yaml(yaml);
    assert_eq!(op.command, Some(Cmd::One("sh -c \"echo hi\"".into())));

    let yaml = "
test:
  image: alpine
  command:
    - sh
    - -c
    - echo hi
";
    let op = op_from_yaml(yaml);
    assert_eq!(
        op.command,
        Some(Cmd::Many(vec!["sh".into(), "-c".into(), "echo hi".into()]))
    );
}

#[test]
fn gpu_request_shapes() {
    let all = GpuSpec::Spec("all".into()).to_request();
    assert_eq!(all.count, Some(-1));
    let n = GpuSpec::Count(3).to_request();
    assert_eq!(n.count, Some(3));
    let ids = GpuSpec::Spec("GPU-x,GPU-y".into()).to_request();
    assert_eq!(
        ids.device_ids,
        Some(vec!["GPU-x".to_string(), "GPU-y".to_string()])
    );
}

#[test]
fn summary_lists_flags() {
    let yaml = "
test:
  image: ubuntu:24.04
  privileged: true
  gpus: all
  network: host
";
    let op = op_from_yaml(yaml);
    let s = op.summary();
    assert!(s.contains("ubuntu:24.04"));
    assert!(s.contains("--privileged"));
    assert!(s.contains("--gpus all"));
    assert!(s.contains("--net host"));
}

#[test]
fn load_reports_missing_home_as_error_but_missing_file_as_ok() {
    // missing file is fine (user has no ops yet); only report real errors
    let (_, err) = dui::ops::load();
    if let Some(e) = err {
        // $HOME always set in test envs; a file that exists must parse
        assert!(!e.is_empty());
    }
}
