# Historial de cambios

Todas las versiones notables de **pigmacs**. El formato sigue
[Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/) y el proyecto usa
[Versionado Semántico](https://semver.org/lang/es/).

## [4.0.0] — 2026-10-31

Movimiento **plano** en ficheros `.org` grandes: se acaba con la latencia
de hasta ~10 s por pulsación.

### Arreglado

- **Coste cuadrático por tecla** en ficheros grandes: `is_hidden()` y
  `fold_end()` recorrían el fichero entero por consulta y se consultaban
  decenas de veces por pulsación (movimiento, scroll y pintado) → O(n²)
  que empeoraba cuanto más abajo estaba el cursor (160k líneas: ~2,3 s por
  tecla; ahora **~1,6 ms**, plano en cualquier tamaño).
- `zc`/`zo` mutaban el conjunto de pliegues sin reconstruir la visibilidad
  cacheada (en la nueva caché habría dejado secciones «invisibles» hasta el
  siguiente plegado); ahora pasan por `toggle_fold()`.

### Añadido

- Caché de visibilidad `reconstruir_visibles()`: dos pasadas de pila
  monótona que generan `ocultas`, `visibles` (búsqueda binaria) y `cov`;
  sólo se reconstruye al cambiar pliegues o la estructura de líneas.
- Cachés de búsqueda (`matches_query`, `matches_ver`, `texto_ver`) para no
  reescanear el fichero en cada frame y resaltado de coincidencias con
  búsqueda binaria por celda.
- Techo de memoria del deshacer (`UNDO_MAX_BYTES` = 64 MiB): cada snapshot
  clona el fichero entero.
- Sesión 4 «plegues» en `tests/smoke_fase1.py` (11 checks: `Tab`,
  `za`/`zc`/`zo`/`zr`/`zm`, revelado al buscar dentro de un pliegue y `dd`
  sobre una cabecera plegada).
- `CHANGELOG.md` y `CONTRIBUTING.md`.

### Cambiado

- Versión global `4.0.0` en `Cargo.toml`, cabeceras de `src/` y `tests/`,
  README, `AGENTS.md` e informe de UX.

### Notas

- Sin dependencias nuevas (siguen `anyhow`, `crossterm`, `portable-pty`).
- Sin cambios de atajos ni de interfaz.
- 126/126 checks en verde; `cargo build` sin warnings (debug y release).

## [3.1.0] — 2026-10-15

Ronda de elegancia «tty puro» y pendientes #A/#C del informe.

### Añadido

- `src/theme.rs`: paleta semántica única de 16 colores ANSI; todo el pintado
  pasa por ella.
- Resaltado de sintaxis en bloques `#+BEGIN_SRC` (python, bash, rust,
  c/cpp, js/ts y fallback).
- Menú difuso `Ctrl+P` (`htp` → `htop`); Enter lanza lo tecleado si no hay
  coincidencias.
- Tree con banda de cabecera, directorios en color y contador `★N`.
- Aviso de fin de la IA (bell + banda de estado).

### Cambiado

- Barra de estado: píldora de modo con color, `●` de cambios e identidad,
  fichero y posición a la derecha.
- Selecciones con fondo de acento (no vídeo inverso).

### Arreglado

- Pánico por resta sin saturar en la barra de ayuda tras un `resize` en modo
  debug.

## [3.0.1] — 2026-10-14

### Añadido

- Cabecera estándar (proyecto · autor · licencia · versión · fecha) en todos
  los ficheros de `src/` y `tests/`.
- Fichero `LICENSE` (MIT) y campos `authors`/`license` en `Cargo.toml`.

## [3.0.0] — 2026-10-13

### Añadido

- El directorio de trabajo del proceso sigue al tree (`chdir` + `$PWD`), en
  el que confían programas como ranger.
- `Ctrl+T` de ida y vuelta al modo anterior.
- `F1` de ayuda en todos los paneles (incluido el shell).
- `MENU` con argumentos (nvim, lynx/w3m con buscador) y 15 programas.

## [2.0.0]

### Cambiado

- Partida del `main.rs` monolítico (4016 líneas) en módulos temáticos:
  `app`, `ui`, `frame`, `term`, `shell`, `program`, `launcher`, `editor`,
  `browser`, `ai`, `theme`.

## [1.0.1] — 2026-08-22

Primera release pública con binario: editor modal para `.org`, shell PTY
integrado, panel de IA, navegador de ficheros, plegado de cabeceras y
deshacer.

[4.0.0]: https://github.com/kidandromeda/pigmacs/compare/v3.1.0...v4.0.0
[3.1.0]: https://github.com/kidandromeda/pigmacs/compare/v3.0.0...v3.1.0
[3.0.1]: https://github.com/kidandromeda/pigmacs/compare/v3.0.0...v3.0.1
[3.0.0]: https://github.com/kidandromeda/pigmacs/compare/v2.0.0...v3.0.0
[2.0.0]: https://github.com/kidandromeda/pigmacs/compare/v1.0.1...v2.0.0
[1.0.1]: https://github.com/kidandromeda/pigmacs/releases/tag/v1.0.1
