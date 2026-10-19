// Waveform, spectrum and text drawing using the shared SDL renderer.

#include "Visualization.h"
#include "Dsp.h"
#include "UiLayout.h"
#include "AppFrame.h"

namespace {
struct CachedLabel {
    string text;
    SDL_Color color;
    SDL_Texture* texture;
    int width;
    int height;
};

// Only short, stable UI labels enter this bounded cache. Changing a button caption may add
// one more entry, while per-frame waveform statistics continue through the transient path.
vector<CachedLabel> cachedLabels;
TTF_Font* cachedFont = nullptr;
SDL_Renderer* cachedRenderer = nullptr;
constexpr size_t maxCachedLabels = 128;
CachedLabel dynamicInfoLabel{ {}, { 0, 0, 0, 0 }, nullptr, 0, 0 };
// The continuously changing RMS/peak label has one replaceable texture rather than creating
// and destroying a GPU texture every visual frame. Its numbers refresh ten times per second;
// the waveform itself is still redrawn on every frame.
SDL_Texture* waveformStatsTexture = nullptr;
float waveformStatsWidth = 0.0f;
float waveformStatsHeight = 0.0f;
Uint64 waveformStatsUpdatedNs = 0;

void ClearWaveformStatsTexture() {
    if (waveformStatsTexture) SDL_DestroyTexture(waveformStatsTexture);
    waveformStatsTexture = nullptr;
    waveformStatsWidth = waveformStatsHeight = 0.0f;
    waveformStatsUpdatedNs = 0;
}

void EnsureTextCacheOwner() {
    if (cachedFont == gFont && cachedRenderer == renderer) return;
    for (auto& entry : cachedLabels) SDL_DestroyTexture(entry.texture);
    cachedLabels.clear();
    if (dynamicInfoLabel.texture) SDL_DestroyTexture(dynamicInfoLabel.texture);
    dynamicInfoLabel = { {}, { 0, 0, 0, 0 }, nullptr, 0, 0 };
    ClearWaveformStatsTexture();
    cachedFont = gFont;
    cachedRenderer = renderer;
}
} // namespace

void CleanupTextCache() {
    for (auto& entry : cachedLabels) SDL_DestroyTexture(entry.texture);
    cachedLabels.clear();
    if (dynamicInfoLabel.texture) SDL_DestroyTexture(dynamicInfoLabel.texture);
    dynamicInfoLabel = { {}, { 0, 0, 0, 0 }, nullptr, 0, 0 };
    ClearWaveformStatsTexture();
    cachedFont = nullptr;
    cachedRenderer = nullptr;
}

// Format a floating-point value using the requested number of decimal places.
string FormatDouble(double value, int precision) noexcept
{
    try {
        return format("{:.{}f}", value, precision);
    }
    // If std::format fails, the ostringstream fallback still returns a fixed-decimal label.
    catch (...) {
    }

    ostringstream ss;
    ss.setf(ios::fixed);
    ss << setprecision(precision) << value;
    return ss.str();
}

// Convert a numeric value to text for slider legends.
string ValueToText(double value, int precision) noexcept
{
    return FormatDouble(value, precision);
}

// Append the unit suffix selected by WidgetUnit to a formatted value.
string FormatValueWithUnit(double value, WidgetUnit unit, int precision) noexcept
{
    string s = ValueToText(value, precision);
    switch (unit) {
    case WidgetUnit::Pts:     return s + " pts";
    case WidgetUnit::Hz:      return s + " Hz";
    case WidgetUnit::dB:      return s + " dB";
    // Units here are presentation only; the volume slider converts displayed percent separately in
    // AppUpdate.cpp.
    case WidgetUnit::percent: return s + " %";
    case WidgetUnit::cents:   return s + " cents";
    case WidgetUnit::ms:      return s + " ms";
    case WidgetUnit::degrees: return s + "°";
    default:                return s;
    }
}

// Query the rendered text dimensions after applying the requested scale.
void getSurfaceSize(const string& text, float& surfW, float& surfH, double scale) {
    surfW = surfH = 0.0f;

    EnsureTextCacheOwner();
    if (dynamicInfoLabel.texture && dynamicInfoLabel.text == text) {
        surfW = static_cast<float>(dynamicInfoLabel.width) * scale * 0.5f;
        surfH = static_cast<float>(dynamicInfoLabel.height) * scale * 0.5f;
        return;
    }
    for (const auto& entry : cachedLabels) {
        if (entry.text == text) {
            surfW = static_cast<float>(entry.width) * scale * 0.5f;
            surfH = static_cast<float>(entry.height) * scale * 0.5f;
            return;
        }
    }

    int w = 0, h = 0;
    // The caller must supply an initialized font; this sizing helper does not check gFont for
    // null.
    TTF_GetStringSize(gFont, text.c_str(), text.size(), &w, &h);
    surfW = static_cast<float>(w) * scale * 0.5f;
    surfH = static_cast<float>(h) * scale * 0.5f;
}

bool PrecacheStableLabel(const string& text, SDL_Color color) {
    if (text.empty() || !gFont || !renderer) return false;
    EnsureTextCacheOwner();
    for (const auto& entry : cachedLabels) {
        if (entry.text == text && entry.color.r == color.r &&
            entry.color.g == color.g && entry.color.b == color.b &&
            entry.color.a == color.a) return true;
    }
    if (cachedLabels.size() >= maxCachedLabels) return false;
    SDL_Surface* surface = TTF_RenderText_Blended(gFont, text.c_str(), text.size(), color);
    if (!surface) return false;
    SDL_Texture* texture = SDL_CreateTextureFromSurface(renderer, surface);
    const int width = surface->w;
    const int height = surface->h;
    SDL_DestroySurface(surface);
    if (!texture) return false;
    try {
        cachedLabels.push_back({ text, color, texture, width, height });
    } catch (...) {
        SDL_DestroyTexture(texture);
        return false;
    }
    return true;
}

// The calibration counter has a finite range. Prepare its exact percentage
// captions before the audio worker starts so an SDL_ttf texture upload cannot
// interrupt one of the first displayed frames. Stable titles use this cache too.
void PreloadVisualizationStatusText() {
    const SDL_Color spectrumColor{ 200, 200, 255, 255 };
    const SDL_Color waveformColor{ 200, 255, 200, 255 };
    const SDL_Color missingInputColor{ 200, 100, 100, 255 };
    PrecacheStableLabel("Audio Spectrum (20 Hz - 20 kHz @ 48 kHz) - Calibrated noise",
        spectrumColor);
    PrecacheStableLabel("Audio Waveform (Oscilloscope)", waveformColor);
    PrecacheStableLabel("No audio input detected", missingInputColor);
    for (int frame = 0; frame <= PA_CALIBRATION_FRAMES; ++frame) {
        const int progress = (frame * 100) / PA_CALIBRATION_FRAMES;
        PrecacheStableLabel("PortAudio - Noise calibration: " +
            to_string(progress) + "%", spectrumColor);
    }
}

