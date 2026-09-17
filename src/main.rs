use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};


const PLUGIN_ID: &str = "herdr.ports_forwarding";
const LOCAL_ID: &str = "local";

#[derive(Clone, Debug)]
struct Machine {
    id: String,
    label: String,
    target: String,
    session: String,
}

#[derive(Clone, Debug)]
struct Listener {
    port: u16,
    addr: String,
    process: String,
    pid: Option<u32>,
}

#[derive(Clone, Debug)]
struct WorkspacePort {
    workspace_id: String,
    workspace_label: String,
    port: u16,
    process: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Forward {
    machine_id: String,
    label: String,
    target: String,
    remote_port: u16,
    local_port: u16,
    #[serde(default)]
    workspace_label: String,
}

#[derive(Deserialize)]
struct MachineRow {
    id: String,
    label: String,
    target: String,
    #[serde(default)]
    session: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_help();
        return Ok(());
    }
    if args[0] == "pick" {
        if let Ok(port) = env::var("HERDR_PORTS_PORT") {
            if !args.iter().any(|a| a == "--port") {
                args.splice(1..1, ["--port".into(), port]);
            }
        }
    }
    match args[0].as_str() {
        "scan" => cmd_scan(&args[1..]),
        "pick" => cmd_pick(&args[1..]),
        "add" => cmd_add(&args[1..]),
        "list" => cmd_list(),
        "stop" => cmd_stop(&args[1..]),
        "open-picker" => cmd_open_picker(&args[1..]),
        "from-url" => cmd_from_url(),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => {
            print_help();
            bail!("unknown command {other}");
        }
    }
}

fn print_help() {
    println!(
        "herdr-ports — open a remote Herdr workspace port on this laptop

  URL: http://herdr.{{workspace}}.localhost:{{port}}

  scan [Local|machine] [--json]
  pick [--port N] [Local|machine]
  add <machine> <port> [--local-port N] [--workspace NAME] [--open]
  list
  stop <local-port>

Select Local in the Herdr sidebar before picking a remote machine.
"
    );
}

fn herdr_bin() -> String {
    env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into())
}

fn local_machine() -> Machine {
    Machine {
        id: LOCAL_ID.into(),
        label: "Local".into(),
        target: String::new(),
        session: String::new(),
    }
}

fn is_local(machine: &Machine) -> bool {
    machine.id == LOCAL_ID || machine.target.is_empty()
}

fn workspace_slug(label: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for ch in label.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            dash = false;
        } else if !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "workspace".into()
    } else {
        slug
    }
}

fn workspace_url(label: &str, port: u16) -> String {
    format!("http://herdr.{}.localhost:{port}", workspace_slug(label))
}

fn state_dir() -> Result<PathBuf> {
    let path = match env::var_os("HERDR_PLUGIN_STATE_DIR") {
        Some(value) => PathBuf::from(value),
        None => home_dir()?.join(".local/share/herdr-ports"),
    };
    fs::create_dir_all(path.join("ssh"))?;
    Ok(path)
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is unset"))
}

fn forwards_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("forwards.json"))
}

fn load_forwards() -> Result<Vec<Forward>> {
    let path = forwards_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

fn save_forwards(items: &[Forward]) -> Result<()> {
    fs::write(forwards_path()?, serde_json::to_string_pretty(items)? + "\n")?;
    Ok(())
}

fn run_cmd(args: &[&str], input: Option<&str>) -> Result<(i32, String, String)> {
    let mut cmd = Command::new(args[0]);
    cmd.args(&args[1..])
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().with_context(|| args[0].to_string())?;
    if let Some(text) = input {
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(text.as_bytes())?;
        }
    }
    let output = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let code = output.status.code().unwrap_or(1);
    Ok((code, stdout, stderr))
}

fn load_machines() -> Result<Vec<Machine>> {
    let bin = herdr_bin();
    let (code, stdout, _) = run_cmd(&[&bin, "machine", "list", "--json"], None)?;
    if code != 0 {
        return Ok(Vec::new());
    }
    let rows: Vec<MachineRow> = serde_json::from_str(stdout.trim()).unwrap_or_default();
    Ok(rows
        .into_iter()
        .filter(|row| row.enabled)
        .map(|row| Machine {
            id: row.id,
            label: row.label,
            target: row.target,
            session: if row.session.is_empty() {
                "default".into()
            } else {
                row.session
            },
        })
        .collect())
}

