// Keep only the callback pressure signal needed by the adaptive visual policy.
// Audio levels and timing reports belong to the separate development archive.
#pragma once

#include <atomic>
#include <cstdint>

class AudioPressureMonitor final {
public:
    // A callback that runs past its audio period or reports a host x-run asks SDL to
    // reduce optional visual work. Only relaxed atomics run on the audio thread.
    void noteLiveBlock(std::uint64_t frames,
        std::uint64_t processingMicroseconds, double sampleRate,
        bool inputOverflow, bool outputUnderflow) noexcept {
        const bool overBudget = sampleRate > 0.0 &&
            static_cast<double>(processingMicroseconds) * sampleRate >
                static_cast<double>(frames) * 1000000.0;
        if (overBudget || inputOverflow || outputUnderflow)
            visualPressureEvents_.fetch_add(1, std::memory_order_relaxed);
    }

    // The SDL thread consumes this counter once per visual adjustment window.
    std::uint64_t consumeEvents() noexcept {
        return visualPressureEvents_.exchange(0, std::memory_order_relaxed);
    }

private:
    std::atomic<std::uint64_t> visualPressureEvents_{0};
};

extern AudioPressureMonitor g_audioPressure;
