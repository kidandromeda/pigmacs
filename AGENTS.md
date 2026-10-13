# AGENTS.md

Guía para las IAs que desarrollen **pigmacs**. Léela entera antes de tocar código.

## Qué es

Editor TUI modal para ficheros `.org` (estilo emacs / pigmeo), escrito en Rust,
binario único sin dependencias de runtime:

- **shell embebido** en su propio PTY (`portable-pty`) con salida interpretada;
- **emulador VT100 propio** (CSI/OSC/SGR, scroll, pantalla alterna) para dibujar
  tanto el shell como los programas externos;
- **menú `Ctrl+P`** que lanza programas (htop, neovim, tmux…) sobre su propio
  PTY, en el área del editor, con las teclas cedidas salvo las reservadas;
- navegador de ficheros (tree), panel de IA, ayuda integrada;
- todo el render es una **rejilla de celdas propia pintada por diff**: nadie
  escribe a stdout salvo `render_frame()`.

Dependencias (sólo 3): `anyhow`, `crossterm 0.29`, `portable-pty 0.9`.

## Comandos

```bash
cargo build                # debug  → target/debug/pigmacs
cargo build --release      # release → target/release/pigmacs

# las 3 suites (ejecutar desde tests/); deben dar "0 fallos" (80 checks)
cd tests
python3 test_partA.py      #  6 checks — atajos y/render base
python3 smoke_fase1.py     # 39 checks — smoke de fases anteriores
python3 test_fase2.py      # 35 checks — menú, VT100, shell, Ctrl+J, resize…
```

- **No hay tests en Rust**: la garantía son las suites Python, que abren el
  binario en una PTY y comparan pantallas con un mini-emulador VT100
  acumulativo (`tests/ptylib.py`).
- `PIGMACS_BIN=/ruta/a/otro/binario python3 test_*.py` para probar otra
  compilación.
- **Regla de oro**: `cargo build` sin *warnings* + las 3 suites en verde
  **antes** de cada commit.

## Estructura de `src/` (v2.0.0, ~4140 líneas)

| módulo | líneas | contenido |
|---|---|---|
| `main.rs` | 710 | declaraciones `mod` y prelude (`use x::*`); `main()`, `run()` (bucle de eventos), atajos globales (`Shortcut`, `shortcut_of`, `shortcut`), `resize_border`, `handle_help_key`, `handle_sudo_key`, avisos de kernel (`running_on_vt`, `lower_kernel_printk`) |
| `app.rs` | 939 | estado central: `enum Mode`, `struct App` + **`impl App` completo** (teclado por modo, guardado, IA…), `layout()`, `file_writable()` |
| `ui.rs` | 345 | `draw()` compone el frame completo; `draw_help()`, `HELP_TEXT` |
| `frame.rs` | 149 | `Style`, `Cell`, `Frame` (rejilla), `render_frame()` (pintado por diff), `strip_ansi()` |
| `term.rs` | 679 | `Term` + `PState`: emulador VT100 completo (`feed`, `parse_sgr`, scroll, alt screen, línea de comandos…), `draw()` |
| `shell.rs` | 316 | `Shell` (PTY del shell): `key_bytes`/`key_bytes_mode` (tecla→bytes), `send_key_to_shell`, `csi_shell`, `draw_shell_output`, `resize` |
| `program.rs` | 222 | `Program` (PTY del programa), `MENU` (15 entradas: nombre visible + argv), `in_path()`; menú: `menu_entries`, `launch_program`, `close_program`, `handle_menu_key`, `draw_menu` |
| `editor.rs` | 467 | panel del editor: `handle_editor_key`, `handle_search_key`, `compute_matches`, `org_line_style`, `draw_editor_lines` |
| `browser.rs` | 186 | tree de ficheros: `Browser` (`refresh`, `cd`, `cursor_up/down`…), `handle_browser_key`, `draw_browser`, `browser_width` |
| `ai.rs` | 130 | panel de IA: `handle_ai_key`, `draw_ai_pane` |

Notas del split (v2.0.0): se partió un `main.rs` de 4016 líneas. Toda la
visibilidad entre módulos es `pub(crate)` **a propósito**; `main.rs` importa
con wildcards (`use modulo::*`). No «arquitecturar» de nuevo sin necesidad:
el split fue mecánico y deliberadamente aburrido.

## Cómo funciona

