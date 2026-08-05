# pyroclear

A terminal `clear` replacement that burns your screen down before wiping it. 

Written in modern Rust. Zero runtime dependencies beyond `libc`. Highly optimized, flicker-free, and customizable.

---

## Features

- **Live Resize Support**: Queries terminal size on the fly via direct `ioctl` syscalls — no subprocesses. The grid adjusts instantly as you resize.
- **Transparent Background**: Empty cells inherit your terminal's default theme/opacity instead of drawing solid black rectangles.
- **300+ Built-in Palettes**: Categorized beautifully in `--list-colors` with aligned swatches.
- **Interactive TUIs**:
  - **Color Picker (`--pick`)**: Browse, search, filter, and preview palettes in real-time.
  - **Settings Manager (`--settings`)**: Adjust FPS, wind/drift, and flame height in raw mode.
  - **Custom Palette Manager (`--custom`)**: Build, name, delete, and save your own hex gradients.
- **Persistent Configuration**: Settings and palettes are automatically written to `~/.config/pyroclear/config.toml`.
- **Signal-safe**: Interrupted runs (Ctrl-C) restore the terminal state and cursor cleanly.

---

## Installation

Install via Cargo:

```bash
cargo install pyroclear
```

Or build from source:

```bash
# Clone the repository
git clone https://github.com/shreyanth-sureshkrishnaa/pyroclear.git
cd pyroclear

# Build and install to ~/.cargo/bin
cargo build --release
cargo install --path .
```

### Wire it up as `clear`

**Bash / Zsh**
```bash
# Append to ~/.bashrc or ~/.zshrc
alias clear="pyroclear"
```

**Fish**
```fish
# Append to ~/.config/fish/config.fish
alias -s clear="pyroclear"
```

---

## Usage

```
pyroclear [OPTIONS]
```

### Command Modes

| Option | Description | Example |
| :--- | :--- | :--- |
| **`--start`** | Open the onboarding presentation & guide | `pyroclear --start` |
| **`--settings`, `-s`** | Adjust FPS, wind direction, and flame height decay | `pyroclear --settings` |
| **`--pick`, `-p`** | Interactive color palette picker with live swatches | `pyroclear --pick` |
| **`--custom`** | TUI to save, name, manage and run custom gradients | `pyroclear --custom` |
| **`--color <name>`** | Burn with a specific named palette (saves as default) | `pyroclear --color toxic` |
| **`--from <hex> --to <hex>`**| Burn with a one-off custom gradient | `pyroclear --from "#002080" --to "#00f0ff"` |
| **`--list-colors`** | View all 300+ palettes grouped by color family | `pyroclear --list-colors` |
| **`--info`, `-i`** | Display active palette card and configured options | `pyroclear --info` |
| **`--random`, `-r`** | Run with a random palette every time | `pyroclear --random` |
| **`--no-save`** | Run choice without saving it to configuration | `pyroclear --color ocean --no-save` |
| **`--reset`** | Reset configuration to default fire palette | `pyroclear --reset` |
| **`-h, --help`** | Show quick help screen | `pyroclear --help` |

---

## Configuration

Your preferences are saved in:
`~/.config/pyroclear/config.toml`

Custom palettes created in the TUI are stored in:
`~/.config/pyroclear/custom_palettes.toml`

---

## Performance

The physics engine runs at standard ~60 FPS (customizable) with multiple propagation steps per frame. The entire rendering buffer is flushed to stdout in a single write operation, ensuring sub-millisecond execution times even on massive high-refresh-rate displays.

---

## License

This project is licensed under the MIT License.
