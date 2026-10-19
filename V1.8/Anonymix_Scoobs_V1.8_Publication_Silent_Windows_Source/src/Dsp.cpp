// Display-only FFT analysis. A worker reads a snapshot of the microphone ring once per visual frame;
// the microphone-to-speaker DSP does not wait for this transform.

#include "Dsp.h"
#include <condition_variable>
#include <thread>

namespace {
mutex spectrumResultMutex;
condition_variable spectrumWake;
thread spectrumWorker;
bool spectrumRequested = false;
bool spectrumStopping = false;
bool spectrumResultReady = false;
array<float, PA_NUM_BARS> completedSpectrum{};
bool completedCalibrating = true;
int completedCalibrationFrames = 0;
}

namespace {
// These values depend only on the fixed 48 kHz display FFT configuration.
// Construct them on the display worker on first use, never in the audio callback.
struct DisplayFftConstants {
    array<float, PA_FFT_SIZE> hann{};
    array<pair<size_t, size_t>, PA_NUM_BARS> bands{};
    array<float, PA_NUM_BARS> calibrationBoost{};

    DisplayFftConstants() {
        const size_t usableBins = PA_FFT_SIZE / 2;
        const float binWidth = PA_SAMPLE_RATE / PA_FFT_SIZE;
        for (size_t i = 0; i < PA_FFT_SIZE; ++i)
            hann[i] = 0.5f * (1.0f - cos(2.0f * numbers::pi * i / (PA_FFT_SIZE - 1)));
        for (size_t bar = 0; bar < PA_NUM_BARS; ++bar) {
            const float freqStart = PA_FFT_MIN_HZ * pow(PA_FFT_MAX_HZ / PA_FFT_MIN_HZ,
                static_cast<float>(bar) / PA_NUM_BARS);
            const float freqEnd = PA_FFT_MIN_HZ * pow(PA_FFT_MAX_HZ / PA_FFT_MIN_HZ,
                static_cast<float>(bar + 1) / PA_NUM_BARS);
            const size_t start = clamp(static_cast<size_t>(lround(freqStart / binWidth)),
                size_t{1}, usableBins - 1);
            const size_t end = min(usableBins, max(start + 1,
                static_cast<size_t>(ceil(freqEnd / binWidth))));
            bands[bar] = { start, end };
            const float center = (freqStart + freqEnd) / 2.0f;
            calibrationBoost[bar] = center < 200.0f ? 1.6f :
                (center < 1000.0f ? 1.4f : 1.3f);
        }
    }
};
} // namespace

// Permute FFT input indices before the in-place radix-2 butterfly passes.
void BitReversalPermutation(vector<complex<float>>& data)
{
    // Bit reversal reorders input so iterative butterfly passes can operate in place.
    size_t n = data.size();
    size_t j = 0;

    for (size_t i = 0; i < n - 1; ++i) {
        if (i < j) {
            Swap(data[i], data[j]);
        }

        size_t k = n / 2;
        while (k <= j) {
            j -= k;
            k /= 2;
        }
        j += k;
    }
}

// Compute a radix-2 FFT; the input length must be a power of two.
void FFT_CooleyTukey(vector<complex<float>>& data)
{
    size_t n = data.size();

    // Radix-2 butterflies require at least two values and a power-of-two vector length.
    if (n < 2 || (n & (n - 1)) != 0) {
        return;
    }

    BitReversalPermutation(data);

    size_t log2n = 0;
    size_t temp = n;
    while (temp > 1) {
        temp >>= 1;
        log2n++;
    }

    // Combine progressively larger groups, starting with adjacent complex values.
    for (size_t s = 1; s <= log2n; ++s) {
        size_t m = size_t{1} << s;
        size_t m2 = m / 2;

        complex<float> w(1.0f, 0.0f);
        complex<float> wm = polar(1.0f, static_cast<float>(-2.0 * numbers::pi / m));

        for (size_t j = 0; j < m2; ++j) {
            for (size_t k = j; k < n; k += m) {
                complex<float> t = w * data[k + m2];
                complex<float> u = data[k];

                data[k] = u + t;
                data[k + m2] = u - t;
            }
            w *= wm;
        }
    }
}

// Reduce FFT edge discontinuity by weighting each sample with a Hann window.
void ApplyHannWindow(vector<float>& data)
{
    size_t n = data.size();
    for (size_t i = 0; i < n; ++i) {
        // Taper the endpoints; this reduces leakage from discontinuities at FFT window boundaries.
        float window = 0.5f * (1.0f - cos(2.0f * numbers::pi * i / (n - 1)));
        data[i] *= window;
    }
}