// Create and draw an SDL_ttf text surface, then report its rendered dimensions.
bool RenderText(const string& text, float x, float y, SDL_Color color, float& outW,
    float& outH, double scale, bool cacheStableLabel) {
    outW = outH = 0.0f;
    if (!gFont || !renderer) return false;
    EnsureTextCacheOwner();
    if (cacheStableLabel) {
        for (const auto& entry : cachedLabels) {
            if (entry.text == text && entry.color.r == color.r &&
                entry.color.g == color.g && entry.color.b == color.b &&
                entry.color.a == color.a) {
                outW = static_cast<float>(entry.width) * scale * 0.5f;
                outH = static_cast<float>(entry.height) * scale * 0.5f;
                const SDL_FRect dst{ x, y, outW, outH };
                return SDL_RenderTexture(renderer, entry.texture, nullptr, &dst);
            }
        }
    }
    // Dynamic text or a new stable label needs one SDL_ttf surface and one texture.
    SDL_Surface* surf = TTF_RenderText_Blended(gFont, text.c_str(), text.size(), color);
    if (!surf) return false;
    SDL_Texture* tex = SDL_CreateTextureFromSurface(renderer, surf);
    if (!tex) {
        SDL_DestroySurface(surf);
        return false;
    }

    float baseW = static_cast<float>(surf->w);
    float baseH = static_cast<float>(surf->h);
    SDL_DestroySurface(surf);
    outW = baseW * scale * 0.5f;
    outH = baseH * scale * 0.5f;
    SDL_FRect dst = { x, y, outW, outH };
    const bool drawn = SDL_RenderTexture(renderer, tex, nullptr, &dst);
    if (cacheStableLabel && cachedLabels.size() < maxCachedLabels)
        cachedLabels.push_back({ text, color, tex, static_cast<int>(baseW),
            static_cast<int>(baseH) });
    else
        SDL_DestroyTexture(tex);
    return drawn;
}

bool RenderDynamicInfoText(const string& text, float x, float y, SDL_Color color,
    float& outW, float& outH, double scale) {
    outW = outH = 0.0f;
    if (!gFont || !renderer || text.empty()) return false;
    EnsureTextCacheOwner();

    // Hover and slider values can produce hundreds of distinct strings. Keeping them in
    // the fixed-caption cache evicted useful labels and eventually made every frame
    // allocate and destroy a GPU texture. This single slot is independent of that cache.
    if (!dynamicInfoLabel.texture || dynamicInfoLabel.text != text ||
        dynamicInfoLabel.color.r != color.r ||
        dynamicInfoLabel.color.g != color.g ||
        dynamicInfoLabel.color.b != color.b ||
        dynamicInfoLabel.color.a != color.a) {
        SDL_Surface* surface = TTF_RenderText_Blended(gFont, text.c_str(), text.size(), color);
        if (!surface) return false;
        SDL_Texture* replacement = SDL_CreateTextureFromSurface(renderer, surface);
        const int width = surface->w;
        const int height = surface->h;
        SDL_DestroySurface(surface);
        if (!replacement) return false;
        if (dynamicInfoLabel.texture) SDL_DestroyTexture(dynamicInfoLabel.texture);
        dynamicInfoLabel = { text, color, replacement, width, height };
    }

    outW = static_cast<float>(dynamicInfoLabel.width) * scale * 0.5f;
    outH = static_cast<float>(dynamicInfoLabel.height) * scale * 0.5f;
    const SDL_FRect destination{ x, y, outW, outH };
    return SDL_RenderTexture(renderer, dynamicInfoLabel.texture, nullptr, &destination);
}

// The 8x tile is reused for every inclined widget. Only its occupied portion
// is copied; keeping the transparent pixels black and resolving without alpha
// blending produces premultiplied coverage for the final blend into the UI.
static SDL_Texture* obliqueTarget8x = nullptr;
static SDL_Texture* obliqueTarget4x = nullptr;
static SDL_Texture* obliqueTarget2x = nullptr;
static SDL_Texture* obliqueTarget1x = nullptr;
static int obliqueCapacityWidth = 0;
static int obliqueCapacityHeight = 0;
static bool obliqueAllocationUnavailable = false;
static bool obliqueRenderingUnavailable = false;

void CleanupObliqueQuadTargets() {
    if (obliqueTarget8x) SDL_DestroyTexture(obliqueTarget8x);
    if (obliqueTarget4x) SDL_DestroyTexture(obliqueTarget4x);
    if (obliqueTarget2x) SDL_DestroyTexture(obliqueTarget2x);
    if (obliqueTarget1x) SDL_DestroyTexture(obliqueTarget1x);
    obliqueTarget8x = obliqueTarget4x = obliqueTarget2x = obliqueTarget1x = nullptr;
    obliqueCapacityWidth = obliqueCapacityHeight = 0;
}

static bool EnsureObliqueQuadTargets(int neededWidth, int neededHeight) {
    if (!renderer) return false;
    if (obliqueTarget8x && obliqueTarget4x &&
        obliqueCapacityWidth >= neededWidth &&
        obliqueCapacityHeight >= neededHeight) return true;
    if (obliqueAllocationUnavailable) return false;

    const int capacityWidth = max(160, static_cast<int>(ceil(neededWidth / 32.0f)) * 32);
    const int capacityHeight = max(160, static_cast<int>(ceil(neededHeight / 32.0f)) * 32);
    SDL_Texture* new8x = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA8888,
        SDL_TEXTUREACCESS_TARGET, capacityWidth * 8, capacityHeight * 8);
    SDL_Texture* new4x = new8x ? SDL_CreateTexture(renderer,
        SDL_PIXELFORMAT_RGBA8888, SDL_TEXTUREACCESS_TARGET,
        capacityWidth * 4, capacityHeight * 4) : nullptr;
    SDL_Texture* new2x = new4x ? SDL_CreateTexture(renderer,
        SDL_PIXELFORMAT_RGBA8888, SDL_TEXTUREACCESS_TARGET,
        capacityWidth * 2, capacityHeight * 2) : nullptr;
    SDL_Texture* new1x = new2x ? SDL_CreateTexture(renderer,
        SDL_PIXELFORMAT_RGBA8888, SDL_TEXTUREACCESS_TARGET,
        capacityWidth, capacityHeight) : nullptr;
    if (!new8x || !new4x ||
        !SDL_SetTextureScaleMode(new8x, SDL_SCALEMODE_LINEAR) ||
        !SDL_SetTextureScaleMode(new4x, SDL_SCALEMODE_LINEAR) ||
        !SDL_SetTextureBlendMode(new8x, SDL_BLENDMODE_NONE) ||
        !SDL_SetTextureBlendMode(new4x, SDL_BLENDMODE_BLEND_PREMULTIPLIED)) {
        if (new8x) SDL_DestroyTexture(new8x);
        if (new4x) SDL_DestroyTexture(new4x);
        if (new2x) SDL_DestroyTexture(new2x);
        if (new1x) SDL_DestroyTexture(new1x);
        obliqueAllocationUnavailable = true;

        return false;
    }
    const bool softTargetsReady = new2x && new1x &&
        SDL_SetTextureScaleMode(new2x, SDL_SCALEMODE_LINEAR) &&
        SDL_SetTextureScaleMode(new1x, SDL_SCALEMODE_LINEAR) &&
        SDL_SetTextureBlendMode(new2x, SDL_BLENDMODE_NONE) &&
        SDL_SetTextureBlendMode(new1x, SDL_BLENDMODE_BLEND_PREMULTIPLIED);
    if (!softTargetsReady) {
        if (new2x) SDL_DestroyTexture(new2x);
        if (new1x) SDL_DestroyTexture(new1x);
        new2x = new1x = nullptr;
    }
    // Keep the new handles local until cleanup releases the previous targets.
    CleanupObliqueQuadTargets();
    obliqueTarget8x = new8x;
    obliqueTarget4x = new4x;
    obliqueTarget2x = new2x;
    obliqueTarget1x = new1x;
    obliqueCapacityWidth = capacityWidth;
    obliqueCapacityHeight = capacityHeight;
    return true;
}

