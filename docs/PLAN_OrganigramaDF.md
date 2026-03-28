# PLAN: Organigrama Dark Factory

## Objetivo

Crear una vista de **organigrama horizontal (izquierda a derecha)** accesible desde la Sidebar, que muestre la estructura jerarquica de equipos de agentes organizados en **Layers**. Cada nodo del organigrama es un Team con un Coordinator que actua como punto de entrada de solicitudes y canal de escalamiento.

---

## Conceptos Clave

### Layers
- Niveles jerarquicos del organigrama (Layer 1, Layer 2, Layer 3...)
- Configurables desde Settings > Dark Factory
- Cada Layer tiene un nombre (ej: "C-Suite", "Directors", "Teams")
- Los Teams se asignan a una Layer
- **El orden se determina por la posicion en el array `layers[]`** — no hay campo `order` explicito. El indice 0 es el mas a la izquierda (jerarquia mas alta).

### Teams (ya existente, se extiende)
- Cada Team tiene: nombre, miembros, coordinador
- **Nuevo**: cada Team se asigna a una Layer (`layerId`)
- El coordinador es la via de ingreso de solicitudes al equipo

### Vinculos Coordinator-to-Coordinator
- Un coordinador de Layer N puede tener **supervisores** en Layer N-1
- Un coordinador de Layer N puede **supervisar** coordinadores en Layer N+1
- Estos vinculos definen:
  - Quien puede dar indicaciones al coordinador (supervisor → coordinador)
  - A quien escala informacion el coordinador (coordinador → supervisor)

### Alcance de CoordinatorLink
`CoordinatorLink` es **puramente visual** en esta primera version — define las lineas del organigrama pero **no afecta** el routing del sistema de mensajeria (`can_communicate` en `phone/manager.rs`). El sistema de comunicacion sigue usando las reglas existentes (mismo team + coordinator gating). En una version futura se puede evaluar si los links deben propagarse a los `config.json` de cada agente para habilitar comunicacion cross-team via coordinadores.

---

## Modelo de Datos

### Cambios en `types.ts`

```typescript
// Nueva interfaz
export interface DarkFactoryLayer {
  id: string;
  name: string;       // ej: "C-Suite", "Directors", "Operations"
  // El orden se determina por la posicion en el array layers[]
}

// Nueva interfaz para vinculos entre coordinadores
export interface CoordinatorLink {
  supervisorTeamId: string;    // team del supervisor (layer superior)
  subordinateTeamId: string;   // team del supervisado (layer inferior)
}

// Extender Team existente
export interface Team {
  id: string;
  name: string;
  members: TeamMember[];
  coordinatorName?: string;
  layerId?: string;           // NUEVO: a que layer pertenece
}

// Extender DarkFactoryConfig existente
export interface DarkFactoryConfig {
  teams: Team[];
  layers?: DarkFactoryLayer[];         // NUEVO (optional para migracion)
  coordinatorLinks?: CoordinatorLink[]; // NUEVO (optional para migracion)
}
```

### Cambios en Rust (backend) — `src-tauri/src/config/dark_factory.rs`

```rust
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DarkFactoryLayer {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorLink {
    pub supervisor_team_id: String,
    pub subordinate_team_id: String,
}

// Extender Team existente — CRITICO: serde attributes para backward compat
pub struct Team {
    // ... campos existentes ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_id: Option<String>,
}

// Extender DarkFactoryConfig — CRITICO: #[serde(default)] en campos nuevos
pub struct DarkFactoryConfig {
    pub teams: Vec<Team>,
    #[serde(default)]
    pub layers: Vec<DarkFactoryLayer>,
    #[serde(default)]
    pub coordinator_links: Vec<CoordinatorLink>,
}
```

> **CRITICO**: Sin `#[serde(default)]` en `layers` y `coordinator_links`, y sin `#[serde(default, skip_serializing_if = "Option::is_none")]` en `layer_id`, los `teams.json` existentes fallan al deserializar y `load_dark_factory()` retorna `DarkFactoryConfig::default()`, **perdiendo silenciosamente todos los teams existentes**.

---

## Arquitectura de la Vista

### Acceso: Boton en Sidebar

**Ubicacion**: entre el boton de Bombilla (Hints) y el boton de Settings en `TeamFilter.tsx`.

```
[Eye] [Lightbulb] [🏭 NEW] [Settings]
```

- Icono: `&#x1F3ED;` (fabrica) o un SVG custom minimalista
- Tooltip: "Dark Factory"
- Al hacer click: abre una **nueva ventana Tauri** (como Guide), no un modal

### Nueva ventana: Dark Factory

Similar a la ventana Guide (`?window=darkfactory`):

1. **Registro en `main.tsx`**: agregar caso `windowType === "darkfactory"`
2. **Comando Rust**: `open_darkfactory_window` (similar a `open_guide_window`)
3. **Frontend**: `src/darkfactory/App.tsx`

