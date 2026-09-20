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

![Forward a workmbp loopback port](docs/demo.gif)

The recording is a real session: a loopback service on `workmbp` (Tailscale
`chenyangzhaos-macbook-pro`) fails on this laptop until `herdr-ports add`
opens SSH `-L`. [MP4](docs/demo.mp4) · [asciicast](docs/demo.cast) ·
re-record with `docs/record-session.sh`.

Version 0.4.0 adds optional Cloudflare Quick Tunnels for temporary public HTTPS
sharing. The popup keeps cached lists, separate forwarding and browser actions,
three-state status indicators, and discovery of detached workspace servers.

## Persist

Tunnels are SSH `-L` forwards on a ControlMaster that:

- lives under `/tmp/hp-<hash>` (short enough for macOS Unix sockets)
- starts in a new session with SIGHUP ignored, so closing the manager pane
  does not kill the mux
- uses `ControlPersist=yes`
- is recorded in plugin state
- is restored by `./herdr-ports restore` on Herdr startup

Closing the popup leaves its forwards running. A forward stops when you toggle
it off, run `stop`, reboot, or lose its SSH connection. Saved forwards can be
resumed with Space or `restore`.

## UI

The manager opens in a **popup** without changing your pane layout.

1. Select **Local** in the sidebar (the bind happens on this laptop).
2. Run the pick action.
3. Choose a machine, then a workspace.
4. A row shows `○` (closed), `◐` (closed but saved), or `●` (saved and alive).
   Click a row to select it. Press Space to start, resume, or stop its forward.
   Enter or `o` opens its URL, including for local listeners.
5. To share a service publicly, press `t`, then `y` to confirm. Escape stops
   public sharing without stopping the SSH forward.

Use arrow keys (or j/k) and the mouse wheel to select rows. Press Enter or `o` to open
the selected URL, `r` to refresh, and Escape or `q` to go back. Escape or `q`
at the machine list closes the popup. Port conflicts open a local-port prompt.

Alive means the SSH master is running and owns the local forwarding listener.
It does not confirm that the remote application responds. Saved forwards remain
visible after their tunnel stops. `stop <local-port>` also removes an inactive
saved forward. JSON scans include `saved`, `forwarded` (alive), and `status`.

The popup keeps errors visible and leaves active forwards running when closed.
Machine, workspace, and port lists are cached while the popup is open. Going
back, selecting a row, or opening a URL does not rescan. Space updates only the
selected port's status. Press `r` to refresh external changes; a failed refresh
keeps the previous port list visible. Refreshing the workspace list returns to
the machine list; select the machine again to load its workspaces. Displayed URLs use
`http://herdr.{workspace}.localhost:{local-port}`, including remapped ports.

The hostname is a browser address for a loopback-only SSH forward. It does not
provide HTTPS or separate port namespaces: two workspaces cannot use the same
local port at once. If a port is occupied, choose another in the popup. The
remote service must accept the workspace hostname in its HTTP Host header.

## Public sharing with Cloudflare

Select a port and press `t` to open the sharing view. Press `y` to confirm
public access and start a Cloudflare Quick Tunnel. For a remote port, first
press Space in the port list to enable its SSH forward. Local ports work directly.

The view shows a temporary `https://….trycloudflare.com` URL. Enter or `o`
opens that public URL. Escape or `q` stops the public tunnel and returns to
the cached port list. The SSH forward stays active. Public tunnels are not
saved or restored; keep the sharing view open while using the link.

**Anyone with the URL can access the service.** The plugin adds no authentication.
Share only services and data that you intend to make public.

Install `cloudflared` on the host running the popup (on macOS:
`brew install cloudflared`). No Cloudflare account is needed for Quick Tunnels.
The plugin connects to `http://127.0.0.1:{local-port}` and sets the origin Host
header to `herdr.{workspace}.localhost:{local-port}`. Your application must accept
that header. The existing local workspace URL does not change.

Quick Tunnels are for testing, not production: their URLs are temporary,
they have a 200 concurrent-request limit, and they do not support SSE.
A `.cloudflared/config.yaml` can prevent Quick Tunnels from working; the plugin
does not change your Cloudflare configuration. Named tunnels, custom public
domains, and Cloudflare Access configuration are not included.
See [Cloudflare's Quick Tunnel documentation](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/).

## Discovery

The plugin scans TCP listeners and first matches their process ancestry to
Herdr panes. For detached services, it checks the process working directory
against pane directories. It also recognizes an absolute `--directory` or `-d`
argument from `python -m http.server`, which can serve a project while running
from another directory. The most specific matching directory wins; ambiguous
matches are omitted.

Listeners without enough ownership evidence are not assigned to a workspace.
Use `add <machine> <port> --workspace NAME` to forward a known service manually.
Forwarding currently targets IPv4 loopback on the remote host; IPv6-only or
interface-only services need a compatible remote listener.

## Install

Needs `cargo` on the machine that installs the plugin.

```bash
herdr plugin install randomradio/herdr-ports --yes
herdr server reload-config
```

Run the same commands to update a GitHub installation. For a local development
checkout, build and link it instead:

```bash
cargo build --release
cp target/release/herdr-ports herdr-ports
herdr plugin link "$PWD" --enabled
herdr server reload-config
```

Close and reopen an existing popup after an update. To switch a local link back
to a GitHub installation, first run `herdr plugin unlink herdr.ports_forwarding`.

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
./herdr-ports scan workbox --json
./herdr-ports pick workbox
./herdr-ports add workbox 8765 --open
./herdr-ports add workbox 8765 --local-port 18765 --workspace mo
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
- `lsof` or `ss` on the local host to verify forwarding status
- Optional: `cloudflared` on the popup host for public sharing

## Marketplace

Public GitHub repository with topic `herdr-plugin`. Herdr indexes that topic
about every 30 minutes: https://herdr.dev/plugins/

Install from GitHub:

```bash
herdr plugin install randomradio/herdr-ports --yes
```
