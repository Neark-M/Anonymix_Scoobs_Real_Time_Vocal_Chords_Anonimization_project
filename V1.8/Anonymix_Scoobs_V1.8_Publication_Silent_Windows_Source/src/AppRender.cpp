// Per-frame SDL rendering, pointer interaction and slider movement. Called on the SDL event thread.

#include "Widgets.h"
#include "Visualization.h"
#include "AppFrame.h"

struct SliderCaptionTexture {
    const char* name;
    float centerX;
    SDL_Texture* texture = nullptr;
    float width = 0.0f;
    float height = 0.0f;
};

static array<SliderCaptionTexture, 6> sliderCaptions{{
    {"Pitch", 715.0f}, {"Formant", 843.0f}, {"Timbre", 971.0f},
    {"Prosody", 1099.0f}, {"Jitter", 1227.0f}, {"Volume", 1355.0f}
}};
static bool sliderCaptionsPrepared = false;
// Reserve render targets for the responsive canvas. Near full size, 4x samples
// give steep FFT peaks and the rotating square smoother physical pixel edges.
static SDL_Texture* interfaceTarget = nullptr;
static SDL_Texture* interfaceHalfTarget = nullptr;
static SDL_Texture* interfaceResolvedTarget = nullptr;
static int interfaceTargetFactor = 0;
static int interfaceTargetCapacityWidth = 0;
static int interfaceMaximumFactor = 4;
static bool interfaceTargetUnavailable = false;
static int lastResponsiveLayoutWidth = -1;

static void DestroyInterfaceTargets() {
    if (interfaceTarget) SDL_DestroyTexture(interfaceTarget);
    if (interfaceHalfTarget) SDL_DestroyTexture(interfaceHalfTarget);
    if (interfaceResolvedTarget) SDL_DestroyTexture(interfaceResolvedTarget);
    interfaceTarget = interfaceHalfTarget = interfaceResolvedTarget = nullptr;
    interfaceTargetFactor = 0;
    interfaceTargetCapacityWidth = 0;
}

int CurrentInterfaceSampleFactor() { return interfaceTargetFactor; }

bool UseLargeWindowEdgeSmoothing() {
    if (!window) return false;
    int width = 0, height = 0;
    if (!SDL_GetWindowSize(window, &width, &height)) return false;
    static int cachedWidth = -1, cachedHeight = -1;
    static bool enabled = false;
    if (width == cachedWidth && height == cachedHeight)
        return enabled;
    cachedWidth = width;
    cachedHeight = height;
    // Judge the displayed UI scale rather than the fraction of desktop area.
    // A medium window such as 1147x857 still shows steep edges on a large
    // desktop; a percentage-of-desktop test missed it entirely. Hysteresis
    // prevents repeated target changes during a resize near this boundary.
    const float scale = CalculateUiViewport(width, height).scale;
    enabled = scale >= (enabled ? 0.72f : 0.76f);
    return enabled;
}

void RefreshResponsiveUiGeometry() {
    if (!window) return;
    int width = 0, height = 0;
    if (!SDL_GetWindowSize(window, &width, &height)) return;
    const int layoutWidth = static_cast<int>(lround(CalculateUiLayoutWidth(width, height)));
    if (layoutWidth == lastResponsiveLayoutWidth) return;
    lastResponsiveLayoutWidth = layoutWidth;
    const float extra = static_cast<float>(layoutWidth - kUiLayoutWidth);
    const float left = CalculateLeftExpansion(static_cast<float>(layoutWidth));
    const float right = extra - left;
    const auto move = [](const char* name, float newX) {
        widgets(name).setCenterX(static_cast<uint16_t>(lround(newX)));
    };
    widgets("AudioMode").setRectangleShape(
        static_cast<uint16_t>(lround(140.0f + right / 6.0f)), 40);
    widgets("WasapiMode").setRectangleShape(
        static_cast<uint16_t>(lround(100.0f + right / 6.0f)), 40);
    widgets("AudioDevices").setRectangleShape(
        static_cast<uint16_t>(lround(115.0f + right / 6.0f)), 40);
    move("AudioMode", 800.0f + left + right / 6.0f);
    move("WasapiMode", 1060.0f + left + right * 0.5f);
    move("AudioDevices", 1295.0f + extra - right / 6.0f);
    const array<const char*, 4> firstRow{ "EffectPitch", "EffectFormant",
        "PA_Record", "PA_PlaySelected" };
    const array<const char*, 4> secondRow{ "EffectTimbre", "EffectProsody",
        "EffectJitter", "PA_SaveSelected" };
    for (size_t i = 0; i < firstRow.size(); ++i) {
        const auto buttonHalfWidth = static_cast<uint16_t>(lround(
            90.0f + right / 8.0f));
        widgets(firstRow[i]).setRectangleShape(buttonHalfWidth, 35);
        widgets(secondRow[i]).setRectangleShape(buttonHalfWidth, 35);
        const float x = 750.0f + i * 190.0f + left +
            right * (static_cast<float>(i) + 0.5f) / 4.0f;
        move(firstRow[i], x);
        move(secondRow[i], x);
    }
    const array<const char*, 6> sliderNames{ "Slider_PitchShift",
        "Slider_FormantShift", "Slider_Timbre", "Slider_Prosody",
        "Slider_Jitter", "Slider_Volume" };
    for (size_t i = 0; i < sliderNames.size(); ++i) {
        const float x = 715.0f + i * 128.0f + left +
            right * static_cast<float>(i) / 5.0f;
        move(sliderNames[i], x);
        sliderCaptions[i].centerX = x;
    }
    auto& infoPanel = widgets("WidgetInfo");
    infoPanel.setCenterX(static_cast<uint16_t>(lround(1035.0f + left + right * 0.5f)));
    infoPanel.setRectangleShape(static_cast<uint16_t>(lround((750.0f + right) * 0.5f)), 55);
}