**Singleton guard obligatorio**: el comando Rust debe verificar `app.get_webview_window("darkfactory")` antes de crear — si ya existe, solo hacer `set_focus()`. Esto previene multiples instancias con recalculo SVG costoso.

### Estructura de la ventana

```
DarkFactoryApp
├── Titlebar (icon + "dark factory" + minimize/close)
├── Toolbar (zoom controls, fullscreen toggle)
└── OrgChart (area principal scrolleable)
    ├── LayerColumn (por cada layer, de izq a der)
    │   ├── LayerHeader ("Layer 1: C-Suite")
    │   └── TeamNode[] (cards de cada team en esa layer)
    │       ├── TeamName
    │       ├── CoordinatorBadge
    │       └── MemberCount
    └── ConnectionLines (lineas SVG entre coordinadores vinculados)
```

---

## Plan de Implementacion

### Fase 1: Modelo de datos y Settings

**Archivos a modificar:**
- `src/shared/types.ts` — agregar `DarkFactoryLayer`, `CoordinatorLink`, extender `Team` y `DarkFactoryConfig`
- `src-tauri/src/config/dark_factory.rs` — structs Rust con atributos serde correctos (ver seccion Modelo de Datos)
- `src/sidebar/components/SettingsModal.tsx` — agregar UI para:
  - CRUD de Layers (nombre, reordenar arrastrando o con botones up/down)
  - Asignar Team a Layer (dropdown en cada team card)
  - Crear vinculos entre coordinadores (selector "Reports to")
  - **FIX: cambiar `DarkFactoryAPI.save({ teams: [...dfConfig.teams] })` a `DarkFactoryAPI.save({ ...dfConfig })`** para no descartar los campos nuevos

**Detalle de UI en Settings > Dark Factory:**

```
[Layers]
  ┌──────────────────────────────────┐
  │ Layer 1: C-Suite        [Edit][X]│
  │ Layer 2: Directors      [Edit][X]│
  │ Layer 3: Operations     [Edit][X]│
  │         [+ Add Layer]            │
  └──────────────────────────────────┘

[Teams]  (existente, se extiende)
  ┌──────────────────────────────────┐
  │ Team: Alpha Squad                │
  │ Layer: [dropdown: Layer 2 ▼]     │
  │ Coordinator: AgentX              │
  │ Reports to: [dropdown: CEO Team] │
  │ Members: 4                       │
  └──────────────────────────────────┘
```

El campo "Reports to" crea un `CoordinatorLink` donde `supervisorTeamId` es el team seleccionado y `subordinateTeamId` es el team actual.

**Paso 1b — Auditar `sync_agent_configs`**: verificar que los nuevos campos (`layers`, `coordinator_links`) no interfieren con `sync_agent_configs()` en `dark_factory.rs:139`. Esta funcion solo itera `config.teams` asi que es seguro, pero debe confirmarse con un test manual.

### Fase 2: Boton en Sidebar + Ventana Tauri + Zoom

**Archivos a crear:**
- `src/darkfactory/App.tsx` — componente principal
- `src/darkfactory/styles/darkfactory.css` — estilos (importar `../../terminal/styles/variables.css`)
- `src/darkfactory/components/OrgChart.tsx` — layout del organigrama
- `src/darkfactory/components/LayerColumn.tsx` — columna por layer
- `src/darkfactory/components/TeamNode.tsx` — card de cada team
- `src/darkfactory/components/ConnectionLines.tsx` — lineas SVG

**Archivos a modificar:**
- `src/main.tsx` — agregar caso `"darkfactory"`
- `src/sidebar/components/TeamFilter.tsx` — agregar boton Dark Factory
- `src/shared/ipc.ts` — agregar `DarkFactoryAPI.openWindow()`
- `src-tauri/src/commands/window.rs` — agregar `open_darkfactory_window` con singleton guard
- `src-tauri/src/lib.rs` — registrar el nuevo command (no se necesitan cambios en `commands/mod.rs` porque `window.rs` ya es modulo)
- `src-tauri/tauri.conf.json` — verificar capabilities (mismos permisos que Guide)

**Paso 2b — Zoom system** (archivos adicionales que el plan original omitia):

| Archivo | Cambio |
|---------|--------|
| `src/shared/zoom.ts` | Agregar `"darkfactory"` a `WindowType`, agregar mapping a `darkfactoryZoom` en `debouncedSave` |
| `src/shared/types.ts` (`AppSettings`) | Agregar `darkfactoryZoom: number` |
| `src-tauri/src/config/settings.rs` | Agregar `#[serde(default = "default_zoom")] pub darkfactory_zoom: f64` |

