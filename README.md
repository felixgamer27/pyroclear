
```
  ██████╗ ██╗   ██╗██████╗  ██████╗  ██████╗██╗     ███████╗ █████╗ ██████╗ 
  ██╔══██╗╚██╗ ██╔╝██╔══██╗██╔═══██╗██╔════╝██║     ██╔════╝██╔══██╗██╔══██╗
  ██████╔╝ ╚████╔╝ ██████╔╝██║   ██║██║     ██║     █████╗  ███████║██████╔╝
  ██╔═══╝   ╚██╔╝  ██╔══██╗██║   ██║██║     ██║     ██╔══╝  ██╔══██║██╔══██╗
  ██║        ██║   ██║  ██║╚██████╔╝╚██████╗███████╗███████╗██║  ██║██║  ██║
  ╚═╝        ╚═╝   ╚═╝  ╚═╝ ╚═════╝  ╚═════╝╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝
```

A terminal `clear` replacement that burns your screen down before wiping it — a live, physically-simulated fire animation rendered directly in your terminal, in any color you choose.

Written in Rust. Zero runtime dependencies beyond `libc`. Sixty frames a second, no flicker, no subprocess spawning, no external assets.

---

## What it does

Run it in place of `clear`. The terminal ignites from the bottom row, the fire climbs and dies out on its own, and the screen clears the instant the last ember fades — no lingering black frame, no forced background color, no wasted time.

Under the hood it's the classic Doom-fire algorithm: a heat-value grid where every cell inherits its value from the cell below it, decaying and drifting sideways as it rises. It's cheap to simulate, looks convincing, and needs nothing more exotic than a heat-to-color lookup table.

## Features

- **Live terminal-size awareness.** Reads terminal dimensions via a direct `ioctl(TIOCGWINSZ)` call every frame — no subprocess spawning — and rebuilds the simulation grid on the spot if you resize mid-burn.
- **Transparent background.** Unlit cells emit no color at all; your terminal's actual background, transparency, or theme shows through instead of a forced black rectangle.
- **Nine built-in palettes**, plus fully custom gradients.
- **Interactive color picker.** A raw-mode TUI with live swatches — no need to memorize palette names.
- **Persistent configuration.** Whatever you choose is written to disk and reused automatically on every future run.
- **Signal-safe.** Ctrl-C during the animation restores your cursor and terminal state cleanly rather than leaving your shell in a broken state.
- **No dependencies beyond `libc`.** Compiles in seconds, ships as a single static-ish binary.

## Installation

```bash
git clone <your-repo-url> pyroclear
cd pyroclear
cargo build --release
```

The binary is now at `target/release/pyroclear`. Put it somewhere on your `PATH`, or reference it directly.

```bash
mkdir -p ~/.local/bin
cp target/release/pyroclear ~/.local/bin/
```

## Wiring it in as `clear`

**bash / zsh**

```bash
# ~/.bashrc or ~/.zshrc
alias clear='pyroclear'
```

**fish**

```fish
funcsave clear <<'EOF'
function clear
    pyroclear
end
EOF
```

or, more simply:

```fish
alias --save clear 'pyroclear'
```

Reload your shell config and `clear` now burns before it clears.

## Usage

```
pyroclear [OPTIONS]

OPTIONS:
    --color <name>          Named palette (see --list-colors)
    --pick, -p              Interactive TUI color picker
    --from <hex> --to <hex> Custom gradient, e.g. --from "#1a0000" --to "#ffcc00"
    --list-colors           List palettes with live color swatches
    -h, --help              Show this help
```

### Named palettes

| Name     | Description                                    |
|----------|-------------------------------------------------|
| `fire`   | Classic ember red through orange to white       |
| `ice`    | Deep cold through electric cyan to white         |
| `toxic`  | Void black through acid lime to pale             |
| `purple` | Dark void through violet to hot lavender         |
| `plasma` | Deep violet through magenta to white hot         |
| `sunset` | Midnight blue through crimson to gold            |
| `ocean`  | Abyssal black through ocean blue to foam         |
| `lava`   | Basalt black through blood red to neon orange    |
| `mono`   | Black through dim grey to pure white             |

```bash
pyroclear --color ice
```

### Custom gradients

Any two hex colors define a full 37-step ramp, generated at runtime by interpolating in HSV space rather than raw RGB — this avoids the muddy gray midtones a naive linear blend produces, and keeps colors vivid at every heat level.

```bash
pyroclear --from "#1a0000" --to "#ffcc00"
```

### Interactive picker

```bash
pyroclear --pick
```

Opens a raw-mode terminal UI listing every palette with a live color swatch. Arrow keys to navigate, Enter to select, Esc or `q` to cancel. Selecting "Custom" prompts for a pair of hex values inline.

## Persistence

Whichever palette or gradient you select — via `--color`, `--from`/`--to`, or the interactive picker — is written to:

```
~/.config/pyroclear/config.toml
```

Every subsequent run with no color flags reads this file and reuses your last choice automatically. Pass a new flag at any time to change and re-save it.

## How it works

The simulation is a Doom-fire heat grid: the bottom row holds maximum heat and acts as the fire's source. Each frame, every cell above the source pulls its new value from the cell directly below it, minus a small random decay, with a touch of horizontal drift so the flame doesn't rise in perfectly straight columns. After a short ignition phase the source itself begins cooling, so the fire dies out naturally rather than being cut off by a fixed timer.

Heat values map to color through a 37-step palette. The whole frame is redrawn each tick using an ANSI cursor-home escape sequence rather than clearing and rebuilding the screen, which avoids flicker without needing a curses-style dependency.

Color generation and terminal I/O are entirely dependency-free aside from `libc`, which is used for direct `ioctl` terminal-size queries, raw-mode terminal control for the picker, and signal handling.

## Performance

The simulation runs at roughly sixty frames per second with two propagation steps per frame, and comfortably handles terminal widths well beyond 200 columns without dropping frames, since the entire pipeline — grid update, color lookup, and frame buffering — runs as native compiled code with no interpreter or garbage collector in the loop.

## License

Add your preferred license here.