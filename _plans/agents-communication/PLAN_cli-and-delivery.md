# Plan: CLI de agentscommander + Sistema de Delivery de Mensajes

**Branch:** `feature/agent-direct-communication`
**Date:** 2026-03-25

---

## Contexto

Los agentes CLI (Claude Code, Codex, etc.) necesitan comunicarse entre sí. El mecanismo es un CLI integrado en el propio binario de agentscommander que los agentes invocan desde su PTY. El CLI escribe mensajes a archivo, y la instancia de agentscommander que está corriendo se encarga de entregarlos al destinatario.

---

## Arquitectura General

### Dos modos del binario

```
agentscommander.exe               → Modo app (Tauri, sidebar, terminal)
agentscommander.exe send ...      → Modo CLI (escribe mensaje, termina)
agentscommander.exe list-peers .. → Modo CLI (lista peers, termina)
```

El binario detecta si tiene subcomandos y actúa en consecuencia. Sin subcomandos → Tauri app. Con subcomandos → CLI puro, sin ventanas.

---

## Autenticación: Session Token

Cada session de agente recibe un **token UUID único** al ser creada. Este token:

- Se genera en `create_session_inner()` al crear la session
- Se pasa al agente **dentro del prompt de inicialización** (NO como env var, para evitar exposición)
- El agente lo incluye en cada invocación del CLI
- agentscommander valida el token contra la session activa antes de procesar el mensaje
- Si el token no matchea → rechazo, el agente no puede spoofear otro agente

### Prompt de inicialización

Al spawnar un agente, agentscommander inyecta en el PTY stdin un bloque como:

```
Tu token de sesión para comunicarte con otros agentes es: <UUID>

Para enviar mensajes a otros agentes:
  agentscommander send --token <UUID> --to "<agent_name>" --message "..." [--mode wake|active-only|wake-and-sleep|queue] [--get-output] [--agent <agent_cli>]

Para ver agentes disponibles para mensajear:
  agentscommander list-peers --token <UUID>
```

Este prompt se inyecta después de que el agente arranca y está listo (después del primer idle).

---

## Modos de Delivery

### 1. `queue` (default)
Deja el mensaje en el inbox del destinatario. No toca la PTY. El agente lo lee cuando quiera.

### 2. `active-only`
Entrega solo si el agente tiene una session activa y despierta (no idle). Si está idle o no tiene session → encola como `queue`.

### 3. `wake`
Si el agente está idle (esperando input en la PTY), inyecta el mensaje en el stdin de la PTY como si el usuario lo tipeara. Si no hay session activa → encola como `queue`.

### 4. `wake-and-sleep`
- Si no hay session activa: levanta una session temporal no-interactiva con el agente CLI apropiado
- Inyecta el mensaje en el stdin de la PTY
- Monitorea el idle detector
- Cuando el agente vuelve a idle (terminó de responder) → captura el output si aplica, cierra la session
- Es un "one-shot": spawn → deliver → wait for idle → kill

---

## Selección de Agente CLI (para wake-and-sleep)

Cuando hay que levantar una session nueva, se necesita saber qué agente CLI usar (claude, codex, etc.):

### Flag `--agent`
- `auto` (default): usa el último agente CLI levantado en ese repo. Si no hay historial, usa el mismo agente CLI del sender
- Nombre específico (ej: `claude`, `codex`): usa ese agente puntual

### Historial de agentes por repo
agentscommander trackea qué agente CLI fue el último usado en cada repo. Se almacena en `agents.json` o en un campo nuevo en la session persistence.

### Información del sender
El mensaje incluye qué agente CLI usa el sender (para el fallback de `auto` cuando no hay historial en el destinatario).

---

## Flag `--get-output`

Cuando se pasa `--get-output`:

1. El CLI genera un `requestId` UUID
2. Escribe el mensaje con ese `requestId`
3. El CLI **espera** (pollea) un archivo de respuesta en `.agentscommander/responses/<requestId>.json`
4. agentscommander entrega el mensaje al destinatario
5. Cuando el destinatario termina (vuelve a idle en wake/wake-and-sleep), agentscommander captura el output y lo escribe en el archivo de respuesta
6. El CLI lee la respuesta, la imprime a stdout, y termina

Esto permite que el sender capture la respuesta:
```bash
RESULT=$(agentscommander send --token $TOKEN --to "0_repos/project_x" --message "Revisá el endpoint" --mode wake --get-output)
```

Sin `--get-output`: fire-and-forget. El CLI escribe, sale con exit 0.

---

## Discovery: `list-peers`

```bash
agentscommander list-peers --token <UUID>
```

### Reglas de visibilidad

1. Si el agente tiene team(s) en `teams.json` → devuelve la **unión de miembros** de todos sus teams
2. Si el agente NO tiene team → devuelve todos los agentes que comparten su **parent directory**
3. Un agente puede estar en múltiples teams

