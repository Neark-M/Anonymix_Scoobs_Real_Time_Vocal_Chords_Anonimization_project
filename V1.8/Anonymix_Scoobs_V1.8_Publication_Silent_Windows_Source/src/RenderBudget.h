// SDL-thread-only visual quality controller. Audio processing does not read this policy.
#pragma once
#include <array>

class AdaptiveVisualBudget final {
    static constexpr std::array<int, 8> rates_{60, 54, 50, 48, 40, 36, 32, 30};
    unsigned rateIndex_ = 0;
    int fftScale_ = 8;
    unsigned pressureWindows_ = 0;
    unsigned fftPressureWindows_ = 0;
    unsigned healthyWindows_ = 0;

public:
    int Fps() const noexcept { return rates_[rateIndex_]; }
    int FftScale() const noexcept { return fftScale_; }

    // A multi-frame renderer stall is a stronger signal than a rolling average.
    // Keep 8x as the normal raster, but protect a struggling GPU immediately;
    // the ordinary healthy-window recovery will return to 8x later.
    bool ReduceFftAfterStall() noexcept {
        if (fftScale_ != 8) return false;
        fftScale_ = 4;
        fftPressureWindows_ = 0;
        healthyWindows_ = 0;
        return true;
    }

    // Called once per ten-second visual adjustment window. App work excludes SDL_RenderPresent waits:
    // reducing FFT pixels cannot be justified by a blocked Present alone. FFT-heavy
    // frames are counted independently of whether Present took even longer.
    // A repeated overload is required before degrading optional visual quality.
    bool Observe(unsigned frames, unsigned slowAppFrames,
        unsigned fftHeavyFrames, unsigned audioPressureEvents) noexcept {
        // A very slow GPU may produce fewer than 200 frames in ten seconds.
        // Ignoring that report would disable adaptation exactly when needed.
        if (frames < 30) return false;
        const unsigned threshold = frames / 20 > 6 ? frames / 20 : 6;
        const bool cpuPressure = slowAppFrames >= threshold;
        const bool fftPressure = fftHeavyFrames >= threshold;
        const bool audioPressure = audioPressureEvents != 0;
        pressureWindows_ = cpuPressure ? pressureWindows_ + 1 : 0;
        fftPressureWindows_ = fftPressure ? fftPressureWindows_ + 1 : 0;

        if (cpuPressure || fftPressure || audioPressure) {
            healthyWindows_ = 0;
            // Only repeated FFT work, or an audio deadline miss, lowers raster.
            if (fftScale_ == 8 &&
                (fftPressureWindows_ >= 2 || audioPressure)) {
                fftScale_ = 4;
                fftPressureWindows_ = 0;
                return true;
            }
            // CPU-side work or audio pressure can reduce UI rate by one rung.
            // A slow driver Present by itself cannot trigger this step.
            if ((pressureWindows_ >= 2 || audioPressure) &&
                rateIndex_ + 1 < rates_.size()) {
                ++rateIndex_;
                pressureWindows_ = 0;
                return true;
            }
            return false;
        }

        const unsigned healthyThreshold = frames / 200 > 2 ? frames / 200 : 2;
        healthyWindows_ = slowAppFrames <= healthyThreshold ? healthyWindows_ + 1 : 0;
        if (healthyWindows_ < 3) return false;
        healthyWindows_ = 0;
        if (rateIndex_ != 0) {
            --rateIndex_;
            return true;
        }
        if (fftScale_ != 8) {
            fftScale_ = 8;
            return true;
        }
        return false;
    }
};