> **Oportunidad**: fix del bug preexistente donde `"guide"` cae al branch `else` en `zoom.ts` y escribe a `terminalZoom`. Agregar `guideZoom` junto con `darkfactoryZoom` de una sola vez.

### Fase 3: Render del Organigrama

**Layout engine** (CSS Grid + SVG):

```
┌─────────────────────────────────────────────────────────────┐
│                     DARK FACTORY                            │
│                                                             │
│  Layer 1          Layer 2           Layer 3                 │
│  ┌────────┐      ┌────────┐       ┌────────┐              │
│  │  CEO   │──────│  CTO   │───────│ Team A │              │
│  │  Team  │      │  Team  │       └────────┘              │
│  └────────┘      └────────┘       ┌────────┐              │
│                  │  │              │ Team B │              │
│                  │  └─────────────│        │              │
│                  │                 └────────┘              │
│                  ┌────────┐       ┌────────┐              │
│                  │  COO   │───────│ Team C │              │
│                  │  Team  │       └────────┘              │
│                  └────────┘                                │
└─────────────────────────────────────────────────────────────┘
```

**Estrategia de rendering:**

1. **Columnas con CSS Grid**: cada Layer es una columna. `grid-template-columns: repeat(N, 1fr)` donde N = cantidad de layers
2. **Nodos con Flexbox**: dentro de cada columna, los teams se apilan verticalmente con gap
3. **Conexiones con SVG overlay**: un `<svg>` posicionado `absolute` sobre todo el grid. Las lineas se calculan a partir de las posiciones DOM de los nodos (via `getBoundingClientRect`)
4. **Scroll horizontal**: si hay muchas layers, el contenedor tiene `overflow-x: auto`

**Calculo de posiciones para lineas:**
- Cada `TeamNode` reporta su posicion al padre via callback `onMount`
- `ConnectionLines` recibe array de `CoordinatorLink` + mapa de posiciones
- Dibuja paths SVG curvos (bezier) de punto medio derecho del nodo origen a punto medio izquierdo del nodo destino

### Fase 4: Interactividad

- **Hover en TeamNode**: resalta las conexiones de ese team
- **Click en TeamNode**: muestra panel lateral con detalle (miembros, coordinador, vinculos)
- **Zoom**: ctrl+scroll o botones +/- en toolbar (persistido via `darkfactoryZoom`)
- **Pan**: drag en area vacia para mover viewport
- **Responsive**: font-size y node-size se ajustan al zoom

---

## Detalle de Componentes

### `TeamNode.tsx`

```
┌─────────────────────┐
│ ★ Coordinator Name  │  ← badge si es coordinador
│ ─────────────────── │
│ Team Name           │  ← nombre del equipo
│ 4 members           │  ← conteo
│ Layer 2             │  ← indicador de layer
└─────────────────────┘
```

- Color de borde izquierdo: hereda del color del agente coordinador (si tiene sesion activa)
- Estado: opacidad reducida si ningun miembro tiene sesion activa
- Tamano fijo: ~180px ancho, ~80px alto (escalable con zoom)

### `LayerColumn.tsx`

- Header fijo arriba con nombre de layer
- Contenedor flex vertical con los TeamNodes
- Fondo sutilmente diferenciado por layer (alternating opacity)

### `ConnectionLines.tsx`

- SVG overlay `position: absolute; inset: 0; pointer-events: none`
- Paths con curva bezier horizontal
- Color: `var(--statusbar-fg)` por defecto, `var(--statusbar-accent)` en hover
- Grosor: 1.5px, 2.5px en hover
- Animacion: transicion de color/grosor 150ms ease-out

> **Nota**: las variables correctas son `--statusbar-fg` y `--statusbar-accent` (definidas en `src/terminal/styles/variables.css`). No existen `--text-dim` ni `--accent` en el codebase.

---

## Configuracion en Settings

### Seccion "Layers" (nueva en Dark Factory tab)

| Campo | Tipo | Descripcion |
|-------|------|-------------|
| Layer name | text input | Nombre descriptivo del layer |
| Posicion | array index | Determinada por orden en la lista (drag o botones ↑↓) |

### Campos nuevos en Team card (existente)

| Campo | Tipo | Descripcion |
|-------|------|-------------|
| Layer | select | Dropdown con layers disponibles |
| Reports to | select | Dropdown con teams de layers superiores (cuyos coordinadores supervisan a este) |

### Validaciones

- Un team solo puede "reportar a" un team de una layer con **indice menor** en el array (superior jerarquicamente) — sin esta validacion se pueden crear ciclos que rompen el layout del arbol
- Un team sin layer asignada no aparece en el organigrama
- Un layer sin teams aparece como columna vacia (placeholder)

---

## Orden de Ejecucion

