# agentscommander — Prompt de Desarrollo Completo

## Contexto del Proyecto

**agentscommander** es una aplicación de escritorio standalone para Windows que funciona como un **session manager externo de terminales** con las tabs desacopladas en una ventana lateral (sidebar), mientras la terminal activa se muestra en una ventana principal separada. El objetivo es resolver una limitación que ninguna terminal en Windows ofrece nativamente: tener un panel lateral persistente con la lista de sesiones de terminal, independiente de la ventana donde se ejecuta la consola.

### Analogía

Imaginá un setup donde tenés dos ventanas:

- **Ventana A (Sidebar):** Una ventana estrecha y siempre visible que muestra todas tus sesiones/tabs de terminal en formato vertical (como el sidebar de VS Code o las vertical tabs de Edge). Desde acá podés crear, renombrar, reordenar, agrupar y eliminar sesiones.
- **Ventana B (Terminal):** La ventana principal donde se renderiza la terminal activa (la sesión seleccionada en el sidebar). Esta ventana puede maximizarse, snapearse, moverse a otro monitor, etc.

Ambas ventanas son independientes pero están sincronizadas. Al hacer click en una sesión del sidebar, la ventana de terminal cambia a esa sesión.

---

## Stack Tecnológico

| Capa | Tecnología | Justificación |
|---|---|---|
| Framework de app | **Tauri 2.x** | Binario nativo ~5MB, usa WebView del OS, acceso directo a APIs de Windows. Soporte multiventana nativo. |
| Backend / Core | **Rust** | Performance máximo, manejo seguro de memoria, excelente ecosistema para PTY y procesos. |
| Frontend | **SolidJS + TypeScript** | Framework JS más rápido en rendering benchmarks. Reactividad fine-grained sin Virtual DOM. |
| Emulador de terminal | **xterm.js** | Estándar de la industria (usado por VS Code, Hyper, etc). GPU-accelerated con addon WebGL. |
| PTY (pseudo-terminal) | **portable-pty** (crate de Rust) | Abstrae ConPTY en Windows. Multiplataforma para futuro soporte Linux/Mac. |
| Estilos | **CSS vanilla con variables** | Sin overhead de framework CSS. Variables para theming dinámico. |
| Persistencia | **serde + toml** | Config y layouts guardados en archivos TOML legibles por humanos. |
| Async runtime | **tokio** | Runtime async estándar de Rust para manejar múltiples sesiones concurrentes. |
| IPC | **Tauri Commands + Events** | Comunicación bidireccional entre frontend y backend ya integrada en Tauri. |

---

## Arquitectura del Sistema

### Diagrama de Componentes

```
┌─────────────────────────────────────────────────────────┐
│                      TAURI APP                          │
│                                                         │
│  ┌──────────────┐         ┌──────────────────────────┐  │
│  │  SIDEBAR WIN │         │     TERMINAL WINDOW      │  │
│  │  (WebView)   │  IPC    │       (WebView)          │  │
│  │              │◄───────►│                          │  │
│  │  SolidJS UI  │  Events │  xterm.js + WebGL addon  │  │
│  │  Session List│         │  Terminal renderer       │  │
│  │  Controls    │         │                          │  │
│  └──────┬───────┘         └────────────┬─────────────┘  │
│         │                              │                │
│         │      Tauri Commands          │                │
│         └──────────┬───────────────────┘                │
│                    ▼                                    │
│  ┌─────────────────────────────────────────────────┐    │
│  │              RUST BACKEND                       │    │
│  │                                                 │    │
│  │  ┌──────────────┐  ┌────────────────────────┐   │    │
│  │  │ Session Mgr  │  │    PTY Manager         │   │    │
│  │  │              │  │                        │   │    │
│  │  │ - create     │  │ - spawn shell process  │   │    │
│  │  │ - destroy    │  │ - read/write streams   │   │    │
│  │  │ - list       │  │ - resize               │   │    │
│  │  │ - rename     │  │ - ConPTY abstraction   │   │    │
│  │  │ - reorder    │  │                        │   │    │
│  │  │ - group      │  │                        │   │    │
│  │  └──────────────┘  └────────────────────────┘   │    │
│  │                                                 │    │
│  │  ┌──────────────┐  ┌────────────────────────┐   │    │
│  │  │ Config Mgr   │  │    Window Manager      │   │    │
│  │  │              │  │                        │   │    │
│  │  │ - themes     │  │ - multi-window sync    │   │    │
│  │  │ - layouts    │  │ - position/size save   │   │    │
│  │  │ - keybinds   │  │ - focus management     │   │    │
│  │  │ - profiles   │  │ - always-on-top toggle │   │    │
│  │  └──────────────┘  └────────────────────────┘   │    │
│  └─────────────────────────────────────────────────┘    │
│                                                         │
│  ┌─────────────────────────────────────────────────┐    │
│  │              FILESYSTEM                         │    │
│  │  ~/.agentscommander/                              │    │
│  │  ├── config.toml      (configuración global)    │    │
│  │  ├── themes/          (archivos de tema)        │    │
│  │  ├── layouts/         (layouts guardados)       │    │
│  │  └── sessions.toml    (sesiones persistentes)   │    │
│  └─────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
```

