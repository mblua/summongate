# PLAN: Voice-to-Text (Microphone Button)

**Branch:** `feature/voice-to-text`
**Status:** PLANNING
**Created:** 2026-03-23

---

## Goal

Add a microphone button per session in the Sidebar that records audio, sends it to Gemini for transcription, and writes the resulting text into that session's terminal PTY.

---

## User-Facing Behavior

1. A mic icon appears to the **left of the Detach button** on each session item in the Sidebar
2. The button is only visible when voice-to-text is **enabled in Settings** and a **Gemini API key** is configured
3. Click to **start recording** (icon turns red/pulsing), click again to **stop**
4. On stop, audio is sent to backend, transcribed via Gemini, and the text is written to that specific session's PTY stdin (as if the user typed it)
5. No automatic Enter/newline at the end - the user decides when to submit

---

## Architecture

```
User clicks mic on SessionItem (session X)
  -> Browser MediaRecorder starts capturing audio (webm/opus)
  -> User clicks mic again to stop
  -> Audio bytes sent via IPC: invoke("voice_transcribe", { sessionId, audio })
  -> Rust receives audio bytes
  -> Rust calls Gemini API (generativelanguage.googleapis.com)
     POST /v1beta/models/gemini-2.0-flash:generateContent
     Body: audio as inline_data (base64), prompt: "Transcribe this audio exactly"
  -> Gemini returns transcribed text
  -> Rust writes text to PTY via pty_write(sessionId, text_bytes)
  -> Text appears in terminal as if typed
```

**Why backend transcription (not frontend)?**
- API key stays in Rust - never exposed to WebView/DevTools
- Consistent with existing IPC pattern (frontend invokes, backend acts)
- reqwest is already available in the project for HTTP calls

---

## Changes by File

### 1. Settings - Rust Backend

**File:** `src-tauri/src/config/settings.rs`

Add to `AppSettings` struct:
```rust
#[serde(default)]
pub voice_to_text_enabled: bool,
#[serde(default)]
pub gemini_api_key: String,
```

Add to `Default` impl:
```rust
voice_to_text_enabled: false,
gemini_api_key: String::new(),
```

### 2. Settings - Frontend Types

**File:** `src/shared/types.ts`

Add to `AppSettings` interface:
```typescript
voiceToTextEnabled: boolean;
geminiApiKey: string;
```

### 3. Settings Modal

**File:** `src/sidebar/components/SettingsModal.tsx`

Add a new section **"Voice to Text"** after the "Window" section:
- Checkbox: "Enable voice-to-text microphone"
- Text input (type password): "Gemini API Key" (masked, only shown when enabled)

### 4. Rust Voice Command

**New file:** `src-tauri/src/commands/voice.rs`

```rust
#[tauri::command]
pub async fn voice_transcribe(
    settings: State<'_, SettingsState>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    session_id: String,
    audio: Vec<u8>,
) -> Result<String, String>
```

Steps:
1. Read `gemini_api_key` from settings
2. Base64-encode the audio bytes
3. POST to Gemini API with audio as `inline_data` (mime: `audio/webm`)
4. Parse response, extract transcribed text
5. Write text bytes to PTY via `pty_mgr.write(uuid, text.as_bytes())`
6. Return the transcribed text (for frontend feedback)

Register in `src-tauri/src/commands/mod.rs` and in `lib.rs` invoke_handler.

### 5. IPC Layer

**File:** `src/shared/ipc.ts`

Add:
```typescript
export const VoiceAPI = {
  transcribe: (sessionId: string, audio: number[]) =>
    invoke<string>("voice_transcribe", { sessionId, audio }),
};
```

### 6. SessionItem - Mic Button

**File:** `src/sidebar/components/SessionItem.tsx`

Add mic button as the **first action button** (before Detach, around line 137):

```tsx
<Show when={voiceEnabled()}>
  <button
    class={`session-item-mic ${recording() ? "recording" : ""}`}
    title={recording() ? "Stop recording" : "Start recording"}
    onClick={handleMicClick}
  >
    {/* mic icon - Unicode or inline SVG */}
  </button>
</Show>
```

New signals and handler:
- `recording()` signal (boolean)
- `mediaRecorder` ref
- `handleMicClick()`: toggle recording on/off
- On stop: collect audio chunks, convert to byte array, call `VoiceAPI.transcribe()`
- Show visual feedback during recording (CSS pulse animation on the button)

Need access to settings to check `voiceToTextEnabled` and `geminiApiKey`. The sidebar store already loads settings - pass `voiceEnabled` as a derived value.

### 7. SessionItem Styles

**File:** `src/sidebar/styles/session-item.css`

Add styles for `.session-item-mic`:
- Normal state: muted mic icon, same opacity pattern as other buttons
- `.recording` state: red color + CSS pulse animation (150-200ms ease-out per project standards)
- Hover: same pattern as `.session-item-detach`

### 8. Sidebar Store (Settings Access)

**File:** `src/sidebar/stores/sessions.ts`

Ensure settings are accessible to SessionItem. If not already exposed, add a derived signal:
```typescript
export const voiceEnabled = () => {
  const s = settings();
  return s?.voiceToTextEnabled && s?.geminiApiKey?.length > 0;
};
```

---

## Button Order in SessionItem (after change)

```
[Mic] [Detach] [Bridge Dot] [Telegram] [Bot Menu] [Close]
```

---

## Gemini API Details

**Endpoint:** `https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent?key={API_KEY}`

**Request body:**
```json
{
  "contents": [{
    "parts": [
      { "text": "Transcribe this audio exactly as spoken. Return only the transcribed text, nothing else." },
      { "inline_data": { "mime_type": "audio/webm", "data": "<base64>" } }
    ]
  }]
}
```

**Response:** Extract `candidates[0].content.parts[0].text`

**Crate needed:** `reqwest` (already in Cargo.toml for telegram) + `base64` crate for encoding.

---

## Implementation Order

1. **Settings** (Rust struct + TS type + SettingsModal UI)
2. **Gemini API call** (Rust command, test with hardcoded audio)
3. **IPC wiring** (register command, add VoiceAPI to ipc.ts)
4. **Mic button UI** (SessionItem button + recording logic + styles)
5. **Integration test** (end-to-end: record -> transcribe -> PTY write)

---

## Edge Cases

- **No API key configured:** Button hidden (not disabled)
- **Recording in progress + session switch:** Stop recording, discard audio
- **Recording in progress + session closed:** Stop recording, discard audio
- **Gemini API error:** Show brief error in button tooltip or console, do not crash
- **Empty transcription:** Do nothing (no write to PTY)
- **Long audio:** MediaRecorder chunks - collect all before sending. No streaming for MVP
- **Multiple sessions recording simultaneously:** Only one session can record at a time. Starting a new one stops the previous

---

## Out of Scope (Future)

- Streaming transcription (real-time as user speaks)
- Alternative LLM providers (OpenAI Whisper, etc.) - architecture allows it later
- Voice commands (interpreting intent, not just transcribing)
- Auto-submit (adding Enter after transcription)
- Language selection (let Gemini auto-detect for now)
- Push-to-talk keyboard shortcut
