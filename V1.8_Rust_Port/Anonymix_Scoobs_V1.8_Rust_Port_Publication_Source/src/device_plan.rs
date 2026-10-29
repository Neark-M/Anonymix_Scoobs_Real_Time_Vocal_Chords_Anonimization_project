//! Device attempt order and latency hints copied from V1.8 Audio.cpp.
//! This retains the 5 ms predicate; it does not implement a new latency policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The user's requested WASAPI mode; the opened device may fall back to another mode.
pub enum RequestedMode {
    Shared,
    Exclusive,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// One native format attempt. Channel counts describe interleaved input and output.
pub struct ChannelAttempt {
    pub input: i32,
    pub output: i32,
    pub exclusive: bool,
}
/// Try Exclusive stereo/mono capture as available, then the original Shared mono fallback.
pub fn wasapi_attempts(
    mode: RequestedMode,
    max_input: i32,
    max_output: i32,
) -> Vec<ChannelAttempt> {
    let mut result = Vec::new();
    if mode == RequestedMode::Exclusive {
        let output = if max_output >= 2 { 2 } else { 1 };
        let input = if max_input >= 2 { 2 } else { 1 };
        result.push(ChannelAttempt {
            input,
            output,
            exclusive: true,
        });
        if input == 2 {
            result.push(ChannelAttempt {
                input: 1,
                output,
                exclusive: true,
            });
        }
    }
    result.push(ChannelAttempt {
        input: 1,
        output: 1,
        exclusive: false,
    });
    result
}
/// Return latency hints in seconds, in the original attempt order.
/// The extra Shared 5-ms attempt is used only when a default low hint exceeds 5 ms; hints are not guarantees.
pub fn latency_profiles(wasapi: bool, exclusive: bool, input: f64, output: f64) -> Vec<(f64, f64)> {
    let short_profile = wasapi && !exclusive && (input > 0.005 || output > 0.005);
    let mut result = Vec::new();
    if short_profile {
        result.push((input.min(0.005), output.min(0.005)));
    }
    result.push((input, output));
    result
}