void PreloadObliqueQuadTargets() {
    // The central square and slider diamonds fit comfortably in a 160-unit tile.
    // An unusually large inclined widget can grow the same two targets later.
    (void)EnsureObliqueQuadTargets(160, 160);
}

void RenderSolidQuad(const SDL_Vertex (&triangles)[2][3], bool forceEightTimes) {
    if (!renderer) return;
    const auto drawDirectly = [&]() {
        SDL_RenderGeometry(renderer, nullptr, triangles[0], 3, nullptr, 0);
        SDL_RenderGeometry(renderer, nullptr, triangles[1], 3, nullptr, 0);
    };
    // At all angles, the two input triangles share opposite corners. Inspect
    // adjacent edges to exclude horizontal rectangles despite tiny float noise.
    const SDL_FPoint a = triangles[0][0].position;
    const SDL_FPoint b = triangles[0][1].position;
    const SDL_FPoint c = triangles[1][0].position;
    const auto slanted = [](SDL_FPoint p, SDL_FPoint q) {
        const float dx = fabsf(q.x - p.x);
        const float dy = fabsf(q.y - p.y);
        return min(dx, dy) > hypotf(dx, dy) * 0.025f;
    };
    SDL_Texture* parent = SDL_GetRenderTarget(renderer);
    if (obliqueRenderingUnavailable ||
        (!forceEightTimes && !slanted(a, b) && !slanted(b, c)) ||
        parent == nullptr || CurrentInterfaceSampleFactor() == 0) {
        drawDirectly();
        return;
    }

    const array<SDL_FPoint, 4> corners{ a, b, c, triangles[1][1].position };
    float minimumX = corners[0].x, maximumX = corners[0].x;
    float minimumY = corners[0].y, maximumY = corners[0].y;
    for (const auto& point : corners) {
        minimumX = min(minimumX, point.x);
        maximumX = max(maximumX, point.x);
        minimumY = min(minimumY, point.y);
        maximumY = max(maximumY, point.y);
    }
    const float tileX = floorf(minimumX) - 2.0f;
    const float tileY = floorf(minimumY) - 2.0f;
    const int tileWidth = static_cast<int>(ceilf(maximumX)) + 2 -
        static_cast<int>(tileX);
    const int tileHeight = static_cast<int>(ceilf(maximumY)) + 2 -
        static_cast<int>(tileY);
    if (!EnsureObliqueQuadTargets(tileWidth, tileHeight)) {
        drawDirectly();
        return;
    }

    SDL_Vertex local[2][3];
    for (int i = 0; i < 2; ++i) {
        for (int j = 0; j < 3; ++j) {
            local[i][j] = triangles[i][j];
            local[i][j].position.x -= tileX;
            local[i][j].position.y -= tileY;
        }
    }
    bool ready = SDL_SetRenderTarget(renderer, obliqueTarget8x);
    if (ready) {
        ready = SDL_SetRenderScale(renderer, 8.0f, 8.0f);
        SDL_SetRenderDrawColorFloat(renderer, 0.0f, 0.0f, 0.0f, 0.0f);
        SDL_RenderClear(renderer);
        if (ready) {
            ready = SDL_RenderGeometry(renderer, nullptr, local[0], 3, nullptr, 0)
                && SDL_RenderGeometry(renderer, nullptr, local[1], 3, nullptr, 0);
        }
    }
    if (ready) ready = SDL_SetRenderTarget(renderer, obliqueTarget4x);
    if (ready) {
        // NONE copies RGBA values, averaging 2x2 source pixels. Those values
        // are now premultiplied by their 8x shape coverage.
        const SDL_FRect source{ 0.0f, 0.0f,
            tileWidth * 8.0f, tileHeight * 8.0f };
        const SDL_FRect destination{ 0.0f, 0.0f,
            tileWidth * 4.0f, tileHeight * 4.0f };
        ready = SDL_RenderTexture(renderer, obliqueTarget8x, &source, &destination);
    }
    SDL_Texture* result = obliqueTarget4x;
    int resultScale = 4;
    const bool softenSquare = forceEightTimes && UseLargeWindowEdgeSmoothing() &&
        obliqueTarget2x && obliqueTarget1x;
    if (ready && softenSquare &&
        SDL_SetTextureBlendMode(obliqueTarget4x, SDL_BLENDMODE_NONE) &&
        SDL_SetRenderTarget(renderer, obliqueTarget2x)) {
        const SDL_FRect source{ 0.0f, 0.0f,
            tileWidth * 4.0f, tileHeight * 4.0f };
        const SDL_FRect destination{ 0.0f, 0.0f,
            tileWidth * 2.0f, tileHeight * 2.0f };
        if (SDL_RenderTexture(renderer, obliqueTarget4x, &source, &destination) &&
            SDL_SetRenderTarget(renderer, obliqueTarget1x)) {
            const SDL_FRect halfSource{ 0.0f, 0.0f,
                tileWidth * 2.0f, tileHeight * 2.0f };
            const SDL_FRect finalArea{ 0.0f, 0.0f,
                static_cast<float>(tileWidth), static_cast<float>(tileHeight) };
            if (SDL_RenderTexture(renderer, obliqueTarget2x, &halfSource,
                &finalArea)) {
                result = obliqueTarget1x;
                resultScale = 1;
            }
        }
    }
    SDL_SetTextureBlendMode(obliqueTarget4x, SDL_BLENDMODE_BLEND_PREMULTIPLIED);
    if (!SDL_SetRenderTarget(renderer, parent)) {
        obliqueRenderingUnavailable = true;

        return;
    }
    if (ready) {
        const SDL_FRect source{ 0.0f, 0.0f,
            tileWidth * static_cast<float>(resultScale),
            tileHeight * static_cast<float>(resultScale) };
        const SDL_FRect destination{ tileX, tileY,
            static_cast<float>(tileWidth), static_cast<float>(tileHeight) };
        ready = SDL_RenderTexture(renderer, result, &source, &destination);
    }
    if (!ready) {
        obliqueRenderingUnavailable = true;

        drawDirectly();
    }
}

