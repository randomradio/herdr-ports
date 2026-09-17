#!/usr/bin/env python3
"""Forward Herdr workspace listeners to herdr.{workspace}.localhost:{port}."""

from __future__ import annotations

import errno
import hashlib
import json
import os
import re
import shlex
import shutil
import socket
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable
from urllib.parse import urlparse

PLUGIN_ID = "randomradio.ports"
LOCAL_ID = "local"
SKIP_PROCESS_NAMES = {"sshd", "ssh", "herdr-ports"}
SS_LINE = re.compile(
    r"LISTEN\s+\S+\s+\S+\s+(\S+):(\d+)\s+\S+(?:\s+users:\(\((.+)\)\))?"
)
LSOF_LINE = re.compile(
    r"^(\S+)\s+(\d+)\s+\S+\s+\S+\s+\S+\s+\S+\s+\S+\s+TCP\s+(\S+):(\d+)\s+\(LISTEN\)"
)
PID_IN_USERS = re.compile(r"pid=(\d+)")


@dataclass
class Machine:
    id: str
    label: str
    target: str
    session: str
    enabled: bool


@dataclass
class Listener:
    port: int
    addr: str
    process: str
    pid: int | None = None


@dataclass
class WorkspacePort:
    workspace_id: str
    workspace_label: str
    pane_id: str
    port: int
    process: str
    addr: str

    def slug(self) -> str:
        return workspace_slug(self.workspace_label)

    def url(self, local_port: int | None = None) -> str:
        return workspace_url(self.workspace_label, local_port or self.port)


@dataclass
class Forward:
    machine_id: str
    label: str
    target: str
    remote_port: int
    local_port: int
    workspace_label: str = ""


LOCAL = Machine(id=LOCAL_ID, label="Local", target="", session="", enabled=True)


def herdr_bin() -> str:
    return os.environ.get("HERDR_BIN_PATH") or "herdr"


def is_local(machine: Machine) -> bool:
    return machine.id == LOCAL_ID or not machine.target


