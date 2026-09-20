use super::*;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEventKind,
    },
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{
        self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate,
        EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

struct Terminal;

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            Show,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

enum Action {
    Select(usize),
    Toggle(usize),
    Back,
    Refresh,
    Open(usize),
}

fn clipped(text: &str, width: usize) -> String {
    let mut result = String::new();
    let mut used = 0;
    for c in text.chars().filter(|c| !c.is_control()) {
        let size = c.width().unwrap_or(0);
        if used + size > width {
            break;
        }
        result.push(c);
        used += size;
    }
    result
}

fn draw(
    title: &str,
    rows: &[String],
    selected: usize,
    message: &str,
    footer: &str,
) -> Result<usize> {
    let (width, height) = terminal::size()?;
    let visible = height.saturating_sub(7) as usize;
    let offset = selected.saturating_sub(visible.saturating_sub(1));
    let mut out = io::stdout();
    queue!(
        out,
        BeginSynchronizedUpdate,
        Clear(ClearType::All),
        MoveTo(1, 0),
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        Print(clipped(title, width.saturating_sub(2) as usize)),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    for (i, row) in rows.iter().enumerate().skip(offset).take(visible) {
        queue!(out, MoveTo(1, (i - offset + 2) as u16))?;
        if i == selected {
            queue!(out, SetAttribute(Attribute::Reverse))?;
        }
        let row = clipped(
            &format!(" {} {row}", if i == selected { "›" } else { " " }),
            width.saturating_sub(2) as usize,
        );
        queue!(
            out,
            Print(&row),
            Print(" ".repeat((width.saturating_sub(2) as usize).saturating_sub(row.width()))),
            SetAttribute(Attribute::Reset)
        )?;
    }
    if height >= 5 {
        queue!(
            out,
            MoveTo(1, height - 4),
            SetForegroundColor(Color::Yellow),
            Print(clipped(message, width.saturating_sub(2) as usize)),
            ResetColor,
            MoveTo(1, height - 2),
            SetForegroundColor(Color::DarkGrey),
            Print(clipped(footer, width.saturating_sub(2) as usize)),
            ResetColor
        )?;
    }
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()?;
    Ok(offset)
}

fn select(
    title: &str,
    rows: &[String],
    selected: &mut usize,
    message: &str,
    ports: bool,
) -> Result<Action> {
    *selected = (*selected).min(rows.len().saturating_sub(1));
    let mut painted = None;
    let mut offset = 0;
    loop {
        let footer = if ports {
            "↑↓ select · Space toggle · Enter/o open · r refresh · Esc back"
        } else {
            "↑↓ select · Enter choose · r refresh · Esc back"
        };
        let state = (*selected, terminal::size()?);
        if painted != Some(state) {
            offset = draw(title, rows, *selected, message, footer)?;
            painted = Some(state);
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return Ok(Action::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(Action::Back)
                }
                KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = (*selected + 1).min(rows.len().saturating_sub(1))
                }
                KeyCode::Char(' ') if ports && !rows.is_empty() => {
                    return Ok(Action::Toggle(*selected))
                }
                KeyCode::Enter if ports && !rows.is_empty() => return Ok(Action::Open(*selected)),
                KeyCode::Enter if !rows.is_empty() => return Ok(Action::Select(*selected)),
                KeyCode::Char('o') if ports && !rows.is_empty() => {
                    return Ok(Action::Open(*selected))
                }
                KeyCode::Char('r') => return Ok(Action::Refresh),
                _ => {}
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => *selected = selected.saturating_sub(1),
                MouseEventKind::ScrollDown => {
                    *selected = (*selected + 1).min(rows.len().saturating_sub(1))
                }
                MouseEventKind::Down(MouseButton::Left)
                    if mouse.row >= 2 && mouse.row < terminal::size()?.1.saturating_sub(5) =>
                {
                    let index = offset + mouse.row as usize - 2;
                    if index < rows.len() {
                        *selected = index;
                        if !ports {
                            return Ok(Action::Select(index));
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn local_port_input(port: u16) -> Result<Option<u16>> {
    let mut input = String::new();
    loop {
        draw(
            "Choose local port",
            &[format!("Local port: {input}▏")],
            0,
            &format!("Port {port} is occupied. Enter a different local port (1–65535)."),
            "Enter confirm · Esc cancel",
        )?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Esc => return Ok(None),
                KeyCode::Char(c) if c.is_ascii_digit() && input.len() < 5 => input.push(c),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => {
                    if let Ok(port @ 1..=65535) = input.parse::<u16>() {
                        return Ok(Some(port));
                    }
                }
                _ => {}
            }
        }
    }
}

fn toggle(machine: &Machine, port: &WorkspacePort) -> Result<String> {
    if is_local(machine) {
        return Ok("Already local. Press Enter to open.".into());
    }
    if let Some(saved) = existing_forward(machine, port.port)? {
        if forward_alive(&saved)? {
            stop_forward(saved.local_port)?;
            return Ok("Forward stopped".into());
        }
        restore_one(&saved)?;
        return Ok("Saved forward resumed".into());
    }
    let local = if local_port_holder(port.port).is_some() {
        let Some(local) = local_port_input(port.port)? else {
            return Ok("Cancelled".into());
        };
        local
    } else {
        port.port
    };
    add_forward(machine, port.port, Some(local), &port.workspace_label)?;
    Ok(format!(
        "Forward started: {}",
        workspace_url(&port.workspace_label, local)
    ))
}

pub(super) fn run(args: &[String]) -> Result<()> {
    let mut wanted = None;
    let mut machine_arg = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--port" {
            wanted = Some(
                args.next()
                    .context("--port needs a value")?
                    .parse::<u16>()?,
            );
        } else if arg.starts_with('-') || machine_arg.is_some() {
            bail!("Unexpected argument: {arg}");
        } else {
            machine_arg = Some(resolve_machine(arg)?);
        }
    }
    terminal::enable_raw_mode()?;
    let _terminal = Terminal;
    execute!(io::stdout(), EnterAlternateScreen, Hide, EnableMouseCapture)?;
    let mut machine_selection = 0;
    let mut notice = String::new();
    let mut machines = if let Some(machine) = &machine_arg {
        vec![machine.clone()]
    } else {
        load_targets()?
    };
    let mut workspace_cache = HashMap::new();
    let mut port_cache = HashMap::new();
    loop {
        let rows = machines
            .iter()
            .map(|m| format!("{}   {}", m.label, m.target))
            .collect::<Vec<_>>();
        let machine = match select(
            "Ports / Machines",
            &rows,
            &mut machine_selection,
            &notice,
            false,
        )? {
            Action::Select(i) => &machines[i],
            Action::Back => return Ok(()),
            Action::Refresh => {
                if machine_arg.is_none() {
                    machines = load_targets()?;
                }
                workspace_cache.clear();
                port_cache.clear();
                continue;
            }
            _ => continue,
        };
        let workspaces = match workspace_cache.get(&machine.id).cloned() {
            Some(w) => w,
            None => match load_workspaces(machine) {
                Ok(w) => {
                    workspace_cache.insert(machine.id.clone(), w.clone());
                    w
                }
                Err(e) => {
                    notice = format!("{e:#}");
                    continue;
                }
            },
        };
        let mut ws_selection = 0;
        loop {
            let rows = workspaces
                .iter()
                .map(|(_, label)| label.clone())
                .collect::<Vec<_>>();
            let ws = match select(
                &format!("Ports / {}", machine.label),
                &rows,
                &mut ws_selection,
                "Choose a workspace",
                false,
            )? {
                Action::Select(i) => &workspaces[i],
                Action::Back => break,
                Action::Refresh => {
                    workspace_cache.remove(&machine.id);
                    break;
                }
                _ => continue,
            };
            let cached = port_cache
                .entry((machine.id.clone(), machine.session.clone(), ws.0.clone()))
                .or_default();
            show_ports(machine, ws, &mut wanted, cached)?;
        }
    }
}

#[derive(Default)]
struct CachedPorts {
    ports: Vec<WorkspacePort>,
    rows: Vec<String>,
    selected: usize,
    loaded: bool,
}

fn port_row(machine: &Machine, port: &WorkspacePort) -> Result<String> {
    let saved = existing_forward(machine, port.port)?;
    let symbol = forward_symbol(saved.as_ref())?;
    let local = saved.as_ref().map_or(port.port, |f| f.local_port);
    Ok(format!(
        "{symbol} {}  {}{}",
        workspace_url(&port.workspace_label, local),
        port.process,
        if is_local(machine) { " (local)" } else { "" }
    ))
}

fn refresh_ports(
    machine: &Machine,
    workspace: &(String, String),
    cache: &mut CachedPorts,
) -> Result<()> {
    let selected_port = cache.ports.get(cache.selected).map(|p| p.port);
    let ports = merge_forwarded_ports(
        machine,
        attribute_ports(machine, &scan_listeners(machine)?)?,
    )?
    .into_iter()
    .filter(|p| {
        p.workspace_id == workspace.0
            || (p.workspace_id.is_empty() && p.workspace_label == workspace.1)
    })
    .collect::<Vec<_>>();
    let rows = ports
        .iter()
        .map(|p| port_row(machine, p))
        .collect::<Result<Vec<_>>>()?;
    cache.selected = selected_port
        .and_then(|port| ports.iter().position(|p| p.port == port))
        .unwrap_or(0);
    cache.ports = ports;
    cache.rows = rows;
    cache.loaded = true;
    Ok(())
}

fn show_ports(
    machine: &Machine,
    workspace: &(String, String),
    wanted: &mut Option<u16>,
    cache: &mut CachedPorts,
) -> Result<()> {
    let title = format!("Ports / {} / {}", machine.label, workspace.1);
    let mut notice = String::new();
    if !cache.loaded {
        draw(&title, &[], 0, "Loading ports…", "")?;
        if let Err(e) = refresh_ports(machine, workspace, cache) {
            notice = format!("{e:#}");
        }
    }
    if let Some(port) = wanted.take() {
        cache.selected = cache.ports.iter().position(|p| p.port == port).unwrap_or(0);
    }
    loop {
        let message = if notice.is_empty() {
            if cache.ports.is_empty() {
                "No ports found. Press r to refresh."
            } else {
                "○ closed   ◐ saved, inactive   ● saved, alive · cached; r refreshes"
            }
        } else {
            &notice
        };
        match select(&title, &cache.rows, &mut cache.selected, message, true)? {
            Action::Back => return Ok(()),
            Action::Refresh => {
                draw(&title, &cache.rows, cache.selected, "Refreshing ports…", "")?;
                notice = match refresh_ports(machine, workspace, cache) {
                    Ok(()) => String::new(),
                    Err(e) => format!("{e:#}"),
                };
            }
            Action::Toggle(i) => {
                draw(&title, &cache.rows, cache.selected, "Updating forward…", "")?;
                notice = toggle(machine, &cache.ports[i]).unwrap_or_else(|e| format!("{e:#}"));
                match port_row(machine, &cache.ports[i]) {
                    Ok(row) => cache.rows[i] = row,
                    Err(e) => notice = format!("{e:#}"),
                }
            }
            Action::Open(i) => {
                let p = &cache.ports[i];
                let result = (|| -> Result<()> {
                    let saved = existing_forward(machine, p.port)?;
                    open_browser(&workspace_url(
                        &p.workspace_label,
                        saved.as_ref().map_or(p.port, |f| f.local_port),
                    ))
                })();
                notice = result.err().map(|e| format!("{e:#}")).unwrap_or_default();
            }
            Action::Select(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clips_wide_text_and_strips_terminal_controls() {
        assert_eq!(clipped("界界x", 3), "界");
        assert_eq!(clipped("a\nb\u{1b}c", 3), "abc");
    }
}