| Paso | Descripcion |
|------|-------------|
| 1 | Extender tipos TS + Rust (`DarkFactoryLayer`, `CoordinatorLink`, extender `Team`, `DarkFactoryConfig`) con atributos serde correctos |
| 1b | Auditar `sync_agent_configs` — confirmar que nuevos campos no corrompen per-agent configs |
| 2 | UI en Settings: CRUD de Layers + asignar layer a team + selector "Reports to" + **fix save handler** |
| 3 | Boton en TeamFilter + comando Rust `open_darkfactory_window` con singleton guard |
| 3b | Zoom system: agregar `darkfactoryZoom` a `AppSettings` (TS + Rust) + extender `zoom.ts` WindowType |
| 4 | Scaffold ventana: `main.tsx` routing + `DarkFactoryApp` + titlebar basico |
| 5 | `OrgChart` con layout CSS Grid de layers |
| 6 | `TeamNode` cards con datos reales |
| 7 | `ConnectionLines` SVG con bezier curves |
| 8 | Interactividad: hover highlights, zoom, pan |
| 9 | Polish: animaciones, responsive, edge cases |

---

## Archivos Completos a Tocar

### Crear
| Archivo | Descripcion |
|---------|-------------|
| `src/darkfactory/App.tsx` | Componente principal de la ventana |
| `src/darkfactory/styles/darkfactory.css` | Estilos (importa `variables.css`) |
| `src/darkfactory/components/OrgChart.tsx` | Layout del organigrama |
| `src/darkfactory/components/LayerColumn.tsx` | Columna por layer |
| `src/darkfactory/components/TeamNode.tsx` | Card de cada team |
| `src/darkfactory/components/ConnectionLines.tsx` | Lineas SVG |

### Modificar
| Archivo | Cambio |
|---------|--------|
| `src/shared/types.ts` | `DarkFactoryLayer`, `CoordinatorLink`, extender `Team`, `DarkFactoryConfig`, `AppSettings` (+zoom) |
| `src/shared/ipc.ts` | `DarkFactoryAPI.openWindow()` |
| `src/shared/zoom.ts` | `"darkfactory"` en `WindowType` + mapping zoom key |
| `src/main.tsx` | Routing `"darkfactory"` → `DarkFactoryApp` |
| `src/sidebar/components/TeamFilter.tsx` | Boton Dark Factory |
| `src/sidebar/components/SettingsModal.tsx` | UI layers/links + **fix save `{ ...dfConfig }`** |
| `src-tauri/src/config/dark_factory.rs` | Structs Rust con `#[serde(default)]` |
| `src-tauri/src/config/settings.rs` | `darkfactory_zoom` field |
| `src-tauri/src/commands/window.rs` | `open_darkfactory_window` con singleton guard |
| `src-tauri/src/lib.rs` | Registrar nuevo command |
| `src-tauri/tauri.conf.json` | Verificar capabilities |

---

## Riesgos y Mitigaciones

| # | Riesgo | Severidad | Mitigacion |
|---|--------|-----------|------------|
| 1 | `teams.json` existentes fallan al deserializar campos nuevos | **HIGH** | `#[serde(default)]` en todos los campos nuevos de Rust. Testeado con archivo existente antes de merge |
| 2 | Save handler descarta `layers` y `coordinatorLinks` | **MEDIUM** | Cambiar spread explicito a `{ ...dfConfig }` en SettingsModal |
| 3 | Variables CSS inexistentes causan lineas invisibles | **MEDIUM** | Usar `--statusbar-fg` y `--statusbar-accent` (verificados en `variables.css`) |
| 4 | Zoom no persiste en Dark Factory window | **MEDIUM** | Agregar `darkfactoryZoom` a ambos lados (TS + Rust) + extender `zoom.ts` |
| 5 | Multiples instancias de la ventana causan performance hit | **LOW** | Singleton guard `get_webview_window("darkfactory")` en comando Rust |
| 6 | Ciclos en CoordinatorLinks rompen layout de arbol | **LOW** | Validacion: "Reports to" solo permite teams de layers con indice menor |
| 7 | Rendimiento SVG con muchos nodos | **LOW** | Debounce 100ms en resize via `ResizeObserver` |
| 8 | `sync_agent_configs` podria fallar con campos desconocidos | **LOW** | Auditoria en paso 1b — funcion solo itera `config.teams`, es seguro |

---

## Decisiones Explicitas

1. **`CoordinatorLink` es cosmético**: no afecta `can_communicate` ni se propaga a per-agent `config.json`. Solo define las lineas del organigrama.
2. **Orden por posicion en array**: no hay campo `order` — el indice en `layers[]` determina la posicion visual. Reordenar = mover elementos en el array.
3. **Naming explicito**: `supervisorTeamId` / `subordinateTeamId` en vez de `from`/`to` para evitar ambiguedad.
4. **Ventana separada**: no es un modal ni un tab — es una ventana Tauri independiente como Guide.
