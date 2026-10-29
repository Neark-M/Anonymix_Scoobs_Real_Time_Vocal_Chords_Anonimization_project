//! V1.8 logical canvas, monitor fit, responsive control positions and pointer mapping.
//!
//! SDL window coordinates and renderer output pixels must each use their own
//! viewport, computed by the same formulas. Neither path stretches individual
//! controls. This module makes no SDL calls and owns no audio state.
use crate::ui_geometry::{Point, Rect};

pub const LAYOUT_WIDTH: i32 = 1430;
pub const LAYOUT_HEIGHT: i32 = 1030;
pub const MIN_WINDOW_WIDTH: i32 = 960;
pub const MIN_WINDOW_HEIGHT: i32 = 650;
pub const MAX_LAYOUT_WIDTH: i32 = 2048;
pub const DESKTOP_WIDTH_INSET: i32 = 32;
pub const DESKTOP_HEIGHT_INSET: i32 = 64;
pub const BACKGROUND: [f32; 4] = [0.30, 0.92, 0.35, 1.0];

#[derive(Clone, Copy, Debug, PartialEq)]
/// Uniform aspect-fit scale and centered offset; independent width/height changes never stretch shapes.
pub struct Viewport {
    pub area: Rect,
    pub scale: f32,
    pub layout_width: f32,
}

/// Preserve the original f32 order, including the minimum authored width.
pub fn layout_width(width: i32, height: i32) -> f32 {
    if width <= 0 || height <= 0 {
        return LAYOUT_WIDTH as f32;
    }
    let matching = width as f32 * LAYOUT_HEIGHT as f32 / height as f32;
    matching.clamp(LAYOUT_WIDTH as f32, MAX_LAYOUT_WIDTH as f32)
}
/// Allocate the original 45 percent of additional logical width to the left panel.
pub fn left_expansion(layout: f32) -> f32 {
    (layout - LAYOUT_WIDTH as f32) * 0.45
}
/// Compute the original responsive graph width from the expanded logical layout.
pub fn graph_width(layout: f32) -> f32 {
    600.0 + left_expansion(layout)
}

/// A minimized window has no drawable viewport. The C++ caller also rejects
/// nonpositive dimensions before using the original viewport calculation.
pub fn viewport(width: i32, height: i32) -> Option<Viewport> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let layout = layout_width(width, height);
    let scale = (width as f32 / layout).min(height as f32 / LAYOUT_HEIGHT as f32);
    let fitted_width = layout * scale;
    let fitted_height = LAYOUT_HEIGHT as f32 * scale;
    Some(Viewport {
        area: Rect {
            x: (width as f32 - fitted_width) * 0.5,
            y: (height as f32 - fitted_height) * 0.5,
            w: fitted_width,
            h: fitted_height,
        },
        scale,
        layout_width: layout,
    })
}