### Flujo de Datos

```
Usuario clickea sesión en Sidebar
        │
        ▼
SolidJS emite evento → Tauri Command "switch_session(id)"
        │
        ▼
Rust SessionManager activa la sesión correspondiente
        │
        ▼
Rust emite evento "session_switched" con datos del PTY
        │
        ▼
Terminal Window recibe evento → xterm.js se conecta al stream del PTY activo
        │
        ▼
PTY output → Rust lee bytes → Tauri Event "pty_output" → xterm.js renderiza
        │
        ▼
Usuario tipea en xterm.js → Tauri Command "pty_write(bytes)" → Rust escribe al PTY
```

---

## Estructura del Proyecto

```
agentscommander/
├── src-tauri/                          # Backend Rust
│   ├── Cargo.toml
│   ├── tauri.conf.json                 # Config de Tauri (multiventana, permisos)
│   ├── capabilities/                   # Permisos de Tauri v2
│   │   └── default.json
│   ├── src/
│   │   ├── main.rs                     # Entry point, setup de Tauri
│   │   ├── lib.rs                      # Re-exports de módulos
│   │   ├── commands/                   # Tauri Commands (IPC handlers)
│   │   │   ├── mod.rs
│   │   │   ├── session.rs              # create, destroy, list, rename, switch
│   │   │   ├── pty.rs                  # write, resize
│   │   │   ├── config.rs               # get/set config, themes
│   │   │   └── window.rs               # manage windows, positions
│   │   ├── session/                    # Session management core
│   │   │   ├── mod.rs
│   │   │   ├── manager.rs              # SessionManager struct
│   │   │   ├── session.rs              # Session struct y estado
│   │   │   └── group.rs               # Agrupación de sesiones
│   │   ├── pty/                        # PTY management
│   │   │   ├── mod.rs
│   │   │   ├── manager.rs              # PtyManager: spawn, read, write
│   │   │   └── platform.rs            # Abstracciones específicas del OS
│   │   ├── config/                     # Configuración y persistencia
│   │   │   ├── mod.rs
│   │   │   ├── app_config.rs           # Struct de config global
│   │   │   ├── theme.rs               # Theme definitions
│   │   │   └── keybindings.rs         # Keyboard shortcuts config
│   │   └── window/                     # Window management
│   │       ├── mod.rs
│   │       └── manager.rs              # Posiciones, foco, always-on-top
│   └── icons/                          # Iconos de la app
│
├── src/                                # Frontend SolidJS + TypeScript
│   ├── index.html                      # Entry HTML (carga sidebar o terminal según query param)
│   ├── sidebar/                        # Sidebar Window UI
│   │   ├── App.tsx                     # Root component del sidebar
│   │   ├── components/
│   │   │   ├── SessionList.tsx         # Lista de sesiones con drag & drop
│   │   │   ├── SessionItem.tsx         # Item individual de sesión
│   │   │   ├── SessionGroup.tsx        # Grupo colapsable de sesiones
│   │   │   ├── Toolbar.tsx             # Barra de herramientas (nueva sesión, config)
│   │   │   ├── SearchBar.tsx           # Búsqueda/filtro de sesiones
│   │   │   ├── ContextMenu.tsx         # Menú contextual (click derecho)
│   │   │   └── StatusIndicator.tsx     # Indicador de estado de sesión (activa, idle, error)
│   │   ├── stores/
│   │   │   ├── sessions.ts            # Store reactivo de sesiones (SolidJS createStore)
│   │   │   ├── config.ts             # Store de configuración
│   │   │   └── ui.ts                  # Estado de UI (sidebar width, collapsed groups, etc)
│   │   └── styles/
│   │       ├── sidebar.css
│   │       └── variables.css          # CSS variables para theming
│   │
│   ├── terminal/                       # Terminal Window UI
│   │   ├── App.tsx                     # Root component de la terminal
│   │   ├── components/
│   │   │   ├── TerminalView.tsx        # Wrapper de xterm.js
│   │   │   ├── TerminalTabs.tsx        # Tab bar minimalista (opcional, para multi-pane)
│   │   │   ├── StatusBar.tsx           # Barra inferior (shell, directorio, sesión activa)
│   │   │   └── SplitView.tsx           # Split panes dentro de la terminal window
│   │   ├── stores/
│   │   │   └── terminal.ts            # Estado del terminal activo
│   │   └── styles/
│   │       ├── terminal.css
│   │       └── variables.css
│   │
│   ├── shared/                         # Código compartido entre sidebar y terminal
│   │   ├── types.ts                   # Tipos TypeScript (Session, Config, Theme, etc)
│   │   ├── ipc.ts                     # Wrapper tipado sobre Tauri invoke/listen
│   │   ├── constants.ts              # Constantes compartidas
│   │   └── utils.ts                  # Utilidades comunes
│   │
│   └── assets/
│       ├── fonts/                     # Fuentes custom
│       └── icons/                     # Iconos SVG
│
├── package.json
├── tsconfig.json
├── vite.config.ts                      # Config de Vite (bundler de Tauri)
└── README.md
```