// Snapshot the circular waveform under waveformMutex and release the lock before drawing it.
void RenderWaveform(float x, float y, float width, float height)
{
    SDL_FRect background = { x, y, width, height };
    SDL_SetRenderDrawColorFloat(renderer, 0.15f, 0.15f, 0.2f, 0.9f);
    SDL_RenderFillRect(renderer, &background);

    // Until capture has filled the ring, show a status label rather than reading uninitialized
    // display values.
    if (!waveformDataReady.load(memory_order_acquire)) {
        if (gFont) {
            SDL_Color textColor = { 200, 100, 100, 255 };
            float tw = 0.0f, th = 0.0f;
            RenderText("No audio input detected", x + 10, y + height / 2.0f, textColor, tw, th, 0.6, true);
        }
        return;
    }

    SDL_SetRenderDrawColorFloat(renderer, 0.4f, 0.4f, 0.5f, 0.6f);
    SDL_RenderLine(renderer, x, y + height / 2.0f, x + width, y + height / 2.0f);

    static vector<SDL_FPoint> wavePoints;
    wavePoints.clear();
    wavePoints.reserve(WAVEFORM_SAMPLES);

    // Copy the microphone ring quickly; drawing 2048 line segments never holds the
    // callback's waveform lock.
    array<float, WAVEFORM_SAMPLES> recent{};
    {
        lock_guard<mutex> lock(waveformMutex);
        // Copy the circular buffer in two contiguous spans. The old per-sample
        // modulo loop kept the same mutex that the audio callback needs for longer.
        const size_t start = waveformWritePos % WAVEFORM_SAMPLES;
        const size_t firstSpan = WAVEFORM_SAMPLES - start;
        copy_n(waveformBuffer.data() + start, firstSpan, recent.data());
        if (start != 0)
            copy_n(waveformBuffer.data(), start, recent.data() + firstSpan);
    }
    float xStep = width / static_cast<float>(WAVEFORM_SAMPLES);

    float currentRMS = 0.0f;

    for (size_t i = 0; i < WAVEFORM_SAMPLES; ++i) {
        float sample = recent[i];

        sample = Max(-1.0f, Min(1.0f, sample));

        float px = x + i * xStep;
        // Map signed amplitude to vertical pixels around the midline; positive samples point
        // upward.
        float py = y + height / 2.0f - (sample * height * 0.45f);

        wavePoints.push_back({ px, py });

        currentRMS += sample * sample;

    }

    // RMS summarizes the displayed ring, not the entire recording.
    currentRMS = sqrt(currentRMS / WAVEFORM_SAMPLES);

    // One batched geometry call replaces 2047 separate SDL draw submissions per frame.
    if (wavePoints.size() >= 2) {
        SDL_SetRenderDrawColorFloat(renderer, 0.3f, 0.92f, 0.35f, 1.0f);
        SDL_RenderLines(renderer, wavePoints.data(), static_cast<int>(wavePoints.size()));
    }

    float peak = 0.0f;
    for (size_t i = 0; i < WAVEFORM_SAMPLES; ++i) {
        peak = Max(peak, fabs(recent[i]));
    }

    if (gFont) {
        SDL_Color textColor = { 200, 255, 200, 255 };
        float tw = 0.0f, th = 0.0f;

        string label = "Audio Waveform (Oscilloscope)";
        RenderText(label, x + 10, y + 5, textColor, tw, th, 0.6, true);

        const Uint64 nowNs = SDL_GetTicksNS();
        constexpr Uint64 statsIntervalNs = 100000000ULL;
        if (!waveformStatsTexture || nowNs - waveformStatsUpdatedNs >= statsIntervalNs) {
            SDL_Color statsColor = (currentRMS > 0.01f)
                ? SDL_Color{ 100, 255, 100, 255 } : SDL_Color{ 200, 200, 200, 255 };
            string stats = "RMS: " + FormatDouble(currentRMS * 100.0, 2) +
                "% | Peak: " + FormatDouble(peak * 100.0, 1) + "%";
            SDL_Surface* statsSurface = TTF_RenderText_Blended(gFont, stats.c_str(),
                stats.size(), statsColor);
            if (statsSurface) {
                SDL_Texture* newTexture = SDL_CreateTextureFromSurface(renderer, statsSurface);
                if (newTexture) {
                    ClearWaveformStatsTexture();
                    waveformStatsTexture = newTexture;
                    waveformStatsWidth = statsSurface->w * 0.25f;
                    waveformStatsHeight = statsSurface->h * 0.25f;
                    waveformStatsUpdatedNs = nowNs;
                }
                SDL_DestroySurface(statsSurface);
            }
        }
        if (waveformStatsTexture) {
            const SDL_FRect dst{ x + 10, y + height - 25,
                waveformStatsWidth, waveformStatsHeight };
            SDL_RenderTexture(renderer, waveformStatsTexture, nullptr, &dst);
        }
    }
}

// Draw the FFT at 8x and resolve through 4x and 2x before the 2x interface
// target reaches the window. Reuse the render targets between visual frames.
static SDL_Texture* spectrumTarget = nullptr;
static SDL_Texture* spectrumIntermediateTarget = nullptr;
static SDL_Texture* spectrumResolveTarget = nullptr;
static SDL_Texture* spectrumLogicalTarget = nullptr;
static SDL_Texture* spectrumPixelTarget = nullptr;
static bool spectrumPixelOverlayReady = false;
static SDL_FRect spectrumPixelDestination{};
static SDL_FRect spectrumPixelSource{};
static SDL_FRect spectrumLogicalOverlaySource{};
static bool spectrumTargetUnavailable = false;
static int spectrumTargetWidth = 0;
static int spectrumTargetHeight = 0;
static int spectrumTargetScale = 0;
static int spectrumPreferredScale = 8;

