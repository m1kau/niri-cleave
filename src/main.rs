use std::collections::{HashMap, HashSet};

use niri_ipc::socket::Socket;
use niri_ipc::state::{EventStreamState, EventStreamStatePart};
use niri_ipc::{Action, Event, Request, Response, SizeChange, Window};

const FULL_WIDTH_THRESHOLD: f64 = 0.8;

struct Cleave {
    state: EventStreamState,
    monitors: HashMap<String, f64>,
    claimed: HashSet<u64>,
}

enum Trigger {
    Opened { id: u64, ws: u64 },
    Moved { id: u64, to: Option<u64> },
    Closed { id: u64, ws: u64 },
    OutputsMaybeChanged,
}

impl Cleave {
    fn new() -> std::io::Result<Self> {
        Ok(Cleave {
            state: EventStreamState::default(),
            monitors: fetch_monitor_widths()?,
            claimed: HashSet::new(),
        })
    }

    fn windows(&self) -> &HashMap<u64, Window> {
        &self.state.windows.windows
    }

    fn tiled_on(&self, ws_id: u64) -> Vec<&Window> {
        self.windows()
            .values()
            .filter(|w| w.workspace_id == Some(ws_id) && !w.is_floating)
            .collect()
    }

    fn width_fraction(&self, window: &Window) -> Option<f64> {
        let ws = self.state.workspaces.workspaces.get(&window.workspace_id?)?;
        let monitor = self.monitors.get(ws.output.as_ref()?)?;
        Some(window.layout.tile_size.0 / monitor)
    }

    fn is_full_width(&self, window: &Window) -> bool {
        self.width_fraction(window)
            .is_some_and(|f| f > FULL_WIDTH_THRESHOLD)
    }

    fn plan(&self, event: &Event) -> Option<Trigger> {
        match event {
            Event::WorkspacesChanged { .. } => Some(Trigger::OutputsMaybeChanged),

            Event::WindowOpenedOrChanged { window } => {
                if window.is_floating {
                    return None;
                }
                match self.windows().get(&window.id) {
                    None => Some(Trigger::Opened {
                        id: window.id,
                        ws: window.workspace_id?,
                    }),
                    Some(prev) if prev.workspace_id != window.workspace_id => {
                        Some(Trigger::Moved {
                            id: window.id,
                            to: window.workspace_id,
                        })
                    }
                    _ => None,
                }
            }

            Event::WindowClosed { id } => {
                let ws = self.windows().get(id).and_then(|w| w.workspace_id)?;
                Some(Trigger::Closed { id: *id, ws })
            }

            _ => None,
        }
    }

    fn settle(&mut self, ws_id: u64) -> std::io::Result<()> {
        let tiled = self.tiled_on(ws_id);

        match tiled.len() {
            1 => {
                let id = tiled[0].id;
                let already_full = self.is_full_width(tiled[0]);

                if !already_full {
                    println!("cleave: workspace {ws_id}: solo window {id} -> full width");
                    set_width(id, 100.0)?;
                }
                self.claimed.insert(id);
            }
            2.. => {
                let hogs: Vec<u64> = tiled
                    .iter()
                    .filter(|w| self.claimed.contains(&w.id) && self.is_full_width(w))
                    .map(|w| w.id)
                    .collect();

                for id in hogs {
                    println!("cleave: workspace {ws_id}: reclaiming window {id} -> 50%");
                    set_width(id, 50.0)?;
                    self.claimed.remove(&id);
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
        eprintln!("cleave: niri rejected action: {err:?}");
    }
    Ok(())
}

fn set_width(window_id: u64, percent: f64) -> std::io::Result<()> {
    dispatch(Action::SetWindowWidth {
        id: Some(window_id),
        change: SizeChange::SetProportion(percent),
    })
}

fn fetch_monitor_widths() -> std::io::Result<HashMap<String, f64>> {
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

    let mut app = Cleave::new()?;
    println!("cleave: monitor widths: {:?}", app.monitors);

    loop {
        let event = next_event()?;

        let trigger = app.plan(&event);

        let _ = app.state.apply(event);

        match trigger {
            Some(Trigger::Opened { id, ws }) => {
                let incumbent: Option<(f64, bool)> = {
                    let tiled = app.tiled_on(ws);
                    if tiled.len() == 2 {
                        tiled.iter().find(|w| w.id != id).and_then(|w| {
                            Some((app.width_fraction(w)?, app.claimed.contains(&w.id)))
                        })
                    } else {
                        None
                    }
                };

                app.settle(ws)?;

                if let Some((frac, was_claimed)) = incumbent {
                    let full = frac > FULL_WIDTH_THRESHOLD;
                    if full && !was_claimed {
                    } else {
                        let incumbent_now = if full { 0.5 } else { frac };
                        let mine = ((1.0 - incumbent_now) * 100.0).clamp(10.0, 90.0);
                        println!("cleave: workspace {ws}: sizing new window {id} to {mine:.0}%");
                        set_width(id, mine)?;
                    }
                }
            }

            Some(Trigger::Moved { id, to }) => {
                app.claimed.remove(&id);

                if let Some(ws) = to {
                    if app.tiled_on(ws).len() == 1 {
                        app.settle(ws)?;
                    }
                }
            }

            Some(Trigger::Closed { id, ws }) => {
                app.claimed.remove(&id);
                app.settle(ws)?;
            }

            Some(Trigger::OutputsMaybeChanged) => {
                app.monitors = fetch_monitor_widths()?;
            }

            None => {}
        }
    }
}