### Información por peer

Para cada peer devuelve:
- `name`: nombre extendido (parent/repo)
- `status`: si tiene session activa o no
- `role`: descripción de qué hace — leído del `CLAUDE.md` del repo del peer (por ahora siempre CLAUDE.md)
- `teams`: teams compartidos
- `lastAgent`: último agente CLI usado en ese repo

### Lectura del role prompt

agentscommander lee el `CLAUDE.md` del repo de cada peer y extrae un resumen. Opciones:
- Leer las primeras N líneas (ej: hasta el primer `---`)
- Leer una sección específica (ej: `## Role Prompt` o `## Project Overview`)
- Definir un campo explícito en `.agentscommander/config.json` tipo `"role": "Soy el agente de backend..."`

**Recomendación**: Leer la sección `## Role Prompt` de CLAUDE.md si existe. Si no, las primeras 5 líneas. Fallback: "No role description available."

---

## Formato del Mensaje (archivo outbox)

```json
{
  "id": "uuid-del-mensaje",
  "token": "uuid-session-token-del-sender",
  "from": "0_repos/agentscommander_2",
  "to": "0_repos/project_x",
  "body": "Revisá el endpoint /api/health",
  "mode": "wake",
  "getOutput": false,
  "requestId": null,
  "senderAgent": "claude",
  "preferredAgent": "auto",
  "priority": "normal",
  "timestamp": "2026-03-25T01:00:00Z"
}
```

| Campo | Descripción |
|---|---|
| `id` | UUID único del mensaje |
| `token` | Session token del sender — para validación |
| `from` | Nombre extendido del sender (derivado del token) |
| `to` | Nombre extendido del destinatario |
| `body` | Contenido del mensaje |
| `mode` | `queue`, `active-only`, `wake`, `wake-and-sleep` |
| `getOutput` | Si true, se espera respuesta |
| `requestId` | UUID para correlacionar la respuesta (solo si getOutput) |
| `senderAgent` | Agente CLI del sender (para fallback en auto) |
| `preferredAgent` | `auto` o nombre específico del agente CLI a usar |
| `priority` | `normal`, `high` |
| `timestamp` | ISO 8601 |

---

## Flujo Completo

### Envío normal (fire-and-forget)

```
Agente A ejecuta:
  agentscommander send --token ABC --to "0_repos/project_x" --message "Hola" --mode queue

CLI:
  1. Valida que --token, --to, --message están presentes
  2. Genera message UUID
  3. Escribe .agentscommander/outbox/<uuid>.json
  4. Imprime "Message queued: <uuid>"
  5. Exit 0

MailboxPoller (cada 3s):
  1. Detecta el archivo en outbox/
  2. Lee el mensaje
  3. Valida el token contra sessions activas
  4. Valida que from puede comunicarse con to (peers rules)
  5. Según mode:
     - queue: escribe en <to>/.agentscommander/inbox/
     - active-only: si hay session despierta → inyecta en PTY, sino → inbox
     - wake: si hay session idle → inyecta en PTY, sino → inbox
     - wake-and-sleep: spawn session temporal → inyecta → wait idle → kill
  6. Elimina archivo de outbox/
  7. Emite evento al frontend
```

### Envío con get-output

```
Agente A ejecuta:
  agentscommander send --token ABC --to "0_repos/project_x" --message "Revisá esto" --mode wake --get-output

CLI:
  1. Genera message UUID y requestId
  2. Escribe outbox/<uuid>.json con getOutput: true, requestId: <rid>
  3. Entra en loop de polling: lee .agentscommander/responses/<rid>.json cada 2s
  4. (bloqueante — el CLI no termina hasta recibir respuesta o timeout)

MailboxPoller:
  1. Detecta el mensaje, valida, entrega al agente (wake)
  2. Monitorea el idle detector del agente destinatario
  3. Cuando vuelve a idle: captura el output de la PTY (desde el momento de inyección hasta idle)
  4. Escribe .agentscommander/responses/<rid>.json con el output
  5. El CLI del sender detecta el archivo, lo lee, imprime a stdout, exit 0
```

---

## Implementación — Archivos a crear/modificar

### CLI (Rust)

| Archivo | Acción | Qué |
|---|---|---|
| `src-tauri/src/main.rs` | MODIFICAR | Detectar subcomandos antes de iniciar Tauri. Si hay subcomando → modo CLI, sino → modo app |
| `src-tauri/src/cli/mod.rs` | **CREAR** | Módulo CLI: parse de argumentos, subcomandos |
| `src-tauri/src/cli/send.rs` | **CREAR** | Subcomando `send`: valida args, genera mensaje, escribe a outbox |
| `src-tauri/src/cli/list_peers.rs` | **CREAR** | Subcomando `list-peers`: lee agents.json, filtra peers, lee role prompts |