// Reuse the geometry storage each frame. Reserve the approved 560-pixel graph's worst
// case before the first draw; a future wider graph can still grow these vectors.
struct SpectrumGeometryScratch {
    vector<SDL_FPoint> denseCurve;
    vector<uint8_t> retainedPoints;
    vector<pair<int, int>> curveIntervals;
    vector<SDL_FPoint> curvePoints;
    vector<SDL_Vertex> fillVertices;
    vector<int> fillIndices;
    vector<SDL_Vertex> edgeVertices;
    vector<int> edgeIndices;
};
static SpectrumGeometryScratch spectrumGeometry;
static void DestroySpectrumTargets();

// Index topology depends only on the graph width, never on microphone data.
// Build it at startup and rebuild it only if the graph's sampling width changes.
static void EnsureSpectrumIndices(size_t pointCount) {
    if (pointCount < 2) return;
    auto& fill = spectrumGeometry.fillIndices;
    const size_t fillCount = (pointCount - 1) * 6;
    if (fill.size() != fillCount) {
        fill.clear();
        fill.reserve(fillCount);
        for (size_t i = 0; i + 1 < pointCount; ++i) {
            const int left = static_cast<int>(i * 2);
            fill.push_back(left);
            fill.push_back(left + 1);
            fill.push_back(left + 2);
            fill.push_back(left + 1);
            fill.push_back(left + 2);
            fill.push_back(left + 3);
        }
    }
    auto& edge = spectrumGeometry.edgeIndices;
    const size_t edgeCount = (pointCount - 1) * 5 * 6;
    if (edge.size() != edgeCount) {
        edge.clear();
        edge.reserve(edgeCount);
        for (size_t i = 0; i + 1 < pointCount; ++i) {
            for (size_t band = 0; band < 5; ++band) {
                const int topLeft = static_cast<int>(i * 6 + band);
                const int bottomLeft = topLeft + 1;
                const int topRight = static_cast<int>((i + 1) * 6 + band);
                const int bottomRight = topRight + 1;
                edge.push_back(topLeft);
                edge.push_back(bottomLeft);
                edge.push_back(topRight);
                edge.push_back(bottomLeft);
                edge.push_back(topRight);
                edge.push_back(bottomRight);
            }
        }
    }
}

void PreloadSpectrumGeometry(float width) {
    if (width <= 0.0f) return;
    const size_t count = static_cast<size_t>(ceil(width * 8.0f)) + 1;
    spectrumGeometry.denseCurve.reserve(count);
    spectrumGeometry.retainedPoints.reserve(count);
    spectrumGeometry.curveIntervals.reserve(PA_NUM_BARS * 8);
    spectrumGeometry.curvePoints.reserve(count);
    spectrumGeometry.fillVertices.reserve(count * 2);
    spectrumGeometry.fillIndices.reserve((count - 1) * 6);
    spectrumGeometry.edgeVertices.reserve(count * 6);
    spectrumGeometry.edgeIndices.reserve((count - 1) * 5 * 6);
    EnsureSpectrumIndices(count);
}

void SetSpectrumPreferredScale(int scale) {
    if ((scale == 8 || scale == 4) && scale != spectrumPreferredScale) {
        spectrumPreferredScale = scale;
        // With an allocated 8x/4x/2x chain, 4x can draw directly into its 4x
        // intermediate target. Retain the chain so a temporary quality drop does
        // not destroy/recreate GPU textures during an already slow frame.
        if (scale > spectrumTargetScale && spectrumTarget != nullptr)
            DestroySpectrumTargets();
        // A quality increase retries allocation if an earlier 8x attempt failed.
        spectrumTargetUnavailable = false;
    }
}

int GetSpectrumPreferredScale() { return spectrumPreferredScale; }

static void DestroySpectrumTargets() {
    if (spectrumTarget != nullptr) SDL_DestroyTexture(spectrumTarget);
    if (spectrumIntermediateTarget != nullptr)
        SDL_DestroyTexture(spectrumIntermediateTarget);
    if (spectrumResolveTarget != nullptr) SDL_DestroyTexture(spectrumResolveTarget);
    if (spectrumLogicalTarget != nullptr)
        SDL_DestroyTexture(spectrumLogicalTarget);
    if (spectrumPixelTarget != nullptr)
        SDL_DestroyTexture(spectrumPixelTarget);
    spectrumTarget = nullptr;
    spectrumIntermediateTarget = nullptr;
    spectrumResolveTarget = nullptr;
    spectrumLogicalTarget = nullptr;
    spectrumPixelTarget = nullptr;
    spectrumPixelOverlayReady = false;
}