def workspace_slug(label: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", label.strip().lower()).strip("-")
    return slug or "workspace"


def workspace_url(label: str, port: int) -> str:
    return f"http://herdr.{workspace_slug(label)}.localhost:{port}"


def state_dir() -> Path:
    raw = os.environ.get("HERDR_PLUGIN_STATE_DIR")
    if raw:
        path = Path(raw)
    else:
        path = Path.home() / ".local" / "share" / "herdr-ports"
    path.mkdir(parents=True, exist_ok=True)
    (path / "ssh").mkdir(mode=0o700, exist_ok=True)
    return path


def forwards_path() -> Path:
    return state_dir() / "forwards.json"


def load_forwards() -> list[Forward]:
    path = forwards_path()
    if not path.exists():
        return []
    try:
        raw = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return []
    out: list[Forward] = []
    for item in raw:
        try:
            out.append(
                Forward(
                    machine_id=item["machine_id"],
                    label=item["label"],
                    target=item["target"],
                    remote_port=int(item["remote_port"]),
                    local_port=int(item["local_port"]),
                    workspace_label=item.get("workspace_label", ""),
                )
            )
        except (KeyError, TypeError, ValueError):
            continue
    return out


def save_forwards(items: Iterable[Forward]) -> None:
    payload = [asdict(item) for item in items]
    forwards_path().write_text(json.dumps(payload, indent=2) + "\n")


def run(
    args: list[str],
    *,
    check: bool = False,
    capture: bool = True,
    input_text: str | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        check=check,
        text=True,
        encoding="utf-8",
        errors="replace",
        input=input_text,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )

def load_machines() -> list[Machine]:
    result = run([herdr_bin(), "machine", "list", "--json"])
    if result.returncode != 0:
        return []
    try:
        rows = json.loads(result.stdout or "[]")
    except json.JSONDecodeError:
        return []
    machines = [
        Machine(
            id=row["id"],
            label=row["label"],
            target=row["target"],
            session=row.get("session", "default"),
            enabled=bool(row.get("enabled", True)),
        )
        for row in rows
    ]
    return [machine for machine in machines if machine.enabled]


def load_targets() -> list[Machine]:
    return [LOCAL, *load_machines()]


def resolve_machine(selector: str | None) -> Machine:
    targets = load_targets()
    if selector is None:
        raise SystemExit("Pass a machine: Local, or a saved machine label")
    if selector.lower() in {"local", LOCAL_ID, "."}:
        return LOCAL
    matches = [
        machine
        for machine in targets
        if selector in {machine.id, machine.label, machine.target}
    ]
    if len(matches) == 1:
        return matches[0]
    if not matches:
        raise SystemExit(f"Unknown machine '{selector}'. Use Local or `herdr machine list`.")
    raise SystemExit(f"Machine '{selector}' is ambiguous; use the profile id.")


def parse_ss(text: str) -> list[Listener]:
    listeners: list[Listener] = []
    for line in text.splitlines():
        match = SS_LINE.search(line)
        if not match:
            continue
        addr, port_s, users = match.group(1), match.group(2), match.group(3)
        process = "unknown"
        pid = None
        if users:
            name = users.split(",", 1)[0].strip().strip('"')
            if name:
                process = name
            pid_match = PID_IN_USERS.search(users)
            if pid_match:
                pid = int(pid_match.group(1))
        listeners.append(Listener(port=int(port_s), addr=addr, process=process, pid=pid))
    return listeners


def parse_lsof(text: str) -> list[Listener]:
    listeners: list[Listener] = []
    for line in text.splitlines():
        match = LSOF_LINE.match(line)
        if not match:
            continue
        process, pid_s, addr, port_s = match.groups()
        listeners.append(
            Listener(port=int(port_s), addr=addr, process=process, pid=int(pid_s))
        )
    return listeners


def parse_listeners(text: str) -> list[Listener]:
    if any(line.startswith("LISTEN") or "LISTEN " in line[:16] for line in text.splitlines()):
        parsed = parse_ss(text)
        if parsed:
            return dedupe_listeners(parsed)
    parsed = parse_lsof(text)
    if parsed:
        return dedupe_listeners(parsed)
    return dedupe_listeners(parse_ss(text))


def dedupe_listeners(items: Iterable[Listener]) -> list[Listener]:
    seen: set[tuple[str, int]] = set()
    out: list[Listener] = []
    for item in items:
        if item.process in SKIP_PROCESS_NAMES:
            continue
        key = (item.addr, item.port)
        if key in seen:
            continue
        seen.add(key)
        out.append(item)
    out.sort(key=lambda item: (item.port, item.addr))
    return out


def local_port_holder(port: int) -> str | None:
    if _bind_free("127.0.0.1", port) and _bind_free("::1", port):
        return None
    if shutil.which("lsof"):
        result = run(["lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN"])
        for line in (result.stdout or "").splitlines()[1:]:
            parts = line.split()
            if len(parts) >= 2:
                return f"{parts[0]} pid {parts[1]}"
    return "another process"


def _bind_free(host: str, port: int) -> bool:
    family = socket.AF_INET6 if ":" in host else socket.AF_INET
    try:
        sock = socket.socket(family, socket.SOCK_STREAM)
    except OSError:
        return True
    try:
        if family == socket.AF_INET6:
            sock.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
        sock.bind((host, port))
        return True
    except OSError as exc:
        if exc.errno in {errno.EADDRINUSE, errno.EACCES, errno.EPERM}:
            return False
        return True
    finally:
        sock.close()


def remote_scan_script() -> str:
    return """
set -e
if command -v ss >/dev/null 2>&1; then
  ss -ltnpH 2>/dev/null || ss -ltnp
elif command -v lsof >/dev/null 2>&1; then
  lsof -nP -iTCP -sTCP:LISTEN
else
  echo 'remote host needs ss or lsof' >&2
  exit 1
fi
"""


def control_path(target: str) -> Path:
    digest = hashlib.sha256(target.encode()).hexdigest()[:20]
    return state_dir() / "ssh" / digest


def ssh_base(target: str) -> list[str]:
    return [
        "ssh",
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        f"ControlPath={control_path(target)}",
    ]

def ssh(target: str, extra: list[str], *, check: bool = False) -> subprocess.CompletedProcess[str]:
    return run([*ssh_base(target), *extra, target], check=check)


def ensure_master(target: str) -> None:
    check = ssh(target, ["-O", "check"])
    if check.returncode == 0:
        return
    result = run(
        [
            *ssh_base(target),
            "-fN",
            "-o",
            "ControlMaster=yes",
            "-o",
            "ControlPersist=300",
            target,
        ]
    )
    if result.returncode != 0:
        err = (result.stderr or result.stdout or "").strip()
        raise SystemExit(err or f"ssh to {target} failed")


def run_on(
    machine: Machine, argv: list[str], input_text: str | None = None
) -> subprocess.CompletedProcess[str]:
    if is_local(machine):
        return run(argv, input_text=input_text)
    ensure_master(machine.target)
    return run(
        [*ssh_base(machine.target), machine.target, *argv],
        input_text=input_text,
    )


def herdr_argv(machine: Machine, args: list[str]) -> list[str]:
    cmd = [herdr_bin() if is_local(machine) else "herdr"]
    if not is_local(machine) and machine.session:
        cmd += ["--session", machine.session]
    cmd += args
    return cmd


def parse_json_blob(text: str) -> dict:
    start = text.find("{")
    if start < 0:
        raise json.JSONDecodeError("no JSON object", text, 0)
    return json.loads(text[start:])


def herdr_result(machine: Machine, args: list[str]) -> dict:
    result = run_on(machine, herdr_argv(machine, args))
    if result.returncode != 0:
        err = (result.stderr or result.stdout or "").strip()
        raise SystemExit(err or f"herdr {' '.join(args)} failed on {machine.label}")
    try:
        payload = parse_json_blob(result.stdout or "")
    except json.JSONDecodeError as exc:
        raise SystemExit(f"herdr {' '.join(args)} returned invalid JSON: {exc}") from exc
    return payload.get("result") or payload


def scan_listeners(machine: Machine) -> list[Listener]:
    if is_local(machine):
        if shutil.which("lsof"):
            result = run(["lsof", "-nP", "-iTCP", "-sTCP:LISTEN"])
        elif shutil.which("ss"):
            result = run(["ss", "-ltnpH"])
        else:
            raise SystemExit("Local scan needs lsof or ss")
        if result.returncode != 0:
            raise SystemExit((result.stderr or result.stdout or "listen scan failed").strip())
        return parse_listeners(result.stdout or "")
    ensure_master(machine.target)
    result = run(
        [*ssh_base(machine.target), machine.target, "bash", "-s"],
        input_text=remote_scan_script(),
    )
    if result.returncode != 0:
        err = (result.stderr or result.stdout or "").strip()
        raise SystemExit(err or f"port scan on {machine.target} failed")
    return parse_listeners(result.stdout or "")


def pid_parent_map(machine: Machine) -> dict[int, int]:
    result = run_on(machine, ["ps", "-axo", "pid=,ppid="])
    mapping: dict[int, int] = {}
    if result.returncode != 0:
        return mapping
    for line in (result.stdout or "").splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        try:
            mapping[int(parts[0])] = int(parts[1])
        except ValueError:
            continue
    return mapping


def ancestors(pid: int, parents: dict[int, int]) -> set[int]:
    seen: set[int] = set()
    current: int | None = pid
    while current and current not in seen:
        seen.add(current)
        current = parents.get(current)
    return seen


def pane_pid_index(machine: Machine) -> list[tuple[set[int], str, str, str, str]]:
    workspaces = {
        row["workspace_id"]: row.get("label") or row["workspace_id"]
        for row in herdr_result(machine, ["workspace", "list"]).get("workspaces", [])
    }
    panes = herdr_result(machine, ["pane", "list"]).get("panes", [])
    index: list[tuple[set[int], str, str, str, str]] = []
    for pane in panes:
        pane_id = pane.get("pane_id")
        workspace_id = pane.get("workspace_id") or ""
        if not pane_id:
            continue
        info = herdr_result(machine, ["pane", "process-info", "--pane", pane_id]).get(
            "process_info", {}
        )
        pids: set[int] = set()
        shell_pid = info.get("shell_pid")
        if shell_pid:
            pids.add(int(shell_pid))
        for proc in info.get("foreground_processes") or []:
            if proc.get("pid"):
                pids.add(int(proc["pid"]))
        cwd = pane.get("cwd") or pane.get("foreground_cwd") or ""
        label = workspaces.get(workspace_id, workspace_id or "workspace")
        index.append((pids, workspace_id, label, pane_id, cwd))
    return index


def attribute_ports(machine: Machine, listeners: list[Listener]) -> list[WorkspacePort]:
    parents = pid_parent_map(machine)
    panes = pane_pid_index(machine)
    found: list[WorkspacePort] = []
    seen: set[tuple[str, int]] = set()
    for listener in listeners:
        matched = None
        if listener.pid:
            tree = ancestors(listener.pid, parents)
            for pids, workspace_id, label, pane_id, _cwd in panes:
                if pids & tree:
                    matched = (workspace_id, label, pane_id)
                    break
        if matched is None:
            continue
        key = (matched[0], listener.port)
        if key in seen:
            continue
        seen.add(key)
        found.append(
            WorkspacePort(
                workspace_id=matched[0],
                workspace_label=matched[1],
                pane_id=matched[2],
                port=listener.port,
                process=listener.process,
                addr=listener.addr,
            )
        )
    found.sort(key=lambda item: (item.workspace_label, item.port))
    return found


def spec(local_port: int, remote_port: int) -> str:
    return f"127.0.0.1:{local_port}:127.0.0.1:{remote_port}"


def add_forward(
    machine: Machine,
    remote_port: int,
    local_port: int | None,
    workspace_label: str = "",
) -> Forward:
    local_port = remote_port if local_port is None else local_port
    holder = local_port_holder(local_port)
    if holder:
        raise SystemExit(
            f"Local port {local_port} is held by {holder}. "
            "Not remapped. Pass --local-port PORT to bind a different local port."
        )
    if is_local(machine):
        forward = Forward(
            machine_id=machine.id,
            label=machine.label,
            target="",
            remote_port=remote_port,
            local_port=local_port,
            workspace_label=workspace_label,
        )
        items = load_forwards()
        items = [
            item
            for item in items
            if not (item.machine_id == LOCAL_ID and item.local_port == local_port)
        ]
        items.append(forward)
        save_forwards(items)
        return forward
    ensure_master(machine.target)
    result = ssh(
        machine.target,
        ["-O", "forward", "-L", spec(local_port, remote_port)],
    )
    if result.returncode != 0:
        err = (result.stderr or result.stdout or "").strip()
        raise SystemExit(err or "ssh -O forward failed")
    items = [
        item
        for item in load_forwards()
        if not (item.target == machine.target and item.local_port == local_port)
    ]
    forward = Forward(
        machine_id=machine.id,
        label=machine.label,
        target=machine.target,
        remote_port=remote_port,
        local_port=local_port,
        workspace_label=workspace_label,
    )
    items.append(forward)
    save_forwards(items)
    return forward


def stop_forward(local_port: int) -> None:
    items = load_forwards()
    match = next((item for item in items if item.local_port == local_port), None)
    if match is None:
        raise SystemExit(f"No tracked forward on local port {local_port}")
    if match.target:
        ssh(match.target, ["-O", "cancel", "-L", spec(match.local_port, match.remote_port)])
    save_forwards([item for item in items if item is not match])


def open_browser(url: str) -> None:
    opener = "open" if sys.platform == "darwin" else "xdg-open"
    if shutil.which(opener):
        subprocess.Popen([opener, url], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def parse_localhost_port(url: str) -> int | None:
    parsed = urlparse(url)
    host = (parsed.hostname or "").lower()
    if host in {"localhost", "127.0.0.1", "::1"}:
        pass
    elif host.endswith(".localhost"):
        pass
    else:
        return None
    if parsed.port:
        return parsed.port
    if parsed.scheme == "https":
        return 443
    if parsed.scheme == "http":
        return 80
    return None


def clicked_url() -> str | None:
    env = os.environ.get("HERDR_PLUGIN_CLICKED_URL")
    if env:
        return env
    raw = os.environ.get("HERDR_PLUGIN_CONTEXT_JSON")
    if not raw:
        return None
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return None
    url = data.get("clicked_url")
    return url if isinstance(url, str) else None


def format_workspace_ports(machine: Machine, ports: list[WorkspacePort]) -> str:
    lines = [f"Machine: {machine.label}"]
    if not ports:
        lines.append("No listeners in Herdr workspace panes.")
        lines.append("Start the dev server in a pane of that workspace.")
        return "\n".join(lines)
    lines.append(
        f"{'#':<4}{'WORKSPACE':<18}{'PORT':<8}{'PROCESS':<16}{'URL':<42}LOCAL"
    )
    for index, item in enumerate(ports, start=1):
        holder = local_port_holder(item.port)
        local = f"HELD by {holder}" if holder else "FREE"
        url = item.url().removeprefix("http://")
        lines.append(
            f"{index:<4}{item.workspace_label:<18}{item.port:<8}{item.process:<16}{url:<42}{local}"
        )
    return "\n".join(lines)


def prompt(text: str) -> str:
    try:
        return input(text).strip()
    except EOFError:
        return ""


def cmd_scan(argv: list[str]) -> int:
    json_out = "--json" in argv
    selector = next((arg for arg in argv if arg != "--json"), None)
    machine = resolve_machine(selector)
    listeners = scan_listeners(machine)
    ports = attribute_ports(machine, listeners)
    if json_out:
        print(
            json.dumps(
                {
                    "machine": asdict(machine),
                    "ports": [asdict(item) | {"url": item.url()} for item in ports],
                },
                indent=2,
            )
        )
        return 0
    print(format_workspace_ports(machine, ports))
    return 0


def cmd_list(argv: list[str]) -> int:
    del argv
    items = load_forwards()
    if not items:
        print("No tracked forwards.")
        return 0
    print(f"{'LOCAL':<8}{'REMOTE':<8}{'URL'}")
    for item in items:
        label = item.workspace_label or item.label
        url = workspace_url(label, item.local_port) if label else f"http://127.0.0.1:{item.local_port}"
        print(f"{item.local_port:<8}{item.remote_port:<8}{url}")
    return 0


def cmd_add(argv: list[str]) -> int:
    if len(argv) < 2:
        raise SystemExit(
            "usage: herdr_ports.py add <machine> <port> [--local-port N] [--open] [--workspace NAME]"
        )
    selector, port_s, *rest = argv
    remote_port = int(port_s)
    local_port = None
    do_open = False
    workspace_label = ""
    index = 0
    while index < len(rest):
        if rest[index] == "--local-port" and index + 1 < len(rest):
            local_port = int(rest[index + 1])
            index += 2
            continue
        if rest[index] == "--workspace" and index + 1 < len(rest):
            workspace_label = rest[index + 1]
            index += 2
            continue
        if rest[index] == "--open":
            do_open = True
            index += 1
            continue
        raise SystemExit(f"unknown argument {rest[index]}")
    machine = resolve_machine(selector)
    if not workspace_label:
        ports = attribute_ports(machine, scan_listeners(machine))
        match = next((item for item in ports if item.port == remote_port), None)
        if match:
            workspace_label = match.workspace_label
    forward = add_forward(machine, remote_port, local_port, workspace_label)
    url = workspace_url(workspace_label or machine.label, forward.local_port)
    print(f"Forwarded {machine.label}:{forward.remote_port} -> {url}")
    if do_open:
        open_browser(url)
    return 0


def cmd_stop(argv: list[str]) -> int:
    if len(argv) != 1:
        raise SystemExit("usage: herdr_ports.py stop <local-port>")
    stop_forward(int(argv[0]))
    print(f"Stopped local port {argv[0]}")
    return 0


def cmd_pick(argv: list[str]) -> int:
    wanted = None
    selector = None
    index = 0
    while index < len(argv):
        if argv[index] == "--port" and index + 1 < len(argv):
            wanted = int(argv[index + 1])
            index += 2
            continue
        if argv[index].startswith("-"):
            raise SystemExit(f"unknown argument {argv[index]}")
        selector = argv[index]
        index += 1
    targets = load_targets()
    print("Forwards bind on THIS computer. Select Local, then a remote machine.")
    print("Targets:")
    for index, item in enumerate(targets, start=1):
        extra = f"  {item.target}" if item.target else ""
        print(f"  {index}) {item.label}{extra}")
    if selector:
        machine = resolve_machine(selector)
    else:
        choice = prompt("Select target number, or q: ")
        if choice.lower() in {"", "q"}:
            return 0
        try:
            machine = targets[int(choice) - 1]
        except (ValueError, IndexError):
            print("Invalid selection.")
            return 1
    listeners = scan_listeners(machine)
    ports = attribute_ports(machine, listeners)
    print(format_workspace_ports(machine, ports))
    if not ports:
        prompt("Press enter to close. ")
        return 1
    if wanted is not None:
        match = next((item for item in ports if item.port == wanted), None)
        if match is None:
            print(f"No workspace listener on port {wanted}.")
            prompt("Press enter to close. ")
            return 1
        return _forward_choice(machine, match)
    choice = prompt("Select number, port, or q: ")
    if choice.lower() in {"", "q"}:
        return 0
    selected = None
    if choice.isdigit():
        number = int(choice)
        if 1 <= number <= len(ports):
            selected = ports[number - 1]
        else:
            selected = next((item for item in ports if item.port == number), None)
    if selected is None:
        print("Invalid selection.")
        return 1
    return _forward_choice(machine, selected)


def _forward_choice(machine: Machine, item: WorkspacePort) -> int:
    holder = local_port_holder(item.port)
    if holder:
        print(
            f"Local {item.port} is held by {holder}. "
            "This plugin does not steal that port."
        )
        remap = prompt("Empty to cancel, or a free local port to remap: ")
        if not remap:
            return 1
        local_port = int(remap)
    else:
        local_port = item.port
    if is_local(machine) and local_port == item.port:
        url = item.url()
        print(f"Already local: {url}")
        open_browser(url)
        prompt("Press enter to close. ")
        return 0
    forward = add_forward(machine, item.port, local_port, item.workspace_label)
    url = item.url(forward.local_port)
    print(f"Forwarded {machine.label}:{forward.remote_port} -> {url}")
    open_browser(url)
    prompt("Press enter to close. ")
    return 0


def cmd_open_picker(argv: list[str]) -> int:
    extra: list[str] = []
    if argv and argv[0] == "--port":
        extra = argv[:2]
    result = run(
        [
            herdr_bin(),
            "plugin",
            "pane",
            "open",
            "--plugin",
            PLUGIN_ID,
            "--entrypoint",
            "picker",
            "--placement",
            "popup",
            *(["--env", f"HERDR_PORTS_PORT={extra[1]}"] if extra else []),
        ]
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr or result.stdout or "plugin pane open failed\n")
        return result.returncode
    if result.stdout:
        sys.stdout.write(result.stdout)
    return 0


def cmd_from_url(argv: list[str]) -> int:
    del argv
    url = clicked_url()
    if not url:
        raise SystemExit("No clicked URL in plugin context.")
    port = parse_localhost_port(url)
    if port is None:
        raise SystemExit(f"Not a localhost URL: {url}")
    return cmd_open_picker(["--port", str(port)])


def cmd_help(_argv: list[str]) -> int:
    print(
        """herdr-ports — open a remote Herdr workspace port on this laptop

  URL: http://herdr.{workspace}.localhost:{port}

  scan [Local|machine] [--json]
  pick [--port N] [Local|machine]
  add <machine> <port> [--local-port N] [--workspace NAME] [--open]
  list
  stop <local-port>

Select Local in the Herdr sidebar before picking a remote machine.
"""
    )
    return 0


COMMANDS = {
    "scan": cmd_scan,
    "pick": cmd_pick,
    "add": cmd_add,
    "list": cmd_list,
    "stop": cmd_stop,
    "open-picker": cmd_open_picker,
    "from-url": cmd_from_url,
    "help": cmd_help,
    "--help": cmd_help,
    "-h": cmd_help,
}


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if not argv:
        return cmd_help([])
    command, *rest = argv
    if command == "pick":
        env_port = os.environ.get("HERDR_PORTS_PORT")
        if env_port and "--port" not in rest:
            rest = ["--port", env_port, *rest]
    handler = COMMANDS.get(command)
    if handler is None:
        cmd_help([])
        return 2
    return handler(rest)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130) from None