### Session Token (Rust)

| Archivo | Acción | Qué |
|---|---|---|
| `src-tauri/src/session/session.rs` | MODIFICAR | Agregar campo `token: Uuid` a `Session` |
| `src-tauri/src/session/manager.rs` | MODIFICAR | Generar token en `create_session` |
| `src-tauri/src/phone/agent_registry.rs` | MODIFICAR | Incluir token en agents.json? NO — el token es secreto. Solo validar contra SessionManager |

### Delivery (Rust)

| Archivo | Acción | Qué |
|---|---|---|
| `src-tauri/src/phone/mailbox.rs` | MODIFICAR | Agregar lógica de delivery por modo (queue/active-only/wake/wake-and-sleep), validación de token, captura de output |
| `src-tauri/src/phone/types.rs` | MODIFICAR | Actualizar OutboxMessage con campos nuevos (token, mode, getOutput, requestId, senderAgent, preferredAgent) |
| `src-tauri/src/pty/manager.rs` | MODIFICAR | Agregar método para inyectar texto en el stdin de una PTY existente |

### Prompt de inicialización

| Archivo | Acción | Qué |
|---|---|---|
| `src-tauri/src/commands/session.rs` | MODIFICAR | Después de spawn y primer idle, inyectar prompt con token y instrucciones CLI |

### Agent History

| Archivo | Acción | Qué |
|---|---|---|
| `src-tauri/src/phone/agent_registry.rs` | MODIFICAR | Trackear `lastAgentCli` por repo path |

### Frontend

| Archivo | Acción | Qué |
|---|---|---|
| `src/shared/types.ts` | MODIFICAR | Agregar campos de delivery mode, token-related types |
| `src/shared/ipc.ts` | MODIFICAR | Si se agregan nuevos eventos/commands |

---

## Orden de ejecución

### Step 1: CLI skeleton
1. Modificar `main.rs` para detectar subcomandos
2. Crear módulo `cli/` con parse de args
3. Implementar `send` básico (escribe outbox, fire-and-forget, sin validación de token)
4. Test: ejecutar `agentscommander send --to X --message Y` desde una terminal

### Step 2: Session Token
1. Agregar `token: Uuid` a `Session` struct
2. Generar en `create_session`
3. Almacenar en SessionManager
4. Agregar validación de token en MailboxPoller
5. Test: verificar que el token se genera y persiste

### Step 3: Prompt de inicialización
1. Después del primer idle de una session, inyectar prompt con token e instrucciones
2. El prompt incluye: token, sintaxis de `send`, sintaxis de `list-peers`
3. Test: crear session, verificar que el prompt aparece en la PTY

### Step 4: Delivery modes
1. Implementar `queue` (ya existe — escribir a inbox)
2. Implementar `active-only` (verificar session status antes de entregar)
3. Implementar `wake` (inyectar en PTY stdin si idle)
4. Implementar `wake-and-sleep` (spawn temporal, inyectar, wait idle, kill)
5. Test manual de cada modo

### Step 5: get-output
1. Agregar requestId al mensaje
2. CLI entra en polling loop esperando response file
3. MailboxPoller captura output de PTY entre inyección e idle
4. Escribe response file
5. Test: enviar con --get-output, verificar que llega la respuesta

### Step 6: list-peers
1. Implementar subcomando `list-peers`
2. Leer agents.json, filtrar por teams o parent directory
3. Leer CLAUDE.md de cada peer para extraer role
4. Devolver JSON o texto formateado
5. Test: ejecutar list-peers, verificar output

### Step 7: Agent History
1. Trackear último agente CLI usado por repo
2. Usar para el fallback de `--agent auto` en wake-and-sleep

---

## Consideraciones

- **Seguridad del token**: El token NO va en env vars, NO va en agents.json, NO va en archivos accesibles. Solo existe en memoria (SessionManager) y en el prompt que recibió el agente. El outbox file contiene el token pero se borra después de procesado.
- **Timeout para get-output**: El CLI debe tener un timeout configurable (default 5 minutos). Si no llega respuesta, exit con error.
- **Captura de output para get-output**: Se necesita un mecanismo para capturar el output de la PTY entre el momento de inyección y el retorno a idle. Posiblemente usar el vt100 crate que ya está en dependencias para capturar el texto renderizado.
- **wake-and-sleep cleanup**: Si la session temporal crashea o el agente no vuelve a idle, agentscommander debe tener un timeout para matar la session forzosamente.
- **Concurrencia**: Múltiples mensajes wake al mismo agente se deben encolar, no inyectar simultáneamente.
- **CLI binary path**: El CLI es el mismo binario que la app. Los agentes necesitan conocer el path al binario. Se puede pasar en el prompt de inicialización o detectar automáticamente.
