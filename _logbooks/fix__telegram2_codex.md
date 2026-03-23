# Logbook: fix/telegram2_codex

## Problem Statement

- Broken:
  El bridge con Telegram no estaba dejando una traza integral y el intento de activar Telegram desde la UI fallaba o quedaba sin feedback claro.
- Expected:
  1. Todo el tráfico terminal <-> PTY <-> bridge <-> Telegram y todo `last_prompt` debía quedar registrado.
  2. Si Telegram no podía activarse, la UI debía decir por qué.
- Observed:
  1. Existían logs parciales (`telegram-bridge.log`, `diag-raw.log`, `diag-sent.log`), pero no una auditoría completa de bytes/textos en todos los puntos de verdad.
  2. Con `telegramBots: []` el botón `T` del sidebar no daba feedback visible.

## Chronological Log

### 2026-03-23 - Initial inspection

- Change:
  Ninguno. Lectura de `src-tauri/src/telegram/*`, `src-tauri/src/pty/*`, `src-tauri/src/commands/*`, `src/shared/ipc.ts`, `src/terminal/components/TerminalView.tsx`, `CLAUDE.md`.
- How tested:
  Inspección de código y configuración local.
- Result:
  Confirmado que el bridge tenía logging parcial y truncado, y que no existía una bitácora integral del flujo.

### 2026-03-23 - Hypothesis 1

- Hypothesis:
  La mejor forma de dejar evidencia completa era auditar en los puntos de verdad del backend:
  `pty_write`, read loop del PTY, entrada desde Telegram, salida hacia Telegram y persistencia de `last_prompt`.

### 2026-03-23 - Add backend audit log

- Change:
  Se agregó `src-tauri/src/audit.rs` con escritura append-only en `~/.agentscommander-dev/audit-io.jsonl`.
  Se instrumentaron:
  - `src-tauri/src/commands/pty.rs`
  - `src-tauri/src/pty/manager.rs`
  - `src-tauri/src/telegram/bridge.rs`
  - `src-tauri/src/commands/session.rs`
  - `src-tauri/src/lib.rs`
- How tested:
  `rtk cargo fmt`
  `rtk cargo check`
- Result:
  Pass. Compiló correctamente.

### 2026-03-23 - Discovery 1

- Change:
  Ninguno. Inspección de `C:\Users\maria\.agentscommander-dev\settings.json`.
- How tested:
  Lectura del archivo de settings cargado por la app.
- Result:
  Se encontró `telegramBots: []`.
  Esto explica por qué el attach de Telegram no podía activarse en esta sesión de prueba.

### 2026-03-23 - Discovery 2

- Change:
  Ninguno. Inspección de `C:\Users\maria\.agentscommander-dev\audit-io.jsonl`.
- How tested:
  Lectura del archivo generado tras correr la app.
- Result:
  Pass. El archivo se estaba llenando con eventos `pty_output` y otras trazas de auditoría, confirmando que el logger nuevo estaba funcionando.

### 2026-03-23 - Hypothesis 2

- Hypothesis:
  El problema visible del botón `T` no era un fallo interno del bridge sino un caso UX silencioso: con cero bots configurados, la UI no informaba nada.

### 2026-03-23 - Add UI feedback for Telegram attach

- Change:
  Se actualizó `src/sidebar/components/SessionItem.tsx` para:
  - mostrar alerta si no hay bots configurados;
  - mostrar alerta si el attach o detach falla;
  - mantener selección normal si hay uno o varios bots.
- How tested:
  `rtk cargo check`
  `rtk npx tsc --noEmit`
- Result:
  Pass. Backend y frontend compilaron correctamente.

### 2026-03-23 - Run app for validation

- Change:
  Ninguno de código. Ejecución de la app en dev para validar comportamiento.
- How tested:
  `rtk npm run kill-dev`
  `rtk npm run tauri dev`
- Result:
  Pass. La app levantó correctamente en dev.

### 2026-03-23 - Negative result

- Change:
  Ninguno de código.
- How tested:
  Intento de probar activación de Telegram sin bots en settings.
- Result:
  Fail esperado. No podía activarse porque la configuración local no tenía bots (`telegramBots: []`).
  El hallazgo fue útil y cambió la hipótesis: no era solo un problema del bridge, también era un problema de feedback en UI.

## Evidence

