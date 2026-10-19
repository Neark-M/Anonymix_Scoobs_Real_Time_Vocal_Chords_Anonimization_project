// SDL drawing and text helpers. Call from the UI thread after renderer and font initialization.

#pragma once
#include "State.h"
// Format a floating-point value using the requested number of decimal places.
string FormatDouble(double, int precision = 2) noexcept;
// Convert a numeric value to text for slider legends.
string ValueToText(double, int precision = 0) noexcept;
// Append the unit suffix selected by WidgetUnit to a formatted value.
string FormatValueWithUnit(double, WidgetUnit, int precision = 2) noexcept;
// Query the rendered text dimensions after applying the requested scale.
void getSurfaceSize(const string&, float&, float&, double scale = 1.0);
// Draw text and report its dimensions. Cache labels whose contents are stable across frames.
bool RenderText(const string&, float, float, SDL_Color, float&, float&, double scale = 1.0,
    bool cacheStableLabel = false);
// Rasterize one finite button-state label before its first appearance. Rendering later uses
// the same bounded cache and the same text/color key; call only on the SDL thread.
bool PrecacheStableLabel(const string&, SDL_Color);
// Prepare finite startup status labels while the renderer is idle, before opening audio.
void PreloadVisualizationStatusText();
// Cache the changing value shown in the information panel separately from fixed captions.
// Its texture is replaced only when the actual text or color changes.
bool RenderDynamicInfoText(const string&, float, float, SDL_Color, float&, float&,
    double scale = 1.0);
// Release the label textures before closing the font or destroying the renderer.
void CleanupTextCache();
// Render slanted quads into a reusable 8x tile. Axis-aligned rectangles stay
// on the normal UI path. Force the moving square even at axis-aligned instants.
void RenderSolidQuad(const SDL_Vertex (&triangles)[2][3], bool forceEightTimes = false);
void PreloadObliqueQuadTargets();
void CleanupObliqueQuadTargets();


// Snapshot the circular waveform under waveformMutex, then draw it without holding the lock.
// The callback uses try_to_lock so UI rendering cannot delay the audio callback.
void RenderWaveform(float, float, float, float);

// Draw the smoothed bars computed in Dsp.cpp; no FFT is performed here.
void RenderFFTSpectrum_PortAudio(float, float, float, float);
// At small window sizes, composite the already resolved FFT directly onto the
// final window pixels, then draw its title above it.
void CompositeSmallSpectrumToWindow();
// Precreate the initial 8x/4x/2x spectrum texture chain on the SDL thread.
void PreloadSpectrumRenderTargets(float width, float height);
// Reserve reusable FFT curve/vertex/index arrays before the first rendered frame.
void PreloadSpectrumGeometry(float width);
// Use 8x normally; a sustained measured FFT bottleneck may select 4x later.
void SetSpectrumPreferredScale(int scale);
int GetSpectrumPreferredScale();
// Release the GPU texture while the SDL renderer is still alive.
void CleanupSpectrumRenderTarget();