---

## Especificación Funcional Detallada

### 1. Gestión de Sesiones

#### 1.1 Crear Sesión

- Al crear una nueva sesión, el backend debe:
  1. Generar un UUID único para la sesión.
  2. Spawnar un nuevo proceso PTY usando `portable-pty`.
  3. El shell por defecto debe leerse de la config (`config.toml`), con fallback a `powershell.exe`.
  4. Registrar la sesión en el `SessionManager`.
  5. Emitir evento `session_created { id, name, shell, timestamp }` al frontend.
- El frontend sidebar agrega la sesión a la lista con animación de entrada.
- La sesión se nombra automáticamente con un patrón incremental: "Session 1", "Session 2", etc. O si hay un profile asociado, usa el nombre del profile.

#### 1.2 Destruir Sesión

- Matar el proceso PTY asociado (SIGTERM, luego SIGKILL tras timeout de 3s).
- Remover del `SessionManager`.
- Si era la sesión activa, auto-switch a la siguiente sesión disponible.
- Si no quedan sesiones, mostrar un placeholder en la terminal window con botón "New Session".
- Emitir evento `session_destroyed { id }`.
- Pedir confirmación si la sesión tiene un proceso hijo activo (ej: un script corriendo).

#### 1.3 Renombrar Sesión

- Double-click en el nombre de la sesión en el sidebar activa modo edición inline.
- Enter confirma, Escape cancela.
- Validación: no vacío, máximo 50 caracteres, no duplicado dentro del mismo grupo.

#### 1.4 Reordenar Sesiones

- Drag & drop nativo dentro del sidebar.
- Soporte para mover entre grupos.
- Persistir el orden en `sessions.toml`.
- Animación suave de reordenamiento.

#### 1.5 Agrupar Sesiones

- Las sesiones pueden pertenecer a un grupo (o a ninguno).
- Los grupos son colapsables con animación.
- Los grupos tienen nombre y color customizable.
- Click derecho → "Move to group" → selector de grupo o "New group".
- Los grupos se persisten en `sessions.toml`.

#### 1.6 Perfiles de Shell

- Se pueden definir perfiles en `config.toml`:
  ```toml
  [[profiles]]
  name = "PowerShell"
  command = "powershell.exe"
  args = ["-NoLogo"]
  icon = "powershell"
  color = "#012456"
  env = { TERM = "xterm-256color" }
  working_directory = "~"

  [[profiles]]
  name = "CMD"
  command = "cmd.exe"
  icon = "cmd"
  color = "#000000"

  [[profiles]]
  name = "WSL - Ubuntu"
  command = "wsl.exe"
  args = ["-d", "Ubuntu"]
  icon = "linux"
  color = "#E95420"

  [[profiles]]
  name = "Git Bash"
  command = "C:\\Program Files\\Git\\bin\\bash.exe"
  args = ["--login", "-i"]
  icon = "git"
  color = "#F05032"
  ```
- Al crear nueva sesión, se puede elegir el profile desde un dropdown en el toolbar del sidebar.

### 2. Ventana Sidebar

#### 2.1 Layout

