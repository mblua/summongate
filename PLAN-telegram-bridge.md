# Plan: Telegram Bridge para termgate

## Contexto

Agregar un bridge bidireccional entre sesiones de terminal y Telegram. El PTY read loop ya captura todo el output — lo reutilizamos para enviar a Telegram. Los mensajes de Telegram se inyectan en el PTY stdin. Referencia: `amp-backend/crates/telegram-dispatcher` (solo como referencia, no dependencia).

---

## Data Flow

```
PTY stdout → read loop → clone bytes → mpsc channel → Bridge Output Task
                ↓                                          ↓
          emit pty_output                          strip ANSI, buffer lines
          (a xterm.js, sin cambios)                throttle 35ms entre envíos
                                                   → Telegram sendMessage API

Telegram getUpdates → Bridge Poll Task → pty_write() → PTY stdin
                                       → emit telegram_incoming (UI indicator)
```

---

## Estructura de Archivos

### Nuevos
| Archivo | Propósito |
|---|---|
| `src-tauri/src/telegram/mod.rs` | Declaración de módulos |
| `src-tauri/src/telegram/types.rs` | TelegramBotConfig, BridgeInfo, BridgeStatus, RepoConfig |
| `src-tauri/src/telegram/api.rs` | `get_updates()`, `send_message()` (standalone, reqwest) |
| `src-tauri/src/telegram/bridge.rs` | Output task + poll task + ANSI strip + buffering |
| `src-tauri/src/telegram/manager.rs` | TelegramBridgeManager: attach/detach/list, output sender map |
| `src-tauri/src/commands/telegram.rs` | Tauri commands |
| `src/sidebar/stores/bridges.ts` | SolidJS store para estado de bridges |

### Modificados
| Archivo | Cambios |
|---|---|
| `src-tauri/Cargo.toml` | +reqwest (json), +strip-ansi-escapes, +tokio-util (CancellationToken) |
| `src-tauri/src/lib.rs` | Registrar módulo telegram, manage states, register commands |
| `src-tauri/src/commands/mod.rs` | `pub mod telegram;` |
| `src-tauri/src/config/settings.rs` | `telegram_bots: Vec<TelegramBotConfig>` con `#[serde(default)]` |
| `src-tauri/src/pty/manager.rs` | Read loop: check output sender map, clone bytes si bridge activo |
| `src-tauri/src/commands/session.rs` | Auto-attach: leer `.termgate/config.json` al crear sesión |
| `src-tauri/src/errors.rs` | Agregar TelegramError variant |
| `src/shared/types.ts` | TelegramBotConfig, BridgeInfo, BridgeStatus interfaces |
| `src/shared/ipc.ts` | TelegramAPI + event listeners |
| `src/sidebar/components/SettingsModal.tsx` | Sección "Telegram Bots" |
| `src/sidebar/App.tsx` | Listeners para bridge events, inicializar bridges store |
| `src/sidebar/styles/sidebar.css` | Estilos para bot cards, bridge indicator, attach button |

---

## Tipos

### Rust
```rust
// telegram/types.rs
pub struct TelegramBotConfig {
    pub id: String,
    pub label: String,
    pub token: String,       // Bot API token
    pub chat_id: i64,        // Chat donde enviar/recibir
    pub color: String,
}

pub struct BridgeInfo {
    pub bot_id: String,
    pub bot_label: String,
    pub session_id: String,
    pub status: BridgeStatus, // Active | Error(String) | Detaching
    pub color: String,
}

// Per-repo config (.termgate/config.json)
pub struct RepoConfig {
    pub telegram_bot: Option<String>,  // bot label
}
```

### TypeScript
```typescript
interface TelegramBotConfig { id, label, token, chatId, color }
interface BridgeInfo { botId, botLabel, sessionId, status, color }
type BridgeStatus = "active" | { error: string } | "detaching"
```

---

## Tauri Commands Nuevos

| Comando | Descripción |
|---|---|
| `telegram_attach(session_id, bot_id)` → `BridgeInfo` | Asigna bot a sesión (exclusivo) |
| `telegram_detach(session_id)` | Desconecta bridge |
| `telegram_list_bridges()` → `BridgeInfo[]` | Lista bridges activos |
| `telegram_get_bridge(session_id)` → `BridgeInfo?` | Bridge de una sesión específica |
| `telegram_send_test(bot_id)` | Test de conexión desde Settings |

