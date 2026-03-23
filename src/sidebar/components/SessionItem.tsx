import { Component, createSignal, Show, For, onCleanup } from "solid-js";
import type { Session, SessionStatus, TelegramBotConfig } from "../../shared/types";
import { SessionAPI, TelegramAPI, SettingsAPI, WindowAPI, PtyAPI, VoiceAPI, DebugAPI } from "../../shared/ipc";
import { getConsoleText } from "../../shared/console-capture";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { bridgesStore } from "../stores/bridges";
import { settingsStore } from "../stores/settings";

// Global recording state: only one session can record at a time
const [recordingSessionId, setRecordingSessionId] = createSignal<string | null>(null);
const [processingSessionId, setProcessingSessionId] = createSignal<string | null>(null);
let globalStopFn: (() => void) | null = null;

function statusClass(status: SessionStatus): string {
  if (typeof status === "string") return status;
  return "exited";
}

const SessionItem: Component<{
  session: Session;
  isActive: boolean;
}> = (props) => {
  const [editing, setEditing] = createSignal(false);
  const [editValue, setEditValue] = createSignal("");
  const [showBotMenu, setShowBotMenu] = createSignal(false);
  const [availableBots, setAvailableBots] = createSignal<TelegramBotConfig[]>([]);
  const [micError, setMicError] = createSignal<string | null>(null);
  const [recordingSeconds, setRecordingSeconds] = createSignal(0);
  const [audioLevel, setAudioLevel] = createSignal(0);
  let inputRef!: HTMLInputElement;
  let recordingTimer: ReturnType<typeof setInterval> | null = null;
  let levelTimer: ReturnType<typeof setInterval> | null = null;

  // Per-instance recording state (not shared across SessionItem instances)
  let localRecorder: MediaRecorder | null = null;
  let localAudioCtx: AudioContext | null = null;
  let localAnalyser: AnalyserNode | null = null;
  let localChunks: Blob[] = [];
  let localMimeType = "";

  const bridge = () => bridgesStore.getBridge(props.session.id);
  const isRecording = () => recordingSessionId() === props.session.id;
  const isProcessing = () => processingSessionId() === props.session.id;

  onCleanup(() => {
    if (recordingTimer) clearInterval(recordingTimer);
    if (levelTimer) clearInterval(levelTimer);
  });

  const startAudioLevelMonitor = (stream: MediaStream) => {
    try {
      localAudioCtx = new AudioContext();
      localAnalyser = localAudioCtx.createAnalyser();
      localAnalyser.fftSize = 256;
      const source = localAudioCtx.createMediaStreamSource(stream);
      source.connect(localAnalyser);
      const dataArray = new Uint8Array(localAnalyser.frequencyBinCount);

      levelTimer = setInterval(() => {
        if (!localAnalyser) return;
        localAnalyser.getByteFrequencyData(dataArray);
        const sum = dataArray.reduce((a, b) => a + b, 0);
        const avg = sum / dataArray.length / 255;
        setAudioLevel(avg);
      }, 50);
    } catch {
      // Audio context not available
    }
  };

  const stopAudioLevelMonitor = () => {
    if (levelTimer) {
      clearInterval(levelTimer);
      levelTimer = null;
    }
    if (localAudioCtx) {
      localAudioCtx.close().catch(() => {});
      localAudioCtx = null;
      localAnalyser = null;
    }
    setAudioLevel(0);
  };

  const stopRecording = () => {
    if (localRecorder && localRecorder.state !== "inactive") {
      localRecorder.stop();
    }
  };

  const startRecording = async () => {
    // Stop any other session's recording first
    if (recordingSessionId() && globalStopFn) {
      globalStopFn();
    }

    setMicError(null);
    setRecordingSeconds(0);

    try {
      console.log("[Voice] Requesting microphone access...");
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      console.log("[Voice] Microphone access granted, tracks:", stream.getAudioTracks().length);
      localChunks = [];

      const mimeType = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
        ? "audio/webm;codecs=opus"
        : undefined;
      console.log("[Voice] MediaRecorder mimeType:", mimeType || "default");
      const recorder = new MediaRecorder(stream, mimeType ? { mimeType } : undefined);
      localRecorder = recorder;
      localMimeType = recorder.mimeType || "audio/webm";
      setRecordingSessionId(props.session.id);
      globalStopFn = stopRecording;

      recordingTimer = setInterval(() => {
        setRecordingSeconds((s) => s + 1);
      }, 1000);

      startAudioLevelMonitor(stream);

      recorder.ondataavailable = (e) => {
        if (e.data.size > 0) localChunks.push(e.data);
      };

      recorder.onerror = (e) => {
        console.error("[Voice] MediaRecorder error:", e);
      };

      recorder.onstop = async () => {
        console.log("[Voice] Recording stopped, chunks:", localChunks.length);
        stream.getTracks().forEach((t) => t.stop());

        if (recordingTimer) {
          clearInterval(recordingTimer);
          recordingTimer = null;
        }
        stopAudioLevelMonitor();

        setRecordingSessionId(null);
        localRecorder = null;
        globalStopFn = null;

        if (localChunks.length === 0) return;

        setProcessingSessionId(props.session.id);

        const blob = new Blob(localChunks, { type: localMimeType });
        console.log("[Voice] Audio blob size:", blob.size, "type:", blob.type);
        const buffer = await blob.arrayBuffer();
        const bytes = Array.from(new Uint8Array(buffer));
        console.log("[Voice] Sending", bytes.length, "bytes to Gemini...");

        try {
          const text = await VoiceAPI.transcribe(bytes, localMimeType);
          console.log("[Voice] Transcription result:", text);
          if (text) {
            const encoder = new TextEncoder();
            await PtyAPI.write(props.session.id, encoder.encode(text));
            console.log("[Voice] Text written to PTY");
          }
        } catch (err: any) {
          const msg = typeof err === "string" ? err : err?.message || "Transcription failed";
          console.error("[Voice] Transcription failed:", msg);
          setMicError(msg);
          DebugAPI.saveLogs(getConsoleText()).catch(() => {});
          setTimeout(() => setMicError(null), 5000);
        } finally {
          setProcessingSessionId(null);
        }
      };

      console.log("[Voice] Recording started");
      recorder.start();
    } catch (err: any) {
      const msg = err?.message || err?.name || "Microphone access failed";
      console.error("[Voice] Microphone access failed:", msg, err);
      setMicError(msg);
      DebugAPI.saveLogs(getConsoleText()).catch(() => {});
      setRecordingSessionId(null);
      globalStopFn = null;
      if (recordingTimer) {
        clearInterval(recordingTimer);
        recordingTimer = null;
      }
      setTimeout(() => setMicError(null), 5000);
    }
  };

  const handleMicClick = (e: MouseEvent) => {
    e.stopPropagation();
    if (isProcessing()) return;
    if (isRecording()) {
      stopRecording();
    } else {
      void startRecording();
    }
  };

  const handleTelegramClick = async (e: MouseEvent) => {
    e.stopPropagation();
    const b = bridge();
    if (b) {
      await TelegramAPI.detach(props.session.id);
    } else {
      const settings = await SettingsAPI.get();
      const bots = settings.telegramBots || [];
      if (bots.length === 1) {
        await TelegramAPI.attach(props.session.id, bots[0].id);
      } else if (bots.length > 1) {
        setAvailableBots(bots);
        setShowBotMenu(true);
      }
    }
  };

  const handleBotSelect = async (botId: string) => {
    setShowBotMenu(false);
    await TelegramAPI.attach(props.session.id, botId);
  };

  const handleClick = async () => {
    if (!editing()) {
      await SessionAPI.switch(props.session.id);
      const detachedLabel = `terminal-${props.session.id.replace(/-/g, "")}`;
      const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
      if (!detachedWin) {
        (await WebviewWindow.getByLabel("terminal"))?.setFocus();
      }
    }
  };

  const handleDoubleClick = (e: MouseEvent) => {
    e.stopPropagation();
    setEditValue(props.session.name);
    setEditing(true);
    requestAnimationFrame(() => {
      inputRef?.focus();
      inputRef?.select();
    });
  };

  const confirmRename = () => {
    const val = editValue().trim();
    if (val && val !== props.session.name) {
      SessionAPI.rename(props.session.id, val);
    }
    setEditing(false);
  };

  const cancelRename = () => {
    setEditing(false);
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      confirmRename();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelRename();
    }
  };

  const handleDetach = (e: MouseEvent) => {
    e.stopPropagation();
    WindowAPI.detach(props.session.id);
  };

  const handleClose = (e: MouseEvent) => {
    e.stopPropagation();
    SessionAPI.destroy(props.session.id);
  };

  const formatTime = (s: number) => {
    const m = Math.floor(s / 60);
    const sec = s % 60;
    return `${m}:${sec.toString().padStart(2, "0")}`;
  };

  return (
    <div
      class={`session-item session-item-enter ${props.isActive ? "active" : ""}`}
      onClick={handleClick}
    >
      <div
        class={`session-item-status ${props.session.waitingForInput ? "waiting" : statusClass(props.session.status)}`}
      />
      <div class="session-item-info">
        <Show
          when={editing()}
          fallback={
            <div class="session-item-name" onDblClick={handleDoubleClick}>
              {props.session.name}
            </div>
          }
        >
          <input
            ref={inputRef!}
            class="session-item-rename-input"
            value={editValue()}
            onInput={(e) => setEditValue(e.currentTarget.value)}
            onKeyDown={handleKeyDown}
            onBlur={confirmRename}
            maxLength={50}
            onClick={(e) => e.stopPropagation()}
          />
        </Show>

        <Show when={isRecording()}>
          <div class="session-item-voice-indicator recording">
            <div class="voice-dot" />
            <div class="voice-level-bar">
              <div
                class="voice-level-fill"
                style={{ width: `${Math.min(audioLevel() * 100 * 2.5, 100)}%` }}
              />
            </div>
            <span class="voice-time">{formatTime(recordingSeconds())}</span>
          </div>
        </Show>

        <Show when={isProcessing()}>
          <div class="session-item-voice-indicator processing">
            <div class="voice-spinner" />
            <span class="voice-processing-text">Transcribing...</span>
          </div>
        </Show>

        <Show when={micError()}>
          <div class="session-item-voice-indicator error">
            <span class="voice-error-text">{micError()}</span>
          </div>
        </Show>

        <Show when={!isRecording() && !isProcessing() && !micError()}>
          <Show when={props.session.gitBranch}>
            <div class="session-item-branch" title={props.session.gitBranch!}>
              {props.session.gitBranch}
            </div>
          </Show>
          <div class="session-item-shell">{props.session.shell}</div>
        </Show>
      </div>
      <Show when={settingsStore.voiceEnabled}>
        <button
          class={`session-item-mic ${isRecording() ? "recording" : ""} ${isProcessing() ? "processing" : ""} ${micError() ? "error" : ""}`}
          onClick={handleMicClick}
          title={isRecording() ? "Stop recording" : isProcessing() ? "Transcribing..." : micError() ? micError()! : "Voice to text"}
        >
          &#x1F399;
        </button>
      </Show>
      <button
        class="session-item-detach"
        onClick={handleDetach}
        title="Detach to own window"
      >
        &#x29C9;
      </button>
      <Show when={bridge()}>
        <div
          class="session-item-bridge-dot"
          style={{ background: bridge()!.color }}
          title={`Telegram: ${bridge()!.botLabel}`}
        />
      </Show>
      <button
        class={`session-item-telegram ${bridge() ? "active" : ""}`}
        onClick={handleTelegramClick}
        title={bridge() ? "Detach Telegram" : "Attach Telegram"}
        style={bridge() ? { color: bridge()!.color } : {}}
      >
        T
      </button>
      <Show when={showBotMenu()}>
        <div class="session-item-bot-menu" onClick={(e) => e.stopPropagation()}>
          <For each={availableBots()}>
            {(bot) => (
              <button
                class="session-item-bot-option"
                onClick={() => handleBotSelect(bot.id)}
              >
                <span class="settings-color-dot" style={{ background: bot.color }} />
                {bot.label}
              </button>
            )}
          </For>
        </div>
      </Show>
      <button class="session-item-close" onClick={handleClose} title="Close session">
        &#x2715;
      </button>
    </div>
  );
};

export default SessionItem;