```
┌─────────────────────────────┐
│  ⚡ agentscommander    [—][×] │  ← Title bar (custom, draggable)
├─────────────────────────────┤
│  [🔍 Search sessions...]    │  ← Search/filter bar
├─────────────────────────────┤
│  📁 Backend (3)         ▾   │  ← Grupo colapsable
│    ● API Server      🟢    │  ← Sesión activa (indicador verde)
│      PowerShell             │     Shell type (subtle)
│    ○ Database        🔵    │  ← Sesión inactiva
│      WSL - Ubuntu           │
│    ○ Logs            🔵    │
│      PowerShell             │
├─────────────────────────────┤
│  📁 Frontend (2)        ▾   │
│    ○ Dev Server      🔵    │
│      PowerShell             │
│    ○ Tests           🔵    │
│      Git Bash               │
├─────────────────────────────┤
│  📁 Ungrouped (1)       ▾   │
│    ○ Scratch         🔵    │
│      CMD                    │
├─────────────────────────────┤
│                             │
│                             │  ← Espacio libre (drop zone para reordenar)
│                             │
├─────────────────────────────┤
│  [+ New Session ▾]  [⚙]    │  ← Toolbar inferior
└─────────────────────────────┘
```

#### 2.2 Comportamiento de la Ventana Sidebar

- **Ancho configurable** (mínimo 200px, máximo 400px), persistido.
- **Always-on-top opcional** (toggle en config o desde UI).
- **Posición persistida** (recordar dónde estaba al cerrar).
- **Transparencia/opacidad configurable** vía CSS.
- **Minimizable a system tray** (icono en la bandeja del sistema con menú contextual).
- Al cerrar el sidebar, se cierra toda la aplicación (previa confirmación si hay sesiones activas).

#### 2.3 Menú Contextual (Click Derecho en Sesión)

- Rename
- Duplicate
- Move to Group → [lista de grupos] | New Group
- Change Color
- Close Session
- Copy Session ID (para scripts/automatización)

#### 2.4 Menú Contextual (Click Derecho en Grupo)

- Rename Group
- Change Group Color
- Collapse/Expand
- Close All Sessions in Group
- Delete Group (mueve sesiones a Ungrouped)

### 3. Ventana Terminal

#### 3.1 Layout

```
┌──────────────────────────────────────────────────┐
│  Session: API Server (PowerShell)     [—][□][×]  │  ← Title bar con nombre de sesión activa
├──────────────────────────────────────────────────┤
│                                                  │
│  PS C:\Users\dev\projects\api>                   │
│  > npm start                                     │
│  Server running on port 3000...                  │
│  █                                               │  ← xterm.js con WebGL renderer
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
├──────────────────────────────────────────────────┤
│  PowerShell │ ~/projects/api │ 128x32 │ 2.3MB    │  ← Status bar
└──────────────────────────────────────────────────┘
```

#### 3.2 Comportamiento

- **xterm.js con WebGL addon** para máximo rendimiento de renderizado.
- **Resize automático**: al redimensionar la ventana, se recalcula las filas/columnas y se envía resize al PTY.
- **Scrollback configurable** (por defecto 10,000 líneas).
- **Búsqueda dentro del terminal** con Ctrl+Shift+F (usa xterm.js search addon).
- **Copy/Paste**: Ctrl+C (cuando hay selección) para copiar, Ctrl+V para pegar, Ctrl+Shift+C para copiar siempre.
- **Font configurable** (familia, tamaño, line height, letter spacing).
- **Ligatures support** (xterm.js ligature addon).
- **Links clickeables** (xterm.js web links addon): URLs se detectan y abren en el navegador.
- **Unicode y emoji** completo.

#### 3.3 Split Panes (Fase 2)

- Dentro de la terminal window, poder dividir en panes (horizontal/vertical).
- Cada pane puede mostrar una sesión diferente (seleccionable desde el sidebar).
- Shortcuts: Ctrl+Shift+H (split horizontal), Ctrl+Shift+V (split vertical).
- Resize de panes con drag en el divisor.

### 4. Sistema de Temas

#### 4.1 Estructura de un Tema

```toml
# ~/.agentscommander/themes/cyberpunk.toml

[metadata]
name = "Cyberpunk"
author = "agentscommander"
version = "1.0"

[colors]
# Sidebar
sidebar_bg = "#0a0a0f"
sidebar_fg = "#e0e0e0"
sidebar_accent = "#ff00ff"
sidebar_hover = "#1a1a2f"
sidebar_active = "#2a1a3f"
sidebar_border = "#333355"
group_header_bg = "#111122"

# Terminal
terminal_bg = "#0d0d1a"
terminal_fg = "#e8e8e8"
terminal_cursor = "#ff00ff"
terminal_selection = "rgba(255, 0, 255, 0.3)"

# ANSI Colors (terminal)
ansi_black = "#1a1a2e"
ansi_red = "#ff3366"
ansi_green = "#33ff99"
ansi_yellow = "#ffcc33"
ansi_blue = "#3399ff"
ansi_magenta = "#ff33cc"
ansi_cyan = "#33ccff"
ansi_white = "#e8e8e8"
ansi_bright_black = "#4a4a5e"
ansi_bright_red = "#ff6699"
ansi_bright_green = "#66ffbb"
ansi_bright_yellow = "#ffdd66"
ansi_bright_blue = "#66bbff"
ansi_bright_magenta = "#ff66dd"
ansi_bright_cyan = "#66ddff"
ansi_bright_white = "#ffffff"

# Status bar
status_bg = "#111122"
status_fg = "#888899"

# Accents
success = "#33ff99"
warning = "#ffcc33"
error = "#ff3366"
info = "#3399ff"

[fonts]
terminal_family = "'JetBrains Mono', 'Cascadia Code', monospace"
terminal_size = 14
terminal_line_height = 1.2
ui_family = "'Inter', 'Segoe UI', sans-serif"
ui_size = 13
```

