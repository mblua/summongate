# Diagnostico: Emision de mensajes AgentsCommander a Telegram

**Fecha:** 2026-03-21
**Branch:** `fix/telegram`
**Estado:** En progreso - solucion v2 (vt100) desplegada, pendiente de validacion

---

## Problema

La emision de mensajes desde AgentsCommander hacia Telegram llega corrupta. Los mensajes que se ven limpios en la terminal de Claude Code llegan a Telegram con:

1. Texto sin espacios ("HolaMariano,teescucho" en vez de "Hola Mariano, te escucho")
2. Caracteres de spinner mezclados con contenido real
3. Chrome del TUI (status bar, tips, progress bars) filtrandose
4. Duplicacion masiva de contenido

---

## Causa raiz

El bridge usaba `strip_ansi_escapes::strip()` para limpiar la salida del PTY antes de enviarla a Telegram. Esta libreria **solo elimina secuencias ANSI escape** pero **no simula el terminal**. Cuando el TUI de Claude Code redibuja la pantalla (cursor movement, line clearing, etc.), al stripear los escapes:

- Los cursor movements se pierden, y el texto de distintas posiciones de pantalla queda concatenado
- Los espacios que el terminal renderiza via posicionamiento de cursor desaparecen
- Los spinner frames se intercalan con el contenido real en una sola linea

### Ejemplo concreto

**Lo que Claude Code muestra en pantalla:**
```
Soy Claude Opus 4.6 (claude-opus-4-6), el modelo mas capaz de Anthropic.
Antes de arrancar, dejame hacer el pull inicial.
```

**Lo que llegaba a Telegram (v1 - strip_ansi_escapes):**
```
✢B·reBwrein✢wgin…*g…
●Repo al día.HolaMariano,teescucho.Enquéandamoshoy?✻ Brewing…
```

---

## Diagnostico realizado

### Herramienta de diagnostico

Se agrego un `DiagLogger` en `bridge.rs` que escribe dos archivos sin truncar:

- `~/.agentscommander/diag-raw.log` - Todo lo que llega del PTY (post procesamiento, pre filtro)
- `~/.agentscommander/diag-sent.log` - Todo lo que efectivamente se envia a Telegram

Ambos con timestamp por entrada. Se truncan al iniciar un nuevo bridge para tener capturas limpias.

### Log de la primera prueba (v1 - solo filtro mejorado)

Archivo: `diag-sent.log` capturado 2026-03-21 ~09:45 UTC

```
--- [09:46:01.874] ---
Q u è
--- [09:46:03.653] ---
m o d e l o s o s ?
```
**Problema 1:** Echo caracter por caracter. Cada keystroke llega como chunk separado con espacios.

```
--- [09:46:07.373] ---
✽✻✶Pon*Ptoi✢ntfiic·faictian✢tgi…*ng…✶✻cat✽it✻✶fa*ic✢·ti✢nf*oi✶✻Pt✽n✻✶o*P●Soy Claude Opus 4.6(claude-opus-4-6),elmodelomáscapazdeAnthropic.
```
**Problema 2:** Spinner chars intercalados + espacios perdidos en el contenido real.

```
--- [09:46:16.342] ---
✻Pontificating…✽Pontificating…Pontificating…Pontificating…✻Pontificating…Pontificating…
✶Pontificating…Pontificating…*Pontificating…Pontificating…✢Pontificating…·Pontificating…
[... cientos de repeticiones ...]
```
**Problema 3:** Bloques masivos de thinking noise pasando el filtro.

```
--- [09:46:25.590] ---
i…Pontificating…(running stop hook)*Pontificating…✶Pontificating…✻Pontificating…
```
**Problema 4:** Hook notifications mezcladas con spinners.

### Log de la segunda prueba (v2 - filtro mejorado, sin vt100)

Archivo: `diag-sent.log` capturado 2026-03-21 ~10:18 UTC

Se agrego "Brewing" y "Noodling" al filtro, pero aparecieron nuevos verbs no listados. Claude Code **randomiza** el verbo de thinking en cada sesion, haciendo imposible mantener una lista exhaustiva:

