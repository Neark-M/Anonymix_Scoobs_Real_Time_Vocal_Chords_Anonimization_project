//! Minimal SDL 3.4.16 / SDL_ttf 3.2.2 C declarations used by the V1.8 UI.
//! Values/layouts are checked against the official headers by the stage-9
//! development oracle. No C++ widget or renderer code is linked into Rust.
use crate::ui_geometry::{Point, Rect, Vertex};
use std::ffi::{c_char, c_void};

pub const INIT_VIDEO: u32 = 0x20;
pub const WINDOW_FLAGS: u64 = 0x20 | 0x2000;
pub const PIXELFORMAT_RGBA8888: u32 = 0x16462004;
pub const TEXTURE_TARGET: i32 = 2;
pub const SCALE_LINEAR: i32 = 1;
pub const BLEND_NONE: u32 = 0;
pub const BLEND_ALPHA: u32 = 1;
pub const BLEND_PREMULTIPLIED: u32 = 0x10;
pub const EVENT_QUIT: u32 = 0x100;
pub const EVENT_DISPLAY_USABLE_BOUNDS_CHANGED: u32 = 0x158;
pub const EVENT_WINDOW_CLOSE_REQUESTED: u32 = 0x210;
pub const EVENT_WINDOW_DISPLAY_CHANGED: u32 = 0x213;
pub const EVENT_MOUSE_MOTION: u32 = 0x400;
pub const EVENT_MOUSE_DOWN: u32 = 0x401;
pub const EVENT_MOUSE_UP: u32 = 0x402;
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// SDL integer rectangle in physical/window coordinates, according to the called SDL API.
pub struct IntRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// SDL_ttf byte RGBA color; unlike geometry colors, components range from 0 to 255.
pub struct TextColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}
impl TextColor {
    /// Construct an opaque text color from byte red, green and blue components.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
/// Exact SDL3 mouse-button event layout. Read only when the event discriminant selects this variant.
pub struct MouseButton {
    pub kind: u32,
    pub reserved: u32,
    pub timestamp: u64,
    pub window: u32,
    pub which: u32,
    pub button: u8,
    pub down: bool,
    pub clicks: u8,
    pub padding: u8,
    pub x: f32,
    pub y: f32,
}
#[repr(C)]
#[derive(Clone, Copy)]
/// Exact SDL3 mouse-motion event layout, including window-relative float coordinates.
pub struct MouseMotion {
    pub kind: u32,
    pub reserved: u32,
    pub timestamp: u64,
    pub window: u32,
    pub which: u32,
    pub state: u32,
    pub x: f32,
    pub y: f32,
    pub x_rel: f32,
    pub y_rel: f32,
}
#[repr(C)]
pub union Event {
    pub kind: u32,
    pub button: MouseButton,
    pub motion: MouseMotion,
    pub padding: [u8; 128],
    pub alignment: u64,
}
impl Default for Event {
    fn default() -> Self {
        Self { padding: [0; 128] }
    }
}
#[repr(C)]
/// Borrowed SDL3 surface layout. Its pixel pointer remains owned by SDL until destruction.
pub struct Surface {
    pub flags: u32,
    pub format: u32,
    pub w: i32,
    pub h: i32,
    pub pitch: i32,
    pub pixels: *mut c_void,
    pub refcount: i32,
    pub reserved: *mut c_void,
}
pub type Handle = *mut c_void;
pub type Init = unsafe extern "C" fn(u32) -> bool;
pub type Quit = unsafe extern "C" fn();
pub type CreateWindow =
    unsafe extern "C" fn(*const c_char, i32, i32, u64, *mut Handle, *mut Handle) -> bool;
pub type Geometry =
    unsafe extern "C" fn(Handle, Handle, *const Vertex, i32, *const i32, i32) -> bool;
pub type DrawTexture = unsafe extern "C" fn(Handle, Handle, *const Rect, *const Rect) -> bool;
pub type DrawPoints = unsafe extern "C" fn(Handle, *const Point, i32) -> bool;

pub type AppInit = unsafe extern "C" fn(*mut Handle, i32, *mut *mut c_char) -> i32;
pub type AppIterate = unsafe extern "C" fn(Handle) -> i32;
pub type AppEvent = unsafe extern "C" fn(Handle, *mut Event) -> i32;
pub type AppQuit = unsafe extern "C" fn(Handle, i32);
pub type EnterCallbacks =
    unsafe extern "C" fn(i32, *mut *mut c_char, AppInit, AppIterate, AppEvent, AppQuit) -> i32;