#### 4.2 Temas Incluidos

Incluir al menos 3 temas de fábrica:

1. **Noir** (por defecto): Fondo oscuro profundo, acentos en blanco y un color vibrante (ej: cyan).
2. **Cyberpunk**: Neones magenta/cyan sobre negro.
3. **Solarized Dark**: El clásico Solarized adaptado.

### 5. Configuración Global

```toml
# ~/.agentscommander/config.toml

[general]
default_shell = "powershell.exe"
default_shell_args = ["-NoLogo"]
theme = "noir"
language = "en"
confirm_on_close = true
start_minimized = false
start_with_windows = false

[sidebar]
width = 280
always_on_top = false
position = "left"        # left o right (lado de la pantalla donde se ancla)
opacity = 1.0            # 0.0 a 1.0
show_shell_type = true   # Mostrar el tipo de shell debajo del nombre
show_status_icon = true  # Indicador de estado (activa, idle, etc)
minimize_to_tray = true

[terminal]
font_family = "'Cascadia Code', 'JetBrains Mono', monospace"
font_size = 14
line_height = 1.2
scrollback = 10000
cursor_style = "block"   # block, underline, bar
cursor_blink = true
copy_on_select = false
bell_enabled = false
webgl_renderer = true

[keybindings]
new_session = "Ctrl+Shift+N"
close_session = "Ctrl+Shift+W"
next_session = "Ctrl+Tab"
prev_session = "Ctrl+Shift+Tab"
toggle_sidebar = "Ctrl+Shift+B"
search_sessions = "Ctrl+Shift+P"
search_terminal = "Ctrl+Shift+F"
split_horizontal = "Ctrl+Shift+H"
split_vertical = "Ctrl+Shift+V"
focus_sidebar = "Ctrl+Shift+S"
rename_session = "F2"
```

### 6. Keyboard Shortcuts Globales

Los shortcuts deben funcionar **tanto en la ventana sidebar como en la terminal**. Tauri v2 soporta global shortcuts registrados a nivel del OS.

| Shortcut | Acción |
|---|---|
| Ctrl+Shift+N | Nueva sesión (con profile por defecto) |
| Ctrl+Shift+W | Cerrar sesión activa |
| Ctrl+Tab | Siguiente sesión |
| Ctrl+Shift+Tab | Sesión anterior |
| Ctrl+Shift+B | Toggle sidebar visibility |
| Ctrl+Shift+P | Foco en search bar del sidebar |
| Ctrl+Shift+F | Búsqueda dentro del terminal |
| Ctrl+Shift+S | Foco en sidebar |
| F2 | Renombrar sesión seleccionada |
| Ctrl+1...9 | Ir a sesión N |
| Ctrl+Shift+H | Split horizontal |
| Ctrl+Shift+V | Split vertical |

### 7. System Tray

- Icono en la bandeja del sistema con menú:
  - Show/Hide Sidebar
  - Show/Hide Terminal
  - Quick New Session → [lista de profiles]
  - Sessions → [lista de sesiones activas]
  - Settings
  - Quit

---

## Especificación Técnica Detallada

### Backend Rust — Módulos Clave

#### `session/manager.rs`

