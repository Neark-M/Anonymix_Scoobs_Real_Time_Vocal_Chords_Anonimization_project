//! Compare the port with frozen outputs from extracted V1.8 C++ source.
//! No SDL window, C++ compiler or GPU is needed to execute these fixtures.
use anonymix_v18_dsp_port::{
    ui_animation::VisualizerAnimation,
    ui_budget::AdaptiveVisualBudget,
    ui_geometry::{waveform_points, Point, QuadGeometry, Rect, SpectrumGeometry, Vertex},
    ui_layout::{fit_monitor, viewport, window_point_to_ui, ResponsiveGeometry, WindowSize},
    ui_raster::{
        interface_factor, spectrum_pixel_fit, LargeWindowSmoothing, ObliqueTile, SpectrumTargets,
    },
    ui_timing::{DeadlinePacer, FramePressure, RenderFrameTiming},
};
struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let result = self.data[self.offset..self.offset + N].try_into().unwrap();
        self.offset += N;
        result
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.bytes())
    }
    fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.bytes())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.bytes())
    }
    fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.bytes())
    }
    fn f64(&mut self) -> f64 {
        f64::from_le_bytes(self.bytes())
    }
    fn point(&mut self) -> Point {
        Point {
            x: self.f32(),
            y: self.f32(),
        }
    }
    fn rect(&mut self) -> Rect {
        Rect {
            x: self.f32(),
            y: self.f32(),
            w: self.f32(),
            h: self.f32(),
        }
    }
    fn count(&mut self) -> u32 {
        assert_eq!(&self.bytes::<8>(), b"ANONUI08");
        self.u32()
    }
    fn finished(&self) {
        assert_eq!(self.offset, self.data.len());
    }
}
fn close(actual: f32, expected: f32, context: &str) {
    let tolerance = 0.0001 + expected.abs() * 0.000001;
    assert!(
        actual.is_finite() && (actual - expected).abs() <= tolerance,
        "{context}: Rust {actual}, C++ {expected}, difference {}",
        (actual - expected).abs()
    );
}
fn point(actual: Point, input: &mut Reader<'_>, context: &str) {
    close(actual.x, input.f32(), context);
    close(actual.y, input.f32(), context);
}
fn rect(actual: Rect, input: &mut Reader<'_>, context: &str) {
    for value in [actual.x, actual.y, actual.w, actual.h] {
        close(value, input.f32(), context);
    }
}

#[test]
fn responsive_layout_pointer_monitor_fit_and_raster_dimensions_match_cpp() {
    let mut input = Reader::new(include_bytes!("../validation/fixtures/ui_layout_input.bin"));
    let mut expected = Reader::new(include_bytes!(
        "../validation/fixtures/ui_layout_expected.bin"
    ));
    let mut smoothing = LargeWindowSmoothing::default();
    for _ in 0..input.count() {
        let width = input.i32();
        let height = input.i32();
        let ow = input.i32();
        let oh = input.i32();
        let usable = WindowSize {
            width: input.i32(),
            height: input.i32(),
        };
        let mouse = input.point();
        let maximum = input.i32();
        let view = viewport(width, height).unwrap();
        rect(view.area, &mut expected, "viewport");
        close(view.scale, expected.f32(), "scale");
        close(view.layout_width, expected.f32(), "layout width");
        let hit = window_point_to_ui(width, height, mouse);
        assert_eq!(
            hit.is_some(),
            expected.u32() != 0,
            "window point {mouse:?} at {width}x{height}"
        );
        point(hit.unwrap_or_default(), &mut expected, "pointer");
        let fit = fit_monitor(usable, WindowSize { width, height }).unwrap();
        for size in [fit.initial, fit.minimum, fit.current] {
            assert_eq!(size.width, expected.i32());
            assert_eq!(size.height, expected.i32());
        }
        let geometry = ResponsiveGeometry::for_window(width, height);
        assert_eq!(geometry.rounded_layout_width, expected.i32());
        assert_eq!(geometry.controls.len(), 18);
        for control in geometry.controls {
            close(control.center.x, expected.f32(), control.name);
            assert_eq!(
                control.half_width as u32,
                expected.u32(),
                "{}",
                control.name
            );
            assert_eq!(
                control.half_height as u32,
                expected.u32(),
                "{}",
                control.name
            );
        }
        for x in geometry.captions {
            close(x, expected.f32(), "caption");
        }
        close(geometry.graph_width, expected.f32(), "graph width");
        assert_eq!(interface_factor(width, height, maximum), expected.i32());
        assert_eq!(smoothing.update(width, height), expected.u32() != 0);
        let graph = Rect {
            x: 20.0,
            y: 510.0,
            w: geometry.graph_width,
            h: 210.0,
        };
        let pixels = spectrum_pixel_fit(ow, oh, graph).unwrap();
        assert_eq!(pixels.small_physical, expected.u32() != 0);
        rect(pixels.destination, &mut expected, "physical FFT");
        for scale in [8, 4, 2] {
            let targets = SpectrumTargets::candidate(graph.w, graph.h, scale);
            for size in [
                Some(targets.main),
                targets.intermediate,
                targets.resolve,
                Some(targets.logical),
                Some(targets.pixel),
            ] {
                assert_eq!(size.map_or(0, |v| v.width), expected.i32());
                assert_eq!(size.map_or(0, |v| v.height), expected.i32());
            }
        }
    }
    input.finished();
    expected.finished();
    assert!(viewport(0, 600).is_none());
    assert!(window_point_to_ui(960, 0, Point::default()).is_none());
    assert!(fit_monitor(
        WindowSize {
            width: 0,
            height: 600
        },
        WindowSize {
            width: 960,
            height: 650
        }
    )
    .is_none());
}