1. **Arranque** (`main()`): raw mode + alternate screen, `App::new()`, shell
   spawneado en un PTY con un hilo lector que acumula la salida en
   `Arc<Mutex<Vec<u8>>>`. Si corre en una VT real, mitiga los avisos de kernel
   del launcher (doble vía: `running_on_vt` / `lower_kernel_printk`).
2. **Bucle** (`run()`): `event::poll(20 ms)` → `shortcut_of()` gestiona los
   atajos globales → el resto pasa al `handle_*_key` del modo actual
   (`app.mode`); si algo cambia (`app.redraw` o crece la salida del shell/IA)
   se llama `draw(app)`, y también en cada timeout.
3. **Render**: `draw()` rellena un `Frame` (celdas + estilos) y
   `render_frame()` sólo escribe las celdas **diferentes** respecto al frame
   anterior, con `queue!` de crossterm.
4. **Shell**: teclas → `key_bytes()` → stdin del PTY; salida → `csi_shell()`
   (interpreta lo mínimo para dejar las líneas correctas).
5. **Programas (Ctrl+P)**: `launch_program` crea otro PTY; la salida pasa por
   **`Term` (VT100 completo)** y se dibuja sobre el área del editor. Mientras
   está activo, sus teclas ceden salvo las reservadas (ver abajo).
6. **Modos**: `Mode = Normal | Insert | Visual | Shell | Ai | Browser |
   Program`. `top_mode` guarda el contexto (p. ej. shell abierto encima de un
   programa que sigue en segundo plano).

## Atajos globales

`shortcut_of()` (`main.rs`): **Ctrl o Alt + letra** (XOR; AltGr no cuenta).

- `q` salir · `s` guardar · `p` menú · `g` cerrar programa · `t` terminal
  (ida y vuelta: desde el shell vuelve al modo anterior) · `v` editor ·
  `f` tree · `a` IA · `n` nueva IA · `e` enviar · `r` redo ·
  `k` limpiar shell · `u`/`d` bordes
- **`j` sólo con Ctrl**: `Ctrl+J` = volver al editor (`Shortcut::ToEditor`);
  `Alt+J` está sin asignar.

Reglas críticas que **no se deben romper**:

- **`Ctrl+J` es el escape de la tty real**: en consola de verdad `Shift+Tab`
  no llega (el teclado del kernel no genera `CSI Z`), pero el kernel sí entrega
  siempre el byte `0x0A`. Debe seguir funcionando desde SHELL y PROG.
- **`Ctrl+T` es toggle**: entra al shell y desde allí vuelve al modo del que
  se abrió (`top_mode`). `?` sólo abre la ayuda en el editor: en el shell y en
  el panel IA es imprimible y debe ceder.
- `Enter` sobre un programa **ya en marcha** en el menú recupera el foco
  **sin relanzarlo**.
- Con un programa activo sólo se consumen `Ctrl/Alt+Q/S/P/G`, `Ctrl+J` y
  `Shift+Tab`; el resto de teclas va al programa.

## Convenciones

- **Español** en código, comentarios, docs, tests y commits. Prefijos del
  historial: `pigmacs:` (código), `docs:`, `chore:`.
- **Git sólo local**: hacer commit está bien; **no hacer `push` ni releases a
  la nube sin que lo pida explícitamente el usuario**.
- Cambios de UI o de atajos → actualizar también `HELP_TEXT` (`src/ui.rs`),
  `README.md` e `informe_ux_terminal.org`.
- Menú: se edita la constante `MENU` (`program.rs`, 14 entradas); en pantalla
  sólo aparecen las que están en el PATH. Los tests sólo pueden asumir
  programas garantizados instalados (`htop`, `tmux`, `ranger`…).
- Dependencias nuevas: sólo si se pide (van 3 en total).

## Documentación

- `README.md` — manual de usuario: instalación, modos, atajos, tests.
- `informe_ux_terminal.org` — informe de UX: bugs corregidos y **§3 «Mejoras
  propuestas»** con TODOs priorizados (A/B/C): la mejor fuente de tareas nuevas.
- `tests/` — `ptylib.py` (harness PTY + emulador), `test_partA.py`,
  `smoke_fase1.py`, `test_fase2.py`.

## Estado (2026-10-13)

- **v2.0.0** commiteada localmente: split de `main.rs` en 10 módulos + menú de
  15 programas. Commit `52facf6` y tag `v2.0.0` **sólo locales** (sin push).
- 80/80 checks en verde, 0 warnings.
