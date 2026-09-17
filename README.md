# Remote Ports

A [Herdr](https://herdr.dev) plugin. Open a port from a remote Herdr workspace
on this laptop as:

```text
http://herdr.{workspace}.localhost:{port}
```

The command is the `herdr-ports` binary (Rust). It uses native saved machines
(`herdr machine add`). It does not wrap `herdr --remote`.

Forwards are stored in plugin state and kept on an SSH ControlMaster that is
not in the picker process group. Closing the pane does not drop them. Herdr
startup runs `herdr-ports restore`. The manager is a split pane, not a popup.

## Install

Requires `cargo` (Rust) for the plugin build step.


## Keybinding

```toml
[[keys.command]]
key = "prefix+shift+p"
type = "plugin_action"
command = "herdr.ports_forwarding.pick"
description = "pick remote port"
```

Reload config after install (global menu → `reload config`).

## Use

Select **Local** in the Herdr sidebar first. Plugin actions run on the selected
server; SSH `-L` must bind on this laptop.

1. `herdr machine add workbox --label workbox`
2. Select Local
3. Press the keybinding
4. Choose a machine, then a workspace
5. Type a port number to start or stop the forward (`ON` / `--`)

The picker stays open so you can forward another port or stop one that is
already on.

## CLI

After install, the binary lives in the plugin root as `herdr-ports`.

```bash
./herdr-ports scan workbox
./herdr-ports add workbox 8765 --open
./herdr-ports list
./herdr-ports stop 8765
```

## Requirements

- Herdr 0.9.0 or newer
- macOS or Linux
- Rust/`cargo` to build
- OpenSSH with key auth: `ssh -o BatchMode=yes <target> true`
- `lsof` or `ss` on each scanned host

## Marketplace

GitHub topic `herdr-plugin`. Index: https://herdr.dev/plugins/
