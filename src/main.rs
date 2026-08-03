// pyroclear — terminal goes up in flames, then clears.
//
// Doom-fire algorithm: a heat grid (0..=36) where the bottom row is
// the fire source. Each frame, every cell pulls its new value from
// the cell below (with a bit of horizontal drift), minus random
// decay, so heat rises and cools. Heat → color via a palette.
// Whole frame is redrawn each tick via ANSI cursor-home, so no
// flicker and no need for a TUI crate.
//
// Terminal size is read via ioctl(TIOCGWINSZ) every frame (cheap
// syscall, no subprocess spawn), so the grid follows you live if
// you resize mid-burn. Randomness is a tiny xorshift64* PRNG seeded
// off the clock — no `rand` crate needed.
//
// COLOR SELECTION
//   pyroclear --color ice|toxic|purple|mono|plasma|sunset|ocean|lava|fire
//   pyroclear --from "#1a0000" --to "#ffcc00"    (custom gradient)
//   pyroclear --list-colors                      (colored swatches)
//   pyroclear --pick / -p                        (interactive TUI picker)
// Whatever you pick is written to ~/.config/pyroclear/config.toml
// and reused automatically on future runs until you change it again.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ESC: &str = "\x1b";
const MAX_HEAT: u8 = 36;

// Sim/render tuning
const FRAME_DELAY: Duration = Duration::from_millis(16); // ~60fps
const STEPS_PER_FRAME: u32 = 2; // extra propagation steps per frame
const MAX_DURATION: Duration = Duration::from_millis(2200);
const SOURCE_COOL_START: f32 = 0.38;
const DIE_OUT_THRESHOLD: u8 = 2;

// ---------------------------------------------------------------------
// Palettes
// ---------------------------------------------------------------------

type Palette = [(u8, u8, u8); 37];

// Punchy rework of the classic Doom fire ramp. The original values
// (from Fabian Sanglard's write-up) are intentionally muted — these
// push saturation in the red-to-orange band for a fiercer look.
const FIRE_PALETTE: Palette = [
    (0x08, 0x00, 0x00), (0x28, 0x02, 0x00), (0x3E, 0x08, 0x00), (0x56, 0x0A, 0x00),
    (0x6E, 0x0C, 0x00), (0x88, 0x10, 0x00), (0x9C, 0x14, 0x00), (0xB2, 0x1A, 0x00),
    (0xC4, 0x20, 0x00), (0xD4, 0x2C, 0x00), (0xE0, 0x38, 0x00), (0xE8, 0x44, 0x00),
    (0xF0, 0x50, 0x00), (0xF4, 0x5A, 0x00), (0xF6, 0x62, 0x00), (0xF6, 0x6A, 0x02),
    (0xF4, 0x72, 0x04), (0xF2, 0x7C, 0x06), (0xEE, 0x86, 0x08), (0xEA, 0x90, 0x0A),
    (0xE6, 0x9A, 0x0C), (0xE2, 0xA4, 0x0E), (0xDC, 0xAE, 0x10), (0xD8, 0xB8, 0x14),
    (0xD4, 0xC0, 0x18), (0xD0, 0xC8, 0x1C), (0xCC, 0xD0, 0x20), (0xD4, 0xD6, 0x38),
    (0xDC, 0xDC, 0x54), (0xE4, 0xE4, 0x74), (0xEA, 0xEA, 0x94), (0xF0, 0xF0, 0xB0),
    (0xF4, 0xF4, 0xC8), (0xF8, 0xF6, 0xDC), (0xFA, 0xFA, 0xEC), (0xFC, 0xFC, 0xF6),
    (0xFF, 0xFF, 0xFF),
];

