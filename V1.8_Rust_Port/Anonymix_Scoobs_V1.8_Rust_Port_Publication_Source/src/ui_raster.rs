//! Original target-size and resolve decisions, without allocating GPU handles.
//! SDL error/fallback handling is performed by ui_renderer.
use crate::{
    ui_geometry::{Point, Rect},
    ui_layout::{viewport, LAYOUT_HEIGHT},
};

/// AppRender's size-dependent smoothing hysteresis is cached by window size.
#[derive(Default)]
pub struct LargeWindowSmoothing {
    cached: Option<(i32, i32)>,
    enabled: bool,
}
impl LargeWindowSmoothing {
    /// Cache the size-dependent smoothing decision with the original 0.76/0.72 hysteresis.
    pub fn update(&mut self, width: i32, height: i32) -> bool {
        if self.cached == Some((width, height)) {
            return self.enabled;
        }
        self.cached = Some((width, height));
        let scale = viewport(width, height).map_or(0.0, |view| view.scale);
        self.enabled = scale >= if self.enabled { 0.72 } else { 0.76 };
        self.enabled
    }
}
/// Preferred whole-interface scale, prior to allocation failures imposing a
/// lower maximum. Tilted figures and the FFT have independent 8x targets.
pub fn interface_factor(width: i32, height: i32, maximum_factor: i32) -> i32 {
    let scale = viewport(width, height).map_or(0.0, |view| view.scale);
    let preferred = if scale < 0.82 {
        3
    } else if scale <= 1.20 {
        4
    } else {
        2
    };
    preferred.min(maximum_factor)
}
#[derive(Clone, Copy, Debug, PartialEq)]
/// Rounded physical-pixel spectrum rectangle for the original compact-window composition route.
pub struct SpectrumPixelFit {
    pub destination: Rect,
    pub small_physical: bool,
}
/// Round both physical endpoints separately. Rounding width alone can shift a
/// one-pixel composite when the surrounding UI is fitted at a fractional DPI.
pub fn spectrum_pixel_fit(
    output_width: i32,
    output_height: i32,
    graph: Rect,
) -> Option<SpectrumPixelFit> {
    let view = viewport(output_width, output_height)?;
    let left = (view.area.x + graph.x * view.scale).round();
    let top = (view.area.y + graph.y * view.scale).round();
    let right = (view.area.x + (graph.x + graph.w) * view.scale).round();
    let bottom = (view.area.y + (graph.y + graph.h) * view.scale).round();
    Some(SpectrumPixelFit {
        destination: Rect {
            x: left,
            y: top,
            w: right - left,
            h: bottom - top,
        },
        small_physical: view.scale < 0.72,
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Allocated target dimensions, including the original capacity rounding.
pub struct TextureSize {
    pub width: i32,
    pub height: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Spectrum resolve-chain dimensions for one raster preference and viewport.
pub struct SpectrumTargets {
    pub main: TextureSize,
    pub intermediate: Option<TextureSize>,
    pub resolve: Option<TextureSize>,
    pub logical: TextureSize,
    pub pixel: TextureSize,
}
impl SpectrumTargets {
    /// One candidate from the original 8,4,2 allocation attempt order. Capacity
    /// grows in 64 logical units, while source rectangles use only actual width.
    pub fn candidate(width: f32, height: f32, scale: i32) -> Self {
        assert!(matches!(scale, 8 | 4 | 2));
        let capacity = (width / 64.0).ceil() as i32 * 64;
        let size = |factor: i32| TextureSize {
            width: capacity * factor,
            height: (height * factor as f32).ceil() as i32,
        };
        Self {
            main: size(scale),
            intermediate: (scale == 8).then(|| size(4)),
            resolve: (scale >= 4).then(|| size(2)),
            logical: size(1),
            pixel: TextureSize {
                width: (capacity as f32 * 0.72).ceil() as i32,
                height: (height * 0.72).ceil() as i32,
            },
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
/// Per-quad supersampling tile; straight axis-aligned controls use the ordinary path.
pub struct ObliqueTile {
    pub area: Rect,
    pub capacity: TextureSize,
    pub final_scale: i32,
}
impl ObliqueTile {
    /// Return None for the original direct-draw branch. No transparent stroke
    /// is added: the 2-unit padding is solely reusable tile capacity.
    pub fn for_quad(
        points: &[Point; 6],
        force_eight: bool,
        has_parent: bool,
        interface_sample_factor: i32,
        large_smoothing: bool,
    ) -> Option<Self> {
        let (a, b, c) = (points[0], points[1], points[3]);
        let slanted = |p: Point, q: Point| {
            let dx = (q.x - p.x).abs();
            let dy = (q.y - p.y).abs();
            dx.min(dy) > dx.hypot(dy) * 0.025
        };
        if (!force_eight && !slanted(a, b) && !slanted(b, c))
            || !has_parent
            || interface_sample_factor == 0
        {
            return None;
        }
        let corners = [a, b, c, points[4]];
        let mut min = corners[0];
        let mut max = corners[0];
        for point in corners {
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
        }
        let x = min.x.floor() - 2.0;
        let y = min.y.floor() - 2.0;
        let width = max.x.ceil() as i32 + 2 - x as i32;
        let height = max.y.ceil() as i32 + 2 - y as i32;
        let capacity = |needed: i32| 160.max((needed as f32 / 32.0).ceil() as i32 * 32);
        Some(Self {
            area: Rect {
                x,
                y,
                w: width as f32,
                h: height as f32,
            },
            capacity: TextureSize {
                width: capacity(width),
                height: capacity(height),
            },
            final_scale: if force_eight && large_smoothing { 1 } else { 4 },
        })
    }
}

/// Compute a full-interface raster target from retained logical capacity and scale factor.
pub fn interface_target_size(capacity_width: i32, factor: i32) -> TextureSize {
    TextureSize {
        width: capacity_width * factor,
        height: LAYOUT_HEIGHT * factor,
    }
}
