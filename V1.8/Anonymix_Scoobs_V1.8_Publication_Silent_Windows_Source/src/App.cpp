// SDL3 callback entry points: initialization, input events, frame update and shutdown. Widget actions are dispatched from SDL_AppEvent.

#define SDL_MAIN_USE_CALLBACKS 1
#ifdef _WIN32
// Include the Windows COM declarations before Common.h exposes std names globally.
// Otherwise Windows SDK's byte and std::byte become ambiguous in the SDK headers.
// Windows' min/max macros would also break std::numeric_limits in Widgets.h.
#ifndef NOMINMAX
#define NOMINMAX 1
#endif
#include <objbase.h>
#if defined(_MSC_VER)
#pragma comment(lib, "Ole32.lib")
#endif
#endif
#include <SDL3/SDL_main.h>
#include "Widgets.h"
#include "Stream.h"
#include "Dsp.h"
#include "Wav.h"
#include "AppFrame.h"
#include "AudioPressure.h"
#include "RenderBudget.h"
#include <thread>
#include <system_error>
#include <condition_variable>
#include <functional>
#include <initializer_list>

// SDL owns the UI and every widget. The device worker owns PortAudio state. SDL disables
// audio buttons during reconnection and observes the result on a later frame.
static thread g_portAudioDeviceWorker;
static mutex g_audioWorkerMutex;
static condition_variable g_audioWorkerWake;
static bool g_audioWorkerInitialized = false;
static bool g_audioWorkerReady = false;
static bool g_audioStartupPublished = false; // SDL thread only.
static bool g_audioWorkerHasRequest = false;
static bool g_audioWorkerRestoreLiveMode = false;
static bool g_audioWorkerRefreshDevices = true;
static bool g_audioWorkerShouldStop = false;
static function<bool()> g_audioWorkerAction;
static bool g_audioWorkerActionDone = true;
static bool g_audioWorkerActionResult = false;
static atomic<int> g_audioReopenState{0}; // 0 idle, 1 worker, 2 success, 3 failure, 4 settling.
// SDL-thread-only intent: wait for a quiet interval after the worker completes.
// A burst of presses then produces at most one follow-up open, with the last mode.
static bool g_deferredDeviceRefresh = false;
static int g_deferredWasapiMode = -1;
static Uint64 g_reopenSettleDeadlineNs = 0;
static constexpr Uint64 kReopenQuietIntervalNs = 500000000ULL;

// A later move to a smaller monitor can require a smaller client-area minimum.
// The normal minimum is 960x650; only a monitor with less usable space relaxes it.
static void UpdateWindowSizeLimits() {
    if (!window) return;
    const SDL_DisplayID display = SDL_GetDisplayForWindow(window);
    SDL_Rect usable{};
    if (display == 0 || !SDL_GetDisplayUsableBounds(display, &usable) ||
        usable.w <= 0 || usable.h <= 0) return;
    const int availableWidth = max(1, usable.w - kUiDesktopWidthInset);
    const int availableHeight = max(1, usable.h - kUiDesktopHeightInset);
    SDL_SetWindowMinimumSize(window,
        min(kUiMinimumWindowWidth, availableWidth),
        min(kUiMinimumWindowHeight, availableHeight));
    int currentWidth = 0, currentHeight = 0;
    if (SDL_GetWindowSize(window, &currentWidth, &currentHeight) &&
        (currentWidth > availableWidth || currentHeight > availableHeight))
        SDL_SetWindowSize(window, min(currentWidth, availableWidth),
            min(currentHeight, availableHeight));
}
// 0: idle; 1/2: starting/stopping; 3/4/5: started/failed/stopped awaiting UI delivery.
static atomic<int> g_recordedPlaybackTransition{0};
// 0 idle, 1/2 starting/stopping, 3/4/5 started/failed/stopped awaiting SDL.
static atomic<int> g_recordingTransition{0};
static bool g_switchToStreamAfterRecording = false; // SDL thread only.
static bool g_switchToStreamAfterPlayback = false; // SDL thread only.

static bool IsAudioStartupPending() {
    lock_guard<mutex> lock(g_audioWorkerMutex);
    return g_portAudioDeviceWorker.joinable() && !g_audioWorkerInitialized;
}

// Use the same unmistakable ON/OFF palette when selectors are created and toggled.
// Light neutral buttons stay legible over the animated green background.
static void SetEffectButtonAppearance(UiWidget& button, bool enabled) {
    if (enabled) {
        button.setWidgetColorRGB(0.05f, 0.38f, 0.16f);
        button.setTextColor(255, 255, 255);
    } else {
        button.setWidgetColorRGB(0.70f, 0.74f, 0.76f);
        button.setTextColor(20, 28, 34);
    }
}

// Materialize every finite button caption after widget setup and font creation. The cached
// texture key includes its text color, so ON and OFF captions use their respective colors.
// Numeric slider values and the changing information panel are intentionally excluded.
static void PreloadButtonStateText() {
    const auto preload = [](const char* widgetName,
        initializer_list<const char*> captions) {
        const SDL_Color color = widgets(widgetName).getTextColor();
        for (const char* caption : captions) PrecacheStableLabel(caption, color);
    };
    preload("AudioMode", { "Mode: Recording", "Mode: Stream", "Switching..." });
    preload("WasapiMode", { "Audio: Starting...", "WASAPI: Shared",
        "WASAPI: Excl.", "Other Host", "No Audio", "Switching..." });
    preload("AudioDevices", { "Reopen Audio", "Reopening..." });
    preload("PA_Record", { "Start Rec", "Stop Rec", "Starting...", "Stopping..." });
    preload("PA_PlaySelected", { "Play", "Stop", "Starting...", "Stopping..." });
    preload("PA_SaveSelected", { "Save WAV", "Saving..." });

    const SDL_Color enabledColor{ 255, 255, 255, 255 };
    const SDL_Color disabledColor{ 20, 28, 34, 255 };
    for (const char* effect : { "Pitch", "Formant", "Timbre", "Prosody", "Jitter" }) {
        PrecacheStableLabel(string(effect) + ": ON", enabledColor);
        PrecacheStableLabel(string(effect) + ": OFF", disabledColor);
    }
}

bool IsAudioDeviceReopening() {
    const int state = g_audioReopenState.load(memory_order_acquire);
    return state == 1 || state == 4;
}

// Closing and enumerating a Windows WASAPI duplex stream can take seconds. This routine runs
// on the worker, with no widget or SDL renderer calls.
static bool PerformAudioDeviceReopen(bool restoreLiveMode, bool portAudioWasReady,
    bool refreshDevices) {
    if (!refreshDevices && portAudioWasReady && PortAudioDeviceListNeedsRefresh()) {
        refreshDevices = true;
    }
    // A mode toggle keeps the existing PortAudio device enumeration. The explicit Reopen
    // command terminates and initializes the library to discover newly connected endpoints.
    if (portAudioWasReady) {
        if (refreshDevices) CleanupPortAudio();
        else ClosePortAudioStreams();
    }
    paIsPlayingSelected.store(false, memory_order_release);

    const bool recovered = InitPortAudio(portAudioWasReady && !refreshDevices);
    if (recovered && restoreLiveMode)
        g_streamModeActive.store(true, memory_order_release);
    return recovered;
}