fn load_targets() -> Result<Vec<Machine>> {
    let mut targets = vec![local_machine()];
    targets.extend(load_machines()?);
    Ok(targets)
}

fn resolve_machine(selector: &str) -> Result<Machine> {
    if selector.eq_ignore_ascii_case("local") || selector == LOCAL_ID || selector == "." {
        return Ok(local_machine());
    }
    let targets = load_targets()?;
    let matches: Vec<_> = targets
        .into_iter()
        .filter(|m| m.id == selector || m.label == selector || m.target == selector)
        .collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap()),
        0 => bail!("Unknown machine '{selector}'. Use Local or `herdr machine list`."),
        _ => bail!("Machine '{selector}' is ambiguous; use the profile id."),
    }
}

fn parse_listeners(text: &str) -> Vec<Listener> {
    let ss = parse_ss(text);
    if text.lines().any(|line| line.contains("LISTEN")) && !ss.is_empty() {
        return dedupe(ss);
    }
    let lsof = parse_lsof(text);
    if !lsof.is_empty() {
        return dedupe(lsof);
    }
    dedupe(ss)
}

fn parse_ss(text: &str) -> Vec<Listener> {
    let re = Regex::new(
        r"LISTEN\s+\S+\s+\S+\s+(\S+):(\d+)\s+\S+(?:\s+users:\(\((.+)\)\))?",
    )
    .unwrap();
    let pid_re = Regex::new(r"pid=(\d+)").unwrap();
    text.lines()
        .filter_map(|line| {
            let caps = re.captures(line)?;
            let addr = caps.get(1)?.as_str().to_string();
            let port: u16 = caps.get(2)?.as_str().parse().ok()?;
            let users = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            let process = users
                .split(',')
                .next()
                .unwrap_or("unknown")
                .trim()
                .trim_matches('"')
                .to_string();
            let pid = pid_re
                .captures(users)
                .and_then(|c| c.get(1)?.as_str().parse().ok());
            Some(Listener {
                port,
                addr,
                process: if process.is_empty() {
                    "unknown".into()
                } else {
                    process
                },
                pid,
            })
        })
        .collect()
}

fn parse_lsof(text: &str) -> Vec<Listener> {
    let re = Regex::new(
        r"^(\S+)\s+(\d+)\s+\S+\s+\S+\s+\S+\s+\S+\s+\S+\s+TCP\s+(\S+):(\d+)\s+\(LISTEN\)",
    )
    .unwrap();
    text.lines()
        .filter_map(|line| {
            let caps = re.captures(line)?;
            Some(Listener {
                process: caps.get(1)?.as_str().to_string(),
                pid: caps.get(2)?.as_str().parse().ok(),
                addr: caps.get(3)?.as_str().to_string(),
                port: caps.get(4)?.as_str().parse().ok()?,
            })
        })
        .collect()
}

fn dedupe(items: Vec<Listener>) -> Vec<Listener> {
    let skip: HashSet<&str> = ["sshd", "ssh", "herdr-ports"].into_iter().collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if skip.contains(item.process.as_str()) {
            continue;
        }
        let key = (item.addr.clone(), item.port);
        if seen.insert(key) {
            out.push(item);
        }
    }
    out.sort_by_key(|item| (item.port, item.addr.clone()));
    out
}

fn port_free(host: &str, port: u16) -> bool {
    TcpListener::bind((host, port)).is_ok()
}

fn local_port_holder(port: u16) -> Option<String> {
    if port_free("127.0.0.1", port) && port_free("::1", port) {
        return None;
    }
    if let Ok((0, stdout, _)) = run_cmd(
        &["lsof", "-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN"],
        None,
    ) {
        if let Some(line) = stdout.lines().nth(1) {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                return Some(format!("{} pid {}", parts[0], parts[1]));
            }
        }
    }
    Some("another process".into())
}