// Descriptions are intentionally short enough to fit the existing information
// panel. Both the effect button and the corresponding slider name use the same
// description. Textures are created once, not 60 times per second.
struct EffectHelp {
    const char* buttonName;
    array<const char*, 2> lines;
    array<SDL_Texture*, 2> textures{};
    array<float, 2> widths{};
    array<float, 2> heights{};
};

static array<EffectHelp, 5> effectHelp{{
    {"EffectPitch", {"Pitch changes how high or low your voice sounds.",
                     "Zero semitones leaves its pitch unchanged."}},
    {"EffectFormant", {"Formant changes the apparent vowel and voice colour.",
                       "Move its ratio to change that character."}},
    {"EffectTimbre", {"Timbre changes how bright or dark your voice sounds.",
                      "Positive is brighter; negative is darker."}},
    {"EffectProsody", {"Prosody adds a rising or falling gesture at voice onsets.",
                       "It does not recognize words or their meaning."}},
    {"EffectJitter", {"Jitter adds small, irregular changes to voice pitch.",
                      "Zero is neutral; try 0.020 on a held vowel."}}
}};

static void PrepareEffectHelp(EffectHelp& help) {
    if (!gFont) return;
    constexpr float scale = 0.68f;
    const SDL_Color darkText{ 14, 22, 28, 255 };
    for (size_t i = 0; i < help.lines.size(); ++i) {
        if (help.textures[i]) continue;
        SDL_Surface* surface = TTF_RenderText_Blended(gFont, help.lines[i],
            strlen(help.lines[i]), darkText);
        if (!surface) continue;
        help.widths[i] = surface->w * scale * 0.5f;
        help.heights[i] = surface->h * scale * 0.5f;
        help.textures[i] = SDL_CreateTextureFromSurface(renderer, surface);
        SDL_DestroySurface(surface);
    }
}

void PreloadStaticUiText() {
    // The SDL thread owns both the font and the renderer. Preparing captions here prevents
    // the first frame and the first one-second tooltip from creating textures on demand.
    if (!renderer || !gFont || sliderCaptionsPrepared) return;
    const SDL_Color captionColor{ 14, 22, 28, 255 };
    bool complete = true;
    for (auto& caption : sliderCaptions) {
        if (caption.texture) continue;
        SDL_Surface* surface = TTF_RenderText_Blended(gFont, caption.name,
            strlen(caption.name), captionColor);
        if (!surface) { complete = false; continue; }
        caption.width = surface->w * 0.95f * 0.5f;
        caption.height = surface->h * 0.95f * 0.5f;
        caption.texture = SDL_CreateTextureFromSurface(renderer, surface);
        SDL_DestroySurface(surface);
        if (!caption.texture) complete = false;
    }
    sliderCaptionsPrepared = complete;
    for (auto& help : effectHelp) PrepareEffectHelp(help);
}