// Keep initialization, stream operations and termination on one long-lived WASAPI thread.
static void PortAudioDeviceWorkerLoop() {
#ifdef _WIN32
    // COM apartments belong to threads. SDL's COM setup on the UI thread does not
    // initialize the PortAudio device worker that enumerates Windows audio endpoints.
    // A successful S_FALSE also requires a matching CoUninitialize on this thread.
    struct WorkerComApartment final {
        HRESULT result = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
        ~WorkerComApartment() {
            if (SUCCEEDED(result)) CoUninitialize();
        }
    } comApartment;
#endif
    bool ready = InitPortAudio();
    {
        lock_guard<mutex> lock(g_audioWorkerMutex);
        g_audioWorkerReady = ready;
        g_audioWorkerInitialized = true;
    }
    g_audioWorkerWake.notify_one();

    for (;;) {
        bool restoreLiveMode = false;
        bool refreshDevices = true;
        function<bool()> action;
        {
            unique_lock<mutex> lock(g_audioWorkerMutex);
            g_audioWorkerWake.wait(lock, [] {
                return g_audioWorkerShouldStop || g_audioWorkerHasRequest || g_audioWorkerAction;
            });
            if (g_audioWorkerShouldStop) break;
            if (g_audioWorkerAction) {
                action = move(g_audioWorkerAction);
                g_audioWorkerAction = nullptr;
            } else {
                restoreLiveMode = g_audioWorkerRestoreLiveMode;
                refreshDevices = g_audioWorkerRefreshDevices;
                g_audioWorkerHasRequest = false;
            }
        }
        if (action) {
            bool result = false;
            try { result = action(); }
            catch (...) { /* The caller observes failure through the existing state. */ }
            {
                lock_guard<mutex> lock(g_audioWorkerMutex);
                g_audioWorkerActionResult = result;
                g_audioWorkerActionDone = true;
            }
            g_audioWorkerWake.notify_one();
            continue;
        }
        ready = PerformAudioDeviceReopen(restoreLiveMode, ready, refreshDevices);
        {
            lock_guard<mutex> lock(g_audioWorkerMutex);
            g_audioWorkerReady = ready;
        }
        g_audioReopenState.store(ready ? 2 : 3, memory_order_release);
    }
    if (ready) CleanupPortAudio();
}

bool IsPortAudioDeviceWorkerThread() {
    return this_thread::get_id() == g_portAudioDeviceWorker.get_id();
}

bool RunPortAudioDeviceAction(bool (*action)()) {
    if (IsPortAudioDeviceWorkerThread()) return action();
    unique_lock<mutex> lock(g_audioWorkerMutex);
    if (!g_audioWorkerInitialized || !g_audioWorkerReady || g_audioWorkerShouldStop ||
        g_audioReopenState.load(memory_order_acquire) != 0) return false;
    g_audioWorkerWake.wait(lock, [] { return !g_audioWorkerAction && g_audioWorkerActionDone; });
    g_audioWorkerActionDone = false;
    g_audioWorkerAction = action;
    lock.unlock();
    g_audioWorkerWake.notify_one();
    lock.lock();
    g_audioWorkerWake.wait(lock, [] { return g_audioWorkerActionDone; });
    return g_audioWorkerActionResult;
}

void RunPortAudioDeviceAction(void (*action)()) {
    if (IsPortAudioDeviceWorkerThread()) { action(); return; }
    // A void action has no result; the bool dispatcher still waits for its completion.
    // Capture the function pointer in a function<bool()> through the same worker queue.
    unique_lock<mutex> lock(g_audioWorkerMutex);
    if (!g_audioWorkerInitialized || !g_audioWorkerReady || g_audioWorkerShouldStop ||
        g_audioReopenState.load(memory_order_acquire) != 0) return;
    g_audioWorkerWake.wait(lock, [] { return !g_audioWorkerAction && g_audioWorkerActionDone; });
    g_audioWorkerActionDone = false;
    g_audioWorkerAction = [action] { action(); return true; };
    lock.unlock();
    g_audioWorkerWake.notify_one();
    lock.lock();
    g_audioWorkerWake.wait(lock, [] { return g_audioWorkerActionDone; });
}

static bool RequestRecordedPlaybackAction(bool start) {
    {
        lock_guard<mutex> lock(g_audioWorkerMutex);
        if (!g_audioWorkerInitialized || !g_audioWorkerReady || g_audioWorkerShouldStop ||
            g_audioReopenState.load(memory_order_acquire) != 0 ||
            g_recordedPlaybackTransition.load(memory_order_acquire) != 0 ||
            g_recordingTransition.load(memory_order_acquire) != 0 ||
            g_audioWorkerAction || !g_audioWorkerActionDone) return false;
        g_recordedPlaybackTransition.store(start ? 1 : 2, memory_order_release);
        g_audioWorkerActionDone = false;
        // The worker owns every PortAudio open/close. Publish only the result here;
        // SDL changes button text after observing that result on its next frame.
        g_audioWorkerAction = [start] {
            bool succeeded = false;
            try {
                if (start) succeeded = StartPortAudioPlaybackSelected();
                else { StopPortAudioPlaybackSelected(); succeeded = true; }
            } catch (...) {
                // Publish failure to SDL through the transition state below.
            }
            g_recordedPlaybackTransition.store(start ? (succeeded ? 3 : 4) : 5,
                memory_order_release);
            return succeeded;
        };
    }
    g_audioWorkerWake.notify_one();
    return true;
}

bool RequestRecordedPlaybackStart() { return RequestRecordedPlaybackAction(true); }
bool RequestRecordedPlaybackStop() { return RequestRecordedPlaybackAction(false); }

bool IsRecordedPlaybackTransitionPending() {
    return g_recordedPlaybackTransition.load(memory_order_acquire) != 0;
}

RecordedPlaybackResult TakeRecordedPlaybackResult() {
    lock_guard<mutex> lock(g_audioWorkerMutex);
    if (!g_audioWorkerActionDone) return RecordedPlaybackResult::None;
    int state = g_recordedPlaybackTransition.load(memory_order_acquire);
    if (state < 3 || state > 5 ||
        !g_recordedPlaybackTransition.compare_exchange_strong(state, 0,
            memory_order_acq_rel)) return RecordedPlaybackResult::None;
    return state == 3 ? RecordedPlaybackResult::Started :
        state == 4 ? RecordedPlaybackResult::StartFailed :
        RecordedPlaybackResult::Stopped;
}

// Reserve the device worker for the complete recording transition. Stop may need to drain
// queued audio, so SDL updates the button only after the worker publishes the result.
static bool RequestRecordingAction(bool start) {
    {
        lock_guard<mutex> lock(g_audioWorkerMutex);
        if (!g_audioWorkerInitialized || !g_audioWorkerReady || g_audioWorkerShouldStop ||
            g_audioReopenState.load(memory_order_acquire) != 0 ||
            g_recordingTransition.load(memory_order_acquire) != 0 ||
            g_recordedPlaybackTransition.load(memory_order_acquire) != 0 ||
            g_audioWorkerAction || !g_audioWorkerActionDone) return false;
        g_recordingTransition.store(start ? 1 : 2, memory_order_release);
        g_audioWorkerActionDone = false;
        g_audioWorkerAction = [start] {
            bool succeeded = false;
            try {
                if (start) succeeded = StartPortAudioRecording();
                else { StopPortAudioRecording(); succeeded = true; }
            } catch (...) {
                // Publish failure to SDL through the transition state below.
            }
            g_recordingTransition.store(start ? (succeeded ? 3 : 4) : 5,
                memory_order_release);
            return succeeded;
        };
    }
    g_audioWorkerWake.notify_one();
    return true;
}

static void CompleteRecordingAction() {
    int result = 0;
    {
        lock_guard<mutex> lock(g_audioWorkerMutex);
        if (!g_audioWorkerActionDone) return;
        result = g_recordingTransition.load(memory_order_acquire);
        if (result < 3 || result > 5) return;
        g_recordingTransition.store(0, memory_order_release);
    }
    auto& button = widgets("PA_Record");
    if (result == 3) {
        button.setButtonFunction(WidgetAction::PA_AudioRecord_Stop);
        button.setText("Stop Rec");
        button.setWidgetColorRGB(1.0f, 0.3f, 0.3f);
    } else {
        button.setButtonFunction(WidgetAction::PA_AudioRecord_Start);
        button.setText("Start Rec");
        button.setWidgetColorRGB(1.0f, 0.55f, 0.55f);
        if (result == 5 && g_switchToStreamAfterRecording) {
            g_switchToStreamAfterRecording = false;
            ActivateStreamMode();
            widgets("AudioMode").setText("Mode: Stream");
            widgets("AudioMode").setWidgetColorRGB(0.3f, 1.0f, 0.3f);
        }
    }
}