```rust
use std::collections::HashMap;
use uuid::Uuid;
use tokio::sync::RwLock;
use std::sync::Arc;

pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<Uuid, Session>>>,
    groups: Arc<RwLock<Vec<SessionGroup>>>,
    active_session: Arc<RwLock<Option<Uuid>>>,
    order: Arc<RwLock<Vec<Uuid>>>,
}

pub struct Session {
    pub id: Uuid,
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    pub group_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub working_directory: String,
    pub color: Option<String>,
    pub status: SessionStatus,
}

pub enum SessionStatus {
    Active,    // Proceso corriendo, seleccionada
    Running,   // Proceso corriendo, no seleccionada
    Idle,      // Shell esperando input
    Exited(i32), // Proceso terminó con exit code
}

pub struct SessionGroup {
    pub id: Uuid,
    pub name: String,
    pub color: String,
    pub collapsed: bool,
    pub order: Vec<Uuid>,
}

impl SessionManager {
    pub async fn create_session(&self, profile: ShellProfile) -> Result<Session, Error>;
    pub async fn destroy_session(&self, id: Uuid) -> Result<(), Error>;
    pub async fn switch_session(&self, id: Uuid) -> Result<(), Error>;
    pub async fn rename_session(&self, id: Uuid, name: String) -> Result<(), Error>;
    pub async fn move_to_group(&self, session_id: Uuid, group_id: Option<Uuid>) -> Result<(), Error>;
    pub async fn reorder(&self, order: Vec<Uuid>) -> Result<(), Error>;
    pub async fn get_all(&self) -> Vec<SessionInfo>;
    pub async fn get_active(&self) -> Option<Uuid>;
}
```

#### `pty/manager.rs`

```rust
use portable_pty::{CommandBuilder, PtySize, PtySystem, PtyPair};
use tokio::sync::mpsc;
use uuid::Uuid;

pub struct PtyManager {
    ptys: HashMap<Uuid, PtyInstance>,
}

struct PtyInstance {
    pair: PtyPair,
    child: Box<dyn portable_pty::Child + Send>,
    reader_tx: mpsc::Sender<Vec<u8>>,
    writer_rx: mpsc::Receiver<Vec<u8>>,
}

impl PtyManager {
    pub fn spawn(&mut self, id: Uuid, cmd: &str, args: &[String], cwd: &str, size: PtySize) -> Result<(), Error>;
    pub fn write(&self, id: Uuid, data: &[u8]) -> Result<(), Error>;
    pub fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), Error>;
    pub fn kill(&mut self, id: Uuid) -> Result<(), Error>;
}
```

#### `commands/session.rs` (Tauri IPC)

```rust
use tauri::State;

#[tauri::command]
pub async fn create_session(
    session_mgr: State<'_, SessionManager>,
    pty_mgr: State<'_, PtyManager>,
    profile_name: Option<String>,
) -> Result<SessionInfo, String>;

#[tauri::command]
pub async fn destroy_session(
    session_mgr: State<'_, SessionManager>,
    pty_mgr: State<'_, PtyManager>,
    id: String,
) -> Result<(), String>;

#[tauri::command]
pub async fn switch_session(
    session_mgr: State<'_, SessionManager>,
    id: String,
) -> Result<(), String>;

#[tauri::command]
pub async fn rename_session(
    session_mgr: State<'_, SessionManager>,
    id: String,
    name: String,
) -> Result<(), String>;

#[tauri::command]
pub async fn list_sessions(
    session_mgr: State<'_, SessionManager>,
) -> Result<Vec<SessionInfo>, String>;

#[tauri::command]
pub async fn pty_write(
    pty_mgr: State<'_, PtyManager>,
    session_id: String,
    data: Vec<u8>,
) -> Result<(), String>;

#[tauri::command]
pub async fn pty_resize(
    pty_mgr: State<'_, PtyManager>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String>;
```

#### `main.rs` — Setup de Tauri Multiventana

```rust
fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // Crear ventana Sidebar
            let sidebar = tauri::WebviewWindowBuilder::new(
                app,
                "sidebar",
                tauri::WebviewUrl::App("index.html?window=sidebar".into()),
            )
            .title("agentscommander")
            .inner_size(280.0, 600.0)
            .min_inner_size(200.0, 400.0)
            .decorations(false)   // Custom titlebar
            .transparent(true)
            .always_on_top(false)
            .build()?;

            // Crear ventana Terminal
            let terminal = tauri::WebviewWindowBuilder::new(
                app,
                "terminal",
                tauri::WebviewUrl::App("index.html?window=terminal".into()),
            )
            .title("Terminal")
            .inner_size(900.0, 600.0)
            .min_inner_size(400.0, 300.0)
            .decorations(false)   // Custom titlebar
            .build()?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::session::create_session,
            commands::session::destroy_session,
            commands::session::switch_session,
            commands::session::rename_session,
            commands::session::list_sessions,
            commands::pty::pty_write,
            commands::pty::pty_resize,
            commands::config::get_config,
            commands::config::set_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running application");
}
```

### Frontend — Tipos TypeScript Compartidos