void PreloadInterfaceRenderTarget() {
    if (!renderer || !window || interfaceTargetUnavailable) return;
    int windowWidth = 0, windowHeight = 0;
    if (!SDL_GetWindowSize(window, &windowWidth, &windowHeight) ||
        windowWidth <= 0 || windowHeight <= 0) return;
    const float displayScale = CalculateUiViewport(windowWidth, windowHeight).scale;
    // Intermediate windows already look smooth at 3x. Near the authored size,
    // use exact 4x -> 2x -> 1x stages so steep FFT and rotated-square edges
    // cannot bypass the sampling filter during a non-integer final fit.
    const int preferredFactor = displayScale < 0.82f
        ? kUiCompactSupersampleFactor
        : (displayScale <= 1.20f ? 4 : kUiSupersampleFactor);
    const int wantedFactor = min(preferredFactor, interfaceMaximumFactor);
    const int desiredWidth = static_cast<int>(ceil(CalculateUiLayoutWidth(
        windowWidth, windowHeight)));
    if (interfaceTarget && interfaceTargetFactor == wantedFactor &&
        interfaceTargetCapacityWidth >= desiredWidth) return;
    // Reallocate only when growing past the reserved width, rather than at
    // every mouse-move event during a continuous resize.
    const int capacityWidth = max(desiredWidth, interfaceTargetFactor == wantedFactor
        ? interfaceTargetCapacityWidth + 128 : kUiLayoutWidth);
    DestroyInterfaceTargets();
    for (int candidate = wantedFactor; candidate >= kUiSupersampleFactor; --candidate) {
        interfaceTarget = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA8888,
            SDL_TEXTUREACCESS_TARGET, capacityWidth * candidate,
            kUiLayoutHeight * candidate);
        if (interfaceTarget) {
            // At 4x this is an exact 2x resolve; at 3x it is 1.5x.
            if (candidate > kUiSupersampleFactor) {
            interfaceHalfTarget = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA8888,
                SDL_TEXTUREACCESS_TARGET, capacityWidth * candidate / 2,
                kUiLayoutHeight * candidate / 2);
            }
            if (candidate == kUiSupersampleFactor || interfaceHalfTarget) {
                interfaceResolvedTarget = SDL_CreateTexture(renderer,
                    SDL_PIXELFORMAT_RGBA8888, SDL_TEXTUREACCESS_TARGET,
                    capacityWidth, kUiLayoutHeight);
                if (interfaceResolvedTarget) {
                    interfaceTargetFactor = candidate;
                    interfaceTargetCapacityWidth = capacityWidth;
                    SDL_SetTextureScaleMode(interfaceTarget, SDL_SCALEMODE_LINEAR);
                    if (interfaceHalfTarget)
                        SDL_SetTextureScaleMode(interfaceHalfTarget, SDL_SCALEMODE_LINEAR);
                    SDL_SetTextureScaleMode(interfaceResolvedTarget, SDL_SCALEMODE_LINEAR);
                    break;
                }
            }
        }

        DestroyInterfaceTargets();
        interfaceMaximumFactor = candidate - 1;
    }
    if (!interfaceTarget) {
        interfaceTargetUnavailable = true;

    }
}

UiWidget* HitTestUiAt(float logicalX, float logicalY) {
    mx = logicalX;
    my = logicalY;
    UiWidget* hit = nullptr;
    int windowWidth = 0, windowHeight = 0;
    if (!SDL_GetWindowSize(window, &windowWidth, &windowHeight)) return nullptr;
    if (mx < 0.0f || mx >= CalculateUiLayoutWidth(windowWidth, windowHeight) ||
        my < 0.0f || my >= kUiLayoutHeight)
        return nullptr; // Letterbox margins never activate controls.
    for (auto& widget : widgets) {
        if (!widget.updatePos()) continue;
        if (IsAudioDeviceReopening() &&
            widget.getType() == WidgetType::Button_StateMono &&
            widget.getName() != "AudioDevices" && widget.getName() != "WasapiMode")
            continue;
        if (g_streamModeActive.load(memory_order_acquire) &&
            widget.getType() == WidgetType::Button_StateMono) {
            const string name = widget.getName();
            if (name != "AudioMode" && name != "AudioDevices" &&
                name != "WasapiMode" && name != "EffectPitch" &&
                name != "EffectFormant" && name != "EffectTimbre" &&
                name != "EffectProsody" && name != "EffectJitter" &&
                name != "Audio mode")
                continue;
        }
        hit = &widget; // Match the topmost (last drawn) eligible widget.
    }
    return hit;
}

void CleanupStaticSliderCaptions() {
    DestroyInterfaceTargets();
    for (auto& caption : sliderCaptions) {
        if (caption.texture != nullptr) SDL_DestroyTexture(caption.texture);
        caption.texture = nullptr;
    }
    for (auto& help : effectHelp) {
        for (auto& tooltipTexture : help.textures) {
            if (tooltipTexture) SDL_DestroyTexture(tooltipTexture);
            tooltipTexture = nullptr;
        }
    }
    sliderCaptionsPrepared = false;
}