fn control_path(target: &str) -> Result<PathBuf> {
    // macOS sockaddr_un.sun_path is 104 bytes. Herdr plugin state dirs are too long.
    let digest = hex_digest(target.as_bytes());
    Ok(PathBuf::from("/tmp").join(format!("hp-{}", &digest[..12])))
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn ssh_base(target: &str) -> Result<Vec<String>> {
    Ok(vec![
        "ssh".into(),
        "-T".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-o".into(),
        "StrictHostKeyChecking=yes".into(),
        "-o".into(),
        format!("ControlPath={}", control_path(target)?.display()),
    ])
}

fn ensure_master(target: &str) -> Result<()> {
    let mut check = ssh_base(target)?;
    check.extend(["-O".into(), "check".into(), target.into()]);
    let args: Vec<&str> = check.iter().map(String::as_str).collect();
    if run_cmd(&args, None)?.0 == 0 {
        return Ok(());
    }
    let mut start = ssh_base(target)?;
    start.extend([
        "-fN".into(),
        "-o".into(),
        "ControlMaster=yes".into(),
        "-o".into(),
        "ControlPersist=300".into(),
        target.into(),
    ]);
    let args: Vec<&str> = start.iter().map(String::as_str).collect();
    let (code, _, stderr) = run_cmd(&args, None)?;
    if code != 0 {
        bail!(stderr.trim().to_string());
    }
    Ok(())
}

fn run_on(machine: &Machine, argv: &[&str], input: Option<&str>) -> Result<(i32, String, String)> {
    if is_local(machine) {
        return run_cmd(argv, input);
    }
    ensure_master(&machine.target)?;
    let mut args = ssh_base(&machine.target)?;
    args.push(machine.target.clone());
    args.extend(argv.iter().map(|s| (*s).to_string()));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_cmd(&refs, input)
}

fn herdr_argv(machine: &Machine, args: &[&str]) -> Vec<String> {
    let mut cmd = vec![if is_local(machine) {
        herdr_bin()
    } else {
        "herdr".into()
    }];
    if !is_local(machine) && !machine.session.is_empty() {
        cmd.extend(["--session".into(), machine.session.clone()]);
    }
    cmd.extend(args.iter().map(|s| (*s).to_string()));
    cmd
}

fn parse_json_blob(text: &str) -> Result<Value> {
    let start = text
        .find('{')
        .ok_or_else(|| anyhow!("no JSON object"))?;
    Ok(serde_json::from_str(&text[start..])?)
}

fn herdr_result(machine: &Machine, args: &[&str]) -> Result<Value> {
    let argv = herdr_argv(machine, args);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let (code, stdout, stderr) = run_on(machine, &refs, None)?;
    if code != 0 {
        bail!(
            "{}",
            if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            }
        );
    }
    let payload = parse_json_blob(&stdout)?;
    Ok(payload.get("result").cloned().unwrap_or(payload))
}

fn scan_listeners(machine: &Machine) -> Result<Vec<Listener>> {
    if is_local(machine) {
        let (code, stdout, stderr) = if Path::new("/usr/sbin/lsof").exists()
            || which("lsof")
        {
            run_cmd(&["lsof", "-nP", "-iTCP", "-sTCP:LISTEN"], None)?
        } else {
            run_cmd(&["ss", "-ltnpH"], None)?
        };
        if code != 0 {
            bail!(stderr.trim().to_string());
        }
        return Ok(parse_listeners(&stdout));
    }
    ensure_master(&machine.target)?;
    let script = r#"
set -e
if command -v ss >/dev/null 2>&1; then
  ss -ltnpH 2>/dev/null || ss -ltnp
elif command -v lsof >/dev/null 2>&1; then
  lsof -nP -iTCP -sTCP:LISTEN
else
  echo 'remote host needs ss or lsof' >&2
  exit 1
fi
"#;
    let mut args = ssh_base(&machine.target)?;
    args.extend([machine.target.clone(), "bash".into(), "-s".into()]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, stdout, stderr) = run_cmd(&refs, Some(script))?;
    if code != 0 {
        bail!(stderr.trim().to_string());
    }
    Ok(parse_listeners(&stdout))
}

