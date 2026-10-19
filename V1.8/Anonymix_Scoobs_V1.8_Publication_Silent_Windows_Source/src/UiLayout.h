// Dimensions of the authored interface, in SDL logical render coordinates.
// The whole interface scales by one factor when the window aspect ratio changes;
// widgets and mouse hit boxes therefore keep exactly the same geometry.
#pragma once

inline constexpr int kUiLayoutWidth = 1430;
// The bottom slider captions end around y=1010. Keep room for the complete
// text while removing the unused part of the old 1040-pixel canvas.
inline constexpr int kUiLayoutHeight = 1030;
inline constexpr int kUiMinimumWindowWidth = 960;
inline constexpr int kUiMinimumWindowHeight = 650;
// The left visual area and the control group can spread into wide windows.
// Bound growth to keep render targets within a reasonable size on ultrawide
// monitors; remaining space, if any, stays green and noninteractive.
inline constexpr int kUiMaximumLayoutWidth = 2048;
inline constexpr int kUiSupersampleFactor = 2;
inline constexpr int kUiCompactSupersampleFactor = 3;
// This is the previous UI background color (0.30, 0.92, 0.35), as seen in
// the waveform trace and the authored window before aspect-safe resizing.
inline constexpr float kUiBackgroundRed = 0.30f;
inline constexpr float kUiBackgroundGreen = 0.92f;
inline constexpr float kUiBackgroundBlue = 0.35f;

struct UiViewport {
    float x, y, width, height, scale, layoutWidth;
};

// Expand layout space horizontally for wide windows. Individual controls and
// text are repositioned, never stretched. This keeps the full slider captions
// visible in height while filling much of an unusually wide client area.
inline float CalculateUiLayoutWidth(int width, int height) {
    if (width <= 0 || height <= 0) return static_cast<float>(kUiLayoutWidth);
    const float matchingWidth = static_cast<float>(width) * kUiLayoutHeight / height;
    const float minimum = static_cast<float>(kUiLayoutWidth);
    const float maximum = static_cast<float>(kUiMaximumLayoutWidth);
    return matchingWidth < minimum ? minimum :
        (matchingWidth > maximum ? maximum : matchingWidth);
}

inline float CalculateLeftExpansion(float layoutWidth) {
    return (layoutWidth - kUiLayoutWidth) * 0.45f;
}

inline float CalculateGraphWidth(float layoutWidth) {
    return 600.0f + CalculateLeftExpansion(layoutWidth);
}

// Use the same aspect-preserving fit for output pixels and window-space mouse
// coordinates. Both transforms agree on the expanded width at any DPI.
inline UiViewport CalculateUiViewport(int width, int height) {
    const float layoutWidth = CalculateUiLayoutWidth(width, height);
    const float scaleX = static_cast<float>(width) / layoutWidth;
    const float scaleY = static_cast<float>(height) / kUiLayoutHeight;
    const float scale = scaleX < scaleY ? scaleX : scaleY;
    const float fittedWidth = layoutWidth * scale;
    const float fittedHeight = kUiLayoutHeight * scale;
    return { (width - fittedWidth) * 0.5f,
        (height - fittedHeight) * 0.5f, fittedWidth, fittedHeight, scale,
        layoutWidth };
}

inline bool WindowPointToUi(int width, int height, float windowX,
    float windowY, float& uiX, float& uiY) {
    if (width <= 0 || height <= 0) return false;
    const UiViewport viewport = CalculateUiViewport(width, height);
    if (windowX < viewport.x || windowY < viewport.y ||
        windowX >= viewport.x + viewport.width ||
        windowY >= viewport.y + viewport.height) return false;
    uiX = (windowX - viewport.x) / viewport.scale;
    uiY = (windowY - viewport.y) / viewport.scale;
    return true;
}
// Leave room for the OS frame/title bar beyond SDL's usable desktop rectangle.
inline constexpr int kUiDesktopWidthInset = 32;
inline constexpr int kUiDesktopHeightInset = 64;