```
--- [10:18:21.992] ---
✢Bre·Bwriewng✢i…ng*…✻Brewing… ✽Brewing… ✽Brewing… [...]
--- [10:18:41.813] ---
●Repo al día.HolaMariano,teescucho.Enquéandamoshoy?✻ Brewing…
```

Los mismos problemas de v1 persisten porque la causa raiz (strip_ansi_escapes) no cambio.

---

## Solucion implementada (v3 - vt100)

### Cambio arquitectural

Reemplazo de `strip_ansi_escapes::strip()` por el crate `vt100` que implementa un **emulador de terminal virtual completo**.

**Antes (roto):**
```
PTY bytes --> strip_ansi_escapes::strip() --> texto concatenado sin espacios --> filtro --> Telegram
```

**Despues (v3):**
```
PTY bytes --> vt100::Parser (terminal virtual 50x220) --> screen rows con espacios correctos --> diff vs estado anterior --> filtro --> Telegram
```

### Cambios en archivos

**`src-tauri/Cargo.toml`:**
- Agregado: `vt100 = "0.15"`

**`src-tauri/src/telegram/bridge.rs`:**

1. **output_task**: Mantiene un `vt100::Parser` persistente. Cada chunk de bytes se procesa a traves del parser. Se leen las filas de pantalla con `screen.contents_between()` y se comparan contra el estado anterior via `HashSet` para extraer solo filas nuevas.

2. **clean_terminal_output**: Simplificado. Con vt100, cada linea es una fila de terminal limpia. El filtro ahora:
   - Detecta thinking lines por **patron** (`Capitalized_word + "..."/"..."`) en vez de lista de verbos. Esto es future-proof contra nuevos verbos random.
   - Filtra TUI chrome via patterns (bypass permissions, Context/Usage bars, tips, etc.)
   - Filtra ASCII art logo, box-drawing lines, Braille spinners, hook notifications
   - Umbral alphanumeric al 30%

3. **DiagLogger**: Se mantiene para comparacion continua.

### Detalle del diffing de pantalla

```rust
// Virtual terminal: properly renders cursor movement, overwrites, etc.
let mut vt = vt100::Parser::new(50, 220, 0);
let mut prev_rows: Vec<String> = Vec::new();

// On each chunk:
vt.process(&data);
let screen = vt.screen();
let mut current_rows: Vec<String> = Vec::new();
for row in 0..screen.size().0 {
    let row_text = screen.contents_between(row, 0, row, screen.size().1);
    current_rows.push(row_text.trim_end().to_string());
}

// Find new rows not in previous screen state
let prev_set: HashSet<&str> = prev_rows.iter().map(|s| s.as_str()).collect();
let new_lines: Vec<&str> = current_rows.iter()
    .filter(|line| !line.trim().is_empty() && !prev_set.contains(line.as_str()))
    .collect();
```

### Deteccion de thinking lines (pattern-based)

```rust
fn is_thinking_line(s: &str) -> bool {
    // Strip optional leading spinner char
    let check = if CLAUDE_SPINNERS.contains(&first_char) {
        s[first_char.len_utf8()..].trim()
    } else { s };

    // "Word..." or "Word..." where Word is a single capitalized word
    if check.ends_with('...') || check.ends_with("...") {
        let word_part = check.trim_end_matches('...').trim_end_matches("...");
        word_part.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
            && word_part.chars().all(|c| c.is_alphabetic())
    }
}
```

---

## v3 Results (FAILED)

La v3 con vt100 + HashSet diff tambien fallo. Analisis del `diag-sent.log`:

1. **Streaming char-by-char**: Cada caracter que Claude escribe cambia el contenido de la fila. HashSet ve "nuevo string" y lo emite. Resultado: ~60 mensajes a Telegram para una sola oracion.
2. **Spinners con trailing garbage**: `✢ Slithering...s.` - contenido residual de posiciones adyacentes en el screen rompe el pattern matching de `is_thinking_line()`.
3. **Box-drawing pegado al spinner**: `Honking...    ──────────` en la misma fila pasa el filtro.

---

## Solucion implementada (v4 - RowTracker + stabilization)

### Cambio arquitectural

Reemplazo del HashSet diff por un **RowTracker con stabilization timer per-row**. La idea clave: no emitir cuando una fila cambia, sino cuando DEJA de cambiar.

