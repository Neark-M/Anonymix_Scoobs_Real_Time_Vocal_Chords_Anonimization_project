//! Binary reference outputs produced by extracted V1.8 C++ code, not Rust.
use anonymix_v18_dsp_port::{
    observation::DisplayRings,
    spectrum::{smooth_display, DisplayFftConstants, SpectrumAnalyzer, BARS, FFT_SIZE},
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
        let output = self.data[self.offset..self.offset + N].try_into().unwrap();
        self.offset += N;
        output
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.bytes())
    }
    fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.bytes())
    }
    fn finished(&self) {
        assert_eq!(self.offset, self.data.len());
    }
}
fn close(actual: f32, expected: f32, context: &str) {
    let tolerance = 0.00002 + expected.abs() * 0.00005;
    assert!(
        (actual - expected).abs() <= tolerance,
        "{context}: actual {actual}, C++ {expected}, difference {}",
        (actual - expected).abs()
    );
}

#[test]
fn display_fft_calibration_smoothing_and_both_rings_match_cpp() {
    let mut input = Reader::new(include_bytes!("../validation/fixtures/display_input.bin"));
    let mut expected = Reader::new(include_bytes!(
        "../validation/fixtures/display_expected.bin"
    ));
    assert_eq!(&input.bytes::<8>(), b"ANONFFT7");
    let frames = input.u32();
    let constants = DisplayFftConstants::default();
    for &hann in &constants.hann {
        close(hann, expected.f32(), "Hann coefficient");
    }
    for &(start, end) in &constants.bands {
        assert_eq!(start, expected.u32() as usize);
        assert_eq!(end, expected.u32() as usize);
    }
    for &boost in &constants.calibration_boost {
        assert_eq!(boost, expected.f32());
    }
    let rings = DisplayRings::default();
    let mut analyzer = SpectrumAnalyzer::default();
    let mut smoothed = [0.0; BARS];
    let mut samples = [0.0; FFT_SIZE];
    let mut maximum = 0.0_f32;
    for index in 0..frames {
        let elapsed = input.f32();
        let blocks = input.u32();
        for _ in 0..blocks {
            let size = input.u32() as usize;
            let data: Vec<_> = (0..size).map(|_| input.f32()).collect();
            rings.try_push(&data);
        }
        assert!(rings.snapshot_fft(&mut samples));
        let frame = analyzer.analyze(&samples);
        for &value in &frame.magnitudes {
            let reference = expected.f32();
            maximum = maximum.max((value - reference).abs());
            close(value, reference, &format!("FFT frame {index}"));
        }
        assert_eq!(frame.calibrating, expected.u32() != 0);
        assert_eq!(frame.calibration_frames, expected.u32());
        smooth_display(&frame.magnitudes, &mut smoothed, 0.34, elapsed);
        for &value in &smoothed {
            close(value, expected.f32(), "display smoothing");
        }
        let ready = expected.u32() != 0;
        let wave = rings.snapshot_waveform();
        assert_eq!(wave.is_some(), ready);
        for i in 0..2048 {
            let sample = expected.f32();
            assert_eq!(
                wave.as_ref().map(|w| w.samples[i]).unwrap_or(0.0),
                sample,
                "waveform ordering"
            );
        }
        close(
            wave.as_ref().map(|w| w.rms).unwrap_or(0.0),
            expected.f32(),
            "waveform RMS",
        );
        close(
            wave.as_ref().map(|w| w.peak).unwrap_or(0.0),
            expected.f32(),
            "waveform peak",
        );
    }
    input.finished();
    expected.finished();
    eprintln!("Display: {frames} C++ reference frames, maximum FFT absolute difference {maximum}");
}