```typescript
// src/shared/types.ts

export interface Session {
  id: string;
  name: string;
  shell: string;
  shellArgs: string[];
  groupId: string | null;
  createdAt: string;
  workingDirectory: string;
  color: string | null;
  status: SessionStatus;
}

export type SessionStatus = 'active' | 'running' | 'idle' | { exited: number };

export interface SessionGroup {
  id: string;
  name: string;
  color: string;
  collapsed: boolean;
  order: string[];
}

export interface ShellProfile {
  name: string;
  command: string;
  args: string[];
  icon: string;
  color: string;
  env: Record<string, string>;
  workingDirectory: string;
}

export interface AppConfig {
  general: GeneralConfig;
  sidebar: SidebarConfig;
  terminal: TerminalConfig;
  keybindings: Record<string, string>;
}

export interface Theme {
  metadata: { name: string; author: string; version: string };
  colors: Record<string, string>;
  fonts: {
    terminalFamily: string;
    terminalSize: number;
    terminalLineHeight: number;
    uiFamily: string;
    uiSize: number;
  };
}
```

### Frontend — IPC Wrapper

```typescript
// src/shared/ipc.ts

import { invoke } from '@tauri-apps/api/core';
import { listen, emit } from '@tauri-apps/api/event';
import type { Session, SessionGroup, AppConfig } from './types';

export const SessionAPI = {
  create: (profileName?: string) =>
    invoke<Session>('create_session', { profileName }),

  destroy: (id: string) =>
    invoke<void>('destroy_session', { id }),

  switch: (id: string) =>
    invoke<void>('switch_session', { id }),

  rename: (id: string, name: string) =>
    invoke<void>('rename_session', { id, name }),

  list: () =>
    invoke<Session[]>('list_sessions'),
};

export const PtyAPI = {
  write: (sessionId: string, data: Uint8Array) =>
    invoke<void>('pty_write', { sessionId, data: Array.from(data) }),

  resize: (sessionId: string, cols: number, rows: number) =>
    invoke<void>('pty_resize', { sessionId, cols, rows }),
};

export const ConfigAPI = {
  get: () => invoke<AppConfig>('get_config'),
  set: (config: Partial<AppConfig>) => invoke<void>('set_config', { config }),
};

// Event listeners
export function onPtyOutput(callback: (data: { sessionId: string; data: number[] }) => void) {
  return listen<{ sessionId: string; data: number[] }>('pty_output', (event) => {
    callback(event.payload);
  });
}

export function onSessionCreated(callback: (session: Session) => void) {
  return listen<Session>('session_created', (e) => callback(e.payload));
}

export function onSessionDestroyed(callback: (data: { id: string }) => void) {
  return listen<{ id: string }>('session_destroyed', (e) => callback(e.payload));
}

export function onSessionSwitched(callback: (data: { id: string }) => void) {
  return listen<{ id: string }>('session_switched', (e) => callback(e.payload));
}
```

---

## Requisitos de UI/UX

### Estética General

- **Tono visual**: Industrial-dark. No generic dark mode. Pensá en el dashboard de un spaceship, no en "otra app Electron oscura".
- **Font del UI**: Una sans-serif con personalidad (ej: "Geist", "Outfit", "General Sans"). NO usar Inter, Roboto, Arial.
- **Font del terminal**: Cascadia Code (default, incluida con Windows Terminal) con fallback a JetBrains Mono.
- **Bordes**: Mínimos. Usar separación por color/opacidad en lugar de líneas.
- **Animaciones**: Suaves pero perceptibles. Transiciones de 150-200ms para cambios de estado. Ease-out para entradas, ease-in para salidas.
- **Iconos**: Lucide icons o similar set minimalista. No usar emoji como iconos.

### Detalles de Micro-interacción

- Hover en sesión: background sutil + ligero scale (1.01).
- Sesión activa: barra lateral izquierda de 3px con el color de acento.
- Drag & drop: sesión se eleva con sombra, hueco placeholder aparece donde se va a soltar.
- Switch de sesión: terminal hace un fade/crossfade rápido (100ms).
- Nueva sesión: aparece con slide-down + fade-in.
- Cerrar sesión: collapse hacia arriba + fade-out.
- Grupo colapsado: rotación del ícono de flecha con transición.

### Responsive

- El sidebar tiene un ancho mínimo de 200px y máximo de 400px.
- Si el sidebar se hace muy angosto (<220px), los textos secundarios (shell type) se ocultan.
- La terminal window es completamente independiente y se comporta como cualquier ventana de Windows.

---

## Fases de Desarrollo

### Fase 1 — MVP Core (Prioridad máxima)

