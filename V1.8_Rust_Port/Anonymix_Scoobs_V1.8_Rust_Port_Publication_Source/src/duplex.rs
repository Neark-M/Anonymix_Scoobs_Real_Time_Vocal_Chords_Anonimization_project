//! V1.8 live host-block adapter, independent of the native C ABI.
//! The owner supplies record/preview routes and read-only display observers.
use crate::{capture::CaptureProcessor, live::LiveChain, Settings, STREAM_BLOCK_SIZE};

/// Callback-owned capture and live DSP history with fixed 256-frame scratch arrays.
pub struct DuplexProcessor {
    capture: CaptureProcessor,
    live: LiveChain,
    cleaned: [f32; STREAM_BLOCK_SIZE],
    processed: [f32; STREAM_BLOCK_SIZE],
}
impl Default for DuplexProcessor {
    fn default() -> Self {
        Self {
            capture: CaptureProcessor::default(),
            live: LiveChain::default(),
            cleaned: [0.0; STREAM_BLOCK_SIZE],
            processed: [0.0; STREAM_BLOCK_SIZE],
        }
    }
}
impl DuplexProcessor {
    /// Process one PortAudio callback. Keep the original 256-frame subdivisions,
    /// including the final partial chunk. Null input does not advance DSP state.
    /// Before live activation the duplex stream cleans capture but emits silence.
    pub fn process_host(
        &mut self,
        input: Option<&[f32]>,
        output: Option<&mut [f32]>,
        frames: usize,
        input_channels: usize,
        output_channels: usize,
        exclusive: bool,
        generation: u32,
        live_active: bool,
        settings: Settings,
    ) {
        self.process_host_routed(
            input,
            output,
            frames,
            input_channels,
            output_channels,
            exclusive,
            generation,
            live_active,
            settings,
            |_| {},
            |out, _, _| {
                if let Some(out) = out {
                    out.fill(0.0);
                }
            },
        );
    }

    /// Route cleaned capture to the recording queue only outside live mode.
    /// Exclusive preview is rendered per 256-frame subdivision, including when
    /// input is absent. Shared idle output is always silent on this stream.
    pub fn process_host_routed(
        &mut self,
        input: Option<&[f32]>,
        output: Option<&mut [f32]>,
        frames: usize,
        input_channels: usize,
        output_channels: usize,
        exclusive: bool,
        generation: u32,
        live_active: bool,
        settings: Settings,
        record: impl FnMut(&[f32]),
        idle: impl FnMut(Option<&mut [f32]>, usize, usize),
    ) {
        self.process_host_dynamic(
            input,
            output,
            frames,
            input_channels,
            output_channels,
            exclusive,
            generation,
            || (live_active, settings),
            record,
            idle,
        );
    }

    /// The desktop callback reads independent control atomics for each original
    /// 256-frame subdivision, rather than freezing them across a large host call.
    pub fn process_host_dynamic(
        &mut self,
        input: Option<&[f32]>,
        output: Option<&mut [f32]>,
        frames: usize,
        input_channels: usize,
        output_channels: usize,
        exclusive: bool,
        generation: u32,
        controls: impl FnMut() -> (bool, Settings),
        record: impl FnMut(&[f32]),
        idle: impl FnMut(Option<&mut [f32]>, usize, usize),
    ) {
        self.process_host_observed(
            input,
            output,
            frames,
            input_channels,
            output_channels,
            exclusive,
            generation,
            controls,
            record,
            idle,
            |_| {},
        );
    }

    /// Display observers see read-only slices. They cannot
    /// modify the cleaned samples or the processed output supplied to the device.
    pub fn process_host_observed(
        &mut self,
        input: Option<&[f32]>,
        mut output: Option<&mut [f32]>,
        frames: usize,
        input_channels: usize,
        output_channels: usize,
        exclusive: bool,
        generation: u32,
        mut controls: impl FnMut() -> (bool, Settings),
        mut record: impl FnMut(&[f32]),
        mut idle: impl FnMut(Option<&mut [f32]>, usize, usize),
        mut captured: impl FnMut(&[f32]),
    ) {
        assert!(matches!(input_channels, 1 | 2) && matches!(output_channels, 1 | 2));
        if let Some(data) = input {
            assert_eq!(data.len(), frames * input_channels);
        }
        if let Some(data) = &output {
            assert_eq!(data.len(), frames * output_channels);
        }
        if input.is_none() {
            for start in (0..frames).step_by(STREAM_BLOCK_SIZE) {
                let count = STREAM_BLOCK_SIZE.min(frames - start);
                let out = output
                    .as_deref_mut()
                    .map(|v| &mut v[start * output_channels..(start + count) * output_channels]);
                if exclusive {
                    idle(out, count, output_channels);
                } else if let Some(out) = out {
                    out.fill(0.0);
                }
            }
            return;
        }
        let input = input.unwrap();
        for start in (0..frames).step_by(STREAM_BLOCK_SIZE) {
            let count = STREAM_BLOCK_SIZE.min(frames - start);
            self.capture.process_chunk(
                &input[start * input_channels..(start + count) * input_channels],
                input_channels,
                exclusive,
                generation,
                &mut self.cleaned[..count],
            );
            captured(&self.cleaned[..count]);
            let (live_active, settings) = controls();
            if live_active && output.is_some() {
                self.live.process_block(
                    &self.cleaned[..count],
                    &mut self.processed[..count],
                    settings,
                );
                if let Some(data) = output.as_deref_mut() {
                    for i in 0..count {
                        for ch in 0..output_channels {
                            data[(start + i) * output_channels + ch] = self.processed[i];
                        }
                    }
                }
            } else {
                record(&self.cleaned[..count]);
                let out = output
                    .as_deref_mut()
                    .map(|v| &mut v[start * output_channels..(start + count) * output_channels]);
                if exclusive {
                    idle(out, count, output_channels);
                } else if let Some(out) = out {
                    out.fill(0.0);
                }
            }
        }
    }
}
