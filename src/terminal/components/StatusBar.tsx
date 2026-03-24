import { Component, Show, onCleanup } from "solid-js";
import { terminalStore } from "../stores/terminal";
import { settingsStore } from "../../shared/stores/settings";
import { voiceRecorder, formatRecordingTime } from "../../shared/voice-recorder";
import { PtyAPI } from "../../shared/ipc";

const StatusBar: Component<{ detached?: boolean }> = (props) => {
  let mouseUpHandler: (() => void) | null = null;

  const isRecording = () => !!voiceRecorder.recordingSessionId();
  const isProcessing = () => !!voiceRecorder.processingSessionId();

  const handleMicDown = (e: MouseEvent) => {
    e.preventDefault();
    const sessionId = terminalStore.activeSessionId;
    if (!sessionId || isProcessing()) return;

    void voiceRecorder.start(sessionId);

    // Use document mouseup so release works anywhere on screen
    mouseUpHandler = () => {
      voiceRecorder.stop();
      cleanup();
    };
    document.addEventListener("mouseup", mouseUpHandler);
  };

  const cleanup = () => {
    if (mouseUpHandler) {
      document.removeEventListener("mouseup", mouseUpHandler);
      mouseUpHandler = null;
    }
  };

  onCleanup(cleanup);

  const handleClearInput = () => {
    const sessionId = terminalStore.activeSessionId;
    if (!sessionId) return;
    // Ctrl+U: kills line backward in readline/bash/most coding agents
    const encoder = new TextEncoder();
    PtyAPI.write(sessionId, encoder.encode("\x15"));
  };

  return (
    <div class="status-bar">
      <div class="status-bar-left">
        <Show when={props.detached}>
          <div class="status-bar-item">
            <span class="status-bar-detached">DETACHED</span>
          </div>
        </Show>
        <Show when={terminalStore.activeShell}>
          <div class="status-bar-item">
            <span class="status-bar-accent">{terminalStore.activeShell}</span>
          </div>
        </Show>
        <Show when={terminalStore.termSize.cols > 0}>
          <div class="status-bar-item">
            {terminalStore.termSize.cols}x{terminalStore.termSize.rows}
          </div>
        </Show>
        <Show when={isRecording()}>
          <div class="status-bar-item status-bar-recording">
            <span class="status-bar-rec-dot" />
            <span>{formatRecordingTime(voiceRecorder.recordingSeconds())}</span>
          </div>
        </Show>
        <Show when={isProcessing()}>
          <div class="status-bar-item status-bar-processing">
            <span class="status-bar-spinner" />
            <span>Transcribing...</span>
          </div>
        </Show>
        <Show when={voiceRecorder.micError()}>
          <div class="status-bar-item status-bar-error">
            {voiceRecorder.micError()}
          </div>
        </Show>
      </div>
      <Show when={terminalStore.activeSessionId}>
        <div class="status-bar-actions">
          <Show when={settingsStore.voiceEnabled}>
            <button
              class={`status-bar-btn status-bar-btn-mic ${isRecording() ? "recording" : ""} ${isProcessing() ? "processing" : ""}`}
              onMouseDown={handleMicDown}
              title={isRecording() ? "Release to stop" : isProcessing() ? "Transcribing..." : "Hold to record (Ctrl+Shift+R)"}
              disabled={isProcessing()}
            >
              &#x1F399;
            </button>
          </Show>
          <button
            class="status-bar-btn status-bar-btn-clear"
            onClick={handleClearInput}
            title="Clear agent input (Ctrl+U)"
          >
            &#x232B;
          </button>
        </div>
      </Show>
    </div>
  );
};

export default StatusBar;