- [ ] Setup del proyecto Tauri 2 + SolidJS + TypeScript.
- [ ] Backend: SessionManager básico (create, destroy, list, switch).
- [ ] Backend: PtyManager con portable-pty (spawn, read, write, resize).
- [ ] Backend: Tauri commands + events para IPC.
- [ ] Frontend Sidebar: Lista de sesiones funcional (sin grupos ni drag&drop).
- [ ] Frontend Terminal: xterm.js con WebGL addon, conectado al PTY.
- [ ] Multiventana: Sidebar y Terminal como ventanas Tauri separadas, sincronizadas.
- [ ] Custom titlebar en ambas ventanas.
- [ ] Keyboard shortcuts básicos (nueva sesión, cerrar, switch).

### Fase 2 — Funcionalidad Completa

- [ ] Grupos de sesiones con collapse/expand.
- [ ] Drag & drop para reordenar sesiones y mover entre grupos.
- [ ] Perfiles de shell (PowerShell, CMD, WSL, Git Bash).
- [ ] Renombrar sesiones (inline edit).
- [ ] Menú contextual (click derecho).
- [ ] Búsqueda de sesiones en sidebar.
- [ ] Split panes en la terminal window.
- [ ] Status bar en la terminal.
- [ ] Persistencia de sesiones y layout en TOML.

### Fase 3 — Pulido y Personalización

- [ ] Sistema de temas completo (carga desde TOML).
- [ ] 3 temas incluidos (Noir, Cyberpunk, Solarized Dark).
- [ ] Configuración global editable desde UI (ventana de settings).
- [ ] System tray con menú contextual.
- [ ] Always-on-top toggle para sidebar.
- [ ] Opacidad/transparencia configurable.
- [ ] Persistencia de posición y tamaño de ventanas.
- [ ] Auto-start con Windows (opcional).
- [ ] Keybindings configurables desde UI.
- [ ] xterm.js addons: search, ligatures, web links.

### Fase 4 — Extras

- [ ] Exportar/importar configuración.
- [ ] Session history (log de comandos ejecutados).
- [ ] Notificaciones cuando un proceso largo termina.
- [ ] Snippets/Quick commands (enviar un comando predefinido a la sesión activa).
- [ ] Soporte multiplataforma (Linux, macOS).

---

## Dependencias del Proyecto

### Rust (Cargo.toml)

```toml
[dependencies]
tauri = { version = "2", features = ["tray-icon", "protocol-asset"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
portable-pty = "0.8"
uuid = { version = "1", features = ["v4", "serde"] }
toml = "0.8"
chrono = { version = "0.4", features = ["serde"] }
dirs = "5"
log = "0.4"
env_logger = "0.11"
thiserror = "2"
```

### Node (package.json)

```json
{
  "dependencies": {
    "@tauri-apps/api": "^2",
    "@tauri-apps/plugin-shell": "^2",
    "@xterm/xterm": "^5",
    "@xterm/addon-webgl": "^0.18",
    "@xterm/addon-fit": "^0.10",
    "@xterm/addon-search": "^0.15",
    "@xterm/addon-web-links": "^0.11",
    "@xterm/addon-ligatures": "^0.9",
    "solid-js": "^1.9"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2",
    "typescript": "^5",
    "vite": "^6",
    "vite-plugin-solid": "^2"
  }
}
```

---

## Instrucciones para el LLM

1. **Empezá siempre por la Fase 1.** No implementes features de fases posteriores hasta que el MVP funcione end-to-end.
2. **Testeá cada módulo de Rust de forma aislada** antes de conectarlo al frontend.
3. **El flujo PTY es crítico**: spawn → read loop (async) → emit evento → xterm.js renderiza. Si esto no funciona, nada funciona. Priorizalo.
4. **Usá `portable-pty`** para el manejo de PTY, NO intentes usar ConPTY directamente salvo que haya un bug bloqueante.
5. **Las dos ventanas (sidebar y terminal) deben ser WebviewWindows separadas en Tauri**, no tabs ni iframes. Usan el mismo bundle de frontend pero cargan componentes distintos según un query parameter (`?window=sidebar` vs `?window=terminal`).
6. **Todos los tipos deben estar definidos en `shared/types.ts`** y los structs equivalentes en Rust deben ser serializables con serde para que el IPC sea type-safe.
7. **No uses ningún framework CSS**. CSS vanilla con variables. El theming se logra inyectando CSS variables desde el tema TOML.
8. **xterm.js debe usar el WebGL addon** para rendering. Fallback al canvas renderer si WebGL no está disponible.
9. **Persistí todo en archivos TOML** en `~/.agentscommander/`. No uses bases de datos ni localStorage.
10. **Custom titlebar**: Ambas ventanas deben tener `decorations: false` en Tauri y un titlebar custom en HTML/CSS que soporte drag (usando `data-tauri-drag-region`).