fn which(name: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| {
            env::split_paths(&paths).any(|dir| dir.join(name).exists())
        })
        .unwrap_or(false)
}

fn pid_parent_map(machine: &Machine) -> Result<HashMap<u32, u32>> {
    let (code, stdout, _) = run_on(machine, &["ps", "-axo", "pid=,ppid="], None)?;
    let mut map = HashMap::new();
    if code != 0 {
        return Ok(map);
    }
    for line in stdout.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() == 2 {
            if let (Ok(pid), Ok(ppid)) = (parts[0].parse(), parts[1].parse()) {
                map.insert(pid, ppid);
            }
        }
    }
    Ok(map)
}

fn ancestors(pid: u32, parents: &HashMap<u32, u32>) -> HashSet<u32> {
    let mut seen = HashSet::new();
    let mut current = Some(pid);
    while let Some(pid) = current {
        if !seen.insert(pid) {
            break;
        }
        current = parents.get(&pid).copied();
    }
    seen
}

fn attribute_ports(machine: &Machine, listeners: &[Listener]) -> Result<Vec<WorkspacePort>> {
    let parents = pid_parent_map(machine)?;
    let workspaces = herdr_result(machine, &["workspace", "list"])?;
    let mut labels = HashMap::new();
    if let Some(rows) = workspaces.get("workspaces").and_then(Value::as_array) {
        for row in rows {
            if let (Some(id), label) = (
                row.get("workspace_id").and_then(Value::as_str),
                row.get("label").and_then(Value::as_str),
            ) {
                labels.insert(id.to_string(), label.unwrap_or(id).to_string());
            }
        }
    }
    let panes = herdr_result(machine, &["pane", "list"])?;
    let mut pane_index: Vec<(HashSet<u32>, String, String)> = Vec::new();
    if let Some(rows) = panes.get("panes").and_then(Value::as_array) {
        for pane in rows {
            let Some(pane_id) = pane.get("pane_id").and_then(Value::as_str) else {
                continue;
            };
            let workspace_id = pane
                .get("workspace_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let info = herdr_result(machine, &["pane", "process-info", "--pane", pane_id])?;
            let process_info = info.get("process_info").cloned().unwrap_or(info);
            let mut pids = HashSet::new();
            if let Some(pid) = process_info.get("shell_pid").and_then(Value::as_u64) {
                pids.insert(pid as u32);
            }
            if let Some(procs) = process_info
                .get("foreground_processes")
                .and_then(Value::as_array)
            {
                for proc in procs {
                    if let Some(pid) = proc.get("pid").and_then(Value::as_u64) {
                        pids.insert(pid as u32);
                    }
                }
            }
            let label = labels
                .get(&workspace_id)
                .cloned()
                .unwrap_or_else(|| workspace_id.clone());
            pane_index.push((pids, workspace_id, label));
        }
    }
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for listener in listeners {
        let Some(pid) = listener.pid else { continue };
        let tree = ancestors(pid, &parents);
        let matched = pane_index.iter().find(|(pids, _, _)| !pids.is_disjoint(&tree));
        let Some((_, workspace_id, label)) = matched else {
            continue;
        };
        if !seen.insert((workspace_id.clone(), listener.port)) {
            continue;
        }
        found.push(WorkspacePort {
            workspace_id: workspace_id.clone(),
            workspace_label: label.clone(),
            port: listener.port,
            process: listener.process.clone(),
        });
    }
    found.sort_by(|a, b| {
        a.workspace_label
            .cmp(&b.workspace_label)
            .then(a.port.cmp(&b.port))
    });
    Ok(found)
}

fn spec(local_port: u16, remote_port: u16) -> String {
    format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}")
}

fn existing_forward(machine: &Machine, remote_port: u16) -> Result<Option<Forward>> {
    Ok(load_forwards()?.into_iter().find(|item| {
        (item.machine_id == machine.id
            || (!machine.target.is_empty() && item.target == machine.target))
            && item.remote_port == remote_port
    }))
}

