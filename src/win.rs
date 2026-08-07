// win.rs — hand-rolled Win32 console bindings (Windows only, zero deps).
// The few pieces of the Windows console pyroclear needs: raw mode,
// terminal size, and Ctrl+C. Everything else is plain ANSI output, which
// works because virtual-terminal (VT) processing is enabled here.

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::io;

const STD_INPUT_HANDLE: u32 = (-10i32) as u32;
const STD_OUTPUT_HANDLE: u32 = (-11i32) as u32;

const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
const ENABLE_LINE_INPUT: u32 = 0x0002;
const ENABLE_ECHO_INPUT: u32 = 0x0004;
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

#[repr(C)]
struct Coord {
    x: i16,
    y: i16,
}

#[repr(C)]
struct SmallRect {
    left: i16,
    top: i16,
    right: i16,
    bottom: i16,
}

#[repr(C)]
struct ConsoleScreenBufferInfo {
    dw_size: Coord,
    dw_cursor_position: Coord,
    w_attributes: u16,
    sr_window: SmallRect,
    dw_maximum_window_size: Coord,
}

extern "system" {
    fn GetStdHandle(n_std_handle: u32) -> *mut c_void;
    fn GetConsoleMode(h_console: *mut c_void, lp_mode: *mut u32) -> i32;
    fn SetConsoleMode(h_console: *mut c_void, dw_mode: u32) -> i32;
    fn GetConsoleScreenBufferInfo(
        h_console: *mut c_void,
        lp_info: *mut ConsoleScreenBufferInfo,
    ) -> i32;
    pub fn SetConsoleCtrlHandler(handler: unsafe extern "system" fn(u32) -> i32, add: i32) -> i32;
}

// ── Raw mode ──────────────────────────────────────────────────────────

pub struct RawConsole {
    in_mode: u32,
    out_mode: u32,
    out_saved: bool,
}

pub fn enter_raw() -> io::Result<RawConsole> {
    unsafe {
        let hin = GetStdHandle(STD_INPUT_HANDLE);
        let hout = GetStdHandle(STD_OUTPUT_HANDLE);

        let mut in_mode = 0u32;
        if GetConsoleMode(hin, &mut in_mode) == 0 {
            return Err(io::Error::last_os_error());
        }
        let raw = in_mode & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT)
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        if SetConsoleMode(hin, raw) == 0 {
            return Err(io::Error::last_os_error());
        }

        // Let conhost interpret ANSI escapes; Windows Terminal does anyway.
        let mut out_mode = 0u32;
        let out_saved = GetConsoleMode(hout, &mut out_mode) != 0;
        if out_saved {
            SetConsoleMode(hout, out_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }

        Ok(RawConsole {
            in_mode,
            out_mode,
            out_saved,
        })
    }
}

pub fn leave_raw(saved: &RawConsole) {
    unsafe {
        SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), saved.in_mode);
        if saved.out_saved {
            SetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), saved.out_mode);
        }
    }
}

// ── Terminal size ─────────────────────────────────────────────────────

pub fn terminal_size() -> Option<(usize, usize)> {
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut info: ConsoleScreenBufferInfo = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(h, &mut info) == 0 {
            return None;
        }
        let w = info.sr_window.right - info.sr_window.left + 1;
        let h_ = info.sr_window.bottom - info.sr_window.top + 1;
        if w > 0 && h_ > 0 {
            Some((w as usize, h_ as usize))
        } else {
            None
        }
    }
}