// Copy recent captured samples under paFftMutex, then compute and publish a private result.
void ComputeFFT_PortAudio()
{
    // Only the display worker (or the SDL fallback if thread creation failed) calls this.
    // Scratch and calibration state remain private to that one caller.
    static vector<float> localBuffer(PA_FFT_SIZE);
    static vector<complex<float>> complexData(PA_FFT_SIZE);
    static vector<float> rawMagnitudes(PA_NUM_BARS);
    static vector<float> computedMagnitudes(PA_NUM_BARS);
    static vector<float> noiseFloor(PA_NUM_BARS, 0.0f);
    static bool calibrating = true;
    static int calibrationFrames = 0;
    static const DisplayFftConstants constants;
    const auto publishResult = [&] {
        lock_guard<mutex> lock(spectrumResultMutex);
        copy_n(computedMagnitudes.begin(), PA_NUM_BARS, completedSpectrum.begin());
        completedCalibrating = calibrating;
        completedCalibrationFrames = calibrationFrames;
        spectrumResultReady = true;
    };
    {
        // Hold the lock only for the snapshot; the FFT work uses local data.
        lock_guard<mutex> lock(paFftMutex);

        if (paFftSamplesAvailable < PA_FFT_SIZE || paFftInputBuffer.empty()) {
            return;
        }

        const size_t capacity = paFftInputBuffer.size();
        const size_t oldest = (paFftWritePos + capacity - PA_FFT_SIZE) % capacity;
        const size_t firstCount = min(PA_FFT_SIZE, capacity - oldest);
        copy_n(paFftInputBuffer.data() + oldest, firstCount, localBuffer.data());
        copy_n(paFftInputBuffer.data(), PA_FFT_SIZE - firstCount,
            localBuffer.data() + firstCount);
    }

    {
        float meanEnergy = 0.0f;
        for (size_t i = 0; i < localBuffer.size(); ++i) {
            meanEnergy += localBuffer[i] * localBuffer[i];
        }
        meanEnergy /= localBuffer.size();

        float stdDev = 0.0f;
        for (size_t i = 0; i < localBuffer.size(); ++i) {
            float diff = localBuffer[i] * localBuffer[i] - meanEnergy;
            stdDev += diff * diff;
        }
        stdDev = sqrt(stdDev / localBuffer.size());

        // Isolated high-energy samples are replaced with their neighbors
        // before windowing so a click does not dominate the visible spectrum.
        float impulseThreshold = meanEnergy + 4.0f * stdDev;

        for (size_t i = 1; i < localBuffer.size() - 1; ++i) {
            float sampleEnergy = localBuffer[i] * localBuffer[i];

            if (sampleEnergy > impulseThreshold) {
                float leftEnergy = localBuffer[i - 1] * localBuffer[i - 1];
                float rightEnergy = localBuffer[i + 1] * localBuffer[i + 1];
                float avgNeighborEnergy = (leftEnergy + rightEnergy) / 2.0f;

                if (avgNeighborEnergy < impulseThreshold * 0.25f) {
                    localBuffer[i] = (localBuffer[i - 1] + localBuffer[i + 1]) / 2.0f;

                    // The impulse is only removed from the display FFT. Logging from this
                    // inner loop can stall the visual frame under Visual Studio's debugger.
                }
            }
        }

    }

    // The display FFT receives cleaned and windowed input. The live
    // microphone-to-speaker DSP does not use this transform.
    // Identical Hann coefficients to ApplyHannWindow, calculated once for this fixed size.
    for (size_t i = 0; i < PA_FFT_SIZE; ++i) {
        complexData[i] = complex<float>(localBuffer[i] * constants.hann[i], 0.0f);
    }

    FFT_CooleyTukey(complexData);

    for (size_t bar = 0; bar < PA_NUM_BARS; ++bar) {
        // These logarithmic bin boundaries stay fixed while samples change.
        const auto [binStart, binEnd] = constants.bands[bar];

        float sum = 0.0f;
        size_t count = 0;

        for (size_t bin = binStart; bin < binEnd; ++bin) {
            sum += abs(complexData[bin]);
            count++;
        }

        rawMagnitudes[bar] = (count > 0) ? (sum / count) : 0.0f;
    }

    // Accumulate per-bar baseline magnitudes during the startup calibration window.
    if (calibrating) {
        for (size_t bar = 0; bar < PA_NUM_BARS; ++bar) {
            noiseFloor[bar] = Max(noiseFloor[bar], rawMagnitudes[bar]);
        }

        calibrationFrames++;

        if (calibrationFrames >= PA_CALIBRATION_FRAMES) {
            calibrating = false;

            for (size_t bar = 0; bar < PA_NUM_BARS; ++bar) {
                noiseFloor[bar] *= constants.calibrationBoost[bar];
            }

        }

        for (size_t i = 0; i < PA_NUM_BARS; ++i) {
            computedMagnitudes[i] = rawMagnitudes[i];
        }

        publishResult();
        return;
    }

    // Retain prior spectrum energy for the adaptive display noise subtraction.
    static vector<float> spectralHistory(PA_NUM_BARS, 0.0f);

    for (size_t bar = 0; bar < PA_NUM_BARS; ++bar) {
        // Follow new speech energy promptly. Decay more gently to keep the visual
        // contour legible between syllables without a half-second reaction lag.
        const float historyFactor = rawMagnitudes[bar] > spectralHistory[bar]
            ? 0.72f : 0.30f;
        spectralHistory[bar] += (rawMagnitudes[bar] - spectralHistory[bar]) * historyFactor;
    }

    for (size_t bar = 0; bar < PA_NUM_BARS; ++bar) {
        // Subtract each bar noise floor without allowing negative visible magnitudes.
        float cleanMag = Max(0.0f, spectralHistory[bar] - noiseFloor[bar]);
        computedMagnitudes[bar] = cleanMag;
    }

    // Scale the display with its strongest current bar after noise filtering.
    float maxMagFinal = computedMagnitudes.empty() ? 0.0f :
        *max_element(computedMagnitudes.begin(), computedMagnitudes.end());

    if (maxMagFinal > 0.00001f) {
        for (size_t i = 0; i < PA_NUM_BARS; ++i) {
            computedMagnitudes[i] = powf(Min(1.0f,
                computedMagnitudes[i] / maxMagFinal), 0.78f);
        }
    }

    publishResult();

}

