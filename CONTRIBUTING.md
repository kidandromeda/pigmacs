# Contribuir a pigmacs

Gracias por asomarte. Este proyecto es pequeño y con reglas claras; antes de
tocar código lee **`AGENTS.md`**, que es la guía de referencia para personas y
agentes.

## Antes de enviar un cambio

1. **Compila sin warnings**: `cargo build` (y `cargo build --release` si
   cambias algo del lanzador gráfico).
2. **Pasa las tres suites** (desde `tests/`):

   ```bash
   cd tests
   python3 test_partA.py      #  6 checks
   python3 smoke_fase1.py     # 64 checks
   python3 test_fase2.py      # 56 checks
   ```

   Deben dar **0 fallos** (126/126). Para probar otro binario:
   `PIGMACS_BIN=/ruta/binario python3 test_*.py`.
3. **Actualiza la documentación** si cambias atajos o interfaz:
   `HELP_TEXT` (`src/ui.rs`), `README.md` y `informe_ux_terminal.org`.
4. **Añade una entrada en `CHANGELOG.md`** bajo `## [No publicado]` (o en la
   versión que corresponda).

## Convenciones

- **Español** en código, comentarios, documentación, tests y mensajes de
  commit.
- Prefijos de commit: `pigmacs:` (código), `docs:`, `chore:`.
- Sin dependencias nuevas salvo petición explícita (hoy son sólo tres:
  `anyhow`, `crossterm`, `portable-pty`).
- Todo el pintado pasa por `src/theme.rs` (16 colores ANSI, sólo tty real).
- No se rompen las cadenas que usan los tests (`NORMAL`, `▸ htop`, …).

## Versiones

El número de versión vive en `Cargo.toml` y se propaga a:

- las cabeceras de `src/*.rs` y `tests/*.py`,
- la barra de estado (lee `CARGO_PKG_VERSION`),
- `README.md` (ejemplo de instalación e identidad),
- `AGENTS.md` (sección de estructura),
- `CHANGELOG.md`.

## Ramas y releases

- `main` es la rama estable; el trabajo experimental va en ramas
  `experimental/*` y se fusiona cuando está probado.
- Las releases son **tags anotados** `vX.Y.Z` sobre `main` más una release de
  GitHub con el binario `pigmacs` adjunto y su `sha256` en las notas.
