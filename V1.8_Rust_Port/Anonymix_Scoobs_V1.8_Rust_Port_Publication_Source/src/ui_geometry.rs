//! Renderer-independent V1.8 geometry from Visualization.cpp and Widgets.h.
//!
//! These arrays are the original SDL submissions, including the vertical halo
//! and its varying apparent thickness on steep slopes. This is not a replacement
//! contour, an added border, a blur pass or a new frequency normalization.
use crate::{observation::WAVEFORM_SAMPLES, spectrum::BARS};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// Two float logical coordinates; also matches SDL_FPoint in the native ABI.
pub struct Point {
    pub x: f32,
    pub y: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// Float x/y/width/height rectangle matching SDL_FRect.
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// Float RGBA geometry color. Alpha coverage is used by the original spectrum halo.
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// SDL-compatible position, float color and texture coordinate for triangle rendering.
pub struct Vertex {
    pub position: Point,
    pub color: Color,
    pub tex_coord: Point,
}

#[derive(Default)]
/// Reusable CPU triangle buffers for the approved spectrum fill and varying-width halo.
pub struct SpectrumGeometry {
    pub dense_curve: Vec<Point>,
    retained: Vec<u8>,
    intervals: Vec<(usize, usize)>,
    pub curve: Vec<Point>,
    pub fill_vertices: Vec<Vertex>,
    pub fill_indices: Vec<i32>,
    pub edge_vertices: Vec<Vertex>,
    pub edge_indices: Vec<i32>,
}
impl SpectrumGeometry {
    /// Match PreloadSpectrumGeometry. Scratch buffers belong to the visual
    /// thread and retain capacity between frames, as in the C++ source.
    pub fn preload(&mut self, width: f32) {
        if width <= 0.0 {
            return;
        }
        let count = (width * 8.0).ceil() as usize + 1;
        reserve_total(&mut self.dense_curve, count);
        reserve_total(&mut self.retained, count);
        reserve_total(&mut self.intervals, BARS * 8);
        reserve_total(&mut self.curve, count);
        reserve_total(&mut self.fill_vertices, count * 2);
        reserve_total(&mut self.fill_indices, (count - 1) * 6);
        reserve_total(&mut self.edge_vertices, count * 6);
        reserve_total(&mut self.edge_indices, (count - 1) * 5 * 6);
        self.ensure_indices(count);
    }
    /// Index topology depends on retained point count, never on colors/audio.
    fn ensure_indices(&mut self, count: usize) {
        if count < 2 {
            return;
        }
        let fill_count = (count - 1) * 6;
        if self.fill_indices.len() != fill_count {
            self.fill_indices.clear();
            reserve_total(&mut self.fill_indices, fill_count);
            for i in 0..count - 1 {
                let left = (i * 2) as i32;
                self.fill_indices.extend_from_slice(&[
                    left,
                    left + 1,
                    left + 2,
                    left + 1,
                    left + 2,
                    left + 3,
                ]);
            }
        }
        let edge_count = (count - 1) * 5 * 6;
        if self.edge_indices.len() != edge_count {
            self.edge_indices.clear();
            reserve_total(&mut self.edge_indices, edge_count);
            for i in 0..count - 1 {
                for band in 0..5 {
                    let tl = (i * 6 + band) as i32;
                    let bl = tl + 1;
                    let tr = ((i + 1) * 6 + band) as i32;
                    let br = tr + 1;
                    self.edge_indices
                        .extend_from_slice(&[tl, bl, tr, bl, tr, br]);
                }
            }
        }
    }
    /// Generate exactly the existing 96-band monotone cubic and bounded
    /// tessellation. Callers pass 8x normally, 4x on the inherited fallback.
    /// The 2x value is reserved for the original target-allocation fallback.
    pub fn build(&mut self, area: Rect, sampling_scale: u32, magnitudes: &[f32; BARS]) {
        assert!(area.w.is_finite() && area.w > 0.0 && area.h.is_finite() && area.h > 0.0);
        assert!(sampling_scale > 0);
        let Rect {
            x,
            y,
            w: width,
            h: height,
        } = area;
        self.dense_curve.clear();
        let segments = ((width * sampling_scale.min(8) as f32).ceil() as usize).max(1);
        reserve_total(&mut self.dense_curve, segments + 1);
        let at = |index: i32| magnitudes[index.clamp(0, BARS as i32 - 1) as usize];
        for step in 0..=segments {
            let fraction = step as f32 / segments as f32;
            let position = fraction * BARS as f32 - 0.5;
            let first = position.floor() as i32;
            let t = position - first as f32;
            let (p0, p1, p2, p3) = (at(first - 1), at(first), at(first + 1), at(first + 2));
            let before = p1 - p0;
            let between = p2 - p1;
            let after = p3 - p2;
            let start = if before * between > 0.0 {
                2.0 * before * between / (before + between)
            } else {
                0.0
            };
            let end = if between * after > 0.0 {
                2.0 * between * after / (between + after)
            } else {
                0.0
            };
            let t2 = t * t;
            let t3 = t2 * t;
            let cubic = (2.0 * t3 - 3.0 * t2 + 1.0) * p1
                + (t3 - 2.0 * t2 + t) * start
                + (-2.0 * t3 + 3.0 * t2) * p2
                + (t3 - t2) * end;
            let magnitude = cubic.clamp(p1.min(p2), p1.max(p2));
            self.dense_curve.push(Point {
                x: x + fraction * width,
                y: y + height - magnitude.clamp(0.0, 1.0) * height * 0.9,
            });
        }
        self.retained.clear();
        self.retained.resize(self.dense_curve.len(), 0);
        self.intervals.clear();
        for band in 0..BARS {
            let start = band * segments / BARS;
            let end = (band + 1) * segments / BARS;
            self.retained[start] = 1;
            self.retained[end] = 1;
            self.intervals.push((start, end));
            while let Some((first, last)) = self.intervals.pop() {
                if last <= first + 1 {
                    continue;
                }
                let a = self.dense_curve[first];
                let b = self.dense_curve[last];
                let inverse_span = 1.0 / (b.x - a.x);
                let mut error_bound = 0.035_f32;
                let mut worst = None;
                for sample in first + 1..last {
                    let point = self.dense_curve[sample];
                    let straight_y = a.y + (b.y - a.y) * ((point.x - a.x) * inverse_span);
                    let error = (point.y - straight_y).abs();
                    if error > error_bound {
                        error_bound = error;
                        worst = Some(sample);
                    }
                }
                if let Some(worst) = worst {
                    self.retained[worst] = 1;
                    self.intervals.push((first, worst));
                    self.intervals.push((worst, last));
                }
            }
        }
        self.curve.clear();
        reserve_total(&mut self.curve, self.dense_curve.len());
        for (&point, &keep) in self.dense_curve.iter().zip(&self.retained) {
            if keep != 0 {
                self.curve.push(point);
            }
        }
        self.fill_vertices.clear();
        if self.curve.len() >= 3 {
            reserve_total(&mut self.fill_vertices, self.curve.len() * 2);
            for &point in &self.curve {
                let hue = ((point.x - x) / width).clamp(0.0, 1.0);
                let intensity = if point.y < y + height {
                    1.0 - (point.y - y) / height
                } else {
                    0.0
                };
                let r = ((hue - 0.5) * 2.0 + intensity * 0.3).clamp(0.0, 1.0);
                let g = (1.0 - (hue - 0.5).abs() * 2.0 + intensity * 0.2).clamp(0.0, 1.0);
                let b = ((0.5 - hue) * 2.0 + intensity * 0.1).clamp(0.0, 1.0);
                self.fill_vertices.push(Vertex {
                    position: point,
                    color: Color { r, g, b, a: 0.6 },
                    ..Vertex::default()
                });
                self.fill_vertices.push(Vertex {
                    position: Point {
                        x: point.x,
                        y: y + height,
                    },
                    color: Color {
                        r: r * 0.3,
                        g: g * 0.3,
                        b: b * 0.3,
                        a: 0.2,
                    },
                    ..Vertex::default()
                });
            }
            self.ensure_indices(self.curve.len());
        }
        self.edge_vertices.clear();
        if self.curve.len() >= 2 {
            reserve_total(&mut self.edge_vertices, self.curve.len() * 6);
            let offsets = [-4.0, -2.0, -0.55, 0.55, 2.0, 4.0];
            let opacities = [0.0, 0.28, 1.0, 1.0, 0.28, 0.0];
            for &point in &self.curve {
                for i in 0..6 {
                    self.edge_vertices.push(Vertex {
                        position: Point {
                            x: point.x,
                            y: (point.y + offsets[i]).clamp(y + 1.0, y + height - 1.0),
                        },
                        color: Color {
                            r: 0.42,
                            g: 0.92,
                            b: 1.0,
                            a: opacities[i],
                        },
                        ..Vertex::default()
                    });
                }
            }
            self.ensure_indices(self.curve.len());
        }
    }
}
fn reserve_total<T>(values: &mut Vec<T>, wanted: usize) {
    if values.capacity() < wanted {
        values.reserve(wanted - values.len());
    }
}

/// Convert the same chronological microphone snapshot used in Stage 7 into
/// the original oscilloscope polyline. Statistics remain owned by the snapshot.
pub fn waveform_points(output: &mut Vec<Point>, area: Rect, recent: &[f32; WAVEFORM_SAMPLES]) {
    output.clear();
    reserve_total(output, WAVEFORM_SAMPLES);
    let step = area.w / WAVEFORM_SAMPLES as f32;
    for (i, &value) in recent.iter().enumerate() {
        let sample = value.clamp(-1.0, 1.0);
        output.push(Point {
            x: area.x + i as f32 * step,
            y: area.y + area.h / 2.0 - sample * area.h * 0.45,
        });
    }
}

/// Inputs to UiWidget::updatePos. Colors are supplied independently by the
/// widget; the original six positions preserve the diagonal shared by triangles.
#[derive(Clone, Copy, Debug)]
pub struct QuadGeometry {
    pub center: Point,
    pub half_diagonal: [u16; 2],
    pub diagonal_angle: f64,
    pub figure_angle: f64,
    pub angle_offset: f64,
    pub zoom: f64,
}
impl QuadGeometry {
    /// Construct the original centered quad from integer half-dimensions.
    pub fn rectangle(center: Point, half_width: u16, half_height: u16) -> Self {
        let w = half_width as f64;
        let h = half_height as f64;
        let diag = w.hypot(h).round().clamp(0.0, u16::MAX as f64) as u16;
        let corner = if w != 0.0 || h != 0.0 {
            h.atan2(w)
        } else {
            0.0
        };
        let angle =
            (std::f64::consts::PI - 2.0 * corner + std::f64::consts::PI) % std::f64::consts::PI;
        Self {
            center,
            half_diagonal: [diag, diag],
            diagonal_angle: angle,
            figure_angle: 0.0,
            angle_offset: corner,
            zoom: 1.0,
        }
    }
    /// Expand this quad into the original two triangles, retaining vertex order and rounding.
    pub fn positions(&self) -> [Point; 6] {
        let mut points = [Point::default(); 6];
        for i in 0..2 {
            for j in 0..3 {
                let angle = if j == 1 { self.diagonal_angle } else { 0.0 }
                    + (((2 * i + j) % 4) / 2) as f64 * std::f64::consts::PI;
                let distance = self.half_diagonal[usize::from(j == 1)] as f64 * self.zoom;
                points[i * 3 + j] = Point {
                    x: (self.center.x as f64
                        + distance * (angle + self.figure_angle + self.angle_offset).cos())
                        as f32,
                    y: (self.center.y as f64
                        + distance * (angle + self.figure_angle + self.angle_offset).sin())
                        as f32,
                };
            }
        }
        points
    }
}
