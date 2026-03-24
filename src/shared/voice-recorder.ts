import { createSignal } from "solid-js";
import { VoiceAPI, PtyAPI, DebugAPI } from "./ipc";
import { getConsoleText } from "./console-capture";

// Module-level reactive state (one instance per JS context / window)
const [recordingSessionId, setRecordingSessionId] = createSignal<string | null>(null);
const [processingSessionId, setProcessingSessionId] = createSignal<string | null>(null);
const [micError, setMicError] = createSignal<string | null>(null);
const [recordingSeconds, setRecordingSeconds] = createSignal(0);
const [audioLevel, setAudioLevel] = createSignal(0);

// Internal state (not reactive, not exported)
let recorder: MediaRecorder | null = null;
let audioCtx: AudioContext | null = null;
let analyser: AnalyserNode | null = null;
let chunks: Blob[] = [];
let mimeType = "";
let recordingTimer: ReturnType<typeof setInterval> | null = null;
let levelTimer: ReturnType<typeof setInterval> | null = null;

function startAudioLevelMonitor(stream: MediaStream) {
  try {
    audioCtx = new AudioContext();
    analyser = audioCtx.createAnalyser();
    analyser.fftSize = 256;
    const source = audioCtx.createMediaStreamSource(stream);
    source.connect(analyser);
    const dataArray = new Uint8Array(analyser.frequencyBinCount);

    levelTimer = setInterval(() => {
      if (!analyser) return;
      analyser.getByteFrequencyData(dataArray);
      const sum = dataArray.reduce((a, b) => a + b, 0);
      const avg = sum / dataArray.length / 255;
      setAudioLevel(avg);
    }, 50);
  } catch {
    // Audio context not available
  }
}

function stopAudioLevelMonitor() {
  if (levelTimer) {
    clearInterval(levelTimer);
    levelTimer = null;
  }
  if (audioCtx) {
    audioCtx.close().catch(() => {});
    audioCtx = null;
    analyser = null;
  }
  setAudioLevel(0);
}

function clearTimers() {
  if (recordingTimer) {
    clearInterval(recordingTimer);
    recordingTimer = null;
  }
  stopAudioLevelMonitor();
}

async function start(sessionId: string) {
  // Stop any existing recording first
  if (recordingSessionId()) {
    stop();
    // Wait a tick for onstop to fire
    await new Promise((r) => setTimeout(r, 50));
  }

  setMicError(null);
  setRecordingSeconds(0);

  try {
    console.log("[Voice] Requesting microphone access...");
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    console.log("[Voice] Microphone access granted, tracks:", stream.getAudioTracks().length);
    chunks = [];

    const preferredMime = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
      ? "audio/webm;codecs=opus"
      : undefined;
    console.log("[Voice] MediaRecorder mimeType:", preferredMime || "default");
    const rec = new MediaRecorder(stream, preferredMime ? { mimeType: preferredMime } : undefined);
    recorder = rec;
    mimeType = rec.mimeType || "audio/webm";
    setRecordingSessionId(sessionId);

    recordingTimer = setInterval(() => {
      setRecordingSeconds((s) => s + 1);
    }, 1000);

    startAudioLevelMonitor(stream);

    rec.ondataavailable = (e) => {
      if (e.data.size > 0) chunks.push(e.data);
    };

    rec.onerror = (e) => {
      console.error("[Voice] MediaRecorder error:", e);
    };

    rec.onstop = async () => {
      console.log("[Voice] Recording stopped, chunks:", chunks.length);
      stream.getTracks().forEach((t) => t.stop());
      clearTimers();

      const stoppedSessionId = recordingSessionId();
      setRecordingSessionId(null);
      recorder = null;

      if (chunks.length === 0 || !stoppedSessionId) return;

      setProcessingSessionId(stoppedSessionId);

      const blob = new Blob(chunks, { type: mimeType });
      console.log("[Voice] Audio blob size:", blob.size, "type:", blob.type);
      const buffer = await blob.arrayBuffer();
      const bytes = Array.from(new Uint8Array(buffer));
      console.log("[Voice] Sending", bytes.length, "bytes to Gemini...");

      try {
        const text = await VoiceAPI.transcribe(bytes, mimeType);
        console.log("[Voice] Transcription result:", text);
        if (text) {
          const encoder = new TextEncoder();
          await PtyAPI.write(stoppedSessionId, encoder.encode(text));
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
    rec.start();
  } catch (err: any) {
    const msg = err?.message || err?.name || "Microphone access failed";
    console.error("[Voice] Microphone access failed:", msg, err);
    setMicError(msg);
    DebugAPI.saveLogs(getConsoleText()).catch(() => {});
    setRecordingSessionId(null);
    recorder = null;
    clearTimers();
    setTimeout(() => setMicError(null), 5000);
  }
}

function stop() {
  if (recorder && recorder.state !== "inactive") {
    recorder.stop();
  }
}

function toggle(sessionId: string) {
  if (processingSessionId()) return;
  if (recordingSessionId()) {
    stop();
  } else {
    void start(sessionId);
  }
}

export function formatRecordingTime(s: number): string {
  const m = Math.floor(s / 60);
  const sec = s % 60;
  return `${m}:${sec.toString().padStart(2, "0")}`;
}

export const voiceRecorder = {
  // State (reactive signals)
  recordingSessionId,
  processingSessionId,
  micError,
  recordingSeconds,
  audioLevel,

  // Actions
  start,
  stop,
  toggle,
};
