use std::collections::{HashMap, HashSet};
use std::io;
use std::process::exit;
use std::time::{Duration, Instant};

use niri_ipc::socket::Socket;
use niri_ipc::state::{EventStreamState, EventStreamStatePart};
use niri_ipc::{Action, Event, Request, Response, SizeChange, Window};

const FULL_WIDTH_THRESHOLD: f64 = 0.8;

const WIDTH_TOLERANCE: f64 = 0.04;

struct Args {
    max_tiles: usize,
    apply_on_move: bool,
}

const USAGE: &str = "\
niri-cleave: makes niri auto-tile up to N windows per workspace

Usage: cleave [OPTIONS]

  -n <N>          number of windows to manage per workspace (default: 2) (anything above 6 loops the pattern)
  -M, --no-move   don't re-tile windows moved between workspaces
  -h, --help      show this help

";

fn parse_args() -> Args {
    let mut parsed = Args {
        max_tiles: 2,
        apply_on_move: true,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-n" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) if n >= 1 => parsed.max_tiles = n,
                _ => {
                    eprintln!("cleave: -n needs a number of 1 or more");
                    exit(2);
                }
            },
            "-M" | "--no-move" => parsed.apply_on_move = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                exit(0);
            }
            other => {
                eprintln!("cleave: unknown option '{other}'\n\n{USAGE}");
                exit(2);
            }
        }
    }
    parsed
}

enum Trigger {
    Arrived { id: u64, ws: u64 },
    Moved { id: u64, to: Option<u64>, from: Option<u64> },
    Closed { ws: Option<u64> },
    OutputsMaybeChanged,
}

#[derive(Clone)]
struct PendingSnap {
    ws: u64,
    ids: Vec<u64>,
    leftmost: u64,
    share: f64,
    expires: Instant,
}

fn sorted_by_position(mut tiled: Vec<Window>) -> Option<Vec<Window>> {
    if tiled.iter().any(|w| w.layout.pos_in_scrolling_layout.is_none()) {
        return None;
    }
    tiled.sort_by_key(|w| w.layout.pos_in_scrolling_layout);
    Some(tiled)
}

struct Cleave {
    state: EventStreamState,
    monitors: HashMap<String, f64>,
    actions: Socket,
    max_tiles: usize,
    pending_snap: Option<PendingSnap>,
}

impl Cleave {
    fn windows(&self) -> &HashMap<u64, Window> {
        &self.state.windows.windows
    }

