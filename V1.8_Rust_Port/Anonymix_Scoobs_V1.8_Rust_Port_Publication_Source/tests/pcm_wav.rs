//! Conversion and write-failure checks protect the existing file-format contract.
use anonymix_v18_dsp_port::{
    pcm::{capture_pcm16_sample, read_pcm16_sample, write_pcm16_sample},
    wav::{pcm16_wav_header, write_pcm16_wav},
};
use std::io;

#[test]
fn capture_truncation_and_export_rounding_keep_their_distinct_scales() {
    assert_eq!(capture_pcm16_sample(-1.0), -32767);
    assert_eq!(capture_pcm16_sample(1.0), 32767);
    assert_eq!(capture_pcm16_sample(0.5), 16383);
    let mut bytes = [0; 8];
    for (i, value) in [-1.0, 1.0, 0.5 / 32768.0, -0.5 / 32768.0]
        .into_iter()
        .enumerate()
    {
        write_pcm16_sample(&mut bytes, i, value);
    }
    let actual: Vec<_> = (0..4).map(|i| read_pcm16_sample(&bytes, i)).collect();
    assert_eq!(actual, [-32768, 32767, 1, -1]);
}

#[test]
fn wav_rejects_partial_frames_and_propagates_write_errors() {
    assert!(pcm16_wav_header(0, 1, 48_000).is_err());
    assert!(pcm16_wav_header(3, 1, 48_000).is_err());
    assert!(pcm16_wav_header(2, 2, 48_000).is_err());
    struct FailedWriter;
    impl io::Write for FailedWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("test write failure"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    assert!(write_pcm16_wav(&mut FailedWriter, &[0, 0], 1, 48_000).is_err());
}