// All named palettes: (id, display-name, description, from-hex, to-hex).
// "fire" is handled specially (uses FIRE_PALETTE above).
const NAMED_PALETTES: &[(&str, &str, &str, &str, &str)] = &[
    ("fire",   "Fire",   "classic ember red → orange → white",  "",         ""        ),
    ("ice",    "Ice",    "deep cold → electric cyan → white",   "#030a10",  "#c8ffff" ),
    ("toxic",  "Toxic",  "void black → acid lime → pale",       "#020800",  "#bbff44" ),
    ("purple", "Purple", "dark void → violet → hot lavender",   "#060010",  "#e060ff" ),
    ("plasma", "Plasma", "deep violet → magenta → white hot",   "#0a0018",  "#ff50ff" ),
    ("sunset", "Sunset", "midnight blue → crimson → gold",      "#040010",  "#ffaa00" ),
    ("ocean",  "Ocean",  "abyssal black → ocean blue → foam",   "#000810",  "#00e8ff" ),
    ("lava",   "Lava",   "basalt black → blood red → neon orange","#0a0000", "#ff4400" ),
    ("mono",   "Mono",   "black → dim grey → pure white",       "#080808",  "#ffffff" ),
];



// ---------------------------------------------------------------------
// Help & color list (colorized)
// ---------------------------------------------------------------------

fn print_banner() {
    // A short fire-colored ASCII logo rendered with real ANSI RGB.
    let lines = [
        "  ██████╗ ██╗   ██╗██████╗  ██████╗  ██████╗██╗     ███████╗ █████╗ ██████╗ ",
        "  ██╔══██╗╚██╗ ██╔╝██╔══██╗██╔═══██╗██╔════╝██║     ██╔════╝██╔══██╗██╔══██╗",
        "  ██████╔╝ ╚████╔╝ ██████╔╝██║   ██║██║     ██║     █████╗  ███████║██████╔╝",
        "  ██╔═══╝   ╚██╔╝  ██╔══██╗██║   ██║██║     ██║     ██╔══╝  ██╔══██║██╔══██╗",
        "  ██║        ██║   ██║  ██║╚██████╔╝╚██████╗███████╗███████╗██║  ██║██║  ██║",
        "  ╚═╝        ╚═╝   ╚═╝  ╚═╝ ╚═════╝  ╚═════╝╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝",
    ];
    // Gradient from deep red at top to bright orange at bottom
    let colors: [(u8, u8, u8); 6] = [
        (180, 10, 0), (210, 30, 0), (230, 60, 0),
        (240, 100, 0), (250, 160, 10), (255, 200, 30),
    ];
    for (line, &(r, g, b)) in lines.iter().zip(colors.iter()) {
        println!("{ESC}[38;2;{r};{g};{b}m{line}{ESC}[0m");
    }
    println!();
}

fn print_help() {
    print_banner();
    println!("{ESC}[1;37mUSAGE{ESC}[0m");
    println!("    pyroclear [OPTIONS]\n");
    println!("{ESC}[1;37mOPTIONS{ESC}[0m");
    println!("    {ESC}[38;2;255;160;40m--color{ESC}[0m {ESC}[38;2;200;200;200m<name>{ESC}[0m          Named palette (see --list-colors)");
    println!("    {ESC}[38;2;255;160;40m--pick{ESC}[0m, {ESC}[38;2;255;160;40m-p{ESC}[0m              Interactive TUI color picker");
    println!("    {ESC}[38;2;255;160;40m--from{ESC}[0m {ESC}[38;2;200;200;200m<hex>{ESC}[0m {ESC}[38;2;255;160;40m--to{ESC}[0m {ESC}[38;2;200;200;200m<hex>{ESC}[0m  Custom gradient, e.g. --from \"#1a0000\" --to \"#ffcc00\"");
    println!("    {ESC}[38;2;255;160;40m--list-colors{ESC}[0m           List palettes with live color swatches");
    println!("    {ESC}[38;2;255;160;40m-h{ESC}[0m, {ESC}[38;2;255;160;40m--help{ESC}[0m             Show this help\n");
    println!("{ESC}[38;2;150;150;150mYour choice is saved to ~/.config/pyroclear/config.toml{ESC}[0m");
    println!("{ESC}[38;2;150;150;150mand reused automatically next time you run pyroclear with no flags.{ESC}[0m");
}

