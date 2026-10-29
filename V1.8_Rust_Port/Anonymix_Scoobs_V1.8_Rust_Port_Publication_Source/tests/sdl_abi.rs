//! Official SDL header facts, independent of the Rust declarations.
use anonymix_v18_dsp_port::{
    sdl_abi::*,
    ui_geometry::{Point, Rect, Vertex},
};
use std::mem::{align_of, offset_of, size_of};
#[test]
fn sdl3_c_layouts_offsets_event_values_and_raster_flags_match_official_headers() {
    let values = [
        size_of::<*mut ()>() as u64,
        size_of::<bool>() as u64,
        size_of::<Event>() as u64,
        align_of::<Event>() as u64,
        size_of::<MouseButton>() as u64,
        align_of::<MouseButton>() as u64,
        offset_of!(MouseButton, timestamp) as u64,
        offset_of!(MouseButton, window) as u64,
        offset_of!(MouseButton, button) as u64,
        offset_of!(MouseButton, down) as u64,
        offset_of!(MouseButton, x) as u64,
        offset_of!(MouseButton, y) as u64,
        size_of::<MouseMotion>() as u64,
        align_of::<MouseMotion>() as u64,
        offset_of!(MouseMotion, state) as u64,
        offset_of!(MouseMotion, x) as u64,
        offset_of!(MouseMotion, y) as u64,
        offset_of!(MouseMotion, x_rel) as u64,
        offset_of!(MouseMotion, y_rel) as u64,
        size_of::<Surface>() as u64,
        align_of::<Surface>() as u64,
        offset_of!(Surface, w) as u64,
        offset_of!(Surface, h) as u64,
        offset_of!(Surface, pitch) as u64,
        offset_of!(Surface, pixels) as u64,
        offset_of!(Surface, refcount) as u64,
        offset_of!(Surface, reserved) as u64,
        size_of::<Vertex>() as u64,
        align_of::<Vertex>() as u64,
        offset_of!(Vertex, position) as u64,
        offset_of!(Vertex, color) as u64,
        offset_of!(Vertex, tex_coord) as u64,
        size_of::<Point>() as u64,
        size_of::<Rect>() as u64,
        size_of::<IntRect>() as u64,
        size_of::<TextColor>() as u64,
        INIT_VIDEO as u64,
        WINDOW_FLAGS,
        PIXELFORMAT_RGBA8888 as u64,
        TEXTURE_TARGET as u64,
        SCALE_LINEAR as u64,
        BLEND_NONE as u64,
        BLEND_ALPHA as u64,
        BLEND_PREMULTIPLIED as u64,
        EVENT_QUIT as u64,
        EVENT_DISPLAY_USABLE_BOUNDS_CHANGED as u64,
        EVENT_WINDOW_CLOSE_REQUESTED as u64,
        EVENT_WINDOW_DISPLAY_CHANGED as u64,
        EVENT_MOUSE_MOTION as u64,
        EVENT_MOUSE_DOWN as u64,
        EVENT_MOUSE_UP as u64,
        0,
        1,
        2,
    ];
    let fixture = include_bytes!("../validation/fixtures/sdl_abi_expected.bin");
    assert_eq!(fixture.len(), values.len() * 8);
    for (i, (&actual, bytes)) in values.iter().zip(fixture.chunks_exact(8)).enumerate() {
        assert_eq!(
            actual,
            u64::from_le_bytes(bytes.try_into().unwrap()),
            "SDL ABI fact {i}"
        );
    }
}
