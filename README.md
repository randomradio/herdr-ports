# Remote Ports

A [Herdr](https://herdr.dev) plugin. Open a port from a remote Herdr workspace
on this laptop as:

```text
http://herdr.{workspace}.localhost:{port}
```

It uses native saved machines (`herdr machine add`). It does not wrap
`herdr --remote`.

## Install

```bash
herdr plugin install randomradio/herdr-ports --yes
herdr server reload-config
```

Or link a local checkout:

```bash
herdr plugin link /path/to/herdr-ports
```

## Keybinding

Add this to `~/.config/herdr/config.toml`, then reload config
(global menu → `reload config`):

```toml
[[keys.command]]
key = "prefix+shift+p"
type = "plugin_action"
command = "herdr.ports_forwarding.pick"
description = "pick remote port"
```

## Use

Plugin actions run on the selected Herdr server. SSH `-L` must bind on the
laptop, so **select Local** before you pick a remote machine.

1. Save the host: `herdr machine add workbox --label workbox`
2. In the TUI, select **Local**
3. Press the keybinding (or `herdr plugin action invoke herdr.ports_forwarding.pick`)
4. Choose **Local** or a saved machine
5. Choose a workspace port

The picker lists only listeners that belong to a Herdr workspace pane. It
opens `http://herdr.{workspace}.localhost:{port}` in the browser.

If the same local port is already held, the plugin does not steal it. Enter a
free local port, or pick another remote port.

## CLI

```bash
python3 herdr_ports.py scan Local
python3 herdr_ports.py scan workbox
python3 herdr_ports.py add workbox 8765 --open
python3 herdr_ports.py list
python3 herdr_ports.py stop 8765
```

## Requirements

- Herdr 0.9.0 or newer on this laptop
- macOS or Linux
- `python3`
- OpenSSH with key auth: `ssh -o BatchMode=yes <target> true`
- `lsof` or `ss` on each scanned host

## Marketplace

This repository has the GitHub topic `herdr-plugin`. The Herdr marketplace
indexes public topic-tagged repos with a valid `herdr-plugin.toml` about every
30 minutes: <https://herdr.dev/plugins/>