/// Render a gradient swatch of `width` cells between two colors.
fn swatch(from: (u8, u8, u8), to: (u8, u8, u8), width: usize) -> String {
    let mut s = String::new();
    for i in 0..width {
        let t = i as f32 / (width - 1) as f32;
        let r = (from.0 as f32 + t * (to.0 as f32 - from.0 as f32)).round() as u8;
        let g = (from.1 as f32 + t * (to.1 as f32 - from.1 as f32)).round() as u8;
        let b = (from.2 as f32 + t * (to.2 as f32 - from.2 as f32)).round() as u8;
        s.push_str(&format!("{ESC}[48;2;{r};{g};{b}m "));
    }
    s.push_str(&format!("{ESC}[0m"));
    s
}

fn palette_swatch(palette: &Palette, width: usize) -> String {
    let mut s = String::new();
    let step = 36.0 / (width - 1) as f32;
    for i in 0..width {
        let idx = (i as f32 * step).round().clamp(1.0, 36.0) as usize;
        let (r, g, b) = palette[idx];
        s.push_str(&format!("{ESC}[48;2;{r};{g};{b}m "));
    }
    s.push_str(&format!("{ESC}[0m"));
    s
}

fn print_color_list() {
    print_banner();
    println!("{ESC}[1;37mAvailable palettes:{ESC}[0m\n");

    for (id, display, desc, from_hex, to_hex) in NAMED_PALETTES {
        let sw = if *id == "fire" {
            let p = boost_saturation(&FIRE_PALETTE, 1.65);
            palette_swatch(&p, 28)
        } else {
            let from = hex_to_rgb(from_hex).unwrap_or((0, 0, 0));
            let to = hex_to_rgb(to_hex).unwrap_or((255, 255, 255));
            // Build the full palette so the swatch matches what actually burns
            let p = boost_saturation(&generate_palette(from, to), 1.65);
            palette_swatch(&p, 28)
        };
        println!(
            "  {ESC}[1;38;2;255;200;80m{id:<8}{ESC}[0m  {sw}  {ESC}[38;2;180;180;180m{desc}{ESC}[0m"
        );
        let _ = display; // used in picker, suppress unused warning
    }
    println!("\n{ESC}[38;2;150;150;150mCustom gradient: pyroclear --from \"#rrggbb\" --to \"#rrggbb\"{ESC}[0m");
    println!("{ESC}[38;2;150;150;150mInteractive:     pyroclear --pick{ESC}[0m");
}

// ---------------------------------------------------------------------
// Color math: hex <-> rgb <-> hsv, gradient generator
// ---------------------------------------------------------------------