// SDL calls this after UpdatePlaybackAndAnalysis has consumed the stop result. The actual
// device closure finishes on the worker before Stream may use the duplex output.
static void CompletePlaybackModeSwitch() {
    if (!g_switchToStreamAfterPlayback || IsRecordedPlaybackTransitionPending()) return;
    g_switchToStreamAfterPlayback = false;
    bool stopped = false;
    {
        lock_guard<mutex> lock(g_audioWorkerMutex);
        stopped = g_audioWorkerActionDone && g_audioWorkerActionResult;
    }
    auto& modeButton = widgets("AudioMode");
    if (!stopped) {
        modeButton.setText("Mode: Recording");
        modeButton.setWidgetColorRGB(0.8f, 0.8f, 0.8f);
        return;
    }
    ActivateStreamMode();
    modeButton.setText(g_streamModeActive.load(memory_order_acquire)
        ? "Mode: Stream" : "Mode: Recording");
    modeButton.setWidgetColorRGB(g_streamModeActive.load(memory_order_acquire)
        ? 0.3f : 0.8f, g_streamModeActive.load(memory_order_acquire)
        ? 1.0f : 0.8f, g_streamModeActive.load(memory_order_acquire)
        ? 0.3f : 0.8f);
}

// Return true when a request was accepted. Completion is asynchronous;
// CompleteAudioDeviceReopen publishes the actual success/failure on SDL's main thread.
bool ReopenAudioDevices(bool refreshDevices) {
    if (IsRecordedPlaybackTransitionPending() ||
        g_recordingTransition.load(memory_order_acquire) != 0) return false;
    if (paIsRecording.load(memory_order_acquire)) {
        return false;
    }
    if (g_audioReopenState.load(memory_order_acquire) != 0) return false;
    const bool restoreLiveMode = g_streamModeActive.exchange(false, memory_order_acq_rel);
    {
        lock_guard<mutex> lock(g_audioWorkerMutex);
        if (!g_audioWorkerInitialized || g_audioWorkerShouldStop) {
            g_streamModeActive.store(restoreLiveMode, memory_order_release);
            return false;
        }
        g_audioWorkerRestoreLiveMode = restoreLiveMode;
        g_audioWorkerRefreshDevices = refreshDevices;
        g_audioWorkerHasRequest = true;
        g_audioReopenState.store(1, memory_order_release);
    }
    g_audioWorkerWake.notify_one();
    return true;
}

// Observe worker completion before another SDL frame reads audio state or accepts new buttons.
static void CompleteAudioDeviceReopen() {
    const int state = g_audioReopenState.load(memory_order_acquire);
    if (state != 2 && state != 3) return;
    widgets("AudioDevices").setText("Reopen Audio");
    widgets("AudioDevices").setWidgetColorRGB(state == 2 ? 0.55f : 1.0f, 0.85f, 1.0f);
    if (g_streamModeActive.load(memory_order_acquire)) {
        widgets("AudioMode").setText("Mode: Stream");
        widgets("AudioMode").setWidgetColorRGB(0.3f, 1.0f, 0.3f);
    } else {
        widgets("AudioMode").setText("Mode: Recording");
        widgets("AudioMode").setWidgetColorRGB(0.8f, 0.8f, 0.8f);
    }
    const int openedWasapiMode = g_activeWasapiMode.load(memory_order_acquire);
    widgets("WasapiMode").setText(openedWasapiMode == 1 ? "WASAPI: Excl." :
        openedWasapiMode == 0 ? "WASAPI: Shared" :
        openedWasapiMode == -1 ? "Other Host" : "No Audio");
    widgets("WasapiMode").setWidgetColorRGB(openedWasapiMode == 1 ? 0.95f : 0.60f,
        openedWasapiMode == 1 ? 0.72f : 0.85f, 0.95f);
    // A pending click remains editable while the UI waits for a quiet interval.
    // The device worker must finish its transaction before a new mode is applied.
    if (g_deferredDeviceRefresh ||
        (g_deferredWasapiMode >= 0 && g_deferredWasapiMode != openedWasapiMode)) {
        const Uint64 nowNs = SDL_GetTicksNS();
        g_reopenSettleDeadlineNs = nowNs + kReopenQuietIntervalNs;
        g_audioReopenState.store(4, memory_order_release);
        widgets("AudioDevices").setText("Reopening...");
        if (g_deferredWasapiMode >= 0) widgets("WasapiMode").setText("Switching...");
    } else {
        g_deferredDeviceRefresh = false;
        g_deferredWasapiMode = -1;
        g_audioReopenState.store(0, memory_order_release);
    }
}

// Run on SDL's thread after completion. A click resets the deadline; do not keep
// restarting the driver while the user is still changing the intended mode.
static void StartSettledAudioReopen() {
    if (g_audioReopenState.load(memory_order_acquire) != 4 ||
        SDL_GetTicksNS() < g_reopenSettleDeadlineNs) return;
    const bool refresh = g_deferredDeviceRefresh;
    const int mode = g_deferredWasapiMode;
    g_deferredDeviceRefresh = false;
    g_deferredWasapiMode = -1;
    const int previousRequest = g_requestedWasapiMode.load(memory_order_acquire);
    const int openedMode = g_activeWasapiMode.load(memory_order_acquire);
    g_audioReopenState.store(0, memory_order_release);
    if (refresh || (mode >= 0 && mode != openedMode)) {
        if (mode >= 0) g_requestedWasapiMode.store(mode, memory_order_release);
        if (ReopenAudioDevices(refresh)) {
            widgets("AudioDevices").setText("Reopening...");
            if (mode >= 0) widgets("WasapiMode").setText("Switching...");

            return;
        }
        g_requestedWasapiMode.store(previousRequest, memory_order_release);

    }
    widgets("AudioDevices").setText("Reopen Audio");
    widgets("WasapiMode").setText(openedMode == 1 ? "WASAPI: Excl." :
        openedMode == 0 ? "WASAPI: Shared" :
        openedMode == -1 ? "Other Host" : "No Audio");
}

// Initial device enumeration runs while SDL builds and displays its first frames. Publish the
// actual device mode only after the worker completes, and never touch widgets on that worker.
static void CompleteAudioDeviceStartup() {
    bool ready = false;
    {
        lock_guard<mutex> lock(g_audioWorkerMutex);
        if (g_audioStartupPublished || !g_audioWorkerInitialized) return;
        g_audioStartupPublished = true;
        ready = g_audioWorkerReady;
    }
    const int mode = g_activeWasapiMode.load(memory_order_acquire);
    widgets("WasapiMode").setText(ready ? (mode == 1 ? "WASAPI: Excl." :
        mode == 0 ? "WASAPI: Shared" : "Other Host") : "No Audio");
    widgets("WasapiMode").setWidgetColorRGB(ready ? 0.60f : 1.0f, 0.85f, 0.95f);
}

// The publication build intentionally has no SDL diagnostic sink, including for
// startup failures. Return values still govern the normal error and UI paths.
static void SDLCALL DiscardReleaseLog(void*, int, SDL_LogPriority, const char*) {}