/// Right/bottom edges and green margins are noninteractive, as in UiLayout.h.
pub fn window_point_to_ui(width: i32, height: i32, point: Point) -> Option<Point> {
    let view = viewport(width, height)?;
    if point.x < view.area.x
        || point.y < view.area.y
        || point.x >= view.area.x + view.area.w
        || point.y >= view.area.y + view.area.h
    {
        return None;
    }
    Some(Point {
        x: (point.x - view.area.x) / view.scale,
        y: (point.y - view.area.y) / view.scale,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Width and height used by the monitor-fit initial/minimum sizing calculation.
pub struct WindowSize {
    pub width: i32,
    pub height: i32,
}
/// Result of a successful SDL usable-bounds query. Failure remains the SDL
/// caller's decision: startup then keeps the authored 1430x1030 dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorFit {
    pub initial: WindowSize,
    pub minimum: WindowSize,
    pub current: WindowSize,
}
/// Fit initial/current/minimum dimensions inside usable monitor bounds with the original desktop insets.
/// Return None for invalid bounds so the caller can retain the authored size.
pub fn fit_monitor(usable: WindowSize, current: WindowSize) -> Option<MonitorFit> {
    if usable.width <= 0 || usable.height <= 0 {
        return None;
    }
    let width = (usable.width - DESKTOP_WIDTH_INSET).max(1);
    let height = (usable.height - DESKTOP_HEIGHT_INSET).max(1);
    Some(MonitorFit {
        initial: WindowSize {
            width: LAYOUT_WIDTH.min(width),
            height: LAYOUT_HEIGHT.min(height),
        },
        minimum: WindowSize {
            width: MIN_WINDOW_WIDTH.min(width),
            height: MIN_WINDOW_HEIGHT.min(height),
        },
        current: WindowSize {
            width: current.width.min(width),
            height: current.height.min(height),
        },
    })
}

/// Stable registry order for the geometry supplied by AppRender.cpp. A zero
/// half-size marks a slider: its diamond and track are configured separately.
#[derive(Clone, Copy, Debug)]
pub struct ControlPlacement {
    pub name: &'static str,
    pub center: Point,
    pub half_width: u16,
    pub half_height: u16,
}
#[derive(Clone, Debug)]
/// Logical graph/caption/control placements for one window shape.
pub struct ResponsiveGeometry {
    pub rounded_layout_width: i32,
    pub controls: Vec<ControlPlacement>,
    pub captions: [f32; 6],
    pub graph_width: f32,
}
impl ResponsiveGeometry {
    /// C++ rounds the responsive control layout, but the graph's width uses
    /// the unrounded viewport width. Keep that distinction at fractional DPI.
    pub fn for_window(width: i32, height: i32) -> Self {
        let layout = layout_width(width, height).round() as i32;
        let extra = (layout - LAYOUT_WIDTH) as f32;
        let left = left_expansion(layout as f32);
        let right = extra - left;
        let mut controls = Vec::with_capacity(18);
        let mut add = |name, x: f32, y, half_width: f32, half_height| {
            controls.push(ControlPlacement {
                name,
                center: Point {
                    x: x.round() as u16 as f32,
                    y,
                },
                half_width: half_width.round() as u16,
                half_height,
            });
        };
        add(
            "AudioMode",
            800.0 + left + right / 6.0,
            65.0,
            140.0 + right / 6.0,
            40,
        );
        add(
            "WasapiMode",
            1060.0 + left + right * 0.5,
            65.0,
            100.0 + right / 6.0,
            40,
        );
        add(
            "AudioDevices",
            1295.0 + extra - right / 6.0,
            65.0,
            115.0 + right / 6.0,
            40,
        );
        let first = [
            "EffectPitch",
            "EffectFormant",
            "PA_Record",
            "PA_PlaySelected",
        ];
        let second = [
            "EffectTimbre",
            "EffectProsody",
            "EffectJitter",
            "PA_SaveSelected",
        ];
        for i in 0..4 {
            let x = 750.0 + i as f32 * 190.0 + left + right * (i as f32 + 0.5) / 4.0;
            add(first[i], x, 300.0, 90.0 + right / 8.0, 35);
            add(second[i], x, 385.0, 90.0 + right / 8.0, 35);
        }
        let sliders = [
            "Slider_PitchShift",
            "Slider_FormantShift",
            "Slider_Timbre",
            "Slider_Prosody",
            "Slider_Jitter",
            "Slider_Volume",
        ];
        let mut captions = [0.0; 6];
        for i in 0..6 {
            let x = 715.0 + i as f32 * 128.0 + left + right * i as f32 / 5.0;
            add(sliders[i], x, 740.0, 0.0, 0);
            captions[i] = x;
        }
        add(
            "WidgetInfo",
            1035.0 + left + right * 0.5,
            185.0,
            (750.0 + right) * 0.5,
            55,
        );
        Self {
            rounded_layout_width: layout,
            controls,
            captions,
            graph_width: graph_width(layout_width(width, height)),
        }
    }
}