- Main audit log:
  `C:\Users\maria\.agentscommander-dev\audit-io.jsonl`
- Existing bridge diagnostics:
  `C:\Users\maria\.agentscommander-dev\diag-raw.log`
  `C:\Users\maria\.agentscommander-dev\diag-sent.log`
  `C:\Users\maria\.agentscommander-dev\telegram-bridge.log`
- Effective runtime settings:
  `C:\Users\maria\.agentscommander-dev\settings.json`

## Current Status

- Done:
  Auditoría integral backend agregada.
- Done:
  Feedback explícito en UI cuando no hay bots o falla el attach.
- Done:
  Análisis comparativo entre screenshots de Telegram/terminal y `audit-io.jsonl`.
- Pending:
  Reproducir conversación real Telegram <-> terminal con un bot configurado y validar la bitácora completa extremo a extremo.

## Remaining Caveats

- El test funcional completo del bridge depende de tener al menos un bot configurado en `settings.json`.
- La auditoría nueva es de backend y archivo; todavía no existe una vista de logbook dentro de la app.

## Screenshot Analysis

### 2026-03-23 - Compare latest Greenshot captures with logs

- Evidence reviewed:
  - `C:\Users\maria\0_greenshot\2026-03-23 03_24_24-Terminal.png`
  - `C:\Users\maria\0_greenshot\2026-03-23 03_24_54-‎Amp-Issues – (10794).png`
  - `C:\Users\maria\.agentscommander-dev\audit-io.jsonl`
- Result:
  Las capturas y el log coinciden en lo esencial:
  1. `Qué modelo sos?` y `Repetilo` se respondieron correctamente.
  2. `Y vos quién sos?` produjo una respuesta larga de identidad/configuración del agente en el proyecto.
  3. Telegram recibió esa respuesta larga, no una invención del bridge.

### Aciertos

- Acierto:
  La auditoría nueva sí sirve para reconstruir exactamente lo que salió del PTY.
- Acierto:
  El problema no estaba en pérdida de evidencia; el log muestra la conversación relevante.
- Acierto:
  El bridge ya estaba reenviando contenido real de la terminal hacia Telegram.

### Errores / incorrect assumptions

- Error:
  Inicialmente enfoqué demasiado el problema como falta de logging. El caso mostrado por las capturas expone además un problema de calidad de filtrado/selección de contenido.
- Error:
  Asumí que el texto largo visto en Telegram podía ser “ruido” agregado por el bridge. La comparación con `audit-io.jsonl` muestra que ese texto ya estaba realmente en la terminal.
- Error:
  No separé desde el principio dos problemas distintos:
  1. observabilidad del flujo;
  2. criterio de qué líneas del PTY deberían salir a Telegram.

### Updated understanding

- El fix de auditoría fue correcto y útil, pero no resuelve por sí solo el comportamiento que muestran las capturas.
- El próximo fix probable debe ir sobre el bridge `PTY -> Telegram`:
  mejorar el filtro para que no reenvíe bloques de identidad/configuración o respuestas de contexto no deseadas, según la regla exacta que se quiera para Telegram.

### 2026-03-23 - Hypothesis 3

- Hypothesis:
  El bridge está dejando pasar tres clases de ruido que sí vale la pena atacar antes de otra prueba:
  1. fragmentos muy cortos producidos por wrap/redraw (`more`, `4`);
  2. líneas de thinking tipo `Architecting… (thought for 1s)`;
  3. bloque boilerplate de identidad/rol del proyecto (`Claude Code, la CLI oficial...`, `Senior Open Source Marketing Strategist`, etc.).

### 2026-03-23 - Tighten PTY -> Telegram filter

- Change:
  Se ajustó `src-tauri/src/telegram/bridge.rs` para:
  - bloquear fragmentos ASCII muy cortos;
  - bloquear líneas con `(thought for ...)`;
  - bloquear patrones de boilerplate del rol/proyecto detectados en las capturas y en `diag-sent.log`.
- How tested:
  `rtk cargo check`
- Result:
  Pass. Compila correctamente.

### 2026-03-23 - Ready for next app test

- Change:
  Ninguno adicional.
- How tested:
  Evaluación de readiness tras compilación.
- Result:
  Ahora sí hay un avance funcional nuevo que vale la pena reprobar en la app:
  el filtro de salida Telegram quedó más estricto específicamente contra los errores observados en la evidencia previa.