fn merge_forwarded_ports(machine: &Machine, mut ports: Vec<WorkspacePort>) -> Result<Vec<WorkspacePort>> {
    let mut have: HashSet<(String, u16)> = ports
        .iter()
        .map(|p| (p.workspace_label.clone(), p.port))
        .collect();
    for fwd in load_forwards()? {
        let same = fwd.machine_id == machine.id
            || (!machine.target.is_empty() && fwd.target == machine.target);
        if !same {
            continue;
        }
        let label = if fwd.workspace_label.is_empty() {
            fwd.label.clone()
        } else {
            fwd.workspace_label.clone()
        };
        if have.insert((label.clone(), fwd.remote_port)) {
            ports.push(WorkspacePort {
                workspace_id: String::new(),
                workspace_label: label,
                port: fwd.remote_port,
                process: "forwarded".into(),
            });
        }
    }
    Ok(ports)
}

fn add_forward(
    machine: &Machine,
    remote_port: u16,
    local_port: Option<u16>,
    workspace_label: &str,
) -> Result<Forward> {
    let local_port = local_port.unwrap_or(remote_port);
    if let Some(holder) = local_port_holder(local_port) {
        bail!("Local port {local_port} is held by {holder}. Pass --local-port PORT.");
    }
    if is_local(machine) {
        let forward = Forward {
            machine_id: machine.id.clone(),
            label: machine.label.clone(),
            target: String::new(),
            remote_port,
            local_port,
            workspace_label: workspace_label.to_string(),
        };
        let mut items = load_forwards()?;
        items.retain(|item| !(item.machine_id == LOCAL_ID && item.local_port == local_port));
        items.push(forward.clone());
        save_forwards(&items)?;
        return Ok(forward);
    }
    ensure_master(&machine.target)?;
    let mut args = ssh_base(&machine.target)?;
    args.extend([
        "-O".into(),
        "forward".into(),
        "-L".into(),
        spec(local_port, remote_port),
        machine.target.clone(),
    ]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, _, stderr) = run_cmd(&refs, None)?;
    if code != 0 {
        bail!(stderr.trim().to_string());
    }
    let mut items = load_forwards()?;
    items.retain(|item| !(item.target == machine.target && item.local_port == local_port));
    let forward = Forward {
        machine_id: machine.id.clone(),
        label: machine.label.clone(),
        target: machine.target.clone(),
        remote_port,
        local_port,
        workspace_label: workspace_label.to_string(),
    };
    items.push(forward.clone());
    save_forwards(&items)?;
    Ok(forward)
}

fn stop_forward(local_port: u16) -> Result<()> {
    let items = load_forwards()?;
    let Some(matched) = items.iter().find(|item| item.local_port == local_port).cloned() else {
        bail!("No tracked forward on local port {local_port}");
    };
    if !matched.target.is_empty() {
        let mut args = ssh_base(&matched.target)?;
        args.extend([
            "-O".into(),
            "cancel".into(),
            "-L".into(),
            spec(matched.local_port, matched.remote_port),
            matched.target.clone(),
        ]);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let _ = run_cmd(&refs, None)?;
    }
    let rest: Vec<_> = items
        .into_iter()
        .filter(|item| item.local_port != local_port)
        .collect();
    save_forwards(&rest)?;
    Ok(())
}

fn open_browser(url: &str) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = Command::new(opener).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    Ok(())
}

fn prompt(text: &str) -> Result<String> {
    print!("{text}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Ok("q".into());
    }
    Ok(line.trim().to_string())
}


fn format_ports(machine: &Machine, ports: &[WorkspacePort]) -> Result<String> {
    let mut lines = vec![format!("Machine: {}", machine.label)];
    if ports.is_empty() {
        lines.push("No listeners in this workspace, and no tracked forwards.".into());
        return Ok(lines.join("\n"));
    }
    lines.push(format!(
        "{:<4}{:<4}{:<18}{:<8}{:<16}URL",
        "#", "ST", "WORKSPACE", "PORT", "PROCESS"
    ));
    for (index, item) in ports.iter().enumerate() {
        let on = existing_forward(machine, item.port)?.is_some();
        let st = if on { "ON" } else { "--" };
        let url = workspace_url(&item.workspace_label, item.port).replacen("http://", "", 1);
        lines.push(format!(
            "{:<4}{st:<4}{:<18}{:<8}{:<16}{url}",
            index + 1,
            item.workspace_label,
            item.port,
            item.process
        ));
    }
    lines.push("Number starts or stops a forward. q quits.".into());
    Ok(lines.join("\n"))
}