fn hex_to_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some((r, g, b))
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;

    let h = if delta == 0.0 {
        0.0
    } else if max == rf {
        60.0 * (((gf - bf) / delta).rem_euclid(6.0))
    } else if max == gf {
        60.0 * (((bf - rf) / delta) + 2.0)
    } else {
        60.0 * (((rf - gf) / delta) + 4.0)
    };

    let s = if max == 0.0 { 0.0 } else { delta / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let hh = (h / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (hh.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hh as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn lerp_hue(a: f32, b: f32, t: f32) -> f32 {
    let mut d = b - a;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    (a + d * t).rem_euclid(360.0)
}

/// Build a 37-step ramp between two colors, interpolated in HSV.
fn generate_palette(from: (u8, u8, u8), to: (u8, u8, u8)) -> Palette {
    let (mut h0, mut s0, v0) = rgb_to_hsv(from.0, from.1, from.2);
    let (h1, s1, v1) = rgb_to_hsv(to.0, to.1, to.2);
    if v0 < 0.15 {
        h0 = h1;
        s0 = s1 * 0.7;
    }
    let v0 = v0.max(0.08);

    let mut out: Palette = [(0, 0, 0); 37];
    for (i, slot) in out.iter_mut().enumerate() {
        let t = i as f32 / 36.0;
        let h = lerp_hue(h0, h1, t);
        let s = (s0 + t * (s1 - s0)).clamp(0.0, 1.0);
        let v = (v0 + t * (v1 - v0)).clamp(0.0, 1.0);
        *slot = hsv_to_rgb(h, s, v);
    }
    out
}

/// Boost saturation toward 1.0 by `factor`. Applied as a final pass
/// to every palette to ensure vivid, not pastel, colors.
fn boost_saturation(palette: &Palette, factor: f32) -> Palette {
    let mut out = *palette;
    for slot in out.iter_mut() {
        let (h, s, v) = rgb_to_hsv(slot.0, slot.1, slot.2);
        *slot = hsv_to_rgb(h, (s * factor).clamp(0.0, 1.0), v);
    }
    out
}

// ---------------------------------------------------------------------
// Palette selection: CLI args -> saved config -> default
// ---------------------------------------------------------------------

enum PaletteChoice {
    Named(String),
    Custom { from: (u8, u8, u8), to: (u8, u8, u8) },
}

fn config_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/pyroclear/config.toml"))
}

fn load_config() -> Option<PaletteChoice> {
    let path = config_path()?;
    let content = std::fs::read_to_string(path).ok()?;

    let mut palette = None;
    let mut from = None;
    let mut to = None;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('[') || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let val = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "palette" => palette = Some(val),
                "from" => from = Some(val),
                "to" => to = Some(val),
                _ => {}
            }
        }
    }

    if let (Some(f), Some(t)) = (from, to) {
        if let (Some(fc), Some(tc)) = (hex_to_rgb(&f), hex_to_rgb(&t)) {
            return Some(PaletteChoice::Custom { from: fc, to: tc });
        }
    }
    palette.map(PaletteChoice::Named)
}

fn save_config(choice: &PaletteChoice) {
    let Some(path) = config_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = match choice {
        PaletteChoice::Named(name) => format!("[color]\npalette = \"{name}\"\n"),
        PaletteChoice::Custom { from, to } => format!(
            "[color]\nfrom = \"#{:02x}{:02x}{:02x}\"\nto = \"#{:02x}{:02x}{:02x}\"\n",
            from.0, from.1, from.2, to.0, to.1, to.2
        ),
    };
    let _ = std::fs::write(path, content);
}

fn choice_display_name(choice: &PaletteChoice) -> String {
    match choice {
        PaletteChoice::Named(n) => n.clone(),
        PaletteChoice::Custom { from, to } => format!(
            "#{:02x}{:02x}{:02x}→#{:02x}{:02x}{:02x}",
            from.0, from.1, from.2, to.0, to.1, to.2
        ),
    }
}

fn validate_named(name: &str) -> Result<(), String> {
    if NAMED_PALETTES.iter().any(|(id, _, _, _, _)| *id == name) {
        Ok(())
    } else {
        let valid: Vec<&str> = NAMED_PALETTES.iter().map(|(id, _, _, _, _)| *id).collect();
        Err(format!(
            "Unknown palette '{name}'. Valid names: {}\nTry --list-colors or --pick for an interactive selector.",
            valid.join(", ")
        ))
    }
}

