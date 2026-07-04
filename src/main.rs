use std::collections::HashMap;

use niri_ipc::socket::Socket;
use niri_ipc::{Action, Event, Request, Response, Window, Workspace, SizeChange};

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

    fn width_fraction(&self, window: &Window) -> Option<f64> {
        let ws = self.workspaces.get(&window.workspace_id?)?;
        let monitor = self.monitor_widths.get(ws.output.as_ref()?)?;
        Some(window.layout.tile_size.0 / monitor)
    }

    fn is_full_width(&self, window: &Window) -> bool {
        self.width_fraction(window).is_some_and(|f| f > FULL_WIDTH_THRESHOLD)
    }

    fn cleave(&self, ws_id: u64) -> std::io::Result<()> {
        let tiled = self.tiled_on(ws_id);

        match tiled.len() {
            1 => {
                let solo = tiled[0];
                if !self.is_full_width(solo) {
                    println!("cleave: workspace {ws_id}: solo window {} -> maximizing", solo.id);
                    on_column(solo.id, self.focused, Action::MaximizeColumn {})?;
                }
            }
            2.. => {
                for win in &tiled {
                    if self.is_full_width(win) {
                        println!("cleave: workspace {ws_id}: window {} maximized -> splitting", win.id);
                        on_column(win.id, self.focused, Action::SetColumnWidth { change: SizeChange::SetProportion(50.0) })?;
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

fn on_column(target: u64, focused: Option<u64>, action: Action) -> std::io::Result<()> {
    if focused == Some(target) {
        return dispatch(action);
    }
    dispatch(Action::FocusWindow { id: target })?;
    dispatch(action)?;
    if let Some(fid) = focused {
        dispatch(Action::FocusWindow { id: fid })?;
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
                        let incumbent_frac = {
                            let tiled = desk.tiled_on(ws_id);
                            if tiled.len() == 2 {
                                tiled.iter().find(|w| w.id != id).and_then(|w| desk.width_fraction(w)) }
                            else { None }
                        };
                        desk.cleave(ws_id)?;

                        if let Some(frac) = incumbent_frac {
                            let incumbent_now = if frac > FULL_WIDTH_THRESHOLD { 0.5 } else { frac };
                            let mine = ((1.0 - incumbent_now) * 100.0).clamp(10.0, 90.0);
                            println!("cleave: workspace {ws_id}: sizing new window {id} to {mine:.0}%");
                            on_column(id, desk.focused, Action::SetColumnWidth { change: SizeChange::SetProportion(mine) })?;
                        }
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
