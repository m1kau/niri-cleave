use std::collections::HashMap;

use niri_ipc::socket::Socket;
use niri_ipc::{Action, Event, Request, Response, Window, Workspace};

const FULL_WIDTH_THRESHOLD: f64 = 0.8;

struct Desktop {
    windows: HashMap<u64, Window>,
    workspaces: HashMap<u64, Workspace>,
    focused: Option<u64>,
    monitor_widths: HashMap<String, f64>,
}

impl Desktop {
    fn new() -> Self {
        Desktop {
            windows: HashMap::new(),
            workspaces: HashMap::new(),
            focused: None,
            monitor_widths: HashMap::new(),
        }
    }

    fn tiled_on(&self, ws_id: u64) -> Vec<&Window> {
        self.windows
            .values()
            .filter(|w| w.workspace_id == Some(ws_id) && !w.is_floating)
            .collect()
    }

    fn is_full_width(&self, window: &Window) -> bool {
        let Some(ws_id) = window.workspace_id else { return false };
        let Some(ws) = self.workspaces.get(&ws_id) else { return false };
        let Some(output) = ws.output.as_ref() else { return false };
        let Some(monitor_width) = self.monitor_widths.get(output) else { return false };

        window.layout.tile_size.0 / monitor_width > FULL_WIDTH_THRESHOLD
    }

    fn cleave(&self, ws_id: u64) -> std::io::Result<()> {
        let tiled = self.tiled_on(ws_id);

        match tiled.len() {
            1 => {
                let solo = tiled[0];
                if !self.is_full_width(solo) {
                    println!("cleave: workspace {ws_id}: solo window {} -> maximizing", solo.id);
                    toggle_full_width(solo.id, self.focused)?;
                }
            }
            2.. => {
                for win in &tiled {
                    if self.is_full_width(win) {
                        println!("cleave: workspace {ws_id}: window {} maximized -> splitting", win.id);
                        toggle_full_width(win.id, self.focused)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn dispatch(action: Action) -> std::io::Result<()> {
    let mut socket = Socket::connect()?;
    let reply = socket.send(Request::Action(action))?;
    if let Err(err) = reply {
        eprintln!("niri rejected action: {err:?}");
    }
    Ok(())
}

fn toggle_full_width(target: u64, focused: Option<u64>) -> std::io::Result<()> {
    match focused {
        Some(fid) if fid == target => {
            dispatch(Action::MaximizeColumn {})?;
        }
        _ => {
            dispatch(Action::FocusWindow { id: target })?;
            dispatch(Action::MaximizeColumn {})?;
            if let Some(fid) = focused {
                dispatch(Action::FocusWindow { id: fid })?;
            }
        }
    }
    Ok(())
}

fn monitor_widths() -> std::io::Result<HashMap<String, f64>> {
    let mut socket = Socket::connect()?;
    let reply = socket.send(Request::Outputs)?;

    let mut widths = HashMap::new();
    if let Ok(Response::Outputs(outputs)) = reply {
        for (name, output) in outputs {
            if let Some(logical) = output.logical {
                widths.insert(name, logical.width as f64);
            }
        }
    }
    Ok(widths)
}

fn main() -> std::io::Result<()> {
    let mut events = Socket::connect()?;
    events
        .send(Request::EventStream)?
        .expect("niri refused the event stream request");
    let mut next_event = events.read_events();

    let mut desk = Desktop::new();
    desk.monitor_widths = monitor_widths()?;
    println!("monitor widths: {:?}", desk.monitor_widths);

    loop {
        match next_event()? {
            Event::WorkspacesChanged { workspaces } => {
                desk.workspaces = workspaces.into_iter().map(|ws| (ws.id, ws)).collect();
                desk.monitor_widths = monitor_widths()?;
            }

            Event::WindowsChanged { windows } => {
                desk.windows = windows.into_iter().map(|w| (w.id, w)).collect();
                println!("tracking {} windows", desk.windows.len());
            }

            Event::WindowOpenedOrChanged { window } => {
                let id = window.id;
                let ws = window.workspace_id;
                let floating = window.is_floating;
                let brand_new = !desk.windows.contains_key(&id);

                if window.is_focused {
                    desk.focused = Some(id);
                }

                desk.windows.insert(id, window);

                if brand_new && !floating {
                    if let Some(ws_id) = ws {
                        desk.cleave(ws_id)?;
                    }
                }
            }

            Event::WindowClosed { id } => {
                if let Some(departed) = desk.windows.remove(&id) {
                    if let Some(ws_id) = departed.workspace_id {
                        desk.cleave(ws_id)?;
                    }
                }
            }

            Event::WindowFocusChanged { id } => {
                desk.focused = id;
            }

            Event::WindowLayoutsChanged { changes } => {
                for (id, layout) in changes {
                    if let Some(win) = desk.windows.get_mut(&id) {
                        win.layout = layout;
                    }
                }
            }
            _ => {}
        }
    }
}