// Draw spectrum geometry without text; the title is rendered on the current interface target
// after the spectrum image is copied back into it.
static void DrawSpectrumBody(float x, float y, float width, float height,
    int samplingScale, bool targetAlreadyCleared = false)
{
    // The offscreen target was just cleared to the same opaque color. Repainting its
    // entire 8x surface with that color would add millions of redundant pixel writes.
    if (!targetAlreadyCleared) {
        SDL_FRect background = { x, y, width, height };
        SDL_SetRenderDrawColorFloat(renderer, 0.1f, 0.1f, 0.15f, 0.8f);
        SDL_RenderFillRect(renderer, &background);
    }

    // The FFT still has 96 measured bands. Evaluate the curve once per target
    // pixel. Monotone cubic tangents keep each measured band height and round
    // the peak tips without inventing an overshoot or clipping one into a flat top.
    auto& curvePoints = spectrumGeometry.curvePoints;
    auto& denseCurve = spectrumGeometry.denseCurve;
    denseCurve.clear();
    // Tessellate the curve at the active raster density. Sampling its shape
    // only at 4 points per logical unit left straight segments in an 8x image,
    // particularly along steep narrow peaks at full window size.
    const int displaySegments = max(1,
        static_cast<int>(ceil(width * min(samplingScale, 8))));
    denseCurve.reserve(static_cast<size_t>(displaySegments) + 1);
    const int lastBand = static_cast<int>(PA_NUM_BARS) - 1;
    const auto magnitudeAt = [&](int band) {
        return paFftSmoothed[clamp(band, 0, lastBand)];
    };
    for (int pixelStep = 0; pixelStep <= displaySegments; ++pixelStep) {
        const float fractionX = static_cast<float>(pixelStep) / displaySegments;
        const float bandPosition = fractionX * PA_NUM_BARS - 0.5f;
        const int firstBand = static_cast<int>(floor(bandPosition));
        const float t = bandPosition - firstBand;
        const float p0 = magnitudeAt(firstBand - 1);
        const float p1 = magnitudeAt(firstBand);
        const float p2 = magnitudeAt(firstBand + 1);
        const float p3 = magnitudeAt(firstBand + 2);
        const float before = p1 - p0;
        const float between = p2 - p1;
        const float after = p3 - p2;
        const float startSlope = before * between > 0.0f
            ? 2.0f * before * between / (before + between) : 0.0f;
        const float endSlope = between * after > 0.0f
            ? 2.0f * between * after / (between + after) : 0.0f;
        const float t2 = t * t;
        const float t3 = t2 * t;
        const float cubic = (2.0f * t3 - 3.0f * t2 + 1.0f) * p1 +
            (t3 - 2.0f * t2 + t) * startSlope +
            (-2.0f * t3 + 3.0f * t2) * p2 +
            (t3 - t2) * endSlope;
        const float magnitude = clamp(cubic, min(p1, p2), max(p1, p2));
        const SDL_FPoint next{ x + fractionX * width,
            y + height - clamp(magnitude, 0.0f, 1.0f) * height * 0.9f };
        denseCurve.push_back(next);
    }
    // Every raster sample above is evaluated at the full 8x (or adaptive 4x)
    // resolution. Reduce only almost-collinear GPU geometry, with a strict
    // bound on the distance from *every* original sample to the retained line.
    // Work within each spectral band so a narrow peak cannot disappear.
    auto& retained = spectrumGeometry.retainedPoints;
    retained.assign(denseCurve.size(), 0);
    auto& intervals = spectrumGeometry.curveIntervals;
    intervals.clear();
    constexpr float maxLogicalError = 0.035f;
    for (int band = 0; band < static_cast<int>(PA_NUM_BARS); ++band) {
        const int start = band * displaySegments / static_cast<int>(PA_NUM_BARS);
        const int end = (band + 1) * displaySegments / static_cast<int>(PA_NUM_BARS);
        retained[start] = retained[end] = 1;
        intervals.emplace_back(start, end);
        while (!intervals.empty()) {
            const auto [first, last] = intervals.back();
            intervals.pop_back();
            if (last <= first + 1) continue;
            const auto& a = denseCurve[first];
            const auto& b = denseCurve[last];
            const float inverseSpan = 1.0f / (b.x - a.x);
            float maximumError = maxLogicalError;
            int worstPoint = -1;
            for (int sample = first + 1; sample < last; ++sample) {
                const auto& point = denseCurve[sample];
                const float straightY = a.y + (b.y - a.y) *
                    ((point.x - a.x) * inverseSpan);
                const float error = fabsf(point.y - straightY);
                if (error > maximumError) {
                    maximumError = error;
                    worstPoint = sample;
                }
            }
            if (worstPoint >= 0) {
                retained[worstPoint] = 1;
                intervals.emplace_back(first, worstPoint);
                intervals.emplace_back(worstPoint, last);
            }
        }
    }
    curvePoints.clear();
    curvePoints.reserve(denseCurve.size());
    for (size_t i = 0; i < denseCurve.size(); ++i)
        if (retained[i]) curvePoints.push_back(denseCurve[i]);

    if (curvePoints.size() >= 3) {
        // Keep the approved top-to-bottom fill. Shifting its interior vertices
        // sideways across narrow peaks created overlapping strips and streaks.
        auto& fillVertices = spectrumGeometry.fillVertices;
        fillVertices.clear();
        fillVertices.reserve(curvePoints.size() * 2);

        for (size_t i = 0; i < curvePoints.size(); ++i) {
            // Reduced vertex density must not move the spectrum's horizontal
            // color gradient toward the denser, more curved regions.
            float hue = clamp((curvePoints[i].x - x) / width, 0.0f, 1.0f);
            float intensity = (curvePoints[i].y < y + height) ?
                1.0f - (curvePoints[i].y - y) / height : 0.0f;

            float r = Max(0.0f, Min(1.0f, (hue - 0.5f) * 2.0f + intensity * 0.3f));
            float g = Max(0.0f, Min(1.0f, 1.0f - fabs(hue - 0.5f) * 2.0f + intensity * 0.2f));
            float b = Max(0.0f, Min(1.0f, (0.5f - hue) * 2.0f + intensity * 0.1f));

            SDL_Vertex topVertex;
            topVertex.position = curvePoints[i];
            topVertex.color = { r, g, b, 0.6f };

            SDL_Vertex bottomVertex;
            bottomVertex.position = { curvePoints[i].x, y + height };
            bottomVertex.color = { r * 0.3f, g * 0.3f, b * 0.3f, 0.2f };

            fillVertices.push_back(topVertex);
            fillVertices.push_back(bottomVertex);
        }

        // Share colored vertices between neighboring triangles. The indices are
        // prebuilt for the usual graph size and updated only if its width changes.
        auto& fillIndices = spectrumGeometry.fillIndices;
        EnsureSpectrumIndices(curvePoints.size());
        SDL_RenderGeometry(renderer, nullptr, fillVertices.data(),
            static_cast<int>(fillVertices.size()), fillIndices.data(),
            static_cast<int>(fillIndices.size()));
    }

    if (curvePoints.size() >= 2) {
        // Keep the reference's vertical halo, which is broad on flat valleys
        // and peak tips and naturally thin on steep sides. More raster samples
        // smooth the steep edges without widening the contour sideways.
        auto& edgeVertices = spectrumGeometry.edgeVertices;
        edgeVertices.clear();
        constexpr array<float, 6> offsets{ -4.0f, -2.0f, -0.55f,
            0.55f, 2.0f, 4.0f };
        constexpr array<float, 6> opacities{ 0.0f, 0.28f, 1.0f,
            1.0f, 0.28f, 0.0f };
        edgeVertices.reserve(curvePoints.size() * offsets.size());
        for (const auto& point : curvePoints) {
            for (size_t edge = 0; edge < offsets.size(); ++edge) {
                SDL_Vertex vertex{};
                vertex.position = { point.x,
                    clamp(point.y + offsets[edge], y + 1.0f, y + height - 1.0f) };
                vertex.color = { 0.42f, 0.92f, 1.0f,
                    opacities[edge] };
                edgeVertices.push_back(vertex);
            }
        }
        auto& edgeIndices = spectrumGeometry.edgeIndices;
        EnsureSpectrumIndices(curvePoints.size());
        SDL_RenderGeometry(renderer, nullptr, edgeVertices.data(),
            static_cast<int>(edgeVertices.size()), edgeIndices.data(),
            static_cast<int>(edgeIndices.size()));
    }

}