#[test]
fn glitch_counter_rotation_and_nonflashing_orbits_match_cpp() {
    let mut input = Reader::new(include_bytes!(
        "../validation/fixtures/ui_animation_input.bin"
    ));
    let mut expected = Reader::new(include_bytes!(
        "../validation/fixtures/ui_animation_expected.bin"
    ));
    let mut animation = VisualizerAnimation::default();
    for _ in 0..input.count() {
        let now = input.f64();
        let width = input.f32();
        let frame = animation.advance(now, width);
        assert_eq!(frame.seven_bits, expected.i32());
        assert_eq!(frame.ten_bits, expected.i32());
        rect(frame.left_background, &mut expected, "left background");
        rect(frame.right_background, &mut expected, "right background");
        for color in [frame.left_color, frame.right_color] {
            for value in &color[..3] {
                close(*value, expected.f32(), "animated RGB");
            }
        }
        close(frame.center.x, expected.f32(), "moving center");
        let angle = expected.f64();
        assert!(
            (frame.angle - angle).abs() < 1e-12,
            "rotation Rust {}, C++ {angle}",
            frame.angle
        );
        close(frame.orbit_shade, expected.f32(), "orbit shade");
        assert!((0.0..=0.5).contains(&frame.orbit_shade));
        for orbit in frame.orbit {
            rect(orbit, &mut expected, "orbit");
        }
    }
    input.finished();
    expected.finished();
}

#[test]
fn adaptive_visual_ladder_and_fixed_raster_overrides_match_cpp() {
    let mut input = Reader::new(include_bytes!("../validation/fixtures/ui_budget_input.bin"));
    let mut expected = Reader::new(include_bytes!(
        "../validation/fixtures/ui_budget_expected.bin"
    ));
    let mut budgets = [
        AdaptiveVisualBudget::default(),
        AdaptiveVisualBudget::new(4),
        AdaptiveVisualBudget::new(8),
    ];
    for _ in 0..input.count() {
        let kind = input.u32();
        let frames = input.u32();
        let slow = input.u32();
        let heavy = input.u32();
        let audio = input.u32();
        for budget in &mut budgets {
            let changed = if kind == 0 {
                budget.reduce_fft_after_stall()
            } else {
                budget.observe(frames, slow, heavy, audio)
            };
            assert_eq!(changed, expected.u32() != 0);
            assert_eq!(budget.fps(), expected.u32());
            assert_eq!(budget.fft_scale(), expected.u32());
        }
    }
    input.finished();
    expected.finished();
}

