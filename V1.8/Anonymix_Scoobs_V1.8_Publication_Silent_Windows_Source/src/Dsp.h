// Display FFT declarations. These functions analyze floating-point microphone samples for the
// spectrum graph; recorded PCM16 processing belongs to Audio.cpp.

#pragma once
#include "State.h"

// Permute FFT input indices before the in-place radix-2 butterfly passes.
void BitReversalPermutation(vector<complex<float>>&);
// Compute a radix-2 FFT; the input length must be a power of two.
void FFT_CooleyTukey(vector<complex<float>>&);
// Reduce FFT edge discontinuity by weighting each sample with a Hann window.
void ApplyHannWindow(vector<float>&);

// Calculate one spectrum on the display worker; never call from the PortAudio callback.
void ComputeFFT_PortAudio();
void StartDisplayFftWorker();
void RequestDisplayFft();
void PublishDisplayFft(); // SDL thread: copy the most recent completed spectrum.
void StopDisplayFftWorker();
// Blend current FFT bars with earlier values to reduce visual flicker.
void SmoothFFTDisplay_PortAudio(float smoothingFactor = 0.3f,
    float elapsedSeconds = 1.0f / 60.0f);