/// Parses CLI flags. Returns Some(choice) if the user passed color flags.
fn parse_args() -> Option<PaletteChoice> {
    let args: Vec<String> = std::env::args().collect();
    let mut color = None;
    let mut from = None;
    let mut to = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--color" | "-c" => {
                i += 1;
                color = args.get(i).cloned();
            }
            "--from" => {
                i += 1;
                from = args.get(i).cloned();
            }
            "--to" => {
                i += 1;
                to = args.get(i).cloned();
            }
            "--list-colors" | "--list" => {
                print_color_list();
                std::process::exit(0);
            }
            "--pick" | "-p" => {
                let picked = interactive_pick();
                match picked {
                    Some(c) => {
                        save_config(&c);
                        return Some(c);
                    }
                    None => std::process::exit(0),
                }
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    if from.is_some() || to.is_some() {
        let (Some(f), Some(t)) = (from, to) else {
            eprintln!("{ESC}[1;31merror:{ESC}[0m --from and --to must be used together, e.g.\n  pyroclear --from \"#1a0000\" --to \"#ffcc00\"");
            std::process::exit(1);
        };
        let (Some(fc), Some(tc)) = (hex_to_rgb(&f), hex_to_rgb(&t)) else {
            eprintln!("{ESC}[1;31merror:{ESC}[0m Invalid hex color(s) — expected format #rrggbb");
            std::process::exit(1);
        };
        return Some(PaletteChoice::Custom { from: fc, to: tc });
    }

    if let Some(name) = color {
        if let Err(e) = validate_named(&name) {
            eprintln!("{ESC}[1;31merror:{ESC}[0m {e}");
            std::process::exit(1);
        }
        return Some(PaletteChoice::Named(name));
    }

    None
}

fn resolve_choice() -> PaletteChoice {
    if let Some(choice) = parse_args() {
        save_config(&choice);
        return choice;
    }
    load_config().unwrap_or(PaletteChoice::Named("fire".to_string()))
}

fn build_palette(choice: &PaletteChoice) -> Palette {
    const VIVID_FACTOR: f32 = 1.65;

    let raw = match choice {
        PaletteChoice::Named(name) => match name.as_str() {
            "fire" => FIRE_PALETTE,
            other => {
                if let Some((_, _, _, from_hex, to_hex)) =
                    NAMED_PALETTES.iter().find(|(id, _, _, _, _)| *id == other)
                {
                    let from = hex_to_rgb(from_hex).unwrap_or((0, 0, 0));
                    let to = hex_to_rgb(to_hex).unwrap_or((255, 255, 255));
                    generate_palette(from, to)
                } else {
                    // Should have been caught by validate_named, but be safe
                    eprintln!("{ESC}[1;31merror:{ESC}[0m Unknown palette '{other}', using fire.");
                    FIRE_PALETTE
                }
            }
        },
        PaletteChoice::Custom { from, to } => generate_palette(*from, *to),
    };

    boost_saturation(&raw, VIVID_FACTOR)
}

// ---------------------------------------------------------------------
// Interactive TUI color picker (curses-style, pure libc + ANSI)
// ---------------------------------------------------------------------

/// RAII guard: enters raw mode on creation, restores termios on drop.
/// Also switches to the alternate screen buffer.
struct TermRawGuard {
    orig: libc::termios,
}

impl TermRawGuard {
    fn enter() -> io::Result<Self> {
        let mut orig: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut orig) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = orig;
        // Disable canonical mode, echo, signal chars
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
        // Read returns after 1 byte, no timeout
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) };

        // Enter alternate screen, hide cursor
        print!("{ESC}[?1049h{ESC}[?25l");
        io::stdout().flush().ok();

        Ok(Self { orig })
    }
}

impl Drop for TermRawGuard {
    fn drop(&mut self) {
        // Leave alternate screen, show cursor
        print!("{ESC}[?1049l{ESC}[?25h");
        io::stdout().flush().ok();
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.orig) };
    }
}

/// Read one "key event" from stdin in raw mode.
#[derive(Debug, PartialEq)]
enum Key {
    Up,
    Down,
    Enter,
    Char(char),
    Esc,
    Backspace,
    Other,
}