**Antes (v3, roto):**
```
PTY bytes -> vt100 -> HashSet diff (emite cada cambio) -> filtro -> Telegram
```

**Despues (v4):**
```
PTY bytes -> vt100 -> RowTracker por posicion (emite solo filas estables 800ms+) -> AgentFilter -> Telegram
```

### Por que funciona

| Tipo de ruido | Comportamiento | Con stabilization 800ms |
|---|---|---|
| Spinner (`✢ Honking...`) | Cambia cada ~450ms | NUNCA estabiliza, nunca se emite |
| Streaming char-by-char | Cambia cada ~50ms mientras escribe | Solo la linea final completa se emite |
| TUI chrome (status bar) | Estatico | Se emite pero AgentFilter lo descarta |
| Contenido real | Se escribe, para, queda estable | Se emite limpio despues de 800ms |

### Componentes nuevos

1. **RowTracker**: Trackea cada fila del screen vt100 por posicion. Cada fila tiene `content`, `last_changed`, `emitted`. El metodo `harvest_stable()` emite filas que llevan >800ms sin cambiar y no fueron emitidas antes.

2. **Scroll dedup**: `emitted_content: HashSet<String>` previene re-emision cuando el scroll mueve contenido de una fila a otra. Se limpia automaticamente al superar 5000 entries.

3. **AgentFilter trait**: Interface pluggable para filtros por agente. Implementado `ClaudeCodeFilter` con todos los patrones de TUI chrome. Preparado para `CodexFilter`, `AiderFilter`, etc.

4. **`is_thinking_line()` como defense in depth**: Se mantiene dentro del ClaudeCodeFilter como safety net, pero el mecanismo primario es stabilization (el spinner nunca llega al filtro).

### Constantes configurables

```rust
const VT_ROWS: u16 = 50;        // Filas del terminal virtual
const VT_COLS: u16 = 220;       // Columnas del terminal virtual
const STABILIZATION_MS: u64 = 800;  // Tiempo que una fila debe estar estable
const TICK_MS: u64 = 200;       // Intervalo de harvesting
const FLUSH_DELAY_MS: u64 = 500;    // Delay antes de flush a Telegram
```

### Latencia esperada

Worst case: 800ms (stabilization) + 200ms (tick) + 500ms (flush) = **1500ms** desde que el contenido aparece hasta que llega a Telegram. Aceptable para Telegram.

### Decisiones tomadas (sin consultar)

1. **Stabilization a 800ms**: Claude Code cicla spinners cada ~450ms. 800ms es casi 2x el ciclo, garantiza que spinners nunca estabilicen. Si resulta muy lento para el usuario, se puede bajar a 600ms (1.33x el ciclo - mas riesgoso).

2. **Tick a 200ms**: Balance entre responsividad y uso de CPU. Mas bajo = mas responsive pero mas wake-ups. 200ms es negligible.

3. **Mantuve is_thinking_line()**: Como defense-in-depth dentro del filtro. Si stabilization falla por algun edge case (spinner pausado >800ms), el filtro lo atrapa.

4. **HashSet de contenido emitido con limite 5000**: Previene re-emision por scroll. Se limpia al pasar 5000 entries para no crecer infinitamente. 5000 strings es ~100KB, negligible.

5. **AgentFilter como trait + Box<dyn>**: Permite agregar CodexFilter, etc. sin cambiar la estructura del output_task. Por ahora hardcodeado a ClaudeCodeFilter.

---

## Estado actual

- **v4 (RowTracker + stabilization) desplegada** en branch `fix/telegram`, pendiente de validacion
- Los archivos de diagnostico se siguen generando:
  - `~/.agentscommander/diag-raw.log` - filas post-stabilization, pre-filtro
  - `~/.agentscommander/diag-sent.log` - lo que se envia a Telegram
- Compila sin errores, clippy sin warnings nuevos

## Proximos pasos

1. Validar con `diag-sent.log` que el contenido llega limpio a Telegram
2. Ajustar STABILIZATION_MS si la latencia es demasiado alta o si spinners pasan
3. Si OK, remover el DiagLogger (o dejarlo como opt-in)
4. Implementar deteccion automatica del agente (por ahora hardcoded a Claude Code)
5. Merge a main
