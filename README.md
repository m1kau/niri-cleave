# Cleave

A tiny auto-tiler for [niri](https://github.com/niri-wm/niri).
Uses [niri IPC](https://github.com/niri-wm/niri/wiki/IPC) to track and send actions.

Cleave watches events per workspace and re-tiles automatically:

```
1 window                               2 windows              
┌──────────────────────────────────┐   ┌────────────────┐┌────────────────┐
│                                  │   │                ││                │
│                A                 │   │       A        ││       B        │
│                                  │   │                ││                │
└──────────────────────────────────┘   └────────────────┘└────────────────┘

3 windows                              4 windows              
┌────────────────┐┌────────────────┐   ┌────────────────┐┌────────────────┐
│       A        ││       B        │   │       A        ││       C        │
│                │├────────────────┤   ├────────────────┤├────────────────┤
│                ││       C        │   │       B        ││       D        │
└────────────────┘└────────────────┘   └────────────────┘└────────────────┘

5 windows                              6 windows              
┌──────────┐┌──────────┐┌──────────┐   ┌──────────┐┌──────────┐┌──────────┐
│    A     ││    C     ││    E     │   │    A     ││    C     ││    E     │
├──────────┤├──────────┤│          │   ├──────────┤├──────────┤├──────────┤
│    B     ││    D     ││          │   │    B     ││    D     ││    F     │
└──────────┘└──────────┘└──────────┘   └──────────┘└──────────┘└──────────┘
```
Cleave tiles up to `-n <N>` windows per workspace using the 1–6 pattern shown above, wrapping into additional screenfuls every 6 windows. For example, with `-n 8`: windows 1–6 fill the standard six-window layout, window 7 opens maximized to start a new screenful, and window 8 splits that screenful in two. Closing windows reverses this, collapsing back down to a single fullscreen window. Windows dragged onto a workspace from elsewhere are re-tiled the same way as windows opened directly on it.

Any window beyond the `N`th on a workspace is left alone. Cleave stops managing it, and niri falls back to its own default window placement/layout behavior for it.

## Install

Requires niri 26.04+ and a Rust toolchain.

```bash
git clone https://github.com/m1kau/niri-cleave
cd niri-cleave
cargo build --release
```

bin location `target/release/cleave`.

## Usage

Run it manually to try it out:

```bash
./cleave
```

Or start it with niri by adding it to your `config.kdl`:

```kdl
spawn-at-startup "/path/to/cleave" "-n" "6"
```

| flag            | effect                                                 |
|-----------------|--------------------------------------------------------|
| `-n <N>`        | number of windows to manage per workspace (default: 2) |
| `-M, --no-move` | don't re-tile windows moved between workspaces         |
| `-h, --help`    | show help                                              |

## Credit

Inspired by [niri_tile_to_n.py](https://github.com/heyoeyo/niri_tweaks), which does this and more in Python.