fn read_key() -> Key {
    let mut buf = [0u8; 4];
    let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut _, 4) };
    if n <= 0 {
        return Key::Other;
    }
    match &buf[..n as usize] {
        [0x1b, b'[', b'A', ..] => Key::Up,
        [0x1b, b'[', b'B', ..] => Key::Down,
        [0x1b, ..] if n == 1 => Key::Esc,
        [0x0d] | [0x0a] => Key::Enter,
        [0x7f] | [0x08] => Key::Backspace,
        [c] if *c >= 0x20 && *c < 0x7f => Key::Char(*c as char),
        _ => Key::Other,
    }
}

/// Prompt the user for a hex string in raw mode (with echo).
fn prompt_hex(label: &str, row: u16) -> Option<String> {
    let mut input = String::new();
    loop {
        // Clear row and redraw prompt
        print!("{ESC}[{row};1H{ESC}[2K{ESC}[38;2;255;200;80m{label}{ESC}[0m {input}_");
        io::stdout().flush().ok();
        match read_key() {
            Key::Enter => {
                let s = input.trim().to_string();
                if s.is_empty() {
                    return None;
                }
                if hex_to_rgb(&s).is_some() {
                    return Some(s);
                }
                // Invalid — flash an error hint
                print!("{ESC}[{row};1H{ESC}[2K{ESC}[38;2;255;60;60m  ✗ invalid hex (need #rrggbb){ESC}[0m");
                io::stdout().flush().ok();
                std::thread::sleep(Duration::from_millis(800));
                input.clear();
            }
            Key::Backspace => {
                input.pop();
            }
            Key::Char(c) => {
                if input.len() < 7 {
                    input.push(c);
                }
            }
            Key::Esc => return None,
            _ => {}
        }
    }
}

/// Draw the picker UI at the current terminal size.
fn draw_picker(selected: usize, (cols, rows): (usize, usize)) {
    let total = NAMED_PALETTES.len() + 1; // +1 for "Custom"
    let swatch_w = (cols.saturating_sub(30)).clamp(12, 36);

    // Title bar
    print!("{ESC}[H");
    let title = " pyroclear — color picker ";
    let hint = " [↑↓] navigate  [Enter] select  [q/Esc] cancel ";
    let gap = cols.saturating_sub(title.len() + hint.len());
    print!(
        "{ESC}[48;2;30;30;50m{ESC}[38;2;255;200;80m{title}{ESC}[38;2;140;140;160m{}{hint}{ESC}[0m",
        " ".repeat(gap)
    );

    // Palette rows
    let list_start = 2usize;
    let visible = rows.saturating_sub(list_start + 2); // leave bottom margin
    let offset = if selected >= visible { selected - visible + 1 } else { 0 };

    for slot in 0..visible {
        let idx = slot + offset;
        let row = (list_start + slot + 1) as u16;
        print!("{ESC}[{row};1H{ESC}[2K");

        if idx >= total {
            break;
        }

        let is_selected = idx == selected;
        let cursor = if is_selected { "▸" } else { " " };

        if is_selected {
            print!("{ESC}[48;2;20;25;40m");
        }

        let (name, desc, sw) = if idx < NAMED_PALETTES.len() {
            let (id, display, desc, from_hex, to_hex) = NAMED_PALETTES[idx];
            let p = if id == "fire" {
                boost_saturation(&FIRE_PALETTE, 1.65)
            } else {
                let from = hex_to_rgb(from_hex).unwrap_or((0, 0, 0));
                let to = hex_to_rgb(to_hex).unwrap_or((255, 255, 255));
                boost_saturation(&generate_palette(from, to), 1.65)
            };
            (display, desc, palette_swatch(&p, swatch_w))
        } else {
            // "Custom" entry
            let sw = swatch((60, 0, 60), (255, 200, 100), swatch_w);
            ("Custom", "enter your own #rrggbb gradient", sw)
        };

        if is_selected {
            print!("{ESC}[1;38;2;255;220;80m");
        } else {
            print!("{ESC}[38;2;200;200;210m");
        }

        print!(
            "  {cursor} {name:<10}  {sw}  {ESC}[38;2;140;140;160m{desc}{ESC}[0m"
        );

        if is_selected {
            // Fill rest of line with selection background
            print!("{ESC}[48;2;20;25;40m{}{ESC}[0m", " ".repeat(4));
        }
    }

    // Bottom separator
    let sep_row = (list_start + visible + 1) as u16;
    print!("{ESC}[{sep_row};1H{ESC}[2K{ESC}[38;2;60;60;80m{}{ESC}[0m", "─".repeat(cols));
    io::stdout().flush().ok();
}