// Grow FFT texture capacity in 64-logical-pixel steps. A width change while
// dragging the window reuses existing targets until it exceeds that capacity.
void PreloadSpectrumRenderTargets(float width, float height)
{
    if (!renderer || width <= 0.0f || height <= 0.0f) return;
    if (spectrumTarget != nullptr &&
        (static_cast<int>(ceil(width * spectrumTargetScale)) > spectrumTargetWidth ||
            static_cast<int>(ceil(height * spectrumTargetScale)) != spectrumTargetHeight)) {
        DestroySpectrumTargets();
    }
    if (!spectrumTargetUnavailable && spectrumTarget == nullptr) {
        const int capacityWidth = static_cast<int>(ceil(width / 64.0f)) * 64;
        for (int candidateScale : { 8, 4, 2 }) {
            if (candidateScale > spectrumPreferredScale) continue;
            const int targetWidth = capacityWidth * candidateScale;
            const int targetHeight = static_cast<int>(ceil(height * candidateScale));
            spectrumTarget = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA8888,
                SDL_TEXTUREACCESS_TARGET, targetWidth, targetHeight);
            if (spectrumTarget != nullptr && candidateScale == 8)
                spectrumIntermediateTarget = SDL_CreateTexture(renderer,
                    SDL_PIXELFORMAT_RGBA8888, SDL_TEXTUREACCESS_TARGET,
                    capacityWidth * 4,
                    static_cast<int>(ceil(height * 4.0f)));
            if (spectrumTarget != nullptr &&
                (candidateScale != 8 || spectrumIntermediateTarget != nullptr) &&
                candidateScale >= 4)
                spectrumResolveTarget = SDL_CreateTexture(renderer,
                    SDL_PIXELFORMAT_RGBA8888, SDL_TEXTUREACCESS_TARGET,
                    capacityWidth * 2,
                    static_cast<int>(ceil(height * 2.0f)));
            if (spectrumTarget != nullptr &&
                (candidateScale != 8 || spectrumIntermediateTarget != nullptr) &&
                (candidateScale < 4 || spectrumResolveTarget != nullptr)) {
                spectrumTargetScale = candidateScale;
                spectrumTargetWidth = targetWidth;
                spectrumTargetHeight = targetHeight;
                SDL_SetTextureScaleMode(spectrumTarget, SDL_SCALEMODE_LINEAR);
                if (spectrumIntermediateTarget != nullptr)
                    SDL_SetTextureScaleMode(spectrumIntermediateTarget, SDL_SCALEMODE_LINEAR);
                if (spectrumResolveTarget != nullptr)
                    SDL_SetTextureScaleMode(spectrumResolveTarget, SDL_SCALEMODE_LINEAR);
                // Optional last 2x -> 1x resolve for the FFT at every window
                // size. Never sacrifice the 8x FFT path
                // if this small extra target is unavailable.
                spectrumLogicalTarget = SDL_CreateTexture(renderer,
                    SDL_PIXELFORMAT_RGBA8888, SDL_TEXTUREACCESS_TARGET,
                    capacityWidth, static_cast<int>(ceil(height)));
                if (spectrumLogicalTarget)
                    SDL_SetTextureScaleMode(spectrumLogicalTarget,
                        SDL_SCALEMODE_LINEAR);
                // Reserve a final physical-pixel target for small windows.
                // The title is drawn separately after this target reaches
                // the window so its glyphs retain their usual resolution.
                if (spectrumLogicalTarget != nullptr) {
                    spectrumPixelTarget = SDL_CreateTexture(renderer,
                        SDL_PIXELFORMAT_RGBA8888, SDL_TEXTUREACCESS_TARGET,
                        static_cast<int>(ceil(capacityWidth * 0.72f)),
                        static_cast<int>(ceil(height * 0.72f)));
                    if (spectrumPixelTarget != nullptr)
                        SDL_SetTextureScaleMode(spectrumPixelTarget,
                            SDL_SCALEMODE_LINEAR);
                }
                break;
            }

            DestroySpectrumTargets();
        }
        spectrumTargetUnavailable = spectrumTarget == nullptr;
    }

}

