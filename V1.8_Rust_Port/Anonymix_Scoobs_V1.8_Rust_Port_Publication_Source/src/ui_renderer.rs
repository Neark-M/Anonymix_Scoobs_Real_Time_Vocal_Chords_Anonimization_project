//! SDL rendering of the original V1.8 UI. The approved geometry comes from
//! ui_geometry; this file ports texture ownership, text caches and the original
//! 8x -> 4x -> 2x -> 1x resolves, including compact physical-pixel FFT composition.
//! Every object and SDL call remains on the main thread.
use crate::{
    observation::WaveformFrame,
    sdl_abi::*,
    sdl_api::{Sdl, Ttf},
    spectrum::SpectrumFrame,
    ui_animation::VisualizerAnimation,
    ui_geometry::{waveform_points, Color, Point, Rect, SpectrumGeometry, Vertex},
    ui_layout::{self, WindowSize},
    ui_raster::{self, LargeWindowSmoothing, ObliqueTile},
    ui_timing::RenderFrameTiming,
    ui_widgets::{UiModel, WidgetKind, EFFECT_HELP, SLIDER_NAMES},
};
use std::{ffi::CString, path::Path, ptr, rc::Rc};

#[derive(Clone)]
struct Draw {
    api: Rc<Sdl>,
    renderer: Handle,
}
impl Draw {
    fn color(&self, c: Color) {
        unsafe {
            (self.api.draw_color)(self.renderer, c.r, c.g, c.b, c.a);
        }
    }
    fn rgba(&self, r: f32, g: f32, b: f32, a: f32) {
        self.color(Color { r, g, b, a });
    }
    fn fill(&self, r: Rect) {
        unsafe {
            (self.api.fill_rect)(self.renderer, &r);
        }
    }
    fn clear(&self) {
        unsafe {
            (self.api.clear)(self.renderer);
        }
    }
    fn target(&self, t: Handle) -> bool {
        unsafe { (self.api.set_target)(self.renderer, t) }
    }
    fn scale(&self, s: f32) -> bool {
        unsafe { (self.api.render_scale)(self.renderer, s, s) }
    }
    fn texture(&self, t: Handle, source: Option<Rect>, dest: Rect) -> bool {
        unsafe {
            (self.api.texture)(
                self.renderer,
                t,
                source.as_ref().map_or(ptr::null(), |r| r),
                &dest,
            )
        }
    }
    fn geometry(&self, v: &[Vertex], indices: &[i32]) -> bool {
        unsafe {
            (self.api.geometry)(
                self.renderer,
                ptr::null_mut(),
                v.as_ptr(),
                v.len() as i32,
                if indices.is_empty() {
                    ptr::null()
                } else {
                    indices.as_ptr()
                },
                indices.len() as i32,
            )
        }
    }
    fn now(&self) -> u64 {
        unsafe { (self.api.ticks_ns)() }
    }
}
struct Texture {
    raw: Handle,
    w: i32,
    h: i32,
    api: Rc<Sdl>,
}
impl Texture {
    fn target(draw: &Draw, w: i32, h: i32) -> Option<Self> {
        let raw = unsafe {
            (draw.api.create_texture)(draw.renderer, PIXELFORMAT_RGBA8888, TEXTURE_TARGET, w, h)
        };
        if raw.is_null() {
            None
        } else {
            let t = Self {
                raw,
                w,
                h,
                api: Rc::clone(&draw.api),
            };
            unsafe {
                (draw.api.texture_scale)(raw, SCALE_LINEAR);
            }
            Some(t)
        }
    }
    fn blend(&self, mode: u32) -> bool {
        unsafe { (self.api.texture_blend)(self.raw, mode) }
    }
}
impl Drop for Texture {
    fn drop(&mut self) {
        unsafe {
            (self.api.destroy_texture)(self.raw);
        }
    }
}
struct TextImage {
    text: String,
    color: TextColor,
    texture: Texture,
}
impl TextImage {
    fn size(&self, scale: f64) -> (f32, f32) {
        (
            (self.texture.w as f64 * scale * 0.5) as f32,
            (self.texture.h as f64 * scale * 0.5) as f32,
        )
    }
}
#[derive(Default)]
struct TextCache {
    stable: Vec<TextImage>,
    info: Option<TextImage>,
    wave_stats: Option<TextImage>,
    wave_updated: u64,
}
#[derive(Default)]
struct InterfaceTargets {
    main: Option<Texture>,
    half: Option<Texture>,
    resolved: Option<Texture>,
    factor: i32,
    capacity: i32,
    maximum: i32,
    unavailable: bool,
}
impl InterfaceTargets {
    fn clear(&mut self) {
        self.main = None;
        self.half = None;
        self.resolved = None;
        self.factor = 0;
        self.capacity = 0;
    }
    fn ensure(&mut self, draw: &Draw, width: i32, height: i32) {
        if self.unavailable || width <= 0 || height <= 0 {
            return;
        }
        let wanted = ui_raster::interface_factor(width, height, self.maximum);
        let desired = ui_layout::layout_width(width, height).ceil() as i32;
        if self.main.is_some() && self.factor == wanted && self.capacity >= desired {
            return;
        }
        let capacity = desired.max(if self.factor == wanted {
            self.capacity + 128
        } else {
            1430
        });
        self.clear();
        for factor in (2..=wanted).rev() {
            self.main = Texture::target(draw, capacity * factor, 1030 * factor);
            if self.main.is_some() {
                if factor > 2 {
                    self.half = Texture::target(draw, capacity * factor / 2, 1030 * factor / 2);
                }
                if factor == 2 || self.half.is_some() {
                    self.resolved = Texture::target(draw, capacity, 1030);
                    if self.resolved.is_some() {
                        self.factor = factor;
                        self.capacity = capacity;
                        return;
                    }
                }
            }

            self.clear();
            self.maximum = factor - 1;
        }
        self.unavailable = true;
    }
}
#[derive(Default)]
struct ObliqueTargets {
    t: [Option<Texture>; 4],
    capacity: (i32, i32),
    allocation_failed: bool,
    render_failed: bool,
}
impl ObliqueTargets {
    fn clear(&mut self) {
        self.t = [None, None, None, None];
        self.capacity = (0, 0);
    }
    fn ensure(&mut self, draw: &Draw, width: i32, height: i32) -> bool {
        if self.t[0].is_some()
            && self.t[1].is_some()
            && self.capacity.0 >= width
            && self.capacity.1 >= height
        {
            return true;
        }
        if self.allocation_failed {
            return false;
        }
        let w = 160.max((width as f32 / 32.).ceil() as i32 * 32);
        let h = 160.max((height as f32 / 32.).ceil() as i32 * 32);
        let mut next: [Option<Texture>; 4] = [None, None, None, None];
        for (i, s) in [8, 4, 2, 1].into_iter().enumerate() {
            if i == 0 || next[i - 1].is_some() {
                next[i] = Texture::target(draw, w * s, h * s);
            }
        }
        let hard = next[0].as_ref().is_some_and(|t| t.blend(BLEND_NONE))
            && next[1]
                .as_ref()
                .is_some_and(|t| t.blend(BLEND_PREMULTIPLIED));
        if !hard {
            self.allocation_failed = true;

            return false;
        }
        let soft = next[2].as_ref().is_some_and(|t| t.blend(BLEND_NONE))
            && next[3]
                .as_ref()
                .is_some_and(|t| t.blend(BLEND_PREMULTIPLIED));
        if !soft {
            next[2] = None;
            next[3] = None;
        }
        self.clear();
        self.t = next;
        self.capacity = (w, h);
        true
    }
    fn draw(
        &mut self,
        draw: &Draw,
        vertices: &[Vertex; 6],
        forced: bool,
        factor: i32,
        soften: bool,
    ) {
        let parent = unsafe { (draw.api.get_target)(draw.renderer) };
        let positions = vertices.map(|v| v.position);
        let Some(tile) =
            ObliqueTile::for_quad(&positions, forced, !parent.is_null(), factor, soften)
                .filter(|_| !self.render_failed)
        else {
            draw.geometry(&vertices[..3], &[]);
            draw.geometry(&vertices[3..], &[]);
            return;
        };
        if !self.ensure(draw, tile.area.w as i32, tile.area.h as i32) {
            draw.geometry(&vertices[..3], &[]);
            draw.geometry(&vertices[3..], &[]);
            return;
        }
        let raw = |i: usize| self.t[i].as_ref().map_or(ptr::null_mut(), |t| t.raw);
        let handles = [raw(0), raw(1), raw(2), raw(3)];
        let mut local = *vertices;
        for v in &mut local {
            v.position.x -= tile.area.x;
            v.position.y -= tile.area.y;
        }
        let mut ready = draw.target(handles[0]);
        if ready {
            ready = draw.scale(8.);
            draw.rgba(0., 0., 0., 0.);
            draw.clear();
            if ready {
                ready = draw.geometry(&local[..3], &[]) && draw.geometry(&local[3..], &[]);
            }
        }
        let area = |s: f32| Rect {
            x: 0.,
            y: 0.,
            w: tile.area.w * s,
            h: tile.area.h * s,
        };
        if ready {
            ready = draw.target(handles[1]);
            if ready {
                ready = draw.texture(handles[0], Some(area(8.)), area(4.));
            }
        }
        let mut result = handles[1];
        let mut scale = 4.;
        if ready
            && tile.final_scale == 1
            && !handles[2].is_null()
            && !handles[3].is_null()
            && self.t[1].as_ref().unwrap().blend(BLEND_NONE)
            && draw.target(handles[2])
            && draw.texture(handles[1], Some(area(4.)), area(2.))
            && draw.target(handles[3])
            && draw.texture(handles[2], Some(area(2.)), area(1.))
        {
            result = handles[3];
            scale = 1.;
        }
        self.t[1].as_ref().unwrap().blend(BLEND_PREMULTIPLIED);
        if !draw.target(parent) {
            self.render_failed = true;

            return;
        }
        if ready {
            ready = draw.texture(result, Some(area(scale)), tile.area);
        }
        if !ready {
            self.render_failed = true;

            draw.geometry(&vertices[..3], &[]);
            draw.geometry(&vertices[3..], &[]);
        }
    }
}
#[derive(Default)]
struct SpectrumTargets {
    t: [Option<Texture>; 5],
    scale: i32,
    capacity: (i32, i32),
    preferred: i32,
    unavailable: bool,
    overlay: Option<(Rect, Rect, Rect)>,
}
impl SpectrumTargets {
    fn clear(&mut self) {
        self.t = [None, None, None, None, None];
        self.overlay = None;
    }
    fn preferred(&mut self, scale: i32) {
        if matches!(scale, 4 | 8) && self.preferred != scale {
            self.preferred = scale;
            if scale > self.scale && self.t[0].is_some() {
                self.clear();
            }
            self.unavailable = false;
        }
    }
    fn ensure(&mut self, draw: &Draw, width: f32, height: f32) {
        if self.t[0].is_some()
            && ((width * self.scale as f32).ceil() as i32 > self.capacity.0
                || (height * self.scale as f32).ceil() as i32 != self.capacity.1)
        {
            self.clear();
        }
        if self.unavailable || self.t[0].is_some() {
            return;
        }
        for scale in [8, 4, 2] {
            if scale > self.preferred {
                continue;
            }
            let plan = ui_raster::SpectrumTargets::candidate(width, height, scale);
            self.t[0] = Texture::target(draw, plan.main.width, plan.main.height);
            if self.t[0].is_some() {
                if let Some(s) = plan.intermediate {
                    self.t[1] = Texture::target(draw, s.width, s.height);
                }
                if (scale != 8 || self.t[1].is_some()) && scale >= 4 {
                    if let Some(s) = plan.resolve {
                        self.t[2] = Texture::target(draw, s.width, s.height);
                    }
                }
                if (scale != 8 || self.t[1].is_some()) && (scale < 4 || self.t[2].is_some()) {
                    self.scale = scale;
                    self.capacity = (plan.main.width, plan.main.height);
                    self.t[3] = Texture::target(draw, plan.logical.width, plan.logical.height);
                    if self.t[3].is_some() {
                        self.t[4] = Texture::target(draw, plan.pixel.width, plan.pixel.height);
                    }
                    break;
                }
            }

            self.clear();
        }
        self.unavailable = self.t[0].is_none();
    }
    fn body(
        draw: &Draw,
        scratch: &mut SpectrumGeometry,
        area: Rect,
        scale: u32,
        magnitudes: &[f32; 96],
        cleared: bool,
    ) {
        if !cleared {
            draw.rgba(0.1, 0.1, 0.15, 0.8);
            draw.fill(area);
        }
        scratch.build(area, scale, magnitudes);
        if scratch.curve.len() >= 3 {
            draw.geometry(&scratch.fill_vertices, &scratch.fill_indices);
        }
        if scratch.curve.len() >= 2 {
            draw.geometry(&scratch.edge_vertices, &scratch.edge_indices);
        }
    }
    fn draw(
        &mut self,
        draw: &Draw,
        scratch: &mut SpectrumGeometry,
        area: Rect,
        magnitudes: &[f32; 96],
        output: (i32, i32),
    ) {
        self.overlay = None;
        let parent = unsafe { (draw.api.get_target)(draw.renderer) };
        let pixels = ui_raster::spectrum_pixel_fit(output.0, output.1, area);
        self.ensure(draw, area.w, area.h);
        let handles: Vec<_> = self
            .t
            .iter()
            .map(|t| t.as_ref().map_or(ptr::null_mut(), |t| t.raw))
            .collect();
        let mut ready = false;
        let mut logical = false;
        let mut pixel = false;
        let retained4 = self.scale == 8 && self.preferred == 4 && !handles[1].is_null();
        let draw_target = if retained4 { handles[1] } else { handles[0] };
        let draw_scale = if retained4 { 4 } else { self.scale };
        let a = |s: f32| Rect {
            x: 0.,
            y: 0.,
            w: area.w * s,
            h: area.h * s,
        };
        if !handles[0].is_null() {
            if draw.target(draw_target) && draw.scale(draw_scale as f32) {
                draw.rgba(0.1, 0.1, 0.15, 1.);
                draw.fill(a(1.));
                Self::body(draw, scratch, a(1.), draw_scale as u32, magnitudes, true);
                ready = true;
            }
            if ready && !handles[1].is_null() && !retained4 {
                ready = draw.target(handles[1]) && draw.texture(handles[0], Some(a(8.)), a(4.));
            }
            if ready && !handles[2].is_null() {
                let input = if !handles[1].is_null() {
                    handles[1]
                } else {
                    draw_target
                };
                let input_scale = if !handles[1].is_null() { 4 } else { draw_scale };
                ready = draw.target(handles[2])
                    && draw.texture(input, Some(a(input_scale as f32)), a(2.));
            }
            if ready
                && !parent.is_null()
                && !handles[2].is_null()
                && !handles[3].is_null()
                && draw.target(handles[3])
            {
                logical = draw.texture(handles[2], Some(a(2.)), a(1.));
            }
            if logical
                && pixels.is_some_and(|p| {
                    p.small_physical && p.destination.w > 0. && p.destination.h > 0.
                })
                && !handles[4].is_null()
                && draw.target(handles[4])
            {
                let dest = pixels.unwrap().destination;
                pixel = draw.texture(
                    handles[3],
                    Some(a(1.)),
                    Rect {
                        x: 0.,
                        y: 0.,
                        w: dest.w,
                        h: dest.h,
                    },
                );
            }
            draw.target(parent);
            if !ready {
                self.clear();
                self.unavailable = true;
            }
        }
        if ready {
            if pixel {
                let dest = pixels.unwrap().destination;
                self.overlay = Some((
                    Rect {
                        x: 0.,
                        y: 0.,
                        w: dest.w,
                        h: dest.h,
                    },
                    a(1.),
                    dest,
                ));
            }
            let (result, scale) = if logical {
                (handles[3], 1)
            } else if !handles[2].is_null() {
                (handles[2], 2)
            } else if !handles[1].is_null() && draw_target != handles[1] {
                (handles[1], 4)
            } else {
                (draw_target, draw_scale)
            };
            if !pixel && !draw.texture(result, Some(a(scale as f32)), area) {
                self.clear();
                self.unavailable = true;
                Self::body(draw, scratch, area, 2, magnitudes, false);
            }
        } else {
            Self::body(draw, scratch, area, 2, magnitudes, false);
        }
    }
}

