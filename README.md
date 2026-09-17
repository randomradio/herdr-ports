# Remote Ports

A [Herdr](https://herdr.dev) plugin. Forward a listener from a remote Herdr
workspace onto this laptop as:

```text
http://herdr.{workspace}.localhost:{port}
```

Plugin id: `herdr.ports_forwarding`  
Binary: `herdr-ports` (Rust)

It uses native saved machines (`herdr machine add`). It does not wrap
`herdr --remote`.

## Persist

Tunnels are SSH `-L` forwards on a ControlMaster that:

- lives under `/tmp/hp-<hash>` (short enough for macOS Unix sockets)
- starts in a new session with SIGHUP ignored, so closing the manager pane
  does not kill the mux
- uses `ControlPersist=yes`
- is recorded in plugin state
- is restored by `./herdr-ports restore` on Herdr startup

Closing the split pane or typing `q` only leaves the manager. The URL stays
up until you toggle the row off, run `stop`, or reboot.

## UI

The manager is a **split pane**, not a popup.

1. Select **Local** in the sidebar (the bind happens on this laptop).
2. Run the pick action.
3. Choose a machine, then a workspace.
4. A row shows `ON` or `--`. The same number starts or stops that forward.

`q` closes the pane. Empty Enter does nothing.

## Install

Needs `cargo` on the machine that installs the plugin.

```bash
herdr plugin install randomradio/herdr-ports --yes
herdr server reload-config
```

## Keybinding

```toml
[[keys.command]]
key = "prefix+shift+p"
type = "plugin_action"
command = "herdr.ports_forwarding.pick"
description = "pick remote port"
```

Reload config after install (global menu → `reload config`).

## CLI

After install, `herdr-ports` is in the plugin root.

```bash
./herdr-ports scan workbox
./herdr-ports add workbox 8765 --open
./herdr-ports list
./herdr-ports stop 8765
./herdr-ports restore
```

## Requirements

- Herdr 0.9.0 or newer
- macOS or Linux
- Rust/`cargo` to build
- OpenSSH with key auth: `ssh -o BatchMode=yes <target> true`
- `lsof` or `ss` on each scanned host

## Marketplace

Public GitHub repository with topic `herdr-plugin`. Herdr indexes that topic
about every 30 minutes: https://herdr.dev/plugins/

Install from GitHub:

```bash
herdr plugin install randomradio/herdr-ports --yes
```