void StartDisplayFftWorker() {
    try {
        spectrumWorker = thread([] {
            for (;;) {
                {
                    unique_lock<mutex> lock(spectrumResultMutex);
                    spectrumWake.wait(lock, [] { return spectrumStopping || spectrumRequested; });
                    if (spectrumStopping) break;
                    spectrumRequested = false; // Coalesce requests when analysis falls behind.
                }
                try { ComputeFFT_PortAudio(); }
                catch (...) { /* The caller observes failure through the existing state. */ }
            }
        });
    } catch (const system_error&) {
        // RequestDisplayFft falls back to synchronous analysis without a worker.
    }
}

void RequestDisplayFft() {
    if (!spectrumWorker.joinable()) {
        ComputeFFT_PortAudio();
        return;
    }
    {
        lock_guard<mutex> lock(spectrumResultMutex);
        spectrumRequested = true;
    }
    spectrumWake.notify_one();
}

void PublishDisplayFft() {
    // The renderer and smoother only access these globals on SDL's thread.
    lock_guard<mutex> lock(spectrumResultMutex);
    if (!spectrumResultReady) return;
    copy(completedSpectrum.begin(), completedSpectrum.end(), paFftMagnitudes.begin());
    paIsCalibrating = completedCalibrating;
    paCalibrationFrames = completedCalibrationFrames;
    paFftDataReady = true;
    spectrumResultReady = false;
}

void StopDisplayFftWorker() {
    {
        lock_guard<mutex> lock(spectrumResultMutex);
        spectrumStopping = true;
    }
    spectrumWake.notify_one();
    if (spectrumWorker.joinable()) spectrumWorker.join();
}

// Blend current FFT bars with earlier values to reduce visual flicker.
// Blend each computed magnitude into the persistent display bars at the supplied smoothing factor.
void SmoothFFTDisplay_PortAudio(float smoothingFactor, float elapsedSeconds)
{
    // Express the previous per-60-FPS response as a response per elapsed second.
    // Lower visual frame rates then keep approximately the same attack and decay times.
    const float framesAt60Hz = clamp(elapsedSeconds * 60.0f, 0.0f, 4.0f);
    const float fallingFactor = 1.0f - powf(1.0f - smoothingFactor, framesAt60Hz);
    const float risingFactor = 1.0f - powf(1.0f - 0.82f, framesAt60Hz);
    for (size_t i = 0; i < PA_NUM_BARS; ++i) {
        const float factor = paFftMagnitudes[i] > paFftSmoothed[i]
            ? risingFactor : fallingFactor;
        paFftSmoothed[i] += (paFftMagnitudes[i] - paFftSmoothed[i]) * factor;
    }
}

