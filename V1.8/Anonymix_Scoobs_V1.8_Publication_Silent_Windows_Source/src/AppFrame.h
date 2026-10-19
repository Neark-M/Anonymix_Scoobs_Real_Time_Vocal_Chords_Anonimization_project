// Per-frame update and rendering declarations, called in that order from SDL_AppIterate.

#pragma once
#include <cstdint>
#include "UiLayout.h"
class UiWidget;
// CPU-side elapsed times for each renderer phase; GPU work may be deferred by the driver.
struct RenderFrameTiming {
    uint64_t controlsNs = 0;
    uint64_t spectrumNs = 0;
    uint64_t waveformNs = 0;
    uint64_t compositeNs = 0;
    uint64_t presentNs = 0;
};
// Detect completed playback, copy slider settings to audio atomics and update FFT data.
void UpdatePlaybackAndAnalysis();
// Move and draw the interactive controls, waveform and spectrum, then present the frame.
void RenderFrame(double now, RenderFrameTiming* timing = nullptr);
// Create the fixed slider captions and help textures once after SDL renderer/font setup.
void PreloadStaticUiText();
// Allocate the initial 2x interface render target before the first rendered frame.
void PreloadInterfaceRenderTarget();
// Redistribute controls when the window aspect ratio gains usable width.
void RefreshResponsiveUiGeometry();
// The offscreen UI renderer exposes its current sampling scale to the FFT pass.
int CurrentInterfaceSampleFactor();
// Enable the optional final resolve above the medium-window visual scale.
bool UseLargeWindowEdgeSmoothing();
// Refresh hit testing at a pointer event so a click never uses a stale hover from a prior frame.
UiWidget* HitTestUiAt(float logicalX, float logicalY);
// Dispose of cached caption textures before SDL destroys the renderer.
void CleanupStaticSliderCaptions();