fn load_workspaces(machine: &Machine) -> Result<Vec<(String, String)>> {
    let result = herdr_result(machine, &["workspace", "list"])?;
    let mut out = Vec::new();
    if let Some(rows) = result.get("workspaces").and_then(Value::as_array) {
        for row in rows {
            if let Some(id) = row.get("workspace_id").and_then(Value::as_str) {
                let label = row
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string();
                out.push((id.to_string(), label));
            }
        }
    }
    Ok(out)
}

fn toggle_port(machine: &Machine, item: &WorkspacePort) -> Result<()> {
    if let Some(current) = existing_forward(machine, item.port)? {
        stop_forward(current.local_port)?;
        println!("Stopped {}", workspace_url(&item.workspace_label, current.local_port));
        return Ok(());
    }
    let local_port = if let Some(holder) = local_port_holder(item.port) {
        println!("Local {} is held by {holder}. Not stolen.", item.port);
        let remap = prompt("Empty to skip, or a free local port: ")?;
        if remap.is_empty() {
            return Ok(());
        }
        remap.parse()?
    } else {
        item.port
    };
    if is_local(machine) && local_port == item.port {
        let url = workspace_url(&item.workspace_label, item.port);
        println!("Already local: {url}");
        open_browser(&url)?;
        return Ok(());
    }
    let forward = add_forward(machine, item.port, Some(local_port), &item.workspace_label)?;
    let url = workspace_url(&item.workspace_label, forward.local_port);
    println!("Forwarded -> {url}");
    open_browser(&url)?;
    Ok(())
}

fn choose_index(len: usize, text: &str) -> Result<Option<usize>> {
    loop {
        let choice = prompt(text)?;
        if choice.eq_ignore_ascii_case("q") {
            return Ok(None);
        }
        if choice.is_empty() {
            continue;
        }
        if let Ok(number) = choice.parse::<usize>() {
            if (1..=len).contains(&number) {
                return Ok(Some(number - 1));
            }
        }
        println!("Invalid selection. Type a listed number, or q.");
        io::stdout().flush()?;
    }
}

fn cmd_scan(args: &[String]) -> Result<()> {
    let json_out = args.iter().any(|a| a == "--json");
    let selector = args.iter().find(|a| a.as_str() != "--json");
    let Some(selector) = selector else {
        bail!("scan [Local|machine]");
    };
    let machine = resolve_machine(selector)?;
    let ports = merge_forwarded_ports(
        &machine,
        attribute_ports(&machine, &scan_listeners(&machine)?)?,
    )?;
    if json_out {
        let rows: Vec<Value> = ports
            .iter()
            .map(|item| {
                json!({
                    "workspace": item.workspace_label,
                    "port": item.port,
                    "process": item.process,
                    "url": workspace_url(&item.workspace_label, item.port),
                    "forwarded": existing_forward(&machine, item.port).ok().flatten().is_some(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"machine": machine.label, "ports": rows}))?
        );
        return Ok(());
    }
    println!("{}", format_ports(&machine, &ports)?);
    Ok(())
}

fn cmd_list() -> Result<()> {
    let items = load_forwards()?;
    if items.is_empty() {
        println!("No tracked forwards.");
        return Ok(());
    }
    println!("{:<4}{:<8}{:<8}URL", "ST", "LOCAL", "REMOTE");
    for item in items {
        let label = if item.workspace_label.is_empty() {
            &item.label
        } else {
            &item.workspace_label
        };
        println!(
            "{:<4}{:<8}{:<8}{}",
            "ON",
            item.local_port,
            item.remote_port,
            workspace_url(label, item.local_port)
        );
    }
    Ok(())
}

fn cmd_add(args: &[String]) -> Result<()> {
    if args.len() < 2 {
        bail!("usage: herdr-ports add <machine> <port> [--local-port N] [--workspace NAME] [--open]");
    }
    let selector = &args[0];
    let remote_port: u16 = args[1].parse()?;
    let mut local_port = None;
    let mut workspace = String::new();
    let mut open = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--local-port" if i + 1 < args.len() => {
                local_port = Some(args[i + 1].parse()?);
                i += 2;
            }
            "--workspace" if i + 1 < args.len() => {
                workspace = args[i + 1].clone();
                i += 2;
            }
            "--open" => {
                open = true;
                i += 1;
            }
            other => bail!("unknown argument {other}"),
        }
    }
    let machine = resolve_machine(selector)?;
    if workspace.is_empty() {
        if let Ok(ports) = attribute_ports(&machine, &scan_listeners(&machine)?) {
            if let Some(item) = ports.iter().find(|p| p.port == remote_port) {
                workspace = item.workspace_label.clone();
            }
        }
    }
    let forward = add_forward(&machine, remote_port, local_port, &workspace)?;
    let url = workspace_url(
        if workspace.is_empty() {
            &machine.label
        } else {
            &workspace
        },
        forward.local_port,
    );
    println!("Forwarded {}:{} -> {url}", machine.label, forward.remote_port);
    if open {
        open_browser(&url)?;
    }
    Ok(())
}