/// Run the interactive palette picker. Returns the chosen PaletteChoice
/// or None if the user cancelled.
fn interactive_pick() -> Option<PaletteChoice> {
    let _guard = TermRawGuard::enter().ok()?;

    let total = NAMED_PALETTES.len() + 1; // +1 for custom
    let mut selected = 0usize;

    loop {
        let size = terminal_size();
        draw_picker(selected, size);

        match read_key() {
            Key::Up => {
                if selected > 0 {
                    selected -= 1;
                }
            }
            Key::Down => {
                if selected + 1 < total {
                    selected += 1;
                }
            }
            Key::Enter => {
                if selected < NAMED_PALETTES.len() {
                    let (id, _, _, _, _) = NAMED_PALETTES[selected];
                    return Some(PaletteChoice::Named(id.to_string()));
                } else {
                    // Custom hex input — clear area and prompt
                    let (_, rows) = terminal_size();
                    let base = rows as u16 - 4;
                    print!("{ESC}[{base};1H{ESC}[J");
                    print!("{ESC}[{base};1H{ESC}[38;2;180;180;200m  Enter hex colors (e.g. #ff0000){ESC}[0m\n");
                    io::stdout().flush().ok();

                    let from_str = prompt_hex("  From:", base + 2)?;
                    let to_str = prompt_hex("  To:  ", base + 3)?;

                    let from = hex_to_rgb(&from_str)?;
                    let to = hex_to_rgb(&to_str)?;
                    return Some(PaletteChoice::Custom { from, to });
                }
            }
            Key::Esc | Key::Char('q') => return None,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------
// PRNG
// ---------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as u64
            | 1;
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        let span = (hi - lo + 1) as u64;
        lo + (self.next_u64() % span) as i32
    }
}

// ---------------------------------------------------------------------
// Terminal + rendering
// ---------------------------------------------------------------------

fn terminal_size() -> (usize, usize) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_col > 0
            && ws.ws_row > 0
        {
            return (ws.ws_col as usize, ws.ws_row as usize);
        }
    }
    (80, 24)
}

#[derive(PartialEq, Clone, Copy)]
enum CellColor {
    Default,
    Rgb(u8, u8, u8),
}

fn render(buf: &mut String, grid: &[u8], cols: usize, rows: usize, palette: &Palette, label: &str) {
    buf.clear();
    buf.push_str(ESC);
    buf.push_str("[H");

    let mut last: Option<CellColor> = None;
    for y in 0..rows {
        for x in 0..cols {
            let heat = grid[y * cols + x];
            let color = if heat == 0 {
                CellColor::Default
            } else {
                let (r, g, b) = palette[heat as usize];
                CellColor::Rgb(r, g, b)
            };

            if last != Some(color) {
                match color {
                    CellColor::Default => buf.push_str(&format!("{ESC}[49m")),
                    CellColor::Rgb(r, g, b) => {
                        buf.push_str(&format!("{ESC}[48;2;{r};{g};{b}m"))
                    }
                }
                last = Some(color);
            }
            buf.push(' ');
        }
        buf.push('\n');
    }

    // Palette label: bottom-right corner, faint
    if !label.is_empty() && cols > label.len() + 2 {
        let col = cols - label.len() - 1;
        buf.push_str(&format!(
            "{ESC}[{rows};{col}H{ESC}[0m{ESC}[38;2;80;80;80m {label} {ESC}[0m"
        ));
    }
}

