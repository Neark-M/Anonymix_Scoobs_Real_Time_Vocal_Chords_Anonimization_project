//! Original C++ report fixture, reading only the functional visual-pressure column.
use anonymix_v18_dsp_port::audio_pressure::AudioPressure;
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let result = self.bytes[self.offset..self.offset + N].try_into().unwrap();
        self.offset += N;
        result
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }
    fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.take())
    }
    fn f64(&mut self) -> f64 {
        f64::from_le_bytes(self.take())
    }
}
#[test]
fn silent_build_preserves_the_original_audio_pressure_window() {
    let mut input = Reader {
        bytes: include_bytes!("../validation/fixtures/diagnostics_input.bin"),
        offset: 0,
    };
    let mut expected = Reader {
        bytes: include_bytes!("../validation/fixtures/diagnostics_expected.bin"),
        offset: 0,
    };
    assert_eq!(&input.take::<8>(), b"ANONDIAG");
    let pressure = AudioPressure::default();
    for _ in 0..input.u32() {
        let frames = input.u64();
        let micros = input.u64();
        let rate = input.f64();
        let over = input.u32() != 0;
        let under = input.u32() != 0;
        input.f64(); // Host gap was report-only.
        for _ in 0..7 {
            input.f64();
        } // Energy reports no longer exist.
        input.f32();
        let flags = input.u32();
        if flags & 64 != 0 {
            pressure.observe(frames, micros, rate, over, under);
        }
        for _ in 0..22 {
            expected.u64();
        } // Skip removed report columns.
        let actual = if flags & 4 != 0 {
            pressure.take_visual_pressure_events()
        } else {
            0
        };
        assert_eq!(actual, expected.u64());
    }
    assert_eq!(input.offset, input.bytes.len());
    assert_eq!(expected.offset, expected.bytes.len());
}
