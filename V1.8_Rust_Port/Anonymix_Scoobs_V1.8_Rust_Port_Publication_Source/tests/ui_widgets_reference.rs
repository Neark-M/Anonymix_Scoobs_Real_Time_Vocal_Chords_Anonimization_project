//! Full original widget initialization/hit/drag fixture, produced by C++ with
//! the official SDL headers. No SDL library or device is loaded by this test.
use anonymix_v18_dsp_port::{ui_geometry::Point, ui_widgets::UiModel};
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let a = self.data[self.at..self.at + N].try_into().unwrap();
        self.at += N;
        a
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.bytes())
    }
    fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.bytes())
    }
    fn f64(&mut self) -> f64 {
        f64::from_le_bytes(self.bytes())
    }
}
fn close(a: f32, b: f32, step: u32, name: &str, field: &str) {
    assert!(
        (a - b).abs() <= 0.0001 + b.abs() * 0.000001,
        "step {step} {name} {field}: Rust {a}, C++ {b}"
    );
}
#[test]
fn full_widget_setup_hover_quantized_drag_and_hit_selection_match_cpp() {
    let mut input = Reader::new(include_bytes!(
        "../validation/fixtures/ui_widgets_input.bin"
    ));
    let mut expected = Reader::new(include_bytes!(
        "../validation/fixtures/ui_widgets_expected.bin"
    ));
    assert_eq!(&input.bytes::<8>(), b"ANONWID9");
    let count = input.u32();
    let mut ui = UiModel::default();
    for step in 0..count {
        let width = input.u32() as i32;
        let height = input.u32() as i32;
        let mouse = Point {
            x: input.f32(),
            y: input.f32(),
        };
        let target = input.u32() as usize - 1;
        let op = input.u32();
        let offset = input.f32();
        let stream = input.u32() != 0;
        ui.refresh_layout(width, height);
        if op == 1 {
            ui.widgets[target].drag(mouse, Point { x: 0., y: offset });
        }
        let hit = ui.hit_test(mouse, false, stream);
        assert_eq!(
            hit.map_or(0, |i| i + 1),
            expected.u32() as usize,
            "step {step} hit at {mouse:?}"
        );
        for w in &ui.widgets {
            for v in w.vertices {
                close(v.position.x, expected.f32(), step, w.name, "x");
                close(v.position.y, expected.f32(), step, w.name, "y");
            }
            let zoom = expected.f64();
            assert!(
                (w.quad.zoom - zoom).abs() < 1e-12,
                "step {step} {} zoom Rust {}, C++ {zoom}",
                w.name,
                w.quad.zoom
            );
            close(w.quad.center.x, expected.f32(), step, w.name, "handle x");
            close(w.quad.center.y, expected.f32(), step, w.name, "handle y");
            let value = expected.f64();
            let actual = w.slider.as_ref().map_or(0., |s| s.value);
            assert!(
                (actual - value).abs() < 1e-10,
                "step {step} {} legend Rust {actual}, C++ {value}",
                w.name
            );
            for v in w.vertices {
                for (a, label) in [
                    (v.color.r, "red"),
                    (v.color.g, "green"),
                    (v.color.b, "blue"),
                ] {
                    close(a, expected.f32(), step, w.name, label);
                }
            }
        }
    }
    assert_eq!(input.at, input.data.len());
    assert_eq!(expected.at, expected.data.len());
}