fn cmd_stop(args: &[String]) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: herdr-ports stop <local-port>");
    }
    let port: u16 = args[0].parse()?;
    stop_forward(port)?;
    println!("Stopped local port {port}");
    Ok(())
}

fn cmd_pick(args: &[String]) -> Result<()> {
    let mut wanted: Option<u16> = None;
    let mut selector: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--port" && i + 1 < args.len() {
            wanted = Some(args[i + 1].parse()?);
            i += 2;
            continue;
        }
        if args[i].starts_with('-') {
            bail!("unknown argument {}", args[i]);
        }
        selector = Some(args[i].clone());
        i += 1;
    }
    let targets = load_targets()?;
    println!("Forwards bind on THIS computer.");
    println!("Targets:");
    for (index, item) in targets.iter().enumerate() {
        if item.target.is_empty() {
            println!("  {}) {}", index + 1, item.label);
        } else {
            println!("  {}) {}  {}", index + 1, item.label, item.target);
        }
    }
    io::stdout().flush()?;
    let machine = if let Some(selector) = selector {
        resolve_machine(&selector)?
    } else {
        match choose_index(targets.len(), "Select target number, or q: ")? {
            Some(idx) => targets[idx].clone(),
            None => return Ok(()),
        }
    };
    println!("Loading {}...", machine.label);
    io::stdout().flush()?;
    let workspaces = load_workspaces(&machine).unwrap_or_default();
    let mut workspace_id = None;
    let mut workspace_label = None;
    if !workspaces.is_empty() {
        println!("Workspaces on {}:", machine.label);
        for (index, (_id, label)) in workspaces.iter().enumerate() {
            println!("  {}) {label}", index + 1);
        }
        match choose_index(workspaces.len(), "Select workspace number, or q: ")? {
            Some(idx) => {
                workspace_id = Some(workspaces[idx].0.clone());
                workspace_label = Some(workspaces[idx].1.clone());
            }
            None => return Ok(()),
        }
    }
    loop {
        let ports = match scan_listeners(&machine)
            .and_then(|listeners| attribute_ports(&machine, &listeners))
            .and_then(|ports| merge_forwarded_ports(&machine, ports))
        {
            Ok(mut ports) => {
                if let (Some(id), Some(label)) = (&workspace_id, &workspace_label) {
                    ports.retain(|item| {
                        item.workspace_id == *id || item.workspace_label == *label
                    });
                }
                ports
            }
            Err(err) => {
                println!("Scan failed: {err:#}");
                println!("The popup stays open. Enter rescan, or q to quit.");
                io::stdout().flush()?;
                let choice = prompt("q quits, anything else rescan: ")?;
                if choice.eq_ignore_ascii_case("q") {
                    return Ok(());
                }
                continue;
            }
        };
        println!("{}", format_ports(&machine, &ports)?);
        io::stdout().flush()?;
        if let Some(port) = wanted.take() {
            if let Some(item) = ports.iter().find(|item| item.port == port).cloned() {
                if let Err(err) = toggle_port(&machine, &item) {
                    println!("{err:#}");
                }
            } else {
                println!("No workspace listener on port {port}.");
            }
            continue;
        }
        if ports.is_empty() {
            let choice = prompt("No ports. q quits, anything else rescan: ")?;
            if choice.eq_ignore_ascii_case("q") {
                return Ok(());
            }
            continue;
        }
        let choice = prompt("Number to start/stop, or q: ")?;
        if choice.eq_ignore_ascii_case("q") {
            return Ok(());
        }
        if choice.is_empty() {
            continue;
        }
        let selected = if let Ok(number) = choice.parse::<usize>() {
            if (1..=ports.len()).contains(&number) {
                Some(ports[number - 1].clone())
            } else {
                ports
                    .iter()
                    .find(|item| item.port.to_string() == choice)
                    .cloned()
            }
        } else {
            None
        };
        match selected {
            Some(item) => {
                if let Err(err) = toggle_port(&machine, &item) {
                    println!("{err:#}");
                }
            }
            None => println!("Invalid selection. Type a listed number, or q."),
        }
    }
}

