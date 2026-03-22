# PLAN: Rename summongate -> agentscommander

## Resumen

Renombrar todas las referencias de "summongate" / "SummonGate" a "agentscommander" / "AgentsCommander" en el proyecto. Incluye código, config, scripts, docs, paths de filesystem, y el repo de GitHub.

---

## Mapeo de nombres

| Contexto | Antes | Después |
|----------|-------|---------|
| Nombre del binario | `summongate.exe` | `agentscommander.exe` |
| Product name (Tauri) | `SummonGate` | `AgentsCommander` |
| Identifier | `com.summongate.app` | `com.agentscommander.app` |
| Package name (npm) | `summongate` | `agentscommander` |
| Crate name (Rust) | `summongate` | `agentscommander` |
| Lib name (Rust) | `summongate_lib` | `agentscommander_lib` |
| Config dir | `~/.summongate/` | `~/.agentscommander/` |
| Per-repo config | `.summongate/config.json` | `.agentscommander/config.json` |
| HTML title | `summongate` | `agentscommander` |
| Titlebar label | `summongate` | `agentscommander` |

---

## Archivos a modificar (24 archivos, ~69 ocurrencias)

### Críticos (rompen build si no se cambian)

| Archivo | Ocurrencias | Qué cambiar |
|---------|-------------|-------------|
| `src-tauri/Cargo.toml` | 2 | `name = "summongate"`, `name = "summongate_lib"` |
| `src-tauri/src/main.rs` | 1 | `summongate_lib::run()` |
| `src-tauri/tauri.conf.json` | 2 | `productName`, `identifier` |
| `package.json` | 1 | `"name": "summongate"` |
| `package-lock.json` | 2 | `"name": "summongate"` (regenerar con `npm install`) |
| `src-tauri/capabilities/default.json` | 1 | description |
| `src-tauri/gen/schemas/capabilities.json` | 1 | auto-generado, se regenera |

### Código Rust

| Archivo | Ocurrencias | Qué cambiar |
|---------|-------------|-------------|
| `src-tauri/src/lib.rs` | 1 | `.title("summongate")` |
| `src-tauri/src/config/settings.rs` | 5 | `".summongate"` en paths y comments |
| `src-tauri/src/commands/session.rs` | 2 | `".summongate"` path |
| `src-tauri/src/telegram/bridge.rs` | 3 | `".summongate"` path + log message |
| `src-tauri/src/telegram/types.rs` | 1 | comment `.summongate/config.json` |
| `src-tauri/src/commands/telegram.rs` | 1 | `"summongate connected"` message |

### Frontend

| Archivo | Ocurrencias | Qué cambiar |
|---------|-------------|-------------|
| `index.html` | 1 | `<title>summongate</title>` |
| `src/sidebar/components/Titlebar.tsx` | 1 | label en titlebar |

### Scripts

| Archivo | Ocurrencias | Qué cambiar |
|---------|-------------|-------------|
| `scripts/kill-dev.ps1` | 3 | `summongate.exe` -> `agentscommander.exe` |
| `scripts/kill-dev.sh` | 1 | `summongate.exe` -> `agentscommander.exe` |

### CI/CD

| Archivo | Ocurrencias | Qué cambiar |
|---------|-------------|-------------|
| `.github/workflows/release.yml` | 2 | `releaseName`, `releaseBody` |

### Documentación

| Archivo | Ocurrencias | Qué cambiar |
|---------|-------------|-------------|
| `README.md` | 5 | Título, descripciones |
| `CLAUDE.md` | 9 | Nombre del proyecto, paths, referencias |
| `PLAN-telegram-bridge.md` | 7 | Referencias |
| `DIAG-telegram-emission.md` | 6 | Referencias |
| `win-nerds-tab-prompt.md` | 10 | Referencias |

### Cargo.lock

| Archivo | Ocurrencias | Qué cambiar |
|---------|-------------|-------------|
| `src-tauri/Cargo.lock` | 1 | Se regenera automáticamente con `cargo build` |

---

## Orden de ejecución

1. **Rust core** - Cargo.toml, main.rs, lib.rs, settings.rs, session commands, telegram modules
2. **Tauri config** - tauri.conf.json, capabilities
3. **Frontend** - index.html, Titlebar.tsx
4. **npm** - package.json, luego `npm install` para regenerar package-lock.json
5. **Scripts** - kill-dev.ps1, kill-dev.sh
6. **CI** - release.yml
7. **Docs** - README, CLAUDE.md, plans, diags
8. **Verificar build** - `cargo build` (regenera Cargo.lock + schemas)
9. **Migración de datos del usuario** - Renombrar `~/.summongate/` a `~/.agentscommander/`

---

## Consideraciones

- **Cargo.lock y gen/schemas**: Se regeneran automáticamente. No editar a mano.
- **package-lock.json**: Correr `npm install` después de cambiar package.json.
- **Binario PROD instalado**: Si el usuario tiene summongate.exe en Program Files, no se toca. El rename aplica solo al proyecto. El usuario reinstala la nueva versión.
- **Config existente**: `~/.summongate/` necesita migración. Opciones:
  - Renombrar manualmente la carpeta
  - Agregar fallback en settings.rs: si `~/.agentscommander/` no existe pero `~/.summongate/` si, copiar
- **GitHub repo**: Renombrar via GitHub Settings (no es parte de este plan de código)
- **summongate-prompt.md**: Revisar si también necesita rename (es el spec original)