/// Main-thread SDL window, renderer, font and texture owner. Drop releases textures/fonts before their APIs.
pub struct UiRenderer {
    draw: Draw,
    window: Handle,
    ttf: Option<Rc<Ttf>>,
    font: Handle,
    ttf_initialized: bool,
    text: TextCache,
    captions: Vec<Option<TextImage>>,
    help: Vec<[Option<TextImage>; 2]>,
    interface: InterfaceTargets,
    oblique: ObliqueTargets,
    spectrum: SpectrumTargets,
    scratch: SpectrumGeometry,
    wave_points: Vec<Point>,
    animation: VisualizerAnimation,
    large: LargeWindowSmoothing,
}
impl UiRenderer {
    /// Open the original monitor-fitted resizable desktop, initialize text and preload rendering targets.
    pub fn create(sdl_path: &Path, ttf_path: &Path, asset_dir: &Path) -> Result<Self, String> {
        Self::create_inner(sdl_path, ttf_path, asset_dir)
    }
    fn create_inner(sdl_path: &Path, ttf_path: &Path, asset_dir: &Path) -> Result<Self, String> {
        let api = Rc::new(Sdl::load(sdl_path)?);
        unsafe {
            (api.set_main_ready)();
            if !(api.init)(INIT_VIDEO) {
                return Err(api.error_text());
            }
        }
        let mut window = ptr::null_mut();
        let mut renderer = ptr::null_mut();
        let usable = unsafe {
            let mut r = IntRect::default();
            let id = (api.primary_display)();
            if id != 0 && (api.usable_bounds)(id, &mut r) {
                Some(WindowSize {
                    width: r.w,
                    height: r.h,
                })
            } else {
                None
            }
        };
        let fit = usable.and_then(|u| {
            ui_layout::fit_monitor(
                u,
                WindowSize {
                    width: 1430,
                    height: 1030,
                },
            )
        });
        let initial = fit.map_or(
            WindowSize {
                width: 1430,
                height: 1030,
            },
            |f| f.initial,
        );
        if !unsafe {
            (api.create_window)(
                c"Anonymix Scoobs".as_ptr(),
                initial.width,
                initial.height,
                WINDOW_FLAGS,
                &mut window,
                &mut renderer,
            )
        } {
            let error = api.error_text();
            unsafe {
                if !renderer.is_null() {
                    (api.destroy_renderer)(renderer);
                }
                if !window.is_null() {
                    (api.destroy_window)(window);
                }
                (api.quit)();
            }
            return Err(error);
        }
        let mut ui = Self {
            draw: Draw { api, renderer },
            window,
            ttf: None,
            font: ptr::null_mut(),
            ttf_initialized: false,
            text: TextCache::default(),
            captions: Vec::new(),
            help: Vec::new(),
            interface: InterfaceTargets {
                maximum: 4,
                ..InterfaceTargets::default()
            },
            oblique: ObliqueTargets::default(),
            spectrum: SpectrumTargets {
                preferred: 8,
                ..SpectrumTargets::default()
            },
            scratch: SpectrumGeometry::default(),
            wave_points: Vec::with_capacity(2048),
            animation: VisualizerAnimation::default(),
            large: LargeWindowSmoothing::default(),
        };
        unsafe {
            (ui.draw.api.minimum_size)(window, 960.min(initial.width), 650.min(initial.height));
            (ui.draw.api.draw_blend)(renderer, BLEND_ALPHA);
        }
        ui.update_monitor_limits();
        if let Ok(path) = CString::new(asset_dir.join("logo.bmp").to_string_lossy().as_bytes()) {
            unsafe {
                let surface = (ui.draw.api.load_bmp)(path.as_ptr());
                if !surface.is_null() {
                    (ui.draw.api.window_icon)(window, surface);
                    (ui.draw.api.destroy_surface)(surface);
                }
            }
        }
        match Ttf::load(ttf_path) {
            Ok(ttf) => {
                let ttf = Rc::new(ttf);
                if unsafe { (ttf.init)() } {
                    ui.ttf_initialized = true;
                    for path in [
                        "C:/Windows/Fonts/Arial.ttf",
                        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                    ] {
                        let name = CString::new(path).unwrap();
                        ui.font = unsafe { (ttf.open_font)(name.as_ptr(), 54.) };
                        if !ui.font.is_null() {
                            break;
                        }
                    }
                }
                ui.ttf = Some(ttf);
            }
            Err(_error) => {}
        }
        let (w, h) = ui.window_size();
        ui.interface.ensure(&ui.draw, w, h);
        ui.oblique.ensure(&ui.draw, 160, 160);
        let graph = ui_layout::graph_width(ui_layout::layout_width(w, h));
        ui.spectrum.ensure(&ui.draw, graph, 210.);
        ui.scratch.preload(graph);
        ui.preload_text();
        Ok(ui)
    }
    /// Borrow the main-thread SDL function table while its dynamic-library owner remains alive.
    pub fn api(&self) -> &Sdl {
        &self.draw.api
    }
    /// Read current window dimensions in SDL window coordinates, not texture or logical UI pixels.
    pub fn window_size(&self) -> (i32, i32) {
        let mut w = 0;
        let mut h = 0;
        unsafe {
            (self.draw.api.window_size)(self.window, &mut w, &mut h);
        }
        (w, h)
    }
    /// Read current mouse position in window coordinates; the caller applies the aspect-fit mapping.
    pub fn pointer(&self) -> Point {
        let mut p = Point::default();
        unsafe {
            (self.draw.api.mouse)(&mut p.x, &mut p.y);
        }
        p
    }
    /// Recompute the original monitor-fit minimum after moving the window between displays.
    pub fn update_monitor_limits(&self) {
        unsafe {
            let id = (self.draw.api.display_for_window)(self.window);
            let mut r = IntRect::default();
            if id == 0 || !(self.draw.api.usable_bounds)(id, &mut r) {
                return;
            }
            let (w, h) = self.window_size();
            if let Some(f) = ui_layout::fit_monitor(
                WindowSize {
                    width: r.w,
                    height: r.h,
                },
                WindowSize {
                    width: w,
                    height: h,
                },
            ) {
                (self.draw.api.minimum_size)(self.window, f.minimum.width, f.minimum.height);
                if w > f.current.width || h > f.current.height {
                    (self.draw.api.set_window_size)(self.window, f.current.width, f.current.height);
                }
            }
        }
    }
    fn text_image(&self, text: &str, color: TextColor) -> Option<TextImage> {
        if self.font.is_null() || text.is_empty() {
            return None;
        }
        let ttf = self.ttf.as_ref()?;
        let bytes = CString::new(text).ok()?;
        unsafe {
            let surface = (ttf.render)(self.font, bytes.as_ptr(), text.len(), color);
            if surface.is_null() {
                return None;
            }
            let (w, h) = ((*surface).w, (*surface).h);
            let raw = (self.draw.api.texture_from_surface)(self.draw.renderer, surface);
            (self.draw.api.destroy_surface)(surface);
            if raw.is_null() {
                None
            } else {
                Some(TextImage {
                    text: text.into(),
                    color,
                    texture: Texture {
                        raw,
                        w,
                        h,
                        api: Rc::clone(&self.draw.api),
                    },
                })
            }
        }
    }
    fn precache(&mut self, text: &str, color: TextColor) {
        if text.is_empty()
            || self
                .text
                .stable
                .iter()
                .any(|t| t.text == text && t.color == color)
            || self.text.stable.len() >= 128
        {
            return;
        }
        if let Some(image) = self.text_image(text, color) {
            self.text.stable.push(image);
        }
    }
    fn size(&self, text: &str, scale: f64) -> (f32, f32) {
        if let Some(image) = self
            .text
            .info
            .as_ref()
            .filter(|t| t.text == text)
            .or_else(|| self.text.stable.iter().find(|t| t.text == text))
        {
            return image.size(scale);
        }
        if let (Some(ttf), Ok(bytes)) = (&self.ttf, CString::new(text)) {
            if !self.font.is_null() {
                let mut w = 0;
                let mut h = 0;
                unsafe {
                    (ttf.size)(self.font, bytes.as_ptr(), text.len(), &mut w, &mut h);
                }
                return (
                    (w as f64 * scale * 0.5) as f32,
                    (h as f64 * scale * 0.5) as f32,
                );
            }
        }
        (0., 0.)
    }
    fn render_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        color: TextColor,
        scale: f64,
        stable: bool,
    ) {
        if stable {
            if let Some(image) = self
                .text
                .stable
                .iter()
                .find(|t| t.text == text && t.color == color)
            {
                let (w, h) = image.size(scale);
                self.draw
                    .texture(image.texture.raw, None, Rect { x, y, w, h });
                return;
            }
        }
        if let Some(image) = self.text_image(text, color) {
            let (w, h) = image.size(scale);
            self.draw
                .texture(image.texture.raw, None, Rect { x, y, w, h });
            if stable && self.text.stable.len() < 128 {
                self.text.stable.push(image);
            }
        }
    }
    fn info_text(&mut self, text: &str, x: f32, y: f32, color: TextColor, scale: f64) {
        if text.is_empty() {
            return;
        }
        if self
            .text
            .info
            .as_ref()
            .is_none_or(|t| t.text != text || t.color != color)
        {
            if let Some(image) = self.text_image(text, color) {
                self.text.info = Some(image);
            }
        }
        if let Some(image) = &self.text.info {
            let (w, h) = image.size(scale);
            self.draw
                .texture(image.texture.raw, None, Rect { x, y, w, h });
        }
    }
    fn preload_text(&mut self) {
        let dark = TextColor::rgb(0, 0, 0);
        for captions in [
            vec!["Mode: Recording", "Mode: Stream", "Switching..."],
            vec![
                "Audio: Starting...",
                "WASAPI: Shared",
                "WASAPI: Excl.",
                "Other Host",
                "No Audio",
            ],
            vec!["Reopen Audio", "Reopening..."],
            vec!["Start Rec", "Stop Rec", "Starting...", "Stopping..."],
            vec!["Play", "Stop"],
            vec!["Save WAV", "Saving..."],
        ] {
            for text in captions {
                self.precache(text, dark);
            }
        }
        for name in crate::ui_widgets::EFFECT_LABELS {
            self.precache(&format!("{name}: ON"), TextColor::rgb(255, 255, 255));
            self.precache(&format!("{name}: OFF"), TextColor::rgb(20, 28, 34));
        }
        self.precache(
            "Audio Spectrum (20 Hz - 20 kHz @ 48 kHz) - Calibrated noise",
            TextColor::rgb(200, 200, 255),
        );
        self.precache(
            "Audio Waveform (Oscilloscope)",
            TextColor::rgb(200, 255, 200),
        );
        self.precache("No audio input detected", TextColor::rgb(200, 100, 100));
        for n in 0..=60 {
            self.precache(
                &format!("PortAudio - Noise calibration: {}%", n * 100 / 60),
                TextColor::rgb(200, 200, 255),
            );
        }
        self.captions = SLIDER_NAMES
            .iter()
            .map(|s| self.text_image(s, TextColor::rgb(14, 22, 28)))
            .collect();
        self.help = EFFECT_HELP
            .iter()
            .map(|lines| {
                [
                    self.text_image(lines[0], TextColor::rgb(14, 22, 28)),
                    self.text_image(lines[1], TextColor::rgb(14, 22, 28)),
                ]
            })
            .collect();
    }
    /// Set the visual budget's preferred raster scale without changing analyzed audio or FFT band values.
    pub fn set_fft_scale(&mut self, scale: u32) {
        self.spectrum.preferred(scale as i32);
    }
    fn spectrum_title(frame: Option<&SpectrumFrame>) -> String {
        match frame {
            Some(f) if !f.calibrating => {
                "Audio Spectrum (20 Hz - 20 kHz @ 48 kHz) - Calibrated noise".into()
            }
            f => format!(
                "PortAudio - Noise calibration: {}%",
                f.map_or(0, |f| f.calibration_frames) * 100 / 60
            ),
        }
    }
    fn waveform(&mut self, area: Rect, wave: Option<&WaveformFrame>) {
        self.draw.rgba(0.15, 0.15, 0.2, 0.9);
        self.draw.fill(area);
        let Some(wave) = wave else {
            self.render_text(
                "No audio input detected",
                area.x + 10.,
                area.y + area.h / 2.,
                TextColor::rgb(200, 100, 100),
                0.6,
                true,
            );
            return;
        };
        self.draw.rgba(0.4, 0.4, 0.5, 0.6);
        unsafe {
            (self.draw.api.line)(
                self.draw.renderer,
                area.x,
                area.y + area.h / 2.,
                area.x + area.w,
                area.y + area.h / 2.,
            );
        }
        waveform_points(&mut self.wave_points, area, &wave.samples);
        self.draw.rgba(0.3, 0.92, 0.35, 1.);
        unsafe {
            (self.draw.api.lines)(
                self.draw.renderer,
                self.wave_points.as_ptr(),
                self.wave_points.len() as i32,
            );
        }
        self.render_text(
            "Audio Waveform (Oscilloscope)",
            area.x + 10.,
            area.y + 5.,
            TextColor::rgb(200, 255, 200),
            0.6,
            true,
        );
        let now = self.draw.now();
        if self.text.wave_stats.is_none() || now - self.text.wave_updated >= 100_000_000 {
            let color = if wave.rms > 0.01 {
                TextColor::rgb(100, 255, 100)
            } else {
                TextColor::rgb(200, 200, 200)
            };
            let text = format!(
                "RMS: {:.2}% | Peak: {:.1}%",
                wave.rms as f64 * 100.,
                wave.peak as f64 * 100.
            );
            if let Some(image) = self.text_image(&text, color) {
                self.text.wave_stats = Some(image);
                self.text.wave_updated = now;
            }
        }
        if let Some(image) = &self.text.wave_stats {
            let (w, h) = image.size(0.5);
            self.draw.texture(
                image.texture.raw,
                None,
                Rect {
                    x: area.x + 10.,
                    y: area.y + area.h - 25.,
                    w,
                    h,
                },
            );
        }
    }
    /// Draw one complete visual frame. `present=false` is a development-only
    /// capture hook: the production loop always presents the complete result.
    pub fn render(
        &mut self,
        model: &mut UiModel,
        now: f64,
        mouse: Point,
        reopening: bool,
        stream: bool,
        magnitudes: &[f32; 96],
        frame: Option<&SpectrumFrame>,
        wave: Option<&WaveformFrame>,
        present: bool,
    ) -> RenderFrameTiming {
        let start = self.draw.now();
        let (width, height) = self.window_size();
        model.refresh_layout(width, height);
        let mut sampled = false;
        self.draw.rgba(0.30, 0.92, 0.35, 1.);
        self.draw.clear();
        self.interface.ensure(&self.draw, width, height);
        if let Some(target) = &self.interface.main {
            sampled = self.draw.target(target.raw);
            if !sampled {
                self.draw.target(ptr::null_mut());
                self.interface.clear();
                self.interface.unavailable = true;
            }
        }
        if sampled && !self.draw.scale(self.interface.factor as f32) {
            self.draw.target(ptr::null_mut());
            self.interface.clear();
            self.interface.unavailable = true;
            sampled = false;
        }
        let mut ow = 0;
        let mut oh = 0;
        unsafe {
            if !(self.draw.api.output_size)(self.draw.renderer, &mut ow, &mut oh) {
                ow = width;
                oh = height;
            }
        }
        let Some(view) = ui_layout::viewport(ow, oh) else {
            return RenderFrameTiming::default();
        };
        if !sampled {
            let clip = IntRect {
                x: view.area.x as i32,
                y: view.area.y as i32,
                w: view.area.w as i32,
                h: view.area.h as i32,
            };
            unsafe {
                (self.draw.api.render_viewport)(self.draw.renderer, &clip);
            }
            self.draw.scale(view.scale);
        }
        self.draw.rgba(0.30, 0.92, 0.35, 1.);
        self.draw.clear();
        let animation = self.animation.advance(now, view.layout_width);
        for (area, color) in [
            (animation.left_background, animation.left_color),
            (animation.right_background, animation.right_color),
        ] {
            self.draw.rgba(color[0], color[1], color[2], color[3]);
            self.draw.fill(area);
        }
        let central = model.widget_mut("AnimatedWidget");
        central.move_x(animation.center.x);
        central.quad.figure_angle = animation.angle;
        self.draw.rgba(
            animation.orbit_shade,
            animation.orbit_shade,
            animation.orbit_shade,
            1.,
        );
        for area in animation.orbit {
            self.draw.fill(area);
        }
        model.render_update(mouse, reopening, stream);
        let soften = self.large.update(width, height);
        for widget in &model.widgets {
            if let Some(slider) = &widget.slider {
                let (bg, track) = widget.track();
                self.draw.color(slider.background);
                self.draw.fill(bg);
                self.draw.rgba(0., 0., 0., 1.);
                self.draw.fill(track);
                for (text, top) in [(slider.max_text, true), (slider.min_text, false)] {
                    let (tw, th) = self.size(text, 1.);
                    let y = if top {
                        bg.y - th - 8.
                    } else {
                        bg.y + bg.h + 8.
                    };
                    self.render_text(
                        text,
                        widget.center.x - tw / 2.,
                        y,
                        TextColor::rgb(0, 0, 0),
                        1.,
                        true,
                    );
                }
            }
            self.oblique.draw(
                &self.draw,
                &widget.vertices,
                widget.kind == WidgetKind::Animated,
                self.interface.factor,
                soften,
            );
            if matches!(widget.kind, WidgetKind::Button | WidgetKind::Info)
                && !widget.text.is_empty()
            {
                let center = Point {
                    x: (widget.hit_min.x + widget.hit_max.x) * 0.5,
                    y: (widget.hit_min.y + widget.hit_max.y) * 0.5,
                };
                let (tw, th) = self.size(&widget.text, widget.quad.zoom);
                if widget.kind == WidgetKind::Info {
                    self.info_text(
                        &widget.text,
                        center.x - tw / 2.,
                        center.y - th / 2.,
                        widget.text_color,
                        widget.quad.zoom,
                    );
                } else {
                    self.render_text(
                        &widget.text,
                        center.x - tw / 2.,
                        center.y - th / 2.,
                        widget.text_color,
                        widget.quad.zoom,
                        true,
                    );
                }
            }
        }
        let mut caption_sizes = [(0., 0.); 6];
        for (i, image) in self.captions.iter().enumerate() {
            if let Some(image) = image {
                let w = image.texture.w as f32 * 0.95 * 0.5;
                let h = image.texture.h as f32 * 0.95 * 0.5;
                caption_sizes[i] = (w, h);
                self.draw.texture(
                    image.texture.raw,
                    None,
                    Rect {
                        x: model.caption_x[i] - w / 2.,
                        y: 440.,
                        w,
                        h,
                    },
                );
            }
        }
        if let Some(target) = model.help(mouse, &caption_sizes, unsafe { (self.draw.api.ticks)() })
        {
            let left = ui_layout::left_expansion(view.layout_width);
            let right = view.layout_width - 1430. - left;
            let area = Rect {
                x: 660. + left,
                y: 130.,
                w: 750. + right,
                h: 110.,
            };
            self.draw.rgba(0.55, 0.55, 1., 1.);
            self.draw.fill(area);
            for (i, image) in self.help[target].iter().enumerate() {
                if let Some(image) = image {
                    let w = image.texture.w as f32 * 0.68 * 0.5;
                    let h = image.texture.h as f32 * 0.68 * 0.5;
                    self.draw.texture(
                        image.texture.raw,
                        None,
                        Rect {
                            x: area.x + 15.,
                            y: area.y + 20. + i as f32 * 35.,
                            w,
                            h,
                        },
                    );
                }
            }
        }
        let before_spectrum = self.draw.now();
        let gw = ui_layout::graph_width(view.layout_width);
        let graph = Rect {
            x: 20.,
            y: 510.,
            w: gw,
            h: 210.,
        };
        self.spectrum
            .draw(&self.draw, &mut self.scratch, graph, magnitudes, (ow, oh));
        let title = Self::spectrum_title(frame);
        if self.spectrum.overlay.is_none() {
            self.render_text(
                &title,
                graph.x + 10.,
                graph.y + 5.,
                TextColor::rgb(200, 200, 255),
                0.6,
                true,
            );
        }
        let before_wave = self.draw.now();
        self.waveform(
            Rect {
                x: 20.,
                y: 760.,
                w: gw,
                h: 210.,
            },
            wave,
        );
        let before_composite = self.draw.now();
        if sampled {
            let main = self.interface.main.as_ref().unwrap().raw;
            let mut result = main;
            let mut scale = self.interface.factor as f32;
            if let Some(half) = &self.interface.half {
                if self.draw.target(half.raw) {
                    let src = Rect {
                        x: 0.,
                        y: 0.,
                        w: view.layout_width * self.interface.factor as f32,
                        h: 1030. * self.interface.factor as f32,
                    };
                    let dest = Rect {
                        x: 0.,
                        y: 0.,
                        w: src.w * 0.5,
                        h: src.h * 0.5,
                    };
                    if self.draw.texture(main, Some(src), dest) {
                        result = half.raw;
                        scale = self.interface.factor as f32 * 0.5;
                    }
                }
            }
            if let Some(logical) = &self.interface.resolved {
                if view.scale < 1. && self.draw.target(logical.raw) {
                    if self.draw.texture(
                        result,
                        Some(Rect {
                            x: 0.,
                            y: 0.,
                            w: view.layout_width * scale,
                            h: 1030. * scale,
                        }),
                        Rect {
                            x: 0.,
                            y: 0.,
                            w: view.layout_width,
                            h: 1030.,
                        },
                    ) {
                        result = logical.raw;
                        scale = 1.;
                    }
                }
            }
            self.draw.target(ptr::null_mut());
            self.draw.texture(
                result,
                Some(Rect {
                    x: 0.,
                    y: 0.,
                    w: view.layout_width * scale,
                    h: 1030. * scale,
                }),
                view.area,
            );
        } else {
            self.draw.scale(1.);
            unsafe {
                (self.draw.api.render_viewport)(self.draw.renderer, ptr::null());
            }
        }
        if let Some((source, logical_source, destination)) = self.spectrum.overlay.take() {
            self.draw.scale(1.);
            unsafe {
                (self.draw.api.render_viewport)(self.draw.renderer, ptr::null());
            }
            if let Some(pixel) = &self.spectrum.t[4] {
                if !self.draw.texture(pixel.raw, Some(source), destination) {
                    if let Some(logical) = &self.spectrum.t[3] {
                        self.draw
                            .texture(logical.raw, Some(logical_source), destination);
                    }
                }
            }
            let scale = destination.h / 210.;
            self.render_text(
                &title,
                destination.x + 10. * scale,
                destination.y + 5. * scale,
                TextColor::rgb(200, 200, 255),
                0.6 * scale as f64,
                true,
            );
        }
        let before_present = self.draw.now();
        if present {
            unsafe {
                (self.draw.api.present)(self.draw.renderer);
            }
        }
        RenderFrameTiming {
            controls_ns: before_spectrum - start,
            spectrum_ns: before_wave - before_spectrum,
            waveform_ns: before_composite - before_wave,
            composite_ns: before_present - before_composite,
        }
    }
}
impl Drop for UiRenderer {
    fn drop(&mut self) {
        // Textures must die before their renderer; fonts before TTF_Quit. DLL Rc
        // handles then release after every native resource that can call into them.
        self.draw.target(ptr::null_mut());
        self.text = TextCache::default();
        self.captions.clear();
        self.help.clear();
        self.interface.clear();
        self.oblique.clear();
        self.spectrum.clear();
        unsafe {
            if let Some(ttf) = &self.ttf {
                if !self.font.is_null() {
                    (ttf.close_font)(self.font);
                    self.font = ptr::null_mut();
                }
                if self.ttf_initialized {
                    (ttf.quit)();
                }
            }
            (self.draw.api.destroy_renderer)(self.draw.renderer);
            (self.draw.api.destroy_window)(self.window);
            (self.draw.api.quit)();
        }
    }
}
