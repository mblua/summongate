# PLAN: StatusBar Voice & Clear Input Buttons

## Goal

Add two buttons to the Terminal's StatusBar (right-aligned):
1. **Push-to-talk mic** - hold to record, release to stop + transcribe + write to PTY
2. **Clear input** - sends Ctrl+U to the active PTY to clear the coding agent's input line

## Problem: Duplicated Recording Logic

The voice recording logic (MediaRecorder, AudioContext, Gemini transcription, PTY write) currently lives entirely inside `src/sidebar/components/SessionItem.tsx` (lines 9-187). The terminal window needs the same capability. We must not duplicate this code.

## Architecture

### Step 1: Extract shared voice recorder service

Create `src/shared/voice-recorder.ts` - a singleton module that owns all recording state and logic.

**Exports:**
- `voiceRecorder` object with methods: `start(sessionId)`, `stop()`, `toggle(sessionId)`
- Reactive signals: `recordingSessionId()`, `processingSessionId()`, `micError()`, `recordingSeconds()`, `audioLevel()`
- These signals are SolidJS signals, importable from both windows

**Contains (moved from SessionItem.tsx):**
- MediaRecorder creation + chunk collection
- AudioContext analyser for level monitoring
- Recording timer (seconds counter)
- `onstop` handler: blob assembly, Gemini transcription via `VoiceAPI.transcribe()`, PTY write via `PtyAPI.write()`
- Error handling with 5s timeout clear

**Key detail:** Since sidebar and terminal are separate windows (separate JS contexts), each will get its own instance of this module. That is fine - only one window will be recording at a time, and the PTY write goes through Tauri IPC regardless.

### Step 2: Extract shared settings store

Move `src/sidebar/stores/settings.ts` to `src/shared/stores/settings.ts` (or create a shared version). The terminal window needs `voiceEnabled` to conditionally show the mic button.

Alternatively, keep the sidebar store where it is and create a minimal settings accessor in the terminal. Given both windows already import from `src/shared/`, the cleanest path is moving to shared.

**Decision:** Move to `src/shared/stores/settings.ts`. Update sidebar imports.

### Step 3: Refactor SessionItem.tsx

Remove the extracted voice recording logic (lines 9-187). Replace with imports from `voice-recorder.ts`:

```tsx
import { voiceRecorder } from "../../shared/voice-recorder";

// isRecording/isProcessing now come from shared module
const isRecording = () => voiceRecorder.recordingSessionId() === props.session.id;
const isProcessing = () => voiceRecorder.processingSessionId() === props.session.id;
```

The mic button click handler becomes:
```tsx
const handleMicClick = (e: MouseEvent) => {
  e.stopPropagation();
  voiceRecorder.toggle(props.session.id);
};
```

The UI (recording indicator, processing spinner, error display) stays in SessionItem - it just reads from shared signals.

### Step 4: Add buttons to StatusBar

Modify `src/terminal/components/StatusBar.tsx`:

```
[DETACHED] [shell] [cols x rows]          [mic-btn] [clear-btn]
 ---- left items ----                      ---- right items ----
```

**Layout:** Add `justify-content: space-between` to `.status-bar`. Left section has existing items. Right section has the two new buttons.

**Mic button (push-to-talk):**
- `onMouseDown` → `voiceRecorder.start(activeSessionId)`
- `onMouseUp` / `onMouseLeave` → `voiceRecorder.stop()`
- Visual states: default, recording (red pulse), processing (cyan spin), error (amber)
- Only shown when `settingsStore.voiceEnabled`

**Clear input button:**
- `onClick` → `PtyAPI.write(activeSessionId, new TextEncoder().encode("\x15"))` (Ctrl+U)
- Simple icon/label, always visible when there is an active session

### Step 5: CSS for StatusBar buttons

Add to `src/terminal/styles/terminal.css`:
- `.status-bar-actions` - right-aligned flex container
- `.status-bar-btn` - base button style (small, fits 22px height bar)
- `.status-bar-btn-mic` - mic-specific states (recording, processing, error)
- `.status-bar-btn-clear` - clear button style

Match the industrial-dark aesthetic: transparent bg, subtle hover, no borders, 10-11px icons.

### Step 6: Keyboard shortcut (bonus)

Add to `src/shared/shortcuts.ts`:
- `Ctrl+Shift+R` → toggle voice recording on active session (both windows)

This requires the shortcut handler to know the active session ID. It can call `SessionAPI.getActive()` like the existing close-session shortcut does.

## Files Changed

| File | Action | Description |
|------|--------|-------------|
| `src/shared/voice-recorder.ts` | **CREATE** | Shared recording service extracted from SessionItem |
| `src/shared/stores/settings.ts` | **CREATE** | Settings store moved from sidebar to shared |
| `src/sidebar/stores/settings.ts` | **DELETE** | Replaced by shared version |
| `src/sidebar/components/SessionItem.tsx` | **EDIT** | Remove recording logic, import from shared |
| `src/terminal/components/StatusBar.tsx` | **EDIT** | Add mic + clear buttons, import shared modules |
| `src/terminal/styles/terminal.css` | **EDIT** | StatusBar button styles |
| `src/shared/shortcuts.ts` | **EDIT** | Add Ctrl+Shift+R for voice toggle |

## Execution Order

1. Create `src/shared/voice-recorder.ts`
2. Move settings store to `src/shared/stores/settings.ts`
3. Update sidebar imports (settings store path change)
4. Refactor SessionItem.tsx to use shared voice-recorder
5. Update StatusBar.tsx with buttons
6. Add CSS
7. Add keyboard shortcut
8. Build + test

## Risks

- **Separate JS contexts**: Each window loads its own module instances. The mic button in terminal and sidebar are independent - pressing one does not visually update the other. This is acceptable since users will use one or the other, not both simultaneously.
- **MediaRecorder in terminal WebView**: Should work fine since Tauri WebView supports getUserMedia. Need to verify permissions.
- **Ctrl+U universality**: Works in readline/bash, Claude Code, most coding agents. Not guaranteed for all agents but covers 95%+ of cases.