fn resize_grid(cols: usize, rows: usize) -> Vec<u8> {
    let mut grid = vec![0u8; cols * rows];
    for x in 0..cols {
        grid[(rows - 1) * cols + x] = MAX_HEAT;
    }
    grid
}

fn burn(palette: &Palette, label: &str, interrupted: Arc<AtomicBool>) {
    let (mut cols, mut rows) = terminal_size();
    let mut grid = resize_grid(cols, rows);
    let mut rng = Rng::new();

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = write!(out, "{ESC}[?25l{ESC}[2J"); // hide cursor, clear screen

    let start = Instant::now();
    let source_cool_at = MAX_DURATION.mul_f32(SOURCE_COOL_START);
    let mut frame = String::with_capacity(cols * rows * 8);

    loop {
        if interrupted.load(Ordering::Relaxed) {
            break;
        }
        let elapsed = start.elapsed();
        if elapsed > MAX_DURATION {
            break;
        }

        // live resize
        let (new_cols, new_rows) = terminal_size();
        if new_cols != cols || new_rows != rows {
            cols = new_cols;
            rows = new_rows;
            grid = resize_grid(cols, rows);
            frame.reserve(cols * rows * 8);
        }

        for _ in 0..STEPS_PER_FRAME {
            for x in 0..cols {
                for y in 1..rows {
                    let below = grid[y * cols + x];
                    let decay = rng.range(0, 3);
                    let drift = rng.range(-1, 1);
                    let nx = (x as i32 + drift).clamp(0, cols as i32 - 1) as usize;
                    let new_val = (below as i32 - decay).max(0) as u8;
                    grid[(y - 1) * cols + nx] = new_val;
                }
            }

            if elapsed > source_cool_at {
                for x in 0..cols {
                    let idx = (rows - 1) * cols + x;
                    let dec = rng.range(2, 6);
                    grid[idx] = (grid[idx] as i32 - dec).max(0) as u8;
                }
            }
        }

        render(&mut frame, &grid, cols, rows, palette, label);
        let _ = out.write_all(frame.as_bytes());
        let _ = out.flush();

        if elapsed > source_cool_at {
            let peak = grid.iter().copied().max().unwrap_or(0);
            if peak < DIE_OUT_THRESHOLD {
                break;
            }
        }

        std::thread::sleep(FRAME_DELAY);
    }

    let _ = write!(out, "{ESC}[?25h"); // always restore cursor
}

// ---------------------------------------------------------------------
// SIGINT handler
// ---------------------------------------------------------------------

/// Shared flag set by the C-level signal handler; polled by a watcher
/// thread that bridges it into an Arc<AtomicBool> the burn loop checks.
static SIGINT_FIRED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigint_handler(_: libc::c_int) {
    SIGINT_FIRED.store(true, Ordering::Relaxed);
}

/// Install SIGINT → set flag. Returns an Arc the burn loop polls each frame.
fn install_sigint() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let flag2 = Arc::clone(&flag);

    unsafe {
        libc::signal(libc::SIGINT, sigint_handler as *const () as libc::sighandler_t);
    }

    std::thread::spawn(move || loop {
        if SIGINT_FIRED.load(Ordering::Relaxed) {
            flag2.store(true, Ordering::Relaxed);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    });

    flag
}

// ---------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------

fn main() {
    let choice = resolve_choice();
    let label = choice_display_name(&choice);
    let palette = build_palette(&choice);

    let interrupted = install_sigint();

    burn(&palette, &label, interrupted);

    // Final clear — always runs, even after SIGINT (cursor was restored in burn)
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = write!(out, "{ESC}[H{ESC}[2J");
    let _ = out.flush();
}
