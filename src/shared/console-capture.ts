const MAX_ENTRIES = 500;

interface LogEntry {
  ts: string;
  level: "log" | "warn" | "error" | "info";
  args: string;
}

const captured: LogEntry[] = [];

function timestamp(): string {
  const d = new Date();
  return d.toLocaleTimeString("en-GB", { hour12: false }) + "." + String(d.getMilliseconds()).padStart(3, "0");
}

function serialize(args: unknown[]): string {
  return args
    .map((a) => {
      if (typeof a === "string") return a;
      try {
        return JSON.stringify(a, null, 2);
      } catch {
        return String(a);
      }
    })
    .join(" ");
}

function capture(level: LogEntry["level"], origFn: (...args: unknown[]) => void, args: unknown[]) {
  const entry: LogEntry = { ts: timestamp(), level, args: serialize(args) };
  captured.push(entry);
  if (captured.length > MAX_ENTRIES) captured.shift();
  origFn.apply(console, args);
}

const origLog = console.log;
const origWarn = console.warn;
const origError = console.error;
const origInfo = console.info;

console.log = (...args: unknown[]) => capture("log", origLog, args);
console.warn = (...args: unknown[]) => capture("warn", origWarn, args);
console.error = (...args: unknown[]) => capture("error", origError, args);
console.info = (...args: unknown[]) => capture("info", origInfo, args);

// Also capture unhandled errors and promise rejections
window.addEventListener("error", (e) => {
  const msg = `${e.message} at ${e.filename}:${e.lineno}:${e.colno}`;
  captured.push({ ts: timestamp(), level: "error", args: msg });
});

window.addEventListener("unhandledrejection", (e) => {
  const msg = `Unhandled rejection: ${e.reason}`;
  captured.push({ ts: timestamp(), level: "error", args: msg });
});

export function getConsoleLogs(): LogEntry[] {
  return [...captured];
}

export function getConsoleText(): string {
  return captured
    .map((e) => `[${e.ts}] [${e.level.toUpperCase()}] ${e.args}`)
    .join("\n");
}

export function getErrorsOnly(): string {
  return captured
    .filter((e) => e.level === "error" || e.level === "warn")
    .map((e) => `[${e.ts}] [${e.level.toUpperCase()}] ${e.args}`)
    .join("\n");
}

export async function copyConsoleLogs(): Promise<number> {
  const text = getConsoleText();
  await navigator.clipboard.writeText(text);
  return captured.length;
}

export async function copyErrors(): Promise<number> {
  const errors = captured.filter((e) => e.level === "error" || e.level === "warn");
  const text = getErrorsOnly();
  await navigator.clipboard.writeText(text);
  return errors.length;
}
