# Cleave

A tiny tool for [niri](https://github.com/niri-wm/niri). 
Uses [niri IPC](https://github.com/niri-wm/niri/wiki/IPC) to track and send actions.

If a workspace has a single tiled window, it takes the full width of the screen. The moment a second window opens, the screen splits between them, and any additional windows will be added to the right like usual. Close one of the two, and the remaining window takes back the full screen.


## Install
 
Requires niri 25.08+ and a Rust toolchain.
 
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
spawn-at-startup "/path/to/cleave"
```

## Credit
 
Inspired by [niri_tile_to_n.py](https://github.com/heyoeyo/niri_tweaks), which does this and more in Python.