// Move and draw the interactive controls, waveform and spectrum, then present the frame.
void RenderFrame(double now, RenderFrameTiming* timing)
{
    const Uint64 renderStartedNs = SDL_GetTicksNS();
    RefreshResponsiveUiGeometry();
    // Animation phase uses elapsed seconds, not frame count. Faster movement does not
    // request more frames than the visual rate currently scheduled by SDL_AppIterate.
    const double squarePhase = now * 9.0;

    // Mouse input uses the same centered aspect fit as the final compositing.
    float windowMouseX = 0.0f, windowMouseY = 0.0f;
    SDL_GetMouseState(&windowMouseX, &windowMouseY);
    int windowWidth = 0, windowHeight = 0;
    if (!SDL_GetWindowSize(window, &windowWidth, &windowHeight) ||
        !WindowPointToUi(windowWidth, windowHeight, windowMouseX,
            windowMouseY, mx, my)) {
        mx = my = -1.0f;
    }

    // Restore the exact original green (76, 235, 89 in the prior 8-bit
    // screenshot) across the interface and any newly exposed side margins.
    SDL_SetRenderDrawColorFloat(renderer, kUiBackgroundRed,
        kUiBackgroundGreen, kUiBackgroundBlue, 1.0f);
    SDL_RenderClear(renderer);

    // Grow targets when the responsive canvas exceeds their reserved width or
    // when the required edge sampling level changes.
    if (!interfaceTargetUnavailable)
        PreloadInterfaceRenderTarget();
    bool interfaceSupersampled = false;
    if (interfaceTarget != nullptr) {
        interfaceSupersampled = SDL_SetRenderTarget(renderer, interfaceTarget);
        if (!interfaceSupersampled) {
            SDL_SetRenderTarget(renderer, nullptr);
            DestroyInterfaceTargets();
            interfaceTargetUnavailable = true;
        }
    }
    if (interfaceSupersampled && !SDL_SetRenderScale(renderer,
        static_cast<float>(interfaceTargetFactor),
        static_cast<float>(interfaceTargetFactor))) {
        SDL_SetRenderTarget(renderer, nullptr);
        DestroyInterfaceTargets();
        interfaceTargetUnavailable = true;
        interfaceSupersampled = false;

    }
    int outputWidth = 0, outputHeight = 0;
    if (!SDL_GetRenderOutputSize(renderer, &outputWidth, &outputHeight)) {
        outputWidth = windowWidth;
        outputHeight = windowHeight;
    }
    const UiViewport outputViewport = CalculateUiViewport(outputWidth, outputHeight);
    if (!interfaceSupersampled) {
        // If the GPU cannot allocate the texture, keep the application usable
        // with the same hit boxes and aspect ratio in a direct draw fallback.
        const SDL_Rect clip{ static_cast<int>(outputViewport.x),
            static_cast<int>(outputViewport.y),
            static_cast<int>(outputViewport.width),
            static_cast<int>(outputViewport.height) };
        SDL_SetRenderViewport(renderer, &clip);
        SDL_SetRenderScale(renderer, outputViewport.scale, outputViewport.scale);
    }
    // SDL_RenderPresent at the end publishes the complete image.
    SDL_SetRenderDrawColorFloat(renderer, kUiBackgroundRed,
        kUiBackgroundGreen, kUiBackgroundBlue, 1.0f);
    SDL_RenderClear(renderer);

    // Legacy toggle arithmetic controls the decorative animation. Repeated weights are inherited,
    // not an audio data representation.
    sevenBitsValue = int((toggle1 == true)) + 4 * int((toggle2 == true)) + 32 * int((toggle3 == true)) + 32 * int((toggle4 == true)) + 16 * int((toggle5 == true)) + 32 * int((toggle6 == true)) + 64 * int((toggle7 == true));

    tenBitsValue = int((toggle1 == true)) + 4 * int((toggle2 == true)) + 8 * int((toggle3 == true)) + 8 * int((toggle4 == true)) + 16 * int((toggle5 == true)) + 32 * int((toggle6 == true)) + 64 * int((toggle7 == true)) + 128 * int((toggle8 == true)) + 256 * int((toggle9 == true)) + 512 * int((toggle10 == true));

    toggle1 = not toggle1;

    toggle2 = not ((toggle2 == false) xor (toggle1 == false));

    toggle3 = not ((toggle3 == false) xor (((toggle2 == false) and (toggle1 == false))));

    toggle4 = not ((toggle4 == false) xor (((toggle3 == false) and (toggle2 == false) and (toggle1 == false))));

    toggle5 = not ((toggle5 == false) xor (((toggle4 == false) and (toggle3 == false) and (toggle2 == false) and (toggle1 == false))));

    toggle6 = not ((toggle6 == false) xor (((toggle5 == false) and (toggle4 == false) and (toggle3 == false) and (toggle2 == false) and (toggle1 == false))));

    toggle7 = not ((toggle7 == false) xor (((toggle6 == false) and (toggle5 == false) and (toggle4 == false) and (toggle3 == false) and (toggle2 == false) and (toggle1 == false))));

    toggle8 = not ((toggle8 == false) xor (((toggle7 == false) and (toggle6 == false) and (toggle5 == false) and (toggle4 == false) and (toggle3 == false) and (toggle2 == false) and (toggle1 == false))));

    toggle9 = not ((toggle9 == false) xor (((toggle8 == false) and (toggle7 == false) and (toggle6 == false) and (toggle5 == false) and (toggle4 == false) and (toggle3 == false) and (toggle2 == false) and (toggle1 == false))));

    toggle10 = not ((toggle10 == false) xor (((toggle9 == false) and (toggle8 == false) and (toggle7 == false) and (toggle6 == false) and (toggle5 == false) and (toggle4 == false) and (toggle3 == false) and (toggle2 == false) and (toggle1 == false))));

    // Phase-shifted sine values generate continuously changing RGB backgrounds from the frame time
    // in seconds.
    red = (float)(0.5 + 0.5 * SDL_sin(now));
    green = (float)(0.5 + 0.5 * SDL_sin(now + SDL_PI_D * 2 / 3));
    blue = (float)(0.5 + 0.5 * SDL_sin(now + SDL_PI_D * 4 / 3));
    SDL_SetRenderDrawColorFloat(renderer, red, green, blue, SDL_ALPHA_OPAQUE_FLOAT);
    const float leftExpansion = CalculateLeftExpansion(outputViewport.layoutWidth);
    const float rightExpansion = outputViewport.layoutWidth - kUiLayoutWidth -
        leftExpansion;
    const SDL_FRect leftBackground{ 0.0f, 0.0f,
        320.0f + leftExpansion * 0.5f, 480.0f };
    SDL_RenderFillRect(renderer, &leftBackground);

    red = (float)(0.5 + 0.5 * SDL_sin(now + SDL_PI_D));
    green = (float)(0.5 + 0.5 * SDL_sin(now + SDL_PI_D + SDL_PI_D * 2 / 3));
    blue = (float)(0.5 + 0.5 * SDL_sin(now + SDL_PI_D + SDL_PI_D * 4 / 3));
    SDL_SetRenderDrawColorFloat(renderer, red, green, blue, SDL_ALPHA_OPAQUE_FLOAT);

    const SDL_FRect rightBackground{ leftBackground.w, 0.0f,
        320.0f + leftExpansion * 0.5f, 480.0f };
    SDL_RenderFillRect(renderer, &rightBackground);

    // Center the horizontal oscillation in the responsive visualizer width.
    // Both orbiting rectangles follow this same moving center, so the original
    // orbit, speed and glitch phase remain unchanged.
    const float visualizerCenterX = (rightBackground.x + rightBackground.w) * 0.5f;
    widgets("AnimatedWidget").setCenterX(
        visualizerCenterX + 90.0f * sin((SDL_PI_D * tenBitsValue) / 512));

    // A continuous shade avoids the rare white flash caused by converting
    // red == 1.0 to an integer before reducing its brightness.
    const float orbitShade = clamp(red * 0.5f, 0.0f, 0.5f);
    SDL_SetRenderDrawColorFloat(renderer, orbitShade, orbitShade, orbitShade,
        SDL_ALPHA_OPAQUE_FLOAT);

    // Restore the moving center and frame-dependent phase perturbation that produce the
    // original glitch animation. Wall-clock phase adds speed without changing UI FPS.
    const double glitchPhase = squarePhase + SDL_PI_D / 128 * sevenBitsValue;
    const int orbitX = static_cast<int>(widgets("AnimatedWidget").getCenterX());
    rect3.x = orbitX - int(rect3.w / 2) + int(160 * SDL_sin(glitchPhase));
    rect3.y = 240 - int(rect3.h / 2) + int(160 * SDL_sin(glitchPhase + SDL_PI_D / 2));

    SDL_RenderFillRect(renderer, &rect3);

    rect4.x = orbitX - int(rect4.w / 2) + int(160 * SDL_sin(glitchPhase + SDL_PI_D));
    rect4.y = 240 - int(rect4.h / 2) + int(160 * SDL_sin(glitchPhase + SDL_PI_D * 1.5));

    SDL_RenderFillRect(renderer, &rect4);

    // Rotate the center several full turns each second while preserving its
    // original bit-driven angle jumps and horizontal motion. Frame pacing stays
    // at 60 FPS; only the angular speed changes.
    widgets("AnimatedWidget").setFigureAngle(fmod(
        now * 24.0 - SDL_PI_D / 128 * sevenBitsValue, 2 * SDL_PI_D));

	bool test = false;

    // Rebuild hover state from current hit testing each frame. SDL_AppEvent uses the most recently
    // computed pointer.
    hoveredWidget = nullptr;

    for (auto& widget : widgets) {
        // Only the currently pressed widget follows the pointer. Its original click offset avoids
        // a jump at drag start.
        if (pressedWidget != nullptr && &widget == pressedWidget) {
            if (pressedWidget->get_isMouseLeftButtonDown()) {
                // Clamp horizontal drag to the track, then snap PosN variants to their configured
                // steps.
                if (mx - mouseClick_OFFSET.x < pressedWidget->getSlider_xMin()) {
                    pressedWidget->setHandleCenterX(pressedWidget->getSlider_xMin());
                }
                else if (mx - mouseClick_OFFSET.x > pressedWidget->getSlider_xMax()) {
                    pressedWidget->setHandleCenterX(pressedWidget->getSlider_xMax());
                }
                else {
                    if (pressedWidget->getType() != WidgetType::SliderH_PosN && pressedWidget->getType() != WidgetType::SliderH_FreeLimited && pressedWidget->getType() != WidgetType::Slider2D_FreeLimited && pressedWidget->getType() != WidgetType::Slider2D_PosN) {
                        pressedWidget->setHandleCenterX(pressedWidget->getSlider_xMin());
                    }
                    else {
                        if (pressedWidget->getType() != WidgetType::SliderH_FreeLimited && pressedWidget->getType() != WidgetType::Slider2D_FreeLimited) {
                            pressedWidget->setHandleCenterX(int(pressedWidget->getSlider_xMin() + int(((mx - mouseClick_OFFSET.x - pressedWidget->getSlider_xMin()) / pressedWidget->getRangeSizeX()) + 0.5f) * pressedWidget->getRangeSizeX()));
                        }
                        else {
                            pressedWidget->setHandleCenterX(mx - mouseClick_OFFSET.x);
                        }
                    }
                }

                // Apply the corresponding vertical clamp; screen Y increases downward.
                if (my - mouseClick_OFFSET.y < pressedWidget->getSlider_yMin()) {
                    pressedWidget->setHandleCenterY(pressedWidget->getSlider_yMin());
                }
                else if (my - mouseClick_OFFSET.y > pressedWidget->getSlider_yMax()) {
                    pressedWidget->setHandleCenterY(pressedWidget->getSlider_yMax());
                }
                else {
                    if (pressedWidget->getType() != WidgetType::SliderV_PosN && pressedWidget->getType() != WidgetType::SliderV_FreeLimited && pressedWidget->getType() != WidgetType::Slider2D_FreeLimited && pressedWidget->getType() != WidgetType::Slider2D_PosN) {
                        pressedWidget->setHandleCenterY(pressedWidget->getSlider_yMin());
                    }
                    else {
                        if (pressedWidget->getType() != WidgetType::SliderV_FreeLimited && pressedWidget->getType() != WidgetType::Slider2D_FreeLimited) {
                            pressedWidget->setHandleCenterY(pressedWidget->getSlider_yMin() + int(((my - mouseClick_OFFSET.y - pressedWidget->getSlider_yMin()) / pressedWidget->getRangeSizeY()) + 0.5f) * pressedWidget->getRangeSizeY());
                        }
                        else {
                            pressedWidget->setHandleCenterY(my - mouseClick_OFFSET.y);
                        }
                    }
                }

                // Translate handle pixels into displayed legend values after movement; AppUpdate
                // publishes those values.
                pressedWidget->setSlider_xVal(pressedWidget->getHandleCenterX());
                pressedWidget->setSlider_yVal(pressedWidget->getHandleCenterY());

                if (pressedWidget->getType() == WidgetType::SliderH_PosN || pressedWidget->getType() == WidgetType::SliderH_FreeLimited || pressedWidget->getType() == WidgetType::Slider2D_FreeLimited || pressedWidget->getType() == WidgetType::Slider2D_PosN) {
                    pressedWidget->setLegend_xText(FormatValueWithUnit(pressedWidget->getLegend_xVal(), pressedWidget->getLegendX_Unit()));
                }
                else{
                    pressedWidget->setLegend_xText("");
                }

                if (pressedWidget->getType() == WidgetType::SliderV_PosN || pressedWidget->getType() == WidgetType::SliderV_FreeLimited || pressedWidget->getType() == WidgetType::Slider2D_FreeLimited || pressedWidget->getType() == WidgetType::Slider2D_PosN) {
                    pressedWidget->setLegend_yText(FormatValueWithUnit(pressedWidget->getLegend_yVal(), pressedWidget->getLegendY_Unit()));
                }
                else {
					pressedWidget->setLegend_yText("");
                }

                // Show the active control values, separating two axes only for a two-dimensional
                // slider.
                widgets("WidgetInfo").setText(pressedWidget->getLegend_xText() + (pressedWidget->getType() == WidgetType::Slider2D_FreeLimited || pressedWidget->getType() == WidgetType::Slider2D_PosN ? " ; " : "") + pressedWidget->getLegend_yText());
            }
            else {
                if (hoveredWidget == nullptr) {
                    widgets("WidgetInfo").setText("");
                }
                else {
                    widgets("WidgetInfo").setText(hoveredWidget->getLegend_xText() + (hoveredWidget->getType() == WidgetType::Slider2D_FreeLimited || hoveredWidget->getType() == WidgetType::Slider2D_PosN ? " ; " : "") + hoveredWidget->getLegend_yText());
                }
            }
        }

        // updatePos recomputes geometry and returns whether the pointer hits this widget.
        // Hover state must remain true on every frame spent over the same button.
        if (widget.updatePos()) {
            bool shouldIgnoreButton = false;

            try {
                // When streaming, the UI suppresses ordinary button hover/action paths. Names and
                // the live state decide eligibility; button captions are presentation only.
                if (IsAudioDeviceReopening() &&
                    widget.getType() == WidgetType::Button_StateMono &&
                    widget.getName() != "AudioDevices" &&
                    widget.getName() != "WasapiMode") {
                    shouldIgnoreButton = true;
                }
                else if (g_streamModeActive.load(memory_order_acquire) &&
                    widget.getType() == WidgetType::Button_StateMono) {
                    string widgetName = widget.getName();

                    if (widgetName != "AudioMode" &&
                        widgetName != "AudioDevices" &&
                        widgetName != "WasapiMode" &&
                        widgetName != "EffectPitch" &&
                        widgetName != "EffectFormant" &&
                        widgetName != "EffectTimbre" &&
                        widgetName != "EffectProsody" &&
                        widgetName != "EffectJitter" &&
                        widgetName != "Audio mode") {
                        shouldIgnoreButton = true;
                    }
                }
            }
            catch (...) {
            }

            if (!shouldIgnoreButton) {
                hoveredWidget = &widget;
                test = true;

                if (hoveredWidget->getType() == WidgetType::SliderH_PosN || hoveredWidget->getType() == WidgetType::SliderH_FreeLimited) {
                    widgets("WidgetInfo").setText(hoveredWidget->getLegend_xText());
                }
                else if (hoveredWidget->getType() == WidgetType::SliderV_PosN || hoveredWidget->getType() == WidgetType::SliderV_FreeLimited) {
                    widgets("WidgetInfo").setText(hoveredWidget->getLegend_yText());
                }
                else if (hoveredWidget->getType() == WidgetType::Slider2D_FreeLimited || hoveredWidget->getType() == WidgetType::Slider2D_PosN) {
                    widgets("WidgetInfo").setText(hoveredWidget->getLegend_xText() + " ; " + hoveredWidget->getLegend_yText());
                }
                else {
                    hoveredWidget->setLegend_yText("");
                }
            }
            else {
                if (widget.getFigureZoom() != 1.0f) {
                    widget.setFigureZoom(1.0f);
                }
            }

        }
        widget.renderFig();
    }

    // Clear the hover pointer and info text when no eligible control matched the mouse in this
    // frame.
    if (not test) {
		hoveredWidget = nullptr;
        widgets("WidgetInfo").setText("");
    }

    // Short scale legends make room for six sliders. Keep their names separately above
    // the tracks so the controls remain identifiable even when an effect is disabled.
    if (gFont) {
        if (!sliderCaptionsPrepared) PreloadStaticUiText();
        for (const auto& caption : sliderCaptions) {
            if (caption.texture == nullptr) continue;
            const SDL_FRect destination{ caption.centerX - caption.width * 0.5f,
                440.0f, caption.width, caption.height };
            SDL_RenderTexture(renderer, caption.texture, nullptr, &destination);
        }
    }

    // Only effect buttons and their printed slider names provide help. A new
    // target restarts the one-second timer; merely redrawing at 60 FPS does not.
    int helpTarget = -1;
    if (hoveredWidget != nullptr) {
        const string hoveredName = hoveredWidget->getName();
        for (size_t i = 0; i < effectHelp.size(); ++i) {
            if (hoveredName == effectHelp[i].buttonName) {
                helpTarget = static_cast<int>(i);
                break;
            }
        }
    }
    if (helpTarget < 0) {
        for (size_t i = 0; i < effectHelp.size(); ++i) {
            const auto& caption = sliderCaptions[i];
            if (caption.texture &&
                mx >= caption.centerX - caption.width * 0.5f &&
                mx <= caption.centerX + caption.width * 0.5f &&
                my >= 440.0f && my <= 440.0f + caption.height) {
                helpTarget = static_cast<int>(i);
                break;
            }
        }
    }

    static int previousHelpTarget = -1;
    static Uint64 hoverStartedAt = 0;
    const Uint64 currentTicks = SDL_GetTicks();
    if (helpTarget != previousHelpTarget) {
        previousHelpTarget = helpTarget;
        hoverStartedAt = currentTicks;
    }
    if (helpTarget >= 0 && currentTicks - hoverStartedAt >= 1000 && gFont) {
        auto& help = effectHelp[static_cast<size_t>(helpTarget)];
        PrepareEffectHelp(help);

        // Paint over the transient numeric value inside the lavender panel.
        // This stays in the visual thread and never affects the audio callback.
        const SDL_FRect infoArea{ 660.0f + leftExpansion, 130.0f,
            750.0f + rightExpansion, 110.0f };
        SDL_SetRenderDrawColorFloat(renderer, 0.55f, 0.55f, 1.0f, 1.0f);
        SDL_RenderFillRect(renderer, &infoArea);
        for (size_t i = 0; i < help.textures.size(); ++i) {
            if (!help.textures[i]) continue;
            const SDL_FRect destination{ infoArea.x + 15.0f,
                infoArea.y + 20.0f + static_cast<float>(i) * 35.0f,
                help.widths[i], help.heights[i] };
            SDL_RenderTexture(renderer, help.textures[i], nullptr, &destination);
        }
    }

    // Draw the FFT and waveform over the same vertical band as the slider tracks. The spectrum
    // starts at their top edge, and the waveform ends at their bottom edge.
    const Uint64 beforeSpectrumNs = SDL_GetTicksNS();
    const float graphWidth = CalculateGraphWidth(outputViewport.layoutWidth);
    RenderFFTSpectrum_PortAudio(20.0f, 510.0f, graphWidth, 210.0f);
    const Uint64 beforeWaveformNs = SDL_GetTicksNS();
    RenderWaveform(20.0f, 760.0f, graphWidth, 210.0f);
    const Uint64 beforeCompositeNs = SDL_GetTicksNS();

    if (interfaceSupersampled) {
        SDL_Texture* displayTexture = interfaceTarget;
        float displayTextureScale = static_cast<float>(interfaceTargetFactor);
        if (interfaceHalfTarget != nullptr &&
            SDL_SetRenderTarget(renderer, interfaceHalfTarget)) {
            const SDL_FRect sourceArea{ 0.0f, 0.0f,
                outputViewport.layoutWidth * interfaceTargetFactor,
                kUiLayoutHeight * static_cast<float>(interfaceTargetFactor) };
            const SDL_FRect halfArea{ 0.0f, 0.0f,
                outputViewport.layoutWidth * interfaceTargetFactor * 0.5f,
                kUiLayoutHeight * interfaceTargetFactor * 0.5f };
            if (SDL_RenderTexture(renderer, interfaceTarget, &sourceArea, &halfArea)) {
                displayTexture = interfaceHalfTarget;
                displayTextureScale = interfaceTargetFactor * 0.5f;
            }
        }
        // On a roughly full-size window, first resolve 2x to 1x; otherwise
        // the direct fit would still preserve visible diagonal raster steps.
        // The compact route uses the 1.5x texture as its resolved input.
        if (interfaceResolvedTarget != nullptr && outputViewport.scale < 1.0f &&
            SDL_SetRenderTarget(renderer, interfaceResolvedTarget)) {
            const SDL_FRect resolvedArea{ 0.0f, 0.0f,
                outputViewport.layoutWidth,
                static_cast<float>(kUiLayoutHeight) };
            const SDL_FRect sourceArea{ 0.0f, 0.0f,
                outputViewport.layoutWidth * displayTextureScale,
                kUiLayoutHeight * displayTextureScale };
            if (SDL_RenderTexture(renderer, displayTexture, &sourceArea,
                &resolvedArea)) {
                displayTexture = interfaceResolvedTarget;
                displayTextureScale = 1.0f;
            }
        }
        SDL_SetRenderTarget(renderer, nullptr);
        const SDL_FRect destination{ outputViewport.x, outputViewport.y,
            outputViewport.width, outputViewport.height };
        const SDL_FRect sourceArea{ 0.0f, 0.0f,
            outputViewport.layoutWidth * displayTextureScale,
            kUiLayoutHeight * displayTextureScale };
        SDL_RenderTexture(renderer, displayTexture, &sourceArea, &destination);
    } else {
        SDL_SetRenderScale(renderer, 1.0f, 1.0f);
        SDL_SetRenderViewport(renderer, nullptr);
    }
    // At small sizes the FFT owns its final pixels, independently of the
    // interface's intermediate resize and its controls/textures.
    CompositeSmallSpectrumToWindow();
    const Uint64 beforePresent = SDL_GetTicksNS();
    SDL_RenderPresent(renderer);
    if (timing != nullptr) {
        timing->controlsNs = beforeSpectrumNs - renderStartedNs;
        timing->spectrumNs = beforeWaveformNs - beforeSpectrumNs;
        timing->waveformNs = beforeCompositeNs - beforeWaveformNs;
        timing->compositeNs = beforePresent - beforeCompositeNs;
        timing->presentNs = SDL_GetTicksNS() - beforePresent;
    }

}