    fn fresh_tiled(&mut self, ws_id: u64) -> io::Result<Vec<Window>> {
        match self.actions.send(Request::Windows)? {
            Ok(Response::Windows(windows)) => Ok(windows
                .into_iter()
                .filter(|w| w.workspace_id == Some(ws_id) && !w.is_floating)
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    fn width_fraction(&self, window: &Window) -> Option<f64> {
        let ws = self.state.workspaces.workspaces.get(&window.workspace_id?)?;
        let monitor = self.monitors.get(ws.output.as_deref()?)?;
        Some(window.layout.tile_size.0 / monitor)
    }

    fn is_maximized(&self, window: &Window) -> bool {
        self.width_fraction(window)
            .is_some_and(|f| f > FULL_WIDTH_THRESHOLD)
    }

    fn focused_id(&self) -> Option<u64> {
        self.windows().values().find(|w| w.is_focused).map(|w| w.id)
    }

    fn action(&mut self, action: Action) -> io::Result<()> {
        if let Err(err) = self.actions.send(Request::Action(action))? {
            eprintln!("cleave: niri rejected action: {err}");
        }
        Ok(())
    }

    fn refresh_monitors(&mut self) -> io::Result<()> {
        if let Ok(Response::Outputs(outputs)) = self.actions.send(Request::Outputs)? {
            self.monitors = outputs
                .into_iter()
                .filter_map(|(name, out)| Some((name, out.logical?.width as f64)))
                .collect();
        }
        Ok(())
    }

    fn toggle_maximize(&mut self, id: u64) -> io::Result<()> {
        let refocus = self.focused_id().filter(|&f| f != id);
        if refocus.is_some() {
            self.action(Action::FocusWindow { id })?;
        }
        self.action(Action::MaximizeColumn {})?;
        if let Some(f) = refocus {
            self.action(Action::FocusWindow { id: f })?;
        }
        Ok(())
    }

    fn maximize_if_needed(&mut self, window: &Window) -> io::Result<()> {
        if !self.is_maximized(window) {
            println!("cleave: maximizing solo window {}", window.id);
            self.toggle_maximize(window.id)?;
        }
        Ok(())
    }

    fn set_equal_shares(&mut self, ws: u64, ids: &[u64], columns: usize) -> io::Result<()> {
        let share = 100.0 / columns as f64;
        println!("cleave: workspace {ws}: sizing {columns} columns to {share:.0}%");
        for &id in ids {
            self.action(Action::SetWindowWidth {
                id: Some(id),
                change: SizeChange::SetProportion(share),
            })?;
        }
        Ok(())
    }

    fn earlier_groups_intact(&self, earlier: &[Window]) -> bool {
        let mut columns: HashMap<usize, usize> = HashMap::new();
        for w in earlier {
            if let Some((col, _)) = w.layout.pos_in_scrolling_layout {
                *columns.entry(col).or_default() += 1;
            }
        }
        columns.values().all(|&n| n == 2)
            && earlier.iter().all(|w| {
                self.width_fraction(w)
                    .is_some_and(|f| (f - 1.0 / 3.0).abs() < WIDTH_TOLERANCE)
            })
    }

    fn settle_departure(&mut self, ws: u64) -> io::Result<()> {
        let tiled = self.fresh_tiled(ws)?;
        let count = tiled.len();
        if count == 0 || count > self.max_tiles {
            return Ok(());
        }
        let Some(tiled) = sorted_by_position(tiled) else {
            return Ok(());
        };
        let (earlier, group) = tiled.split_at((count - 1) / 6 * 6);
        if !self.earlier_groups_intact(earlier) {
            return Ok(());
        }

        if let [only] = group {
            let col = only.layout.pos_in_scrolling_layout.map(|(col, _)| col);
            let shares_column = earlier
                .iter()
                .any(|w| w.layout.pos_in_scrolling_layout.map(|(c, _)| c) == col);
            if !shares_column {
                let only = only.clone();
                self.maximize_if_needed(&only)?;
            }
            return Ok(());
        }

        let columns: HashSet<usize> = group
            .iter()
            .filter_map(|w| w.layout.pos_in_scrolling_layout.map(|(col, _)| col))
            .collect();
        let columns = columns.len();
        if columns == 0 {
            return Ok(());
        }

        let fractions: Vec<f64> = match group.iter().map(|w| self.width_fraction(w)).collect() {
            Some(fractions) => fractions,
            None => return Ok(()),
        };
        let share_of = (2..=8).find(|&k| {
            let share = 1.0 / k as f64;
            fractions.iter().all(|f| (f - share).abs() < WIDTH_TOLERANCE)
        });
        if share_of.is_some_and(|k| k != columns) {
            let ids: Vec<u64> = group.iter().map(|w| w.id).collect();
            self.set_equal_shares(ws, &ids, columns)?;

            self.pending_snap = Some(PendingSnap {
                ws,
                leftmost: group[0].id,
                ids,
                share: 1.0 / columns as f64,
                expires: Instant::now() + Duration::from_secs(1),
            });
        }
        Ok(())
    }

    fn settle_arrival(&mut self, id: u64, ws: u64) -> io::Result<()> {
        let tiled = self.fresh_tiled(ws)?;
        let count = tiled.len();
        if count == 0 || count > self.max_tiles {
            return Ok(());
        }
        let Some(mut tiled) = sorted_by_position(tiled) else {
            return Ok(());
        };

        if count > 6 {
            let Some(new) = tiled.iter().find(|w| w.id == id) else {
                return Ok(());
            };
            let Some((new_col, _)) = new.layout.pos_in_scrolling_layout else {
                return Ok(());
            };
            let columns: HashSet<usize> = tiled
                .iter()
                .filter_map(|w| w.layout.pos_in_scrolling_layout.map(|(col, _)| col))
                .collect();
            let alone = tiled
                .iter()
                .filter(|w| w.id != id)
                .all(|w| w.layout.pos_in_scrolling_layout.map(|(col, _)| col) != Some(new_col));
            if alone && new_col < columns.iter().copied().max().unwrap_or(0) {
                println!("cleave: moving window {id} to the end of the row");
                let refocus = self.focused_id().filter(|&f| f != id);
                if refocus.is_some() {
                    self.action(Action::FocusWindow { id })?;
                }
                self.action(Action::MoveColumnToIndex { index: columns.len() })?;
                if let Some(f) = refocus {
                    self.action(Action::FocusWindow { id: f })?;
                }
                // re-fetch: every column right of where it sat has shifted
                let Some(fresh) = sorted_by_position(self.fresh_tiled(ws)?) else {
                    return Ok(());
                };
                tiled = fresh;
            }
        }

        let (earlier, group) = tiled.split_at((count - 1) / 6 * 6);
        if !self.earlier_groups_intact(earlier) {
            return Ok(());
        }

        let Some(new) = group.iter().find(|w| w.id == id).cloned() else {
            return Ok(());
        };
        if self.is_maximized(&new) {
            return Ok(());
        }

        if group.len() == 1 {
            return self.maximize_if_needed(&new);
        }

        let maxed: Vec<u64> = group
            .iter()
            .filter(|w| self.is_maximized(w))
            .map(|w| w.id)
            .collect();

        if group.len() == 2 {
            if let [incumbent] = maxed[..] {
                println!("cleave: collapsing window {incumbent} to fit {id}");
                self.toggle_maximize(incumbent)?;
            }
            return Ok(());
        }

        if !maxed.is_empty() {
            return Ok(());
        }

        let mut other_columns: HashMap<usize, usize> = HashMap::new();
        for w in group.iter().filter(|w| w.id != id) {
            if let Some((col, _)) = w.layout.pos_in_scrolling_layout {
                *other_columns.entry(col).or_default() += 1;
            }
        }

        let new_col = match new.layout.pos_in_scrolling_layout {
            Some((col, _)) if !other_columns.contains_key(&col) => col,
            _ => return Ok(()),
        };

        let target = other_columns
            .iter()
            .filter(|&(_, &n)| n == 1)
            .map(|(&col, _)| col)
            .min_by_key(|&col| (col.abs_diff(new_col), std::cmp::Reverse(col)));

        match target {
            Some(target) if target == new_col + 1 => {
                println!("cleave: stacking window {id} into column {target}");
                self.action(Action::ConsumeOrExpelWindowRight { id: Some(id) })?;
            }
            Some(target) if target + 1 == new_col => {
                println!("cleave: stacking window {id} into column {target}");
                self.action(Action::ConsumeOrExpelWindowLeft { id: Some(id) })?;
            }
            Some(target) => {
                println!("cleave: stacking window {id} into column {target}");
                let refocus = self.focused_id().filter(|&f| f != id);
                if refocus.is_some() {
                    self.action(Action::FocusWindow { id })?;
                }
                if new_col < target {
                    self.action(Action::MoveColumnToIndex { index: target - 1 })?;
                    self.action(Action::ConsumeOrExpelWindowRight { id: Some(id) })?;
                } else {
                    self.action(Action::MoveColumnToIndex { index: target + 1 })?;
                    self.action(Action::ConsumeOrExpelWindowLeft { id: Some(id) })?;
                }
                if let Some(f) = refocus {
                    self.action(Action::FocusWindow { id: f })?;
                }
            }
            None => {
                let last = earlier.len() / 2 + other_columns.len() + 1;
                if new_col < last {
                    println!("cleave: moving window {id} to the end of the row");
                    let refocus = self.focused_id().filter(|&f| f != id);
                    if refocus.is_some() {
                        self.action(Action::FocusWindow { id })?;
                    }
                    self.action(Action::MoveColumnToIndex { index: last })?;
                    if let Some(f) = refocus {
                        self.action(Action::FocusWindow { id: f })?;
                    }
                }
            }
        }

        let total_columns = other_columns.len() + usize::from(target.is_none());
        if total_columns >= 3 {
            let ids: Vec<u64> = group.iter().map(|w| w.id).collect();
            self.set_equal_shares(ws, &ids, total_columns)?;

            self.pending_snap = Some(PendingSnap {
                ws,
                leftmost: group[0].id,
                ids,
                share: 1.0 / total_columns as f64,
                expires: Instant::now() + Duration::from_secs(1),
            });
        }
        Ok(())
    }

    fn check_pending_snap(&mut self) -> io::Result<()> {
        let Some(pending) = self.pending_snap.clone() else {
            return Ok(());
        };
        if Instant::now() > pending.expires {
            self.pending_snap = None;
            return Ok(());
        }
        if !pending.ids.iter().all(|id| self.windows().contains_key(id)) {
            self.pending_snap = None;
            return Ok(());
        }

        let settled = pending.ids.iter().all(|id| {
            self.windows()
                .get(id)
                .and_then(|w| self.width_fraction(w))
                .is_some_and(|f| (f - pending.share).abs() < WIDTH_TOLERANCE)
        });
        let focus = self
            .windows()
            .values()
            .find(|w| w.is_focused && w.workspace_id == Some(pending.ws) && !w.is_floating)
            .map(|w| w.id);

        if settled && let Some(focus) = focus {
            self.pending_snap = None;
            println!("cleave: snapping view to the start of the screenful");
            self.action(Action::FocusWindow { id: pending.leftmost })?;
            if focus != pending.leftmost {
                self.action(Action::FocusWindow { id: focus })?;
            }
        }
        Ok(())
    }

    fn plan(&self, event: &Event) -> Option<Trigger> {
        match event {
            Event::WorkspacesChanged { .. } => Some(Trigger::OutputsMaybeChanged),

            Event::WindowOpenedOrChanged { window } if !window.is_floating => {
                match self.windows().get(&window.id) {
                    None => Some(Trigger::Arrived {
                        id: window.id,
                        ws: window.workspace_id?,
                    }),
                    Some(prev) if prev.workspace_id != window.workspace_id => {
                        Some(Trigger::Moved {
                            id: window.id,
                            to: window.workspace_id,
                            from: prev.workspace_id,
                        })
                    }
                    _ => None,
                }
            }

            Event::WindowClosed { id } => Some(Trigger::Closed {
                ws: self.windows().get(id).and_then(|w| w.workspace_id),
            }),

            _ => None,
        }
    }
}

fn main() -> io::Result<()> {
    let args = parse_args();

    let mut events = Socket::connect()?;
    if let Err(err) = events.send(Request::EventStream)? {
        eprintln!("cleave: niri refused the event stream: {err}");
        exit(1);
    }
    let mut next_event = events.read_events();

    let mut app = Cleave {
        state: EventStreamState::default(),
        monitors: HashMap::new(),
        actions: Socket::connect()?,
        max_tiles: args.max_tiles,
        pending_snap: None,
    };
    app.refresh_monitors()?;

    println!(
        "cleave: tiling up to {} windows per workspace, re-tiling moved windows: {}, monitors: {:?}",
        app.max_tiles,
        if args.apply_on_move { "on" } else { "off" },
        app.monitors.keys().collect::<Vec<_>>(),
    );

    loop {
        let event = next_event()?;

        let trigger = app.plan(&event);
        let _ = app.state.apply(event);

        match trigger {
            Some(Trigger::Arrived { id, ws }) => app.settle_arrival(id, ws)?,

            Some(Trigger::Moved { id, to, from }) if args.apply_on_move => {
                if let Some(ws) = to {
                    app.settle_arrival(id, ws)?;
                }
                if let Some(ws) = from {
                    app.settle_departure(ws)?;
                }
            }
            Some(Trigger::Moved { .. }) => {}

            Some(Trigger::Closed { ws: Some(ws) }) => app.settle_departure(ws)?,
            Some(Trigger::Closed { ws: None }) => {}

            Some(Trigger::OutputsMaybeChanged) => app.refresh_monitors()?,

            None => {}
        }

        app.check_pending_snap()?;
    }
}
