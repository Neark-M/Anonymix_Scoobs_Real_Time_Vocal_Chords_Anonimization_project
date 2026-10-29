//! Frame-dependent glitch counters plus wall-clock rotation from AppRender.cpp.
//! The unusual repeated counter weights and uint16 center quantization are
//! inherited behavior. Replacing them with a new animation would change V1.8.
use crate::ui_geometry::{Point, Rect};
use crate::ui_layout::{left_expansion, LAYOUT_WIDTH};
use std::f64::consts::PI;

#[derive(Clone, Debug)]
/// One computed central quad and two orbiting squares with the inherited glitch/color behavior.
pub struct AnimationFrame {
    pub seven_bits: i32,
    pub ten_bits: i32,
    pub left_background: Rect,
    pub right_background: Rect,
    pub left_color: [f32; 4],
    pub right_color: [f32; 4],
    pub center: Point,
    pub angle: f64,
    pub orbit_shade: f32,
    pub orbit: [Rect; 2],
}
#[derive(Default)]
/// Main-thread animation history, including the original counter-dependent rotation quirks.
pub struct VisualizerAnimation {
    toggles: [bool; 10],
}
impl VisualizerAnimation {
    /// `now` is SDL milliseconds converted to seconds, not a new audio clock.
    /// Advance exactly once per rendered visual frame, including lower rungs.
    pub fn advance(&mut self, now: f64, layout_width: f32) -> AnimationFrame {
        let weights7 = [1, 4, 32, 32, 16, 32, 64];
        let weights10 = [1, 4, 8, 8, 16, 32, 64, 128, 256, 512];
        let seven = self.toggles[..7]
            .iter()
            .zip(weights7)
            .map(|(&v, w)| i32::from(v) * w)
            .sum();
        let ten = self
            .toggles
            .iter()
            .zip(weights10)
            .map(|(&v, w)| i32::from(v) * w)
            .sum();
        self.toggles[0] = !self.toggles[0];
        for i in 1..10 {
            self.toggles[i] = !(!self.toggles[i] ^ self.toggles[..i].iter().all(|v| !v));
        }
        let phase_color = |phase: f64| {
            [
                (0.5 + 0.5 * phase.sin()) as f32,
                (0.5 + 0.5 * (phase + PI * 2.0 / 3.0).sin()) as f32,
                (0.5 + 0.5 * (phase + PI * 4.0 / 3.0).sin()) as f32,
                1.0,
            ]
        };
        // Preserve the separate additions used for the right background.
        let right_color = [
            (0.5 + 0.5 * (now + PI).sin()) as f32,
            (0.5 + 0.5 * (now + PI + PI * 2.0 / 3.0).sin()) as f32,
            (0.5 + 0.5 * (now + PI + PI * 4.0 / 3.0).sin()) as f32,
            1.0,
        ];
        let left = left_expansion(layout_width);
        let _right = layout_width - LAYOUT_WIDTH as f32 - left;
        let background_width = 320.0 + left * 0.5;
        let visualizer_center = (background_width + background_width) * 0.5;
        // UiWidget::setCenterX accepts uint16_t: do not silently remove the
        // original truncation when moving the figure into Rust-owned state.
        let center_x = (visualizer_center as f64 + 90.0f32 as f64 * (PI * ten as f64 / 512.0).sin())
            as u16 as f32;
        let glitch = now * 9.0 + PI / 128.0 * seven as f64;
        let orbit = |angle: f64, other: f64| Rect {
            x: (center_x as i32 - 20 + (160.0 * angle.sin()) as i32) as f32,
            y: (240 - 20 + (160.0 * other.sin()) as i32) as f32,
            w: 40.0,
            h: 40.0,
        };
        AnimationFrame {
            seven_bits: seven,
            ten_bits: ten,
            left_background: Rect {
                x: 0.0,
                y: 0.0,
                w: background_width,
                h: 480.0,
            },
            right_background: Rect {
                x: background_width,
                y: 0.0,
                w: background_width,
                h: 480.0,
            },
            left_color: phase_color(now),
            right_color,
            center: Point {
                x: center_x,
                y: 240.0,
            },
            angle: (now * 24.0 - PI / 128.0 * seven as f64) % (2.0 * PI),
            orbit_shade: (right_color[0] * 0.5).clamp(0.0, 0.5),
            orbit: [
                orbit(glitch, glitch + PI / 2.0),
                orbit(glitch + PI, glitch + PI * 1.5),
            ],
        }
    }
}
