//! Used V1.8 widget types, geometry/hit branches, slider quantization and input state.
//! Stable vector indices replace C++ pointers into the named registry. The
//! original render order, vertex sorting, integer edge bounds and hover zoom
//! timing are retained; this is not a new point-in-polygon algorithm.
use crate::{
    sdl_abi::TextColor,
    ui_geometry::{Color, Point, QuadGeometry, Rect, Vertex},
    ui_layout::ResponsiveGeometry,
    Settings,
};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Only widget categories actually used by the original V1.8 screen.
pub enum WidgetKind {
    Animated,
    Button,
    Slider,
    Info,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Logical effect/control action produced by releasing a button on its original hit target.
pub enum UiAction {
    AudioMode,
    WasapiMode,
    Reopen,
    Record,
    Play,
    Save,
    Effect(u8),
}
#[derive(Clone)]
/// Original integer handle position and floating legend/range; their rounding is intentionally distinct.
pub struct Slider {
    pub minimum: f64,
    pub maximum: f64,
    pub value: f64,
    pub min_text: &'static str,
    pub max_text: &'static str,
    pub percent: bool,
    pub background: Color,
}
/// Owned quad, caption, color and optional slider state. Collection indexes replace raw widget pointers.
pub struct Widget {
    pub name: &'static str,
    pub kind: WidgetKind,
    pub action: Option<UiAction>,
    pub text: String,
    pub text_color: TextColor,
    pub quad: QuadGeometry,
    pub vertices: [Vertex; 6],
    pub center: Point,
    pub handle_offset: Point,
    pub zoom_factor: f64,
    pub pressed: bool,
    pub slider: Option<Slider>,
    sorted_x: [usize; 4],
    sorted_y: [usize; 4],
    pub hit_min: Point,
    pub hit_max: Point,
}
impl Widget {
    fn new(name: &'static str, kind: WidgetKind) -> Self {
        let quad = QuadGeometry {
            center: Point::default(),
            half_diagonal: [50, 50],
            diagonal_angle: PI / 2.,
            figure_angle: 0.,
            angle_offset: 0.,
            zoom: 1.,
        };
        let mut widget = Self {
            name,
            kind,
            action: None,
            text: String::new(),
            text_color: TextColor::rgb(0, 0, 0),
            quad,
            vertices: [Vertex::default(); 6],
            center: Point::default(),
            handle_offset: Point::default(),
            zoom_factor: 1.15,
            pressed: false,
            slider: None,
            sorted_x: [0, 1, 3, 4],
            sorted_y: [0, 1, 3, 4],
            hit_min: Point::default(),
            hit_max: Point::default(),
        };
        widget.update_vertices();
        widget.sort(true, true);
        widget.set_color([0., 0., 0.]);
        widget
    }
    /// Set the base color and the inherited per-vertex gradient without adding a border.
    pub fn set_color(&mut self, rgb: [f32; 3]) {
        for v in &mut self.vertices {
            v.color = Color {
                r: rgb[0],
                g: rgb[1],
                b: rgb[2],
                a: 1.,
            };
        }
    }
    /// Update quad dimensions with the original integer half-width/height geometry.
    pub fn rectangle(&mut self, half_width: u16, half_height: u16) {
        let geometry = QuadGeometry::rectangle(self.quad.center, half_width, half_height);
        self.quad.half_diagonal = geometry.half_diagonal;
        self.quad.diagonal_angle = geometry.diagonal_angle;
        self.quad.angle_offset = geometry.angle_offset;
        self.adapt_zoom();
        self.update_vertices();
    }
    /// Move the quad horizontally while retaining its shape and original rounding.
    pub fn move_x(&mut self, x: f32) {
        self.center.x = x as u16 as f32;
        self.quad.center.x = self.center.x + self.handle_offset.x;
    }
    /// Move the quad vertically while retaining its shape and original rounding.
    pub fn move_y(&mut self, y: f32) {
        self.center.y = y as u16 as f32;
        self.quad.center.y = self.center.y + self.handle_offset.y;
    }
    fn adapt_zoom(&mut self) {
        let longest = self.quad.half_diagonal[0].max(self.quad.half_diagonal[1]) as f64;
        self.zoom_factor = if longest > 110. {
            1. + 0.15 * (110. / longest).powf(1.4)
        } else {
            1.15
        };
    }
    fn update_vertices(&mut self) {
        for (v, p) in self.vertices.iter_mut().zip(self.quad.positions()) {
            v.position = p;
        }
    }
    fn sort(&mut self, x: bool, y: bool) {
        for i in 0..3 {
            for j in i + 1..4 {
                if x && self.vertices[self.sorted_x[j]].position.x
                    < self.vertices[self.sorted_x[i]].position.x
                {
                    self.sorted_x.swap(i, j);
                }
                if y && self.vertices[self.sorted_y[j]].position.y
                    < self.vertices[self.sorted_y[i]].position.y
                {
                    self.sorted_y.swap(i, j);
                }
            }
        }
    }
    /// Rebuild rounded/zoomed quad positions and original edge tests from the current pointer.
    pub fn update_pos(&mut self, mouse: Point) -> bool {
        self.update_vertices();
        // Constructor pointer order and the inherited partial-order recheck matter
        // when the same figure rotates through an extreme. Do not sort fresh copies.
        let px = |i: usize| self.vertices[self.sorted_x[i]].position.x;
        let py = |i: usize| self.vertices[self.sorted_y[i]].position.y;
        let sx = px(0) > px(1) || px(0) > px(2) || px(1) > px(2);
        let sy = py(0) > py(1) || py(0) > py(2) || py(1) > py(2);
        self.sort(sx, sy);
        self.hit_min = Point {
            x: self.vertices[self.sorted_x[0]].position.x,
            y: self.vertices[self.sorted_y[0]].position.y,
        };
        self.hit_max = Point {
            x: self.vertices[self.sorted_x[3]].position.x,
            y: self.vertices[self.sorted_y[3]].position.y,
        };
        let p = self.sorted_x.map(|i| self.vertices[i].position);
        let inside = if mouse.x < self.hit_min.x
            || mouse.x > self.hit_max.x
            || mouse.y < self.hit_min.y
            || mouse.y > self.hit_max.y
        {
            false
        } else if (p[0].x == p[1].x && p[0].y == p[2].y) || (p[0].x == p[2].x && p[0].y == p[1].y) {
            true
        } else {
            let lower_first = p[1].y <= self.quad.center.y;
            let left = mouse.x >= p[0].x && mouse.x <= p[1].x;
            let degenerate = p[0].x == p[3].x || p[1].x == p[2].x;
            let middle = !degenerate && mouse.x >= p[1].x && mouse.x <= p[2].x;
            // Same edge pairs as Widgets.h, including the special equal-X branch.
            let (low, high) = if left {
                if lower_first {
                    ((0, 1), (0, 2))
                } else {
                    ((0, 2), (0, 1))
                }
            } else if middle {
                if lower_first {
                    ((1, 3), (0, 2))
                } else {
                    ((0, 2), (1, 3))
                }
            } else if lower_first {
                ((1, 3), (2, 3))
            } else {
                ((2, 3), (1, 3))
            };
            let boundary = |(a, b): (usize, usize)| {
                // C++ computes the quotient in float, then stores a double slope, then
                // truncates the double boundary to int. Preserve all three conversions.
                let slope = ((p[b].y - p[a].y) / (p[b].x - p[a].x)) as f64;
                (p[a].y as f64 + slope * (mouse.x - p[a].x) as f64) as i32
            };
            mouse.y >= boundary(low) as f32 && mouse.y <= boundary(high) as f32
        };
        self.quad.zoom = if inside { self.zoom_factor } else { 1. };
        inside
    }
    fn pressed_color(&mut self) {
        let offset = (0.5 - f64::from(self.pressed)) * 2. * 0.15_f32 as f64;
        for v in &mut self.vertices {
            v.color.r = (v.color.r as f64 + offset) as f32;
            v.color.g = (v.color.g as f64 + offset) as f32;
            v.color.b = (v.color.b as f64 + offset) as f32;
        }
    }
    /// Return the original outer and inner slider track rectangles.
    pub fn track(&self) -> (Rect, Rect) {
        let width = (self.quad.half_diagonal[0] as f32 * 1.5).max(8.);
        (
            Rect {
                x: self.center.x - width / 2.,
                y: self.center.y - 230.,
                w: width,
                h: 460.,
            },
            Rect {
                x: self.center.x - (width / 4.).max(12.) / 2.,
                y: self.center.y - 220.,
                w: (width / 4.).max(12.),
                h: 440.,
            },
        )
    }
    /// Move and clamp the integer slider handle, then convert it to the floating legend value.
    pub fn drag(&mut self, mouse: Point, offset: Point) {
        if self.slider.is_none() {
            return;
        }
        let low = self.center.y as f64 - 200.;
        let high = self.center.y as f64 + 200.;
        let y = ((mouse.y - offset.y) as f64).clamp(low, high);
        self.quad.center.y = y as u16 as f32;
        self.handle_offset.y = self.quad.center.y - self.center.y;
        self.quad.center.x = self.center.x;
        self.handle_offset.x = 0.;
        let slider = self.slider.as_mut().unwrap();
        slider.value = slider.maximum
            - (self.quad.center.y as f64 - low) * (slider.maximum - slider.minimum) / (high - low);
    }
    /// Format the original displayed numeric legend, including its inherited precision.
    pub fn value_text(&self) -> String {
        self.slider
            .as_ref()
            .map(|s| format!("{:.2}{}", s.value, if s.percent { " %" } else { " pts" }))
            .unwrap_or_default()
    }
}

pub const EFFECT_LABELS: [&str; 5] = ["Pitch", "Formant", "Timbre", "Prosody", "Jitter"];
pub const SLIDER_NAMES: [&str; 6] = ["Pitch", "Formant", "Timbre", "Prosody", "Jitter", "Volume"];
pub const EFFECT_HELP: [[&str; 2]; 5] = [
    [
        "Pitch changes how high or low your voice sounds.",
        "Zero semitones leaves its pitch unchanged.",
    ],
    [
        "Formant changes the apparent vowel and voice colour.",
        "Move its ratio to change that character.",
    ],
    [
        "Timbre changes how bright or dark your voice sounds.",
        "Positive is brighter; negative is darker.",
    ],
    [
        "Prosody adds a rising or falling gesture at voice onsets.",
        "It does not recognize words or their meaning.",
    ],
    [
        "Jitter adds small, irregular changes to voice pitch.",
        "Zero is neutral; try 0.020 on a held vowel.",
    ],
];

/// Main-thread widget collection, selection mask, pressed/hovered indexes and tooltip timing.
pub struct UiModel {
    pub widgets: Vec<Widget>,
    pub mask: u8,
    pub hovered: Option<usize>,
    pub pressed: Option<usize>,
    pub click_offset: Point,
    pub caption_x: [f32; 6],
    pub layout_width: f32,
    cached_layout: i32,
    help_target: Option<usize>,
    help_since: u64,
    pointer: Point,
}
impl Default for UiModel {
    fn default() -> Self {
        let mut ui = Self {
            widgets: Vec::with_capacity(19),
            mask: 3,
            hovered: None,
            pressed: None,
            click_offset: Point::default(),
            caption_x: [715., 843., 971., 1099., 1227., 1355.],
            layout_width: 1430.,
            cached_layout: -1,
            help_target: None,
            help_since: 0,
            pointer: Point { x: -1., y: -1. },
        };
        let mut animated = Widget::new("AnimatedWidget", WidgetKind::Animated);
        animated.move_x(320.);
        animated.move_y(240.);
        ui.widgets.push(animated);
        ui.add_button(
            "AudioMode",
            UiAction::AudioMode,
            800.,
            65.,
            140,
            40,
            "Mode: Recording",
            [0.8, 0.8, 0.8],
        );
        ui.add_button(
            "WasapiMode",
            UiAction::WasapiMode,
            1060.,
            65.,
            100,
            40,
            "Audio: Starting...",
            [0.60, 0.85, 0.95],
        );
        ui.add_button(
            "AudioDevices",
            UiAction::Reopen,
            1295.,
            65.,
            115,
            40,
            "Reopen Audio",
            [0.55, 0.85, 1.],
        );
        ui.add_button(
            "PA_Record",
            UiAction::Record,
            1130.,
            300.,
            90,
            35,
            "Start Rec",
            [1., 0.55, 0.55],
        );
        for (i, (name, x, y)) in [
            ("EffectPitch", 750., 300.),
            ("EffectFormant", 940., 300.),
            ("EffectTimbre", 750., 385.),
            ("EffectProsody", 940., 385.),
            ("EffectJitter", 1130., 385.),
        ]
        .into_iter()
        .enumerate()
        {
            ui.add_button(name, UiAction::Effect(1 << i), x, y, 90, 35, "", [0.; 3]);
            ui.effect_palette(i);
        }
        ui.add_button(
            "PA_PlaySelected",
            UiAction::Play,
            1320.,
            300.,
            90,
            35,
            "Play",
            [0.9, 0.9, 0.55],
        );
        ui.add_button(
            "PA_SaveSelected",
            UiAction::Save,
            1320.,
            385.,
            90,
            35,
            "Save WAV",
            [0.70, 0.88, 0.85],
        );
        let names = [
            "Slider_PitchShift",
            "Slider_FormantShift",
            "Slider_Timbre",
            "Slider_Prosody",
            "Slider_Jitter",
            "Slider_Volume",
        ];
        let bg = [
            [0.3, 0.3, 0.55],
            [0.55, 0.3, 0.3],
            [0.25, 0.50, 0.55],
            [0.50, 0.38, 0.68],
            [0.3, 0.55, 0.3],
            [0.55, 0.55, 0.3],
        ];
        let tri0 = [
            [0., 0.65, 0.],
            [0.65, 0., 0.],
            [0., 0.70, 0.75],
            [0.45, 0.23, 0.75],
            [0., 0.65, 0.],
            [0.65, 0.65, 0.],
        ];
        let tri1 = [
            [0., 1., 0.],
            [1., 0., 0.],
            [0.25, 1., 1.],
            [0.78, 0.62, 1.],
            [0., 1., 0.],
            [1., 1., 0.],
        ];
        let special0 = [
            [0., 0.15, 0.],
            [0.15, 0., 0.],
            [0., 0.35, 0.40],
            [0.46, 0.22, 0.72],
        ];
        let special1 = [
            [0.75, 1., 0.75],
            [1., 0.75, 0.75],
            [0.75, 1., 1.],
            [0.83, 0.71, 1.],
        ];
        let ranges = [
            (-12., 12., 0., "-12 ST", "+12 ST"),
            (0.5, 2., 1., "0.5x", "2.0x"),
            (-100., 100., 0., "-100%", "+100%"),
            (-100., 100., 0., "-100%", "+100%"),
            (0., 0.02_f32 as f64, 0., "0.000", "0.020"),
            (0., 100., 50., "0%", "100%"),
        ];
        for i in 0..6 {
            let mut w = Widget::new(names[i], WidgetKind::Slider);
            w.quad.half_diagonal = [55, 40];
            w.adapt_zoom();
            w.move_x(715. + i as f32 * 128.);
            w.move_y(740.);
            for j in 0..6 {
                let rgb = if j < 3 { tri0[i] } else { tri1[i] };
                w.vertices[j].color = Color {
                    r: rgb[0],
                    g: rgb[1],
                    b: rgb[2],
                    a: 1.,
                };
            }
            if i < 4 {
                for (j, rgb) in [(1, special0[i]), (4, special1[i])] {
                    w.vertices[j].color = Color {
                        r: rgb[0],
                        g: rgb[1],
                        b: rgb[2],
                        a: 1.,
                    };
                }
            }
            let (min, max, val, min_text, max_text) = ranges[i];
            // App.cpp supplies ideal double slider values separately from uint16 handles.
            // Formant's neutral handle is 806, while its initial legend is exactly 1.00.
            let position = 540. + (max - val) / (max - min) * 400.;
            w.quad.center.y = position as u16 as f32;
            w.handle_offset.y = w.quad.center.y - 740.;
            w.slider = Some(Slider {
                minimum: min,
                maximum: max,
                value: val,
                min_text,
                max_text,
                percent: matches!(i, 2 | 3 | 5),
                background: Color {
                    r: bg[i][0],
                    g: bg[i][1],
                    b: bg[i][2],
                    a: 1.,
                },
            });
            ui.widgets.push(w);
        }
        let mut info = Widget::new("WidgetInfo", WidgetKind::Info);
        info.rectangle(375, 55);
        info.move_x(1035.);
        info.move_y(185.);
        info.set_color([0.55, 0.55, 1.]);
        ui.widgets.push(info);
        ui
    }
}
impl UiModel {
    fn add_button(
        &mut self,
        name: &'static str,
        action: UiAction,
        x: f32,
        y: f32,
        w: u16,
        h: u16,
        text: &str,
        rgb: [f32; 3],
    ) {
        let mut widget = Widget::new(name, WidgetKind::Button);
        widget.action = Some(action);
        widget.set_color(rgb);
        widget.rectangle(w, h);
        widget.move_x(x);
        widget.move_y(y);
        widget.text = text.into();
        self.widgets.push(widget);
    }
    /// Borrow a known widget by its original stable name; unknown names indicate a programming error.
    pub fn widget(&self, name: &str) -> &Widget {
        self.widgets
            .iter()
            .find(|w| w.name == name)
            .expect("Frozen widget name")
    }
    /// Mutably borrow a known widget on the UI thread; callbacks never access this collection.
    pub fn widget_mut(&mut self, name: &str) -> &mut Widget {
        self.widgets
            .iter_mut()
            .find(|w| w.name == name)
            .expect("Frozen widget name")
    }
    /// Apply visibly distinct selected/unselected DSP colors for the given effect index.
    pub fn effect_palette(&mut self, effect: usize) {
        let bit = 1 << effect;
        let enabled = self.mask & bit != 0;
        let w = self
            .widgets
            .iter_mut()
            .find(|w| w.action == Some(UiAction::Effect(bit)))
            .unwrap();
        w.text = format!(
            "{}: {}",
            EFFECT_LABELS[effect],
            if enabled { "ON" } else { "OFF" }
        );
        w.set_color(if enabled {
            [0.05, 0.38, 0.16]
        } else {
            [0.70, 0.74, 0.76]
        });
        w.text_color = if enabled {
            TextColor::rgb(255, 255, 255)
        } else {
            TextColor::rgb(20, 28, 34)
        };
    }
    /// Toggle one effect bit and immediately update the corresponding control palette.
    pub fn toggle_effect(&mut self, bit: u8) {
        self.mask ^= bit;
        self.effect_palette(bit.trailing_zeros() as usize);
    }
    /// Reapply original responsive placements before hit testing, retaining previous-pointer rounding order.
    pub fn refresh_layout(&mut self, width: i32, height: i32) {
        let geo = ResponsiveGeometry::for_window(width, height);
        self.layout_width = crate::ui_layout::layout_width(width, height);
        if geo.rounded_layout_width == self.cached_layout {
            return;
        }
        self.cached_layout = geo.rounded_layout_width;
        self.caption_x = geo.captions;
        let pointer = self.pointer;
        for placement in geo.controls {
            let widget = self.widget_mut(placement.name);
            if placement.half_width > 0 {
                widget.rectangle(placement.half_width, placement.half_height);
                widget.update_pos(pointer);
            }
            widget.move_x(placement.center.x);
        }
    }
    fn eligible(widget: &Widget, reopening: bool, stream: bool) -> bool {
        if widget.kind != WidgetKind::Button {
            return true;
        }
        if reopening {
            return matches!(widget.action, Some(UiAction::Reopen | UiAction::WasapiMode));
        }
        !stream
            || matches!(
                widget.action,
                Some(
                    UiAction::AudioMode
                        | UiAction::WasapiMode
                        | UiAction::Reopen
                        | UiAction::Effect(_)
                )
            )
    }
    /// Find the original eligible hit target at this logical pointer; reopening/Stream gates still apply.
    pub fn hit_test(&mut self, mouse: Point, reopening: bool, stream: bool) -> Option<usize> {
        self.pointer = mouse;
        let mut hit = None;
        if mouse.x < 0. || mouse.y < 0. || mouse.x >= self.layout_width || mouse.y >= 1030. {
            return None;
        }
        for (i, w) in self.widgets.iter_mut().enumerate() {
            if w.update_pos(mouse) && Self::eligible(w, reopening, stream) {
                hit = Some(i);
            }
        }
        hit
    }
    /// Retain the fresh pressed target and drag offset; begin the original pressed-color transition.
    pub fn mouse_down(&mut self, mouse: Point, reopening: bool, stream: bool) {
        self.hovered = self.hit_test(mouse, reopening, stream);
        self.pressed = self.hovered;
        if let Some(i) = self.pressed {
            let w = &mut self.widgets[i];
            w.pressed = true;
            self.click_offset = Point {
                x: mouse.x - w.quad.center.x,
                y: mouse.y - w.quad.center.y,
            };
            w.pressed_color();
        }
    }
    /// Produce an action only if release hits the same current button that was pressed.
    pub fn release_action(
        &mut self,
        mouse: Point,
        reopening: bool,
        stream: bool,
    ) -> Option<UiAction> {
        self.hovered = self.hit_test(mouse, reopening, stream);
        self.pressed
            .filter(|i| Some(*i) == self.hovered)
            .and_then(|i| self.widgets[i].action)
    }
    /// Call after dispatch: C++ recolors the newly changed button on release.
    pub fn end_press(&mut self) {
        if let Some(i) = self.pressed.take() {
            let w = &mut self.widgets[i];
            w.pressed = false;
            w.pressed_color();
        }
    }
    /// Advance hover/drag/animation-related widget state once per visual frame.
    pub fn render_update(&mut self, mouse: Point, reopening: bool, stream: bool) {
        self.pointer = mouse;
        self.hovered = None;
        let mut info = self.widget("WidgetInfo").text.clone();
        let mut any = false;
        for (i, w) in self.widgets.iter_mut().enumerate() {
            if Some(i) == self.pressed && w.pressed {
                w.drag(mouse, self.click_offset);
                info = w.value_text();
            }
            if w.update_pos(mouse) {
                if Self::eligible(w, reopening, stream) {
                    self.hovered = Some(i);
                    any = true;
                    if w.kind == WidgetKind::Slider {
                        info = w.value_text();
                    }
                } else {
                    w.quad.zoom = 1.;
                }
            }
        }
        if !any {
            info.clear();
        }
        self.widget_mut("WidgetInfo").text = info;
    }
    /// Copy the current slider legends and effect mask into the plain callback control value.
    pub fn settings(&self) -> Settings {
        let mut values = [0.; 6];
        for (i, s) in self
            .widgets
            .iter()
            .filter_map(|w| w.slider.as_ref())
            .enumerate()
        {
            values[i] = s.value as f32;
        }
        Settings {
            mask: self.mask,
            pitch: values[0],
            formant: values[1],
            timbre: values[2],
            prosody: values[3],
            jitter: values[4],
            volume: values[5] / 100.,
        }
    }
    /// Return an effect description only after the same button/caption has been hovered for one second.
    pub fn help(
        &mut self,
        mouse: Point,
        caption_sizes: &[(f32, f32); 6],
        ticks: u64,
    ) -> Option<usize> {
        let mut target = self.hovered.and_then(|i| match self.widgets[i].action {
            Some(UiAction::Effect(bit)) => Some(bit.trailing_zeros() as usize),
            _ => None,
        });
        if target.is_none() {
            for i in 0..5 {
                let (w, h) = caption_sizes[i];
                if mouse.x >= self.caption_x[i] - w / 2.
                    && mouse.x <= self.caption_x[i] + w / 2.
                    && mouse.y >= 440.
                    && mouse.y <= 440. + h
                {
                    target = Some(i);
                    break;
                }
            }
        }
        if target != self.help_target {
            self.help_target = target;
            self.help_since = ticks;
        }
        target.filter(|_| ticks - self.help_since >= 1000)
    }
}