// Allocate visualization state, initialize SDL and PortAudio, create the window, and register all
// named controls.
SDL_AppResult SDL_AppInit(void** appstate, int argc, char* argv[])
{
    SDL_SetLogOutputFunction(DiscardReleaseLog, nullptr);

    // Set up inherited pixel-format state before drawing. Visualization buffers must be sized
    // before callbacks use them.
    pxFormatRGBA64 = SDL_GetPixelFormatDetails(SDL_PIXELFORMAT_RGBA64);
    color_grey = SDL_MapRGBA(pxFormatRGBA64, NULL, grey_r, grey_g, grey_b, grey_a);

    waveformBuffer.resize(WAVEFORM_SAMPLES, 0.0f);
    paFftInputBuffer.resize(PA_FFT_SIZE * 2, 0.0f);
    paFftWritePos = PA_FFT_SIZE;
    paFftSamplesAvailable = PA_FFT_SIZE;
    paFftMagnitudes.resize(PA_NUM_BARS, 0.0f);
    paFftSmoothed.resize(PA_NUM_BARS, 0.0f);

    if (!SDL_Init(SDL_INIT_VIDEO)) {
        return SDL_APP_FAILURE;
    }

    // Fit the initial client area inside the primary monitor's usable desktop.
    // The minimum is also clamped on exceptionally small displays, so the
    // operating system can still place the whole window on screen.
    int initialWidth = kUiLayoutWidth;
    int initialHeight = kUiLayoutHeight;
    SDL_Rect usableBounds{};
    const SDL_DisplayID primaryDisplay = SDL_GetPrimaryDisplay();
    if (primaryDisplay != 0 &&
        SDL_GetDisplayUsableBounds(primaryDisplay, &usableBounds) &&
        usableBounds.w > 0 && usableBounds.h > 0) {
        initialWidth = min(initialWidth,
            max(1, usableBounds.w - kUiDesktopWidthInset));
        initialHeight = min(initialHeight,
            max(1, usableBounds.h - kUiDesktopHeightInset));
    }

    // The renderer is shared by all widgets and visualization routines.
    if (!SDL_CreateWindowAndRenderer("Anonymix Scoobs", initialWidth,
        initialHeight, SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY,
        &window, &renderer)) {
        return SDL_APP_FAILURE;
    }
    SDL_SetWindowMinimumSize(window, min(kUiMinimumWindowWidth, initialWidth),
        min(kUiMinimumWindowHeight, initialHeight));
    UpdateWindowSizeLimits();
    // RenderFrame fits the UI texture into the physical output pixels. Keeping
    // SDL's logical presentation disabled lets us fill the entire window,
    // including the unused aspect-ratio margins, with the original green.
    // Geometry uses per-vertex alpha for the antialiased spectrum outline.
    SDL_SetRenderDrawBlendMode(renderer, SDL_BLENDMODE_BLEND);

    // The icon surface can be discarded after SDL copies it. Keep the missing asset optional:
    // a portable build can still start when logo.bmp is absent.
    SDL_Surface* logoSurface = SDL_LoadBMP("logo.bmp");
    if (logoSurface) {
        SDL_SetWindowIcon(window, logoSurface);
        SDL_DestroySurface(logoSurface);
    }

    gLogoTexture = nullptr;
    gLogoRect = { 0.f, 0.f, 0.f, 0.f };

    // Rasterize letters at twice their logical UI size. The whole interface is
    // drawn to a supersampled target, so diagonal letter strokes gain the
    // same pixel coverage as widget edges during resizing.
    if (!TTF_Init()) {
        gFont = nullptr;
    }
    else {
        gFont = TTF_OpenFont("C:/Windows/Fonts/Arial.ttf", 54);
        if (!gFont) gFont = TTF_OpenFont("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 54);
    }

    float initialMouseX = 0.0f, initialMouseY = 0.0f;
    SDL_GetMouseState(&initialMouseX, &initialMouseY);
    if (!WindowPointToUi(initialWidth, initialHeight, initialMouseX,
        initialMouseY, mx, my)) {
        mx = my = -1.0f;
    }

    hoveredWidget = nullptr;

// Widget names are lookup keys used elsewhere; changes must be applied to every lookup.
    widgets.add("AnimatedWidget");
    widgets("AnimatedWidget").setType(WidgetType::Static_DynaZoom);
    widgets("AnimatedWidget").setHalfDiag1(50);
    widgets("AnimatedWidget").setHalfDiag2(50);
    widgets("AnimatedWidget").setCenterX(320);
    widgets("AnimatedWidget").setCenterY(240);
    widgets("AnimatedWidget").setWidgetColorRGB(0.0, 0.0, 0.0);

    // Button names stay stable across the event, update and render modules.

    // The three top buttons share the same y axis and span the lavender panel's x range.
    // AudioMode is narrower to make room for a WASAPI share-mode selector.
    widgets.add("AudioMode");
    widgets("AudioMode").setType(WidgetType::Button_StateMono);
    widgets("AudioMode").setButtonFunction(WidgetAction::AudioMode_Toggle);
    widgets("AudioMode").setWidgetColorRGB(0.8f, 0.8f, 0.8f);
    widgets("AudioMode").setRectangleShape(140, 40);
    widgets("AudioMode").setCenterX(800);
    widgets("AudioMode").setCenterY(65);
    widgets("AudioMode").setText("Mode: Recording");

    widgets.add("WasapiMode");
    widgets("WasapiMode").setType(WidgetType::Button_StateMono);
    widgets("WasapiMode").setButtonFunction(WidgetAction::WasapiMode_Toggle);
    widgets("WasapiMode").setRectangleShape(100, 40);
    widgets("WasapiMode").setCenterX(1060);
    widgets("WasapiMode").setCenterY(65);
    const int startupWasapiMode = g_activeWasapiMode.load(memory_order_acquire);
    widgets("WasapiMode").setText(startupWasapiMode == 1 ? "WASAPI: Excl." :
        startupWasapiMode == 0 ? "WASAPI: Shared" :
        startupWasapiMode == -1 ? "Other Host" : "No Audio");
    widgets("WasapiMode").setWidgetColorRGB(0.60f, 0.85f, 0.95f);

    // Reopen the default devices on demand after changing or reconnecting a microphone or
    // speaker. This operation stays on the PortAudio device worker.
    widgets.add("AudioDevices");
    widgets("AudioDevices").setType(WidgetType::Button_StateMono);
    widgets("AudioDevices").setButtonFunction(WidgetAction::AudioDevices_Reopen);
    widgets("AudioDevices").setWidgetColorRGB(0.55f, 0.85f, 1.0f);
    widgets("AudioDevices").setRectangleShape(115, 40);
    widgets("AudioDevices").setCenterX(1295);
    widgets("AudioDevices").setCenterY(65);
    widgets("AudioDevices").setText("Reopen Audio");

    // Record, play and save buttons initially store a Start/Save WidgetAction; start/stop actions
    // change the same widget later.
    widgets.add("PA_Record");
    widgets("PA_Record").setType(WidgetType::Button_StateMono);
    widgets("PA_Record").setButtonFunction(WidgetAction::PA_AudioRecord_Start);
    widgets("PA_Record").setWidgetColorRGB(1.0, 0.55, 0.55);
    widgets("PA_Record").setRectangleShape(90, 35);
    widgets("PA_Record").setCenterX(1130);
    widgets("PA_Record").setCenterY(300);
    widgets("PA_Record").setText("Start Rec");

    // Effect buttons are independent toggles. Any combination can be previewed or saved.
    // All recorded previews share the one Play control and a single PortAudio stream.
    const auto addEffectButton = [&](const string& name, WidgetAction action,
        float centerX, float centerY, const string& label, bool enabled) {
        widgets.add(name);
        auto& button = widgets(name);
        button.setType(WidgetType::Button_StateMono);
        button.setButtonFunction(action);
        SetEffectButtonAppearance(button, enabled);
        button.setRectangleShape(90, 35);
        button.setCenterX(centerX);
        button.setCenterY(centerY);
        button.setText(label + (enabled ? ": ON" : ": OFF"));
    };
    addEffectButton("EffectPitch", WidgetAction::EffectPitch_Toggle,
        750, 300, "Pitch", true);
    addEffectButton("EffectFormant", WidgetAction::EffectFormant_Toggle,
        940, 300, "Formant", true);
    addEffectButton("EffectTimbre", WidgetAction::EffectTimbre_Toggle,
        750, 385, "Timbre", false);
    addEffectButton("EffectProsody", WidgetAction::EffectProsody_Toggle,
        940, 385, "Prosody", false);
    addEffectButton("EffectJitter", WidgetAction::EffectJitter_Toggle,
        1130, 385, "Jitter", false);

    widgets.add("PA_PlaySelected");
    widgets("PA_PlaySelected").setType(WidgetType::Button_StateMono);
    widgets("PA_PlaySelected").setButtonFunction(WidgetAction::PA_AudioPlaySelected_Start);
    widgets("PA_PlaySelected").setWidgetColorRGB(0.90f, 0.90f, 0.55f);
    widgets("PA_PlaySelected").setRectangleShape(90, 35);
    widgets("PA_PlaySelected").setCenterX(1320);
    widgets("PA_PlaySelected").setCenterY(300);
    widgets("PA_PlaySelected").setText("Play");

    widgets.add("PA_SaveSelected");
    widgets("PA_SaveSelected").setType(WidgetType::Button_StateMono);
    widgets("PA_SaveSelected").setButtonFunction(WidgetAction::PA_SaveSelected);
    widgets("PA_SaveSelected").setWidgetColorRGB(0.70f, 0.88f, 0.85f);
    widgets("PA_SaveSelected").setRectangleShape(90, 35);
    widgets("PA_SaveSelected").setCenterX(1320);
    widgets("PA_SaveSelected").setCenterY(385);
    widgets("PA_SaveSelected").setText("Save WAV");

    // Vertical pitch control: pixels increase downward, but the displayed semitone value increases
    // upward.
    widgets.add("Slider_PitchShift");
    widgets("Slider_PitchShift").setType(WidgetType::SliderV_FreeLimited);
    widgets("Slider_PitchShift").setBgColor(0.3f, 0.3f, 0.55f);
    widgets("Slider_PitchShift").setHalfDiag1(55);
    widgets("Slider_PitchShift").setHalfDiag2(40);
    widgets("Slider_PitchShift").setCenterX(715);
    widgets("Slider_PitchShift").setCenterY(740);
    widgets("Slider_PitchShift").setTriangleColorRGB(0, 0.0, 0.65, 0.0);
    widgets("Slider_PitchShift").setTriangleColorRGB(1, 0.0, 1.0, 0.0);
    widgets("Slider_PitchShift").setPointColorRGB(0, 1, 0.0, 0.15, 0.0);
    widgets("Slider_PitchShift").setPointColorRGB(1, 1, 0.75, 1.0, 0.75);

    widgets("Slider_PitchShift").setLegendY_Unit(WidgetUnit::Pts);
    widgets("Slider_PitchShift").setLegend_yMin_Val(-12.0);
    widgets("Slider_PitchShift").setLegend_yMax_Val(12.0);
    widgets("Slider_PitchShift").setLegend_yMin_Text("-12 ST");
    widgets("Slider_PitchShift").setLegend_yMax_Text("+12 ST");

    // Position the neutral 0 semitone default at the center of [-12, +12].
    double initialPosY = widgets("Slider_PitchShift").getSlider_yMin() +
        (12.0 - 0.0) / 24.0 * (widgets("Slider_PitchShift").getSlider_yMax() -
            widgets("Slider_PitchShift").getSlider_yMin());
    widgets("Slider_PitchShift").setHandleCenterY(initialPosY);
    widgets("Slider_PitchShift").setSlider_yVal(initialPosY);

    // Formant slider uses a display range of 0.5..2.0 and starts at its neutral 1.00x ratio.
    widgets.add("Slider_FormantShift");
    widgets("Slider_FormantShift").setType(WidgetType::SliderV_FreeLimited);
    widgets("Slider_FormantShift").setBgColor(0.55f, 0.3f, 0.3f);
    widgets("Slider_FormantShift").setHalfDiag1(55);
    widgets("Slider_FormantShift").setHalfDiag2(40);
    widgets("Slider_FormantShift").setCenterX(843);
    widgets("Slider_FormantShift").setCenterY(740);
    widgets("Slider_FormantShift").setTriangleColorRGB(0, 0.65, 0.0, 0.0);
    widgets("Slider_FormantShift").setTriangleColorRGB(1, 1.0, 0.0, 0.0);
    widgets("Slider_FormantShift").setPointColorRGB(0, 1, 0.15, 0.0, 0.0);
    widgets("Slider_FormantShift").setPointColorRGB(1, 1, 1.0, 0.75, 0.75);

    widgets("Slider_FormantShift").setLegendY_Unit(WidgetUnit::Pts);
    widgets("Slider_FormantShift").setLegend_yMin_Val(0.5);
    widgets("Slider_FormantShift").setLegend_yMax_Val(2.0);
    widgets("Slider_FormantShift").setLegend_yMin_Text("0.5x");
    widgets("Slider_FormantShift").setLegend_yMax_Text("2.0x");

    double initialPosY_Formant = widgets("Slider_FormantShift").getSlider_yMin() +
        (2.0 - 1.0) / 1.5 * (widgets("Slider_FormantShift").getSlider_yMax() -
            widgets("Slider_FormantShift").getSlider_yMin());
    widgets("Slider_FormantShift").setHandleCenterY(initialPosY_Formant);

    // Synchronize the initial stored legend value with the visual handle position; the UI
    // publishes this value each frame.
    widgets("Slider_FormantShift").setSlider_yVal(initialPosY_Formant);

    // One independent live timbre slider: the centre is transparent (0%), lower values
    // attenuate higher frequencies, and upper values raise them. Its IIR filter keeps no
    // additional audio block, so changing this value adds no algorithmic buffering.
    widgets.add("Slider_Timbre");
    widgets("Slider_Timbre").setType(WidgetType::SliderV_FreeLimited);
    widgets("Slider_Timbre").setBgColor(0.25f, 0.50f, 0.55f);
    widgets("Slider_Timbre").setHalfDiag1(55);
    widgets("Slider_Timbre").setHalfDiag2(40);
    widgets("Slider_Timbre").setCenterX(971);
    widgets("Slider_Timbre").setCenterY(740);
    widgets("Slider_Timbre").setTriangleColorRGB(0, 0.0, 0.70, 0.75);
    widgets("Slider_Timbre").setTriangleColorRGB(1, 0.25, 1.0, 1.0);
    widgets("Slider_Timbre").setPointColorRGB(0, 1, 0.0, 0.35, 0.40);
    widgets("Slider_Timbre").setPointColorRGB(1, 1, 0.75, 1.0, 1.0);
    widgets("Slider_Timbre").setLegendY_Unit(WidgetUnit::percent);
    widgets("Slider_Timbre").setLegend_yMin_Val(-100.0);
    widgets("Slider_Timbre").setLegend_yMax_Val(100.0);
    widgets("Slider_Timbre").setLegend_yMin_Text("-100%");
    widgets("Slider_Timbre").setLegend_yMax_Text("+100%");
    const double initialPosY_Timbre =
        (widgets("Slider_Timbre").getSlider_yMin() +
         widgets("Slider_Timbre").getSlider_yMax()) * 0.5;
    widgets("Slider_Timbre").setHandleCenterY(initialPosY_Timbre);
    widgets("Slider_Timbre").setSlider_yVal(initialPosY_Timbre);

    // The neutral centre preserves the recorded voice. One slider controls both an
    // onset-based pitch gesture and its small loudness accent in recording and stream modes.
    widgets.add("Slider_Prosody");
    widgets("Slider_Prosody").setType(WidgetType::SliderV_FreeLimited);
    widgets("Slider_Prosody").setBgColor(0.50f, 0.38f, 0.68f);
    widgets("Slider_Prosody").setHalfDiag1(55);
    widgets("Slider_Prosody").setHalfDiag2(40);
    widgets("Slider_Prosody").setCenterX(1099);
    widgets("Slider_Prosody").setCenterY(740);
    widgets("Slider_Prosody").setTriangleColorRGB(0, 0.45, 0.23, 0.75);
    widgets("Slider_Prosody").setTriangleColorRGB(1, 0.78, 0.62, 1.0);
    widgets("Slider_Prosody").setPointColorRGB(0, 1, 0.46, 0.22, 0.72);
    widgets("Slider_Prosody").setPointColorRGB(1, 1, 0.83, 0.71, 1.0);
    widgets("Slider_Prosody").setLegendY_Unit(WidgetUnit::percent);
    widgets("Slider_Prosody").setLegend_yMin_Val(-100.0);
    widgets("Slider_Prosody").setLegend_yMax_Val(100.0);
    widgets("Slider_Prosody").setLegend_yMin_Text("-100%");
    widgets("Slider_Prosody").setLegend_yMax_Text("+100%");
    const double initialProsodyY = (widgets("Slider_Prosody").getSlider_yMin() +
        widgets("Slider_Prosody").getSlider_yMax()) * 0.5;
    widgets("Slider_Prosody").setHandleCenterY(initialProsodyY);
    widgets("Slider_Prosody").setSlider_yVal(initialProsodyY);

    // Jitter varies the pitch ratio by up to +/-2%. The effect starts OFF,
    // and its slider also starts at zero so the user must enable and raise it.
    widgets.add("Slider_Jitter");
    widgets("Slider_Jitter").setType(WidgetType::SliderV_FreeLimited);
    widgets("Slider_Jitter").setBgColor(0.3f, 0.55f, 0.3f);
    widgets("Slider_Jitter").setHalfDiag1(55);
    widgets("Slider_Jitter").setHalfDiag2(40);
    widgets("Slider_Jitter").setCenterX(1227);
    widgets("Slider_Jitter").setCenterY(740);
    widgets("Slider_Jitter").setTriangleColorRGB(0, 0.0, 0.65, 0.0);
    widgets("Slider_Jitter").setTriangleColorRGB(1, 0.0, 1.0, 0.0);

    widgets("Slider_Jitter").setLegendY_Unit(WidgetUnit::Pts);
    widgets("Slider_Jitter").setLegend_yMin_Val(0.0);
    widgets("Slider_Jitter").setLegend_yMax_Val(0.02);
    widgets("Slider_Jitter").setLegend_yMin_Text("0.000");
    widgets("Slider_Jitter").setLegend_yMax_Text("0.020");

    initialPosY = widgets("Slider_Jitter").getSlider_yMax();
    widgets("Slider_Jitter").setHandleCenterY(initialPosY);
    widgets("Slider_Jitter").setSlider_yVal(initialPosY);

    // The UI displays 0..100 percent; AppUpdate divides by 100 before storing the audio volume
    // multiplier.
    widgets.add("Slider_Volume");
    widgets("Slider_Volume").setType(WidgetType::SliderV_FreeLimited);
    widgets("Slider_Volume").setBgColor(0.55f, 0.55f, 0.3f);
    widgets("Slider_Volume").setHalfDiag1(55);
    widgets("Slider_Volume").setHalfDiag2(40);
    widgets("Slider_Volume").setCenterX(1355);
    widgets("Slider_Volume").setCenterY(740);
    widgets("Slider_Volume").setTriangleColorRGB(0, 0.65, 0.65, 0.0);
    widgets("Slider_Volume").setTriangleColorRGB(1, 1.0, 1.0, 0.0);

    widgets("Slider_Volume").setLegendY_Unit(WidgetUnit::percent);
    widgets("Slider_Volume").setLegend_yMin_Val(0.0);
    widgets("Slider_Volume").setLegend_yMax_Val(100.0);
    widgets("Slider_Volume").setLegend_yMin_Text("0%");
    widgets("Slider_Volume").setLegend_yMax_Text("100%");

    initialPosY = widgets("Slider_Volume").getSlider_yMin() +
        (100.0 - 50.0) / 100.0 * (widgets("Slider_Volume").getSlider_yMax() -
            widgets("Slider_Volume").getSlider_yMin());
    widgets("Slider_Volume").setHandleCenterY(initialPosY);

    // This text box shows the current hovered or dragged slider values; AppRender updates its
    // content.
    widgets.add("WidgetInfo");
    widgets("WidgetInfo").setType(WidgetType::TextBox);
    widgets("WidgetInfo").setWidgetColorRGB(0.55, 0.55, 1.0);
    widgets("WidgetInfo").setRectangleShape(375, 55);
    widgets("WidgetInfo").setCenterX(1035);
    widgets("WidgetInfo").setCenterY(185);

    // Start display analysis after its buffers exist. The worker publishes snapshots only;
    // SDL retains exclusive ownership of the visible spectrum and renderer.
    StartDisplayFftWorker();
    RequestDisplayFft();
    PublishDisplayFft();
    SmoothFFTDisplay_PortAudio(0.25f);

    paFftDataReady = true;

    // Prepare Stream's private delay lines before its first button click and before any
    // callback can start. If allocation fails, ActivateStreamMode retains its fallback.
    try { InitStreamMode(); }
    catch (const exception&) {
        // Stream initialization will be retried when the user enables it.
    }

    // Reserve the recording collector's first minute and construct the recorded-preview
    // effect chain before any device callback or asynchronous button action can start.
    PreloadRecordedAudioResources();

    // The first GPU submission may allocate or compile driver resources even though SDL
    // textures already exist. Show the initial "Starting" state before starting audio so
    // a one-time render stall cannot compete with the first real-time callback.
    widgets("WasapiMode").setText("Audio: Starting...");
    // Text and GPU textures belong to SDL's main thread. A later window resize can still
    // rebuild size-dependent targets, which remains a separately measured event.
    PreloadButtonStateText();
    PreloadVisualizationStatusText();
    PreloadStaticUiText();
    PreloadInterfaceRenderTarget();
    PreloadObliqueQuadTargets();
    const float initialGraphWidth = CalculateGraphWidth(
        CalculateUiLayoutWidth(initialWidth, initialHeight));
    PreloadSpectrumRenderTargets(initialGraphWidth, 210.0f);
    try { PreloadSpectrumGeometry(initialGraphWidth); }
    catch (const exception& error) {
        // The existing per-frame reserve remains a fallback if startup allocation fails.
    }

    // All callback-visible state and UI widgets exist. Device setup runs on its worker;
    // the normal SDL loop remains free to paint while WASAPI opens.
    try {
        g_portAudioDeviceWorker = thread(PortAudioDeviceWorkerLoop);
    } catch (const system_error& error) {
        widgets("WasapiMode").setText("No Audio");

    }

    return SDL_APP_CONTINUE;
}

// Run button actions on mouse release only when the pointer remains over the same pressed widget.
SDL_AppResult SDL_AppEvent(void* appstate, SDL_Event* event)
{
    // SDL asks to terminate through this event; returning SDL_APP_SUCCESS triggers shutdown.
    if (event->type == SDL_EVENT_QUIT) {
        return SDL_APP_SUCCESS;
    }

    if (event->type == SDL_EVENT_WINDOW_DISPLAY_CHANGED ||
        event->type == SDL_EVENT_DISPLAY_USABLE_BOUNDS_CHANGED)
        UpdateWindowSizeLimits();

    // Recalculate the hit from this event's position. The last rendered hover
    // can become stale if the user moves and clicks between visual frames, or
    // if a resize moves the interface underneath a stationary pointer.
    if (event->type == SDL_EVENT_MOUSE_MOTION ||
        event->type == SDL_EVENT_MOUSE_BUTTON_DOWN ||
        event->type == SDL_EVENT_MOUSE_BUTTON_UP) {
        RefreshResponsiveUiGeometry();
        const float windowX = event->type == SDL_EVENT_MOUSE_MOTION
            ? event->motion.x : event->button.x;
        const float windowY = event->type == SDL_EVENT_MOUSE_MOTION
            ? event->motion.y : event->button.y;
        int pointerWindowWidth = 0, pointerWindowHeight = 0;
        const bool mapped = SDL_GetWindowSize(window, &pointerWindowWidth,
            &pointerWindowHeight) && WindowPointToUi(pointerWindowWidth,
                pointerWindowHeight, windowX, windowY, mx, my);
        if (mapped)
            hoveredWidget = HitTestUiAt(mx, my);
        else {
            mx = my = -1.0f;
            hoveredWidget = nullptr;
        }
    }

    // A press records the widget selected by RenderFrame hit testing and the pointer-to-handle
    // offset.
    if (event->type == SDL_EVENT_MOUSE_BUTTON_DOWN) {
        if (event->button.button == SDL_BUTTON_LEFT) {
            if (hoveredWidget == nullptr) {
                pressedWidget = nullptr;
            }
            else {
                pressedWidget = hoveredWidget;
                pressedWidget->set_isMouseLeftButtonDown(true);

                // Store the click offset; RenderFrame subtracts it while dragging to retain the
                // original grab point.
                mouseClick_OFFSET.x = mx - pressedWidget->getHandleCenterX();
                mouseClick_OFFSET.y = my - pressedWidget->getHandleCenterY();

                pressedWidget->updatePressedColorRGB();
            }
        }
    }
    // A release acts only on a left-button press that still points at the same hovered button.
    else if (event->type == SDL_EVENT_MOUSE_BUTTON_UP) {
        if (event->button.button == SDL_BUTTON_LEFT) {
            if (pressedWidget != nullptr) {
                // Compare widget addresses rather than captions so dragging off a button cancels
                // its action.
                bool isStillOverButton = (hoveredWidget == pressedWidget);

                if (isStillOverButton && IsAudioDeviceReopening() &&
                    pressedWidget == &widgets("AudioDevices")) {
                    // The running refresh already enumerates the devices. Repeating
                    // that click cannot improve the current request and would start
                    // another full driver teardown after it finishes. During a
                    // pending mode-only change, however, combine an explicit
                    // refresh with that mode change before any new worker open.
                    if (g_audioReopenState.load(memory_order_acquire) == 4) {
                        g_deferredDeviceRefresh = true;
                        g_reopenSettleDeadlineNs = SDL_GetTicksNS() +
                            kReopenQuietIntervalNs;
                    }
                }
                else if (isStillOverButton && IsAudioDeviceReopening() &&
                    pressedWidget == &widgets("WasapiMode")) {
                    const int requested = g_deferredWasapiMode >= 0
                        ? g_deferredWasapiMode :
                        g_requestedWasapiMode.load(memory_order_acquire);
                    g_deferredWasapiMode = requested == 1 ? 0 : 1;
                    g_reopenSettleDeadlineNs = SDL_GetTicksNS() + kReopenQuietIntervalNs;

                }
                else if (isStillOverButton && pressedWidget->getType() == WidgetType::Button_StateMono &&
                    IsAudioDeviceReopening()) {
                    // Ignore audio actions while the device worker owns PortAudio.
                }
                else if (isStillOverButton && IsAudioStartupPending() &&
                    (pressedWidget == &widgets("PA_Record") ||
                        pressedWidget == &widgets("PA_PlaySelected") ||
                        pressedWidget == &widgets("PA_SaveSelected") ||
                        pressedWidget == &widgets("AudioMode") ||
                        pressedWidget == &widgets("AudioDevices") ||
                        pressedWidget == &widgets("WasapiMode"))) {
                    // Ignore actions until initial device setup has completed.
                }
                else if (isStillOverButton && IsRecordedPlaybackTransitionPending() &&
                    (pressedWidget == &widgets("PA_PlaySelected") ||
                        pressedWidget == &widgets("PA_Record") ||
                        pressedWidget == &widgets("AudioMode") ||
                        pressedWidget == &widgets("AudioDevices") ||
                        pressedWidget == &widgets("WasapiMode"))) {
                    // Wait for the previous playback transition to finish.
                }
                else if (isStillOverButton &&
                    g_recordingTransition.load(memory_order_acquire) != 0 &&
                    (pressedWidget == &widgets("PA_Record") ||
                        pressedWidget == &widgets("PA_PlaySelected") ||
                        pressedWidget == &widgets("PA_SaveSelected") ||
                        pressedWidget == &widgets("AudioMode") ||
                        pressedWidget == &widgets("AudioDevices") ||
                        pressedWidget == &widgets("WasapiMode"))) {
                    // Wait for the recording worker before accepting another action.
                }
                else if (isStillOverButton && pressedWidget->getType() == WidgetType::Button_StateMono) {
                    // Selectors are available in both modes. Their atomic mask also drives
                    // the live stream and the next block of recorded playback.
                    const auto toggleEffect = [&](uint8_t bit, const string& name) {
                        const uint8_t before = g_selectedEffects.fetch_xor(bit, memory_order_acq_rel);
                        const bool enabled = (before & bit) == 0;
                        pressedWidget->setText(name + (enabled ? ": ON" : ": OFF"));
                        SetEffectButtonAppearance(*pressedWidget, enabled);
                    };

                    switch (pressedWidget->getButtonFunction()) {
                    case WidgetAction::EffectPitch_Toggle:
                        toggleEffect(EFFECT_PITCH, "Pitch");
                        break;
                    case WidgetAction::EffectFormant_Toggle:
                        toggleEffect(EFFECT_FORMANT, "Formant");
                        break;
                    case WidgetAction::EffectTimbre_Toggle:
                        toggleEffect(EFFECT_TIMBRE, "Timbre");
                        break;
                    case WidgetAction::EffectProsody_Toggle:
                        toggleEffect(EFFECT_PROSODY, "Prosody");
                        break;
                    case WidgetAction::EffectJitter_Toggle:
                        toggleEffect(EFFECT_JITTER, "Jitter");
                        break;
                    case WidgetAction::PA_AudioPlaySelected_Start:
                        if (!g_streamModeActive.load(memory_order_acquire) &&
                            RequestRecordedPlaybackStart()) {
                            pressedWidget->setText("Starting...");
                            pressedWidget->setWidgetColorRGB(1.0f, 0.85f, 0.45f);
                        }
                        break;
                    case WidgetAction::PA_AudioPlaySelected_Stop:
                        if (RequestRecordedPlaybackStop()) {
                            pressedWidget->setText("Stopping...");
                            pressedWidget->setWidgetColorRGB(1.0f, 0.85f, 0.45f);
                        }
                        break;
                    case WidgetAction::PA_SaveSelected:
                        if (!g_streamModeActive.load(memory_order_acquire) &&
                            SavePortAudioSelectedRecording()) {
                            pressedWidget->setText("Saving...");
                            pressedWidget->setWidgetColorRGB(1.0f, 0.85f, 0.45f);
                        }
                        break;
                    case WidgetAction::AudioDevices_Reopen:
                        if (ReopenAudioDevices(true)) {
                            pressedWidget->setText("Reopening...");
                            pressedWidget->setWidgetColorRGB(1.0f, 0.85f, 0.45f);
                        }
                        widgets("PA_PlaySelected").setButtonFunction(
                            WidgetAction::PA_AudioPlaySelected_Start);
                        widgets("PA_PlaySelected").setText("Play");
                        widgets("PA_PlaySelected").setWidgetColorRGB(0.90f, 0.90f, 0.55f);
                        break;
                    case WidgetAction::WasapiMode_Toggle: {
                        // Recording cannot survive a device close; the existing reopen path
                        // rejects it. The UI stays responsive while the worker changes modes.
                        const int current = g_activeWasapiMode.load(memory_order_acquire);
                        const int previousRequest = g_requestedWasapiMode.load(memory_order_acquire);
                        g_requestedWasapiMode.store(current == 1 ? 0 : 1, memory_order_release);
                        if (ReopenAudioDevices(false)) {
                            pressedWidget->setText("Switching...");
                            pressedWidget->setWidgetColorRGB(1.0f, 0.85f, 0.45f);
                            widgets("AudioDevices").setText("Reopening...");
                            widgets("PA_PlaySelected").setButtonFunction(
                                WidgetAction::PA_AudioPlaySelected_Start);
                            widgets("PA_PlaySelected").setText("Play");
                            widgets("PA_PlaySelected").setWidgetColorRGB(0.90f, 0.90f, 0.55f);
                        } else {
                            g_requestedWasapiMode.store(previousRequest, memory_order_release);
                        }
                        break;
                    }
                    case WidgetAction::PA_AudioRecord_Start:
                        if (!g_streamModeActive.load(memory_order_acquire) &&
                            RequestRecordingAction(true)) {
                            pressedWidget->setText("Starting...");
                            pressedWidget->setWidgetColorRGB(1.0f, 0.85f, 0.45f);
                        }
                        break;
                    case WidgetAction::PA_AudioRecord_Stop:
                        if (RequestRecordingAction(false)) {
                            pressedWidget->setText("Stopping...");
                            pressedWidget->setWidgetColorRGB(1.0f, 0.85f, 0.45f);
                        }
                        break;
                    case WidgetAction::AudioMode_Toggle:
                        if (g_streamModeActive.load(memory_order_acquire))
                            DeactivateStreamMode();
                        else {
                            // A live output and a recorded preview must never mix at the speaker.
                            if (paIsRecording.load(memory_order_acquire)) {
                                if (RequestRecordingAction(false)) {
                                    g_switchToStreamAfterRecording = true;
                                    widgets("PA_Record").setText("Stopping...");
                                    widgets("PA_Record").setWidgetColorRGB(1.0f, 0.85f, 0.45f);
                                }
                                break;
                            }
                            if (RequestRecordedPlaybackStop()) {
                                g_switchToStreamAfterPlayback = true;
                                pressedWidget->setText("Switching...");
                                pressedWidget->setWidgetColorRGB(1.0f, 0.85f, 0.45f);
                            }
                            break;
                        }
                        pressedWidget->setText(g_streamModeActive.load(memory_order_acquire)
                            ? "Mode: Stream" : "Mode: Recording");
                        pressedWidget->setWidgetColorRGB(g_streamModeActive.load(memory_order_acquire)
                            ? 0.3f : 0.8f, g_streamModeActive.load(memory_order_acquire)
                            ? 1.0f : 0.8f, g_streamModeActive.load(memory_order_acquire)
                            ? 0.3f : 0.8f);
                        break;
                    default:

                        break;
                    }
                }
                pressedWidget->set_isMouseLeftButtonDown(false);
                pressedWidget->updatePressedColorRGB();
            }

            // Release the UI press state after the action; callbacks continue independently on
            // PortAudio streams.
            pressedWidget = nullptr;
        }
    }

    return SDL_APP_CONTINUE;
}

// Advance the audio/UI state and render one frame. Audio x-runs and sustained
// visual overload adjust only optional rendering work; audio settings stay intact.
SDL_AppResult SDL_AppIterate(void* appstate)
{
    const Uint64 frameStartedNs = SDL_GetTicksNS();
    static Uint64 previousFrameNs = 0;
    static Uint64 reportStartNs = frameStartedNs;
    static Uint64 nextVisualDeadlineNs = 0;
    static unsigned renderedFrameCount = 0;
    static unsigned slowAppFrames = 0;
    static unsigned slowSpectrumFrames = 0;
    static AdaptiveVisualBudget visualBudget;
    Uint64 targetFrameNs = 1000000000ULL /
        static_cast<Uint64>(visualBudget.Fps());

    if (previousFrameNs == 0)
        SetSpectrumPreferredScale(visualBudget.FftScale());
    if (nextVisualDeadlineNs == 0 ||
        frameStartedNs > nextVisualDeadlineNs + targetFrameNs * 3)
        nextVisualDeadlineNs = frameStartedNs + targetFrameNs;

    // Smooth FFT changes by real elapsed time, including skipped display frames.
    const float visualElapsedSeconds = previousFrameNs == 0 ? 1.0f / 60.0f :
        static_cast<float>(frameStartedNs - previousFrameNs) / 1000000000.0f;
    previousFrameNs = frameStartedNs;
    ++renderedFrameCount;

    // The FFT uses the same FPS ladder as the rest of the interface.
    if (frameStartedNs - reportStartNs >= 10000000000ULL) {
        const auto audioPressure = g_audioPressure.consumeEvents();
        if (visualBudget.Observe(renderedFrameCount, slowAppFrames,
            slowSpectrumFrames, audioPressure > 0 ? 1u : 0u)) {
            targetFrameNs = 1000000000ULL /
                static_cast<Uint64>(visualBudget.Fps());
            nextVisualDeadlineNs = frameStartedNs + targetFrameNs;
            SetSpectrumPreferredScale(visualBudget.FftScale());
        }
        reportStartNs = frameStartedNs;
        renderedFrameCount = 0;
        slowAppFrames = 0;
        slowSpectrumFrames = 0;
    }

    CompleteAudioDeviceStartup();
    CompleteAudioDeviceReopen();
    StartSettledAudioReopen();
    CompleteRecordingAction();
    // The device worker owns stream opens and closes; keep SDL drawing while it runs.
    if (!IsAudioDeviceReopening()) UpdatePlaybackAndAnalysis();
    CompletePlaybackModeSwitch();
    PublishDisplayFft();
    RequestDisplayFft();
    SmoothFFTDisplay_PortAudio(0.34f, visualElapsedSeconds);

    const Uint64 beforeRenderNs = SDL_GetTicksNS();
    RenderFrameTiming timing{};
    RenderFrame(static_cast<double>(SDL_GetTicks()) / 1000.0, &timing);
    const Uint64 afterRenderNs = SDL_GetTicksNS();
    const Uint64 drawNs = timing.controlsNs + timing.spectrumNs +
        timing.waveformNs + timing.compositeNs;
    if (timing.spectrumNs > 150000000ULL &&
        visualBudget.ReduceFftAfterStall())
        SetSpectrumPreferredScale(visualBudget.FftScale());

    // RenderPresent can block on the driver. Its time does not falsely count as
    // FFT or UI CPU work when the adaptive policy selects a lower visual tier.
    if (beforeRenderNs - frameStartedNs + drawNs > targetFrameNs)
        ++slowAppFrames;
    if (timing.spectrumNs >= targetFrameNs / 2 &&
        timing.spectrumNs >= timing.controlsNs &&
        timing.spectrumNs >= timing.waveformNs &&
        timing.spectrumNs >= timing.compositeNs)
        ++slowSpectrumFrames;

    // Skip missed deadlines instead of replaying old frames after a long driver wait.
    if (afterRenderNs < nextVisualDeadlineNs)
        SDL_DelayPrecise(nextVisualDeadlineNs - afterRenderNs);
    const Uint64 frameFinishedNs = SDL_GetTicksNS();
    if (frameFinishedNs < nextVisualDeadlineNs) {
        nextVisualDeadlineNs += targetFrameNs;
    } else {
        const Uint64 missedSlots = (frameFinishedNs - nextVisualDeadlineNs) /
            targetFrameNs + 1;
        nextVisualDeadlineNs += missedSlots * targetFrameNs;
    }
    return SDL_APP_CONTINUE;
}

// Release application-owned resources during SDL shutdown. Stop and join the audio callbacks
// before deleting any state they read; this prevents the inherited teardown race.
void SDL_AppQuit(void* appstate, SDL_AppResult result)
{
    JoinSelectedSaveWorker();
    StopDisplayFftWorker();
    CleanupSpectrumRenderTarget();
    CleanupObliqueQuadTargets();
    CleanupStaticSliderCaptions();
    CleanupTextCache();
    // Close PortAudio on the worker thread that initialized WASAPI/COM, then join it before
    // freeing callback-visible application state. A pending reopen finishes before shutdown.
    {
        lock_guard<mutex> lock(g_audioWorkerMutex);
        g_audioWorkerShouldStop = true;
    }
    g_audioWorkerWake.notify_one();
    if (g_portAudioDeviceWorker.joinable()) g_portAudioDeviceWorker.join();

    if (gLogoTexture) {
        SDL_DestroyTexture(gLogoTexture);
        gLogoTexture = nullptr;
    }

    CleanupStreamMode();

    if (gFont) {
        TTF_CloseFont(gFont);
        gFont = nullptr;
    }
    TTF_Quit();

}