## Tauri Events Nuevos

| Evento | Payload | Uso |
|---|---|---|
| `telegram_bridge_attached` | BridgeInfo | Actualizar UI sidebar |
| `telegram_bridge_detached` | { sessionId } | Quitar indicador |
| `telegram_bridge_error` | { sessionId, error } | Mostrar error |
| `telegram_incoming` | { sessionId, text, from } | Indicador visual |

---

## Arquitectura Interna del Bridge

### Desacoplamiento PTY ↔ Telegram
- Shared state: `Arc<tokio::sync::Mutex<HashMap<Uuid, mpsc::Sender<Vec<u8>>>>>`
- El read loop de PTY (std::thread) hace `try_send()` al channel — no bloqueante
- Si el channel está lleno, se descarta (mejor que bloquear el PTY)
- El bridge output task (tokio) consume del channel

### Output Task (por bridge)
1. Recibe `Vec<u8>` del mpsc channel
2. Strip ANSI con `strip-ansi-escapes`
3. Convierte a UTF-8 lossy
4. Buffer de líneas — flush en `\n` o timer 500ms
5. Chunk a ≤4000 chars (Telegram limit 4096)
6. `send_message()` con rate limit 35ms entre envíos

### Poll Task (por bridge)
1. `get_updates(offset, timeout=5)` en loop
2. Filtra por `chat_id`
3. Escribe texto + `\n` al PTY stdin via `PtyManager::write()`
4. Emite `telegram_incoming` event
5. Avanza offset

### Lifecycle
- **Attach**: crea sender en el map, spawns output + poll tasks con CancellationToken
- **Detach**: cancela token, remueve sender del map, limpia
- **Session destroyed**: auto-detach en `destroy_session` command
- **Exclusividad**: `bot_assignments: HashMap<bot_id, session_id>` — un bot = una sesión

---

## Frontend

### Settings Modal — Sección "Telegram Bots"
- Mismo patrón `.settings-button-card` que Agents
- Campos: Label, Token (type=password), Chat ID, Color
- Botón "Test" por bot → `telegram_send_test`
- Botón "+ Add Telegram Bot"

### Session Item — Bridge Indicator
- Dot coloreado (color del bot) cuando hay bridge activo
- Botón Telegram on hover → lista bots disponibles o detach si ya tiene uno

### Per-Repo Auto-Attach
- `.termgate/config.json` → `{ "telegramBot": "mi-bot" }`
- Al crear sesión con cwd: leer config, buscar bot por label, auto-attach si disponible
- Silent skip si bot no existe o está ocupado

---

## Fases de Implementación

### Fase A: Backend Foundation
1. Crear módulo `telegram/` (types, api, bridge, manager)
2. Agregar deps a Cargo.toml
3. Agregar `telegram_bots` a AppSettings con `#[serde(default)]`
4. Modificar PTY read loop para alimentar output sender map
5. `cargo check`

### Fase B: Commands + IPC
6. Crear commands/telegram.rs (5 commands)
7. Registrar en lib.rs
8. Actualizar types.ts e ipc.ts

### Fase C: Settings UI
9. Sección Telegram Bots en SettingsModal
10. Test connection button
11. Persistencia end-to-end

### Fase D: Session Bridge UI
12. Bridge indicator en session items
13. Attach/detach interaction
14. Bridges store + event listeners

### Fase E: Per-Repo Auto-Attach
15. Leer `.termgate/config.json` en create_session
16. Auto-attach logic

---

## Verificación

1. Agregar un bot en Settings, guardar, recargar → persiste en `~/.termgate/settings.json`
2. Test connection → mensaje llega a Telegram
3. Crear sesión, attach bot → output del terminal aparece en Telegram
4. Escribir en Telegram → texto aparece en la terminal
5. Detach → bridge se para limpiamente
6. Crear `.termgate/config.json` en un repo, abrir sesión ahí → auto-attach
7. Destruir sesión con bridge → auto-detach sin errores