fn points(actual: &[Point], expected: &mut Reader<'_>, context: &str) {
    assert_eq!(actual.len(), expected.u32() as usize, "{context} count");
    for &p in actual {
        point(p, expected, context);
    }
}
fn vertices(actual: &[Vertex], expected: &mut Reader<'_>, context: &str) {
    assert_eq!(actual.len(), expected.u32() as usize, "{context} count");
    for v in actual {
        point(v.position, expected, context);
        for value in [v.color.r, v.color.g, v.color.b, v.color.a] {
            close(value, expected.f32(), context);
        }
    }
}
fn indices(actual: &[i32], expected: &mut Reader<'_>, context: &str) {
    assert_eq!(actual.len(), expected.u32() as usize, "{context} count");
    for &index in actual {
        assert_eq!(index, expected.i32(), "{context}");
    }
}
#[test]
fn spectrum_fill_variable_halo_waveform_and_oblique_tiles_match_cpp() {
    let mut input = Reader::new(include_bytes!(
        "../validation/fixtures/ui_geometry_input.bin"
    ));
    let mut expected = Reader::new(include_bytes!(
        "../validation/fixtures/ui_geometry_expected.bin"
    ));
    let mut geometry = SpectrumGeometry::default();
    geometry.preload(600.0);
    let mut wave = Vec::new();
    let mut smoothing = LargeWindowSmoothing::default();
    let mut maximum_dense_error = 0.0_f32;
    for n in 0..input.count() {
        let area = input.rect();
        let scale = input.u32();
        let mut magnitudes = [0.0; 96];
        for v in &mut magnitudes {
            *v = input.f32();
        }
        geometry.build(area, scale, &magnitudes);
        assert_eq!(geometry.dense_curve.len(), expected.u32() as usize);
        for p in &geometry.dense_curve {
            close(p.x, expected.f32(), "dense curve x");
            let ey = expected.f32();
            maximum_dense_error = maximum_dense_error.max((p.y - ey).abs());
            close(p.y, ey, "dense curve y");
        }
        points(&geometry.curve, &mut expected, "retained curve");
        vertices(&geometry.fill_vertices, &mut expected, "FFT fill");
        indices(&geometry.fill_indices, &mut expected, "fill topology");
        vertices(&geometry.edge_vertices, &mut expected, "vertical halo");
        indices(&geometry.edge_indices, &mut expected, "halo topology");
        let mut recent = [0.0; 2048];
        for v in &mut recent {
            *v = input.f32();
        }
        waveform_points(&mut wave, area, &recent);
        points(&wave, &mut expected, "waveform");
        let quad = QuadGeometry {
            center: input.point(),
            half_diagonal: [input.u32() as u16, input.u32() as u16],
            diagonal_angle: input.f64(),
            figure_angle: input.f64(),
            angle_offset: input.f64(),
            zoom: input.f64(),
        };
        let force = input.u32() != 0;
        let factor = input.u32() as i32;
        let parent = input.u32() != 0;
        let width = input.i32();
        let height = input.i32();
        let positions = quad.positions();
        for p in positions {
            point(p, &mut expected, "quad corners");
        }
        // The original function consults its hysteresis helper only for a
        // force-8x shape which actually uses an allocated parent target.
        let eligible = parent && factor != 0;
        let soften = force && eligible && smoothing.update(width, height);
        let tile = ObliqueTile::for_quad(&positions, force, parent, factor, soften);
        assert_eq!(tile.is_some(), expected.u32() != 0, "quad tile case {n}");
        if let Some(tile) = tile {
            rect(tile.area, &mut expected, "quad tile area");
            assert_eq!(tile.capacity.width, expected.i32());
            assert_eq!(tile.capacity.height, expected.i32());
            assert_eq!(tile.final_scale, expected.i32());
        }
        let rectangle =
            QuadGeometry::rectangle(quad.center, (90 + n * 3) as u16, (35 + n % 3 * 5) as u16);
        for p in rectangle.positions() {
            point(p, &mut expected, "rectangle corners");
        }
        let rectangle_tile = ObliqueTile::for_quad(&rectangle.positions(), false, true, 2, false);
        assert_eq!(
            rectangle_tile.is_some(),
            expected.u32() != 0,
            "axis-aligned rectangle sampling"
        );
    }
    eprintln!("108 original-C++ graph/quad cases: maximum dense-curve absolute error {maximum_dense_error}");
    input.finished();
    expected.finished();
}

#[test]
fn frame_deadlines_skip_stalls_and_exclude_present_from_app_pressure_like_cpp() {
    let mut input = Reader::new(include_bytes!("../validation/fixtures/ui_timing_input.bin"));
    let mut expected = Reader::new(include_bytes!(
        "../validation/fixtures/ui_timing_expected.bin"
    ));
    let mut pacer = DeadlinePacer::default();
    for n in 0..input.count() {
        let started = input.u64();
        let after = input.u64();
        let finished = input.u64();
        let fps = input.u32();
        let changed = input.u32() != 0;
        let frame = pacer.begin(started, fps);
        if changed {
            pacer.budget_changed(started, fps);
        }
        close(frame.elapsed_seconds, expected.f32(), "frame elapsed");
        expected.u64(); // Removed report-only frame interval.
        expected.u32(); // Removed report-only interval classification.
        assert_eq!(
            pacer.next_deadline_ns(),
            expected.u64(),
            "frame {n} deadline"
        );
        assert_eq!(pacer.wait_ns(after), expected.u64());
        pacer.finish(after, finished, fps);
        assert_eq!(pacer.next_deadline_ns(), expected.u64());
        expected.u64(); // Removed report-only loop-gap maximum.
        expected.u64(); // Removed report-only wait-overshoot maximum.
        let update = input.u64();
        let timing = RenderFrameTiming {
            controls_ns: input.u64(),
            spectrum_ns: input.u64(),
            waveform_ns: input.u64(),
            composite_ns: input.u64(),
        };
        input.u64(); // Present duration was only used by removed report columns.
        let pressure = FramePressure::classify(update, timing, DeadlinePacer::period_ns(fps));
        for flag in [pressure.slow_app, pressure.fft_heavy] {
            assert_eq!(flag, expected.u32() != 0);
        }
        expected.u32(); // Removed slow-work report.
        expected.u32(); // Removed Present-only report.
        expected.u32(); // Removed dominant-phase report.
    }
    input.finished();
    expected.finished();
}
