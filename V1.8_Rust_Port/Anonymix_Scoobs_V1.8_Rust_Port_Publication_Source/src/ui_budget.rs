//! Direct port of RenderBudget.h. Only the visual thread owns this controller.
//! It never changes the sample rate, audio buffers or selected effects.

pub const FRAME_RATES: [u32; 8] = [60, 54, 50, 48, 40, 36, 32, 30];

/// Original visual-only FPS/raster controller. Audio sample rate and DSP are never modified.
pub struct AdaptiveVisualBudget {
    rate_index: usize,
    fft_scale: u32,
    fixed_test_scale: u32,
    pressure_windows: u32,
    fft_pressure_windows: u32,
    healthy_windows: u32,
}
impl Default for AdaptiveVisualBudget {
    fn default() -> Self {
        Self::new(0)
    }
}
impl AdaptiveVisualBudget {
    /// Zero selects the production policy. Four/eight reproduce the original
    /// build-time A/B raster override; they never freeze the FPS ladder.
    pub fn new(fixed_test_scale: u32) -> Self {
        assert!(matches!(fixed_test_scale, 0 | 4 | 8));
        Self {
            rate_index: 0,
            fft_scale: if fixed_test_scale == 0 {
                8
            } else {
                fixed_test_scale
            },
            fixed_test_scale,
            pressure_windows: 0,
            fft_pressure_windows: 0,
            healthy_windows: 0,
        }
    }
    /// Read the shared interface/FFT cadence rung, initially 60 FPS and never below the 30-FPS policy floor.
    pub fn fps(&self) -> u32 {
        FRAME_RATES[self.rate_index]
    }
    /// Read the preferred spectrum raster scale, normally 8x or 4x under sustained load.
    pub fn fft_scale(&self) -> u32 {
        self.fft_scale
    }
    /// Apply the original immediate 8x-to-4x raster reduction after a long spectrum stall.
    pub fn reduce_fft_after_stall(&mut self) -> bool {
        if self.fixed_test_scale != 0 || self.fft_scale != 8 {
            return false;
        }
        self.fft_scale = 4;
        self.fft_pressure_windows = 0;
        self.healthy_windows = 0;
        true
    }
    /// Called once per ten-second report. Slow Present alone is deliberately
    /// excluded: the renderer may be waiting on the driver rather than FFT work.
    pub fn observe(
        &mut self,
        frames: u32,
        slow_app: u32,
        fft_heavy: u32,
        audio_pressure: u32,
    ) -> bool {
        if frames < 30 {
            return false;
        }
        let threshold = (frames / 20).max(6);
        let cpu = slow_app >= threshold;
        let fft = fft_heavy >= threshold;
        let audio = audio_pressure != 0;
        self.pressure_windows = if cpu {
            self.pressure_windows.wrapping_add(1)
        } else {
            0
        };
        self.fft_pressure_windows = if fft {
            self.fft_pressure_windows.wrapping_add(1)
        } else {
            0
        };
        if cpu || fft || audio {
            self.healthy_windows = 0;
            if self.fixed_test_scale == 0
                && self.fft_scale == 8
                && (self.fft_pressure_windows >= 2 || audio)
            {
                self.fft_scale = 4;
                self.fft_pressure_windows = 0;
                return true;
            }
            if (self.pressure_windows >= 2 || audio) && self.rate_index + 1 < FRAME_RATES.len() {
                self.rate_index += 1;
                self.pressure_windows = 0;
                return true;
            }
            return false;
        }
        let threshold = (frames / 200).max(2);
        self.healthy_windows = if slow_app <= threshold {
            self.healthy_windows.wrapping_add(1)
        } else {
            0
        };
        if self.healthy_windows < 3 {
            return false;
        }
        self.healthy_windows = 0;
        if self.rate_index != 0 {
            self.rate_index -= 1;
            return true;
        }
        if self.fixed_test_scale == 0 && self.fft_scale != 8 {
            self.fft_scale = 8;
            return true;
        }
        false
    }
}