// Resolve the 8x FFT through 4x, 2x and 1x before the final interface fit.
// Keep the 4x intermediate allocated for the adaptive 4x fallback.
void RenderFFTSpectrum_PortAudio(float x, float y, float width, float height)
{
    spectrumPixelOverlayReady = false;
    // The caller may itself be drawing into the 2x interface target. Restore
    // that target after the FFT passes so the remaining UI stays supersampled.
    SDL_Texture* previousTarget = SDL_GetRenderTarget(renderer);
    // The FFT has steep peaks even in the smallest window. Finish its
    // 8x -> 4x -> 2x -> 1x resolve at all sizes before the interface pass.
    // At small physical sizes, the finished FFT is copied directly to window pixels
    // after the rest of the UI; otherwise a second resize would blur the peaks.
    // The separate rotating-square policy remains size-dependent.
    const bool smoothSpectrum = previousTarget != nullptr;
    int outputWidth = 0, outputHeight = 0;
    const bool hasOutputSize = SDL_GetRenderOutputSize(renderer,
        &outputWidth, &outputHeight);
    const UiViewport physicalViewport = hasOutputSize
        ? CalculateUiViewport(outputWidth, outputHeight) : UiViewport{};
    const bool smallPhysicalSpectrum = hasOutputSize &&
        physicalViewport.scale < 0.72f;
    const float physicalLeft = roundf(physicalViewport.x + x * physicalViewport.scale);
    const float physicalTop = roundf(physicalViewport.y + y * physicalViewport.scale);
    const float physicalRight = roundf(physicalViewport.x +
        (x + width) * physicalViewport.scale);
    const float physicalBottom = roundf(physicalViewport.y +
        (y + height) * physicalViewport.scale);
    const float pixelWidth = physicalRight - physicalLeft;
    const float pixelHeight = physicalBottom - physicalTop;
    PreloadSpectrumRenderTargets(width, height);

    bool spectrumReady = false;
    bool logicalResolveReady = false;
    bool pixelResolveReady = false;
    SDL_Texture* drawTarget = spectrumTarget;
    int drawScale = spectrumTargetScale;
    if (spectrumTarget != nullptr) {
        const bool useRetained4x = spectrumTargetScale == 8 &&
            spectrumPreferredScale == 4 && spectrumIntermediateTarget != nullptr;
        if (useRetained4x) drawTarget = spectrumIntermediateTarget;
        drawScale = useRetained4x ? 4 : spectrumTargetScale;
        const bool targetActive = SDL_SetRenderTarget(renderer, drawTarget);
        if (targetActive && SDL_SetRenderScale(renderer,
            static_cast<float>(drawScale),
            static_cast<float>(drawScale))) {
            SDL_SetRenderDrawColorFloat(renderer, 0.1f, 0.1f, 0.15f, 1.0f);
            // Only the used graph region is sampled from a capacity texture.
            const SDL_FRect graphArea{ 0.0f, 0.0f, width, height };
            SDL_RenderFillRect(renderer, &graphArea);
            DrawSpectrumBody(0.0f, 0.0f, width, height, drawScale, true);
            spectrumReady = true;
        }
        if (spectrumReady && spectrumIntermediateTarget != nullptr && !useRetained4x) {
            spectrumReady = SDL_SetRenderTarget(renderer, spectrumIntermediateTarget);
            if (spectrumReady) {
                const SDL_FRect intermediateArea{ 0.0f, 0.0f,
                    width * 4.0f, height * 4.0f };
                const SDL_FRect sourceArea{ 0.0f, 0.0f,
                    width * 8.0f, height * 8.0f };
                spectrumReady = SDL_RenderTexture(renderer, spectrumTarget,
                    &sourceArea, &intermediateArea);
            }
        }
        if (spectrumReady && spectrumResolveTarget != nullptr) {
            spectrumReady = SDL_SetRenderTarget(renderer, spectrumResolveTarget);
            if (spectrumReady) {
                const SDL_FRect resolveArea{ 0.0f, 0.0f, width * 2.0f, height * 2.0f };
                SDL_Texture* input = spectrumIntermediateTarget != nullptr
                    ? spectrumIntermediateTarget : drawTarget;
                const int inputScale = spectrumIntermediateTarget != nullptr
                    ? 4 : drawScale;
                const SDL_FRect sourceArea{ 0.0f, 0.0f,
                    width * inputScale, height * inputScale };
                spectrumReady = SDL_RenderTexture(renderer, input,
                    &sourceArea, &resolveArea);
            }
        }
        if (spectrumReady && smoothSpectrum &&
            spectrumResolveTarget && spectrumLogicalTarget &&
            SDL_SetRenderTarget(renderer, spectrumLogicalTarget)) {
            const SDL_FRect sourceArea{ 0.0f, 0.0f,
                width * 2.0f, height * 2.0f };
            const SDL_FRect destination{ 0.0f, 0.0f, width, height };
            logicalResolveReady = SDL_RenderTexture(renderer,
                spectrumResolveTarget, &sourceArea, &destination);
        }
        if (logicalResolveReady && smallPhysicalSpectrum &&
            spectrumPixelTarget && pixelWidth > 0.0f && pixelHeight > 0.0f &&
            SDL_SetRenderTarget(renderer, spectrumPixelTarget)) {
            const SDL_FRect sourceArea{ 0.0f, 0.0f, width, height };
            const SDL_FRect destination{ 0.0f, 0.0f,
                pixelWidth, pixelHeight };
            pixelResolveReady = SDL_RenderTexture(renderer,
                spectrumLogicalTarget, &sourceArea, &destination);
        }
        SDL_SetRenderTarget(renderer, previousTarget);
        if (!spectrumReady) {
            DestroySpectrumTargets();
            spectrumTargetUnavailable = true;
        }
    }
    if (spectrumReady) {
        if (pixelResolveReady) {
            // Leave this graph out of the supersampled interface texture.
            // Otherwise the 1x physical image is scaled up and down again,
            // blurring the peaks that the final resolve just sharpened.
            spectrumPixelSource = { 0.0f, 0.0f, pixelWidth, pixelHeight };
            spectrumLogicalOverlaySource = { 0.0f, 0.0f, width, height };
            spectrumPixelDestination = { physicalLeft, physicalTop,
                pixelWidth, pixelHeight };
            spectrumPixelOverlayReady = true;
        }
        const SDL_FRect destination{ x, y, width, height };
        SDL_Texture* result = logicalResolveReady ? spectrumLogicalTarget :
            (spectrumResolveTarget != nullptr ? spectrumResolveTarget :
                (spectrumIntermediateTarget != nullptr && drawTarget != spectrumIntermediateTarget
                    ? spectrumIntermediateTarget : drawTarget));
        const int resultScale = result == spectrumLogicalTarget ? 1 :
            result == spectrumResolveTarget ? 2 :
            result == spectrumIntermediateTarget ? 4 : drawScale;
        const SDL_FRect sourceArea{ 0.0f, 0.0f,
            width * resultScale, height * resultScale };
        if (!pixelResolveReady &&
            !SDL_RenderTexture(renderer, result, &sourceArea, &destination)) {
            DestroySpectrumTargets();
            spectrumTargetUnavailable = true;
            DrawSpectrumBody(x, y, width, height, 2);
        }
    } else {
        DrawSpectrumBody(x, y, width, height, 2);
    }

    if (gFont && !spectrumPixelOverlayReady) {
        SDL_Color textColor = { 200, 200, 255, 255 };
        float tw = 0.0f, th = 0.0f;

        string label;
        // The spectrum title switches from progress to the calibrated display label when startup
        // calibration ends.
        if (paIsCalibrating) {
            int progress = (paCalibrationFrames * 100) / PA_CALIBRATION_FRAMES;
            label = "PortAudio - Noise calibration: " + to_string(progress) + "%";
        }
        else {
            label = "Audio Spectrum (20 Hz - 20 kHz @ 48 kHz) - Calibrated noise";
        }

        RenderText(label, x + 10, y + 5, textColor, tw, th, 0.6, true);
    }
}

void CompositeSmallSpectrumToWindow() {
    if (!spectrumPixelOverlayReady || !spectrumPixelTarget) return;
    spectrumPixelOverlayReady = false;
    // RenderFrame has already composited the rest of the interface. This is
    // an exact physical-pixel copy, with no further enlargement or reduction.
    SDL_SetRenderScale(renderer, 1.0f, 1.0f);
    SDL_SetRenderViewport(renderer, nullptr);
    if (!SDL_RenderTexture(renderer, spectrumPixelTarget,
        &spectrumPixelSource, &spectrumPixelDestination)) {
        // Keep the visualization visible if an optional GPU copy fails.
        if (!spectrumLogicalTarget ||
            !SDL_RenderTexture(renderer, spectrumLogicalTarget,
                &spectrumLogicalOverlaySource, &spectrumPixelDestination)) return;
    }
    if (!gFont) return;
    const string label = paIsCalibrating
        ? "PortAudio - Noise calibration: " +
            to_string((paCalibrationFrames * 100) / PA_CALIBRATION_FRAMES) + "%"
        : "Audio Spectrum (20 Hz - 20 kHz @ 48 kHz) - Calibrated noise";
    const float scale = spectrumPixelDestination.h / 210.0f;
    float textWidth = 0.0f, textHeight = 0.0f;
    RenderText(label, spectrumPixelDestination.x + 10.0f * scale,
        spectrumPixelDestination.y + 5.0f * scale,
        SDL_Color{ 200, 200, 255, 255 }, textWidth, textHeight,
        0.6 * scale, true);
}

void CleanupSpectrumRenderTarget() {
    DestroySpectrumTargets();
}