fn cmd_open_picker(args: &[String]) -> Result<()> {
    let bin = herdr_bin();
    let mut argv = vec![
        bin.as_str(),
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN_ID,
        "--entrypoint",
        "picker",
        "--placement",
        "popup",
    ];
    let env_arg;
    if args.first().map(String::as_str) == Some("--port") && args.len() > 1 {
        env_arg = format!("HERDR_PORTS_PORT={}", args[1]);
        argv.extend(["--env", env_arg.as_str()]);
    }
    let (code, stdout, stderr) = run_cmd(&argv, None)?;
    if code != 0 {
        eprint!("{}", if stderr.is_empty() { stdout } else { stderr });
        bail!("plugin pane open failed");
    }
    print!("{stdout}");
    Ok(())
}

fn cmd_from_url() -> Result<()> {
    let url = env::var("HERDR_PLUGIN_CLICKED_URL").ok().or_else(|| {
        env::var("HERDR_PLUGIN_CONTEXT_JSON")
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|v| v.get("clicked_url")?.as_str().map(str::to_string))
    });
    let Some(url) = url else {
        bail!("No clicked URL in plugin context.");
    };
    let port = parse_localhost_port(&url).ok_or_else(|| anyhow!("Not a localhost URL: {url}"))?;
    cmd_open_picker(&["--port".into(), port.to_string()])
}

fn parse_localhost_port(url: &str) -> Option<u16> {
    let rest = url
        .strip_prefix("https://")
        .map(|s| ("https", s))
        .or_else(|| url.strip_prefix("http://").map(|s| ("http", s)))?;
    let (scheme, rest) = rest;
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let hostport = hostport.trim_start_matches('[').trim_end_matches(']');
    let (host, port) = if let Some((h, p)) = hostport.rsplit_once(':') {
        if p.chars().all(|c| c.is_ascii_digit()) {
            (h.trim_end_matches(']'), p.parse().ok())
        } else {
            (hostport, None)
        }
    } else {
        (hostport, None)
    };
    let host = host.to_ascii_lowercase();
    let ok = host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host.ends_with(".localhost");
    if !ok {
        return None;
    }
    port.or(match scheme {
        "https" => Some(443),
        "http" => Some(80),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_and_url() {
        assert_eq!(workspace_slug("spark: ~"), "spark");
        assert_eq!(workspace_url("mo", 8000), "http://herdr.mo.localhost:8000");
    }

    #[test]
    fn parse_ss_skips_sshd() {
        let text = r#"
LISTEN 0 4096 127.0.0.1:4242 0.0.0.0:* users:(("node",pid=2211,fd=23))
LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:(("sshd",pid=1,fd=3))
"#;
        let listeners = parse_listeners(text);
        assert_eq!(listeners.len(), 1);
        assert_eq!(listeners[0].port, 4242);
        assert_eq!(listeners[0].pid, Some(2211));
    }
}


