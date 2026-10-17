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
- **lanzador `Ctrl+X`** de aplicaciones gráficas: lee los `.desktop` del
  sistema y, sin sesión gráfica, lanza `startx` **él mismo heredando la tty
  real** (en consola; el shell embebido es un `/dev/pts` y no sirve) (rama
  experimental);
- navegador de ficheros (tree), panel de IA, ayuda integrada;
- todo el render es una **rejilla de celdas propia pintada por diff**: nadie
  escribe a stdout salvo `render_frame()` y el lanzador gráfico (pantalla
  alterna al entrar/salir de X).

Dependencias (sólo 3): `anyhow`, `crossterm 0.29`, `portable-pty 0.9`.

## Comandos

```bash
cargo build                # debug  → target/debug/pigmacs
cargo build --release      # release → target/release/pigmacs

# las 3 suites (ejecutar desde tests/); deben dar "0 fallos" (115 checks)
cd tests
python3 test_partA.py      #  6 checks — atajos y/render base
python3 smoke_fase1.py     # 53 checks — smoke de fases anteriores
python3 test_fase2.py      # 56 checks — menú, VT100, shell, Ctrl+J, resize, Ctrl+X…
```

- **No hay tests en Rust**: la garantía son las suites Python, que abren el
  binario en una PTY y comparan pantallas con un mini-emulador VT100
  acumulativo (`tests/ptylib.py`).
- `PIGMACS_BIN=/ruta/a/otro/binario python3 test_*.py` para probar otra
  compilación.
- **Regla de oro**: `cargo build` sin *warnings* + las 3 suites en verde
  **antes** de cada commit.

## Estructura de `src/` (v3.1.0, ~5600 líneas)

| módulo | líneas | contenido |
|---|---|---|
| `main.rs` | 776 | declaraciones `mod` y prelude (`use x::*`); `main()`, `run()` (bucle de eventos, con `x_poll()` al principio: X sobre la tty real), atajos globales (`Shortcut`, `shortcut_of`, `shortcut`), `resize_border`, `handle_help_key`, `handle_sudo_key`, avisos de kernel (`running_on_vt`, `lower_kernel_printk`) |
| `app.rs` | 980 | estado central: `enum Mode`, `struct App` + **`impl App` completo** (teclado por modo, guardado, IA…), `layout()`, `file_writable()`, `ensure_visible()` (scroll por filas **visibles**) |
| `ui.rs` | 561 | `draw()` compone el frame completo; `draw_help()`, `HELP_TEXT` |
| `frame.rs` | 153 | `Style`, `Cell`, `Frame` (rejilla), `render_frame()` (pintado por diff), `strip_ansi()` |
| `term.rs` | 683 | `Term` + `PState`: emulador VT100 completo (`feed`, `parse_sgr`, scroll, alt screen, línea de comandos…), `draw()` |
| `shell.rs` | 358 | `Shell` (PTY del shell): `key_bytes`/`key_bytes_mode` (tecla→bytes), `send_key_to_shell`, `run_in_shell`, `csi_shell`, `draw_shell_output` (pinta el **cursor** del shell como bloque), `resize` |
| `program.rs` | 387 | `Program` (PTY del programa), `MENU` (15 entradas: nombre visible + argv), `in_path()`/`which()`; menú: `menu_entries`, `launch_program`, `close_program`, `handle_menu_key`, `draw_menu` + **`draw_popup()`** (popup genérico que comparten el menú y el lanzador) |
| `launcher.rs` | 695 | **lanzador de apps gráficas (Ctrl+X)**: parser de `.desktop` (`parse_desktop`, `desktop_apps`, escapes y códigos de campo), filtro difuso (`filtered`, `subseq`), `launch_app`/`launch_gui` (`startx` directo en tty real o `DISPLAY` por el shell, wrapper temporal), `x_poll`/`volver_a_pigmacs` (ciclo de vida de la sesión X) + `arbol_de`/`matar_arbol` (**Ctrl+G** mata la sesión X entera por `/proc`), `handle_launcher_key`, `draw_launcher` |
| `editor.rs` | 467 | panel del editor: `handle_editor_key`, `handle_search_key`, `compute_matches`, `org_line_style`, `draw_editor_lines` + **resaltado de sintaxis** de bloques SRC (`src_styles`, `lang_of`, `src_state_at`) |
| `browser.rs` | 186 | tree de ficheros: `Browser` (`refresh`, `cd` **arrastra el cwd del proceso**, `cursor_up/down`…), `handle_browser_key`, `draw_browser`, `browser_width` |
| `ai.rs` | 130 | panel de IA: `handle_ai_key`, `draw_ai_pane` |
| `theme.rs` | 110 | **paleta semántica única** (16 colores ANSI): `ACCENT/FG/DIM/SYNTAX_*` + helpers `fg()`, `bold()`, `pill()`, `band()`. Todo el pintado pasa por aquí |

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
6. **Apps gráficas (Ctrl+X)**: `launcher.rs` lee los `.desktop` del sistema.
   Con `DISPLAY`/`WAYLAND_DISPLAY` el comando va al shell embebido
   (`run_in_shell()`); **sin sesión gráfica en una tty real, pigmacs lanza
   `startx` él mismo** (`Command::new(startx).arg(cliente)`, heredando la
   stdio = la consola, para que `tty` reporte `/dev/ttyN` y Xorg reciba
   `-keeptty` — mandarlo al shell embebido no funciona: es un `/dev/pts`).
   Entonces `app.x` queda ocupada: **`x_poll()` (`launcher.rs`) manda el
   bucle de eventos** — no pinta nada ni lee teclas (X tiene la consola) y,
   al terminar, restaura pantalla alterna + raw mode + `force_full`. Sin tty
   real (tmux/xterm) se usa el shell embebido como fallback. `Terminal=true`
   se abre por `launch_program` como las del menú. El popup lo comparte con
   el menú (`draw_popup`); el wrapper temporal resuelve los argumentos
   (`startx` sólo admite un cliente sin argumentos).
7. **Modos**: `Mode = Normal | Insert | Visual | Shell | Ai | Browser |
   Program`. `top_mode` guarda el contexto (p. ej. shell abierto encima de un
   programa que sigue en segundo plano).
8. **Ctrl+G** cierra el programa del menú (`Ctrl+P`) o **mata la app gráfica
   cuando X tiene la consola**: `arbol_de()` recorre el `ppid` de `/proc`
   (startx → xinit → Xorg y el cliente) y `matar_arbol()` le manda `SIGTERM`
   con el `kill` del sistema; una segunda pulsación manda `SIGKILL`. Sin
   `libc` ni dependencias nuevas.

## Atajos globales

`shortcut_of()` (`main.rs`): **Ctrl o Alt + letra** (XOR; AltGr no cuenta).

- `q` salir · `s` guardar · `p` menú · `x` lanzador de apps gráficas ·
  `g` cerrar programa · `t` terminal
  (ida y vuelta: desde el shell vuelve al modo anterior) · `v` editor ·
  `f` tree · `a` IA · `n` nueva IA · `e` enviar · `r` redo ·
  `k` limpiar shell · `u`/`d` bordes
- **`j` sólo con Ctrl**: `Ctrl+J` = volver al editor (`Shortcut::ToEditor`);
  `Alt+J` está sin asignar.
- `p` y `x` ceden en modo SHELL (igual que `a`, `f`, `t`, `v`, `u`, `d`):
  el shell es de readline/tmux.

Reglas críticas que **no se deben romper**:

- **`Ctrl+J` es el escape de la tty real**: en consola de verdad `Shift+Tab`
  no llega (el teclado del kernel no genera `CSI Z`), pero el kernel sí entrega
  siempre el byte `0x0A`. Debe seguir funcionando desde SHELL y PROG.
- **`Ctrl+T` es toggle**: entra al shell y desde allí vuelve al modo del que
  se abrió (`top_mode`). `?` sólo abre la ayuda en el editor: en el shell y en
  el panel IA es imprimible y debe ceder.
- `Enter` sobre un programa **ya en marcha** en el menú recupera el foco
  **sin relanzarlo**.
- Con un programa activo sólo se consumen `Ctrl/Alt+Q/S/P/G/X`, `Ctrl+J` y
  `Shift+Tab`; el resto de teclas va al programa.

## Convenciones

- **Español** en código, comentarios, docs, tests y commits. Prefijos del
  historial: `pigmacs:` (código), `docs:`, `chore:`.
- **Git sólo local**: hacer commit está bien; **no hacer `push` ni releases a
  la nube sin que lo pida explícitamente el usuario**.
- Cambios de UI o de atajos → actualizar también `HELP_TEXT` (`src/ui.rs`),
  `README.md` e `informe_ux_terminal.org`.
- Menú: se edita la constante `MENU` (`program.rs`, 15 entradas); en pantalla
  sólo aparecen las que están en el PATH. Los tests sólo pueden asumir
  programas garantizados instalados (`htop`, `tmux`, `ranger`…).
- Lanzador gráfico: la lista de apps viene de los `.desktop` del sistema, no
  de una constante; los tests **no pueden asumir** ninguna app instalada
  (sólo el título y el pie del popup, que son fijos).
- Dependencias nuevas: sólo si se pide (van 3 en total).

## Documentación

- `README.md` — manual de usuario: instalación, modos, atajos, tests.
- `informe_ux_terminal.org` — informe de UX: bugs corregidos y **§3 «Mejoras
  propuestas»** con TODOs priorizados (A/B/C): la mejor fuente de tareas nuevas.
- `tests/` — `ptylib.py` (harness PTY + emulador), `test_partA.py`,
  `smoke_fase1.py`, `test_fase2.py`.

## Plan de fase 3 (rediseño visual «tty puro») — entregado en v3.1.0

El usuario lo pidió el 2026-10-13; **v3.1.0 cumple la mayor parte** (ver
§7 del `informe_ux_terminal.org` y el *Estado* de abajo). Objetivo:
que pigmacs sea *bonita*, con la elegancia que se puede lograr en una
**tty real de 16 colores**.

**Restricciones duras (decisión del usuario):**

- La app es **sólo para tty** (consola de texto del kernel). Lo que sólo se
  vea mejor en 256 colores / truecolor / entorno gráfico **no se hace**.
- Paleta limitada a los **16 colores ANSI**; nada de `Color::AnsiValue`,
  `Color::Rgb` ni detección de «soporte truecolor».
- Sin emojis ni glifos powerline: **no existen en la fuente VGA de la
  consola**. Glifos duros actuales (ya usados y probados en tty real):
  `─│┌┐└┘ ├┤ ▸ ★ ● · ⋯`. Las esquinas redondeadas `╭╮╰╯` (U+256D-2570)
  **hay que verificarlas en consola real** antes de usarlas; si no están en
  la fuente, se quedan las rectas `┌┐└┘`.
- Nada de dependencias nuevas para esto (siguen valiendo sólo 3).
- Los tests (`tests/*.py`) comparan **texto**, no estilos: el rediseño es
  compatible, pero **no romper** cadenas asumidas por los tests
  (`NORMAL`, `▸ htop`, `▸ ranger`, mensajes de status…).

**Parte A — elegancia (render): estado en v3.1.0**

1. ✅ `src/theme.rs`: paleta **semántica única** en 16 colores. Todo el
   pintado pasa por ella; ya no hay colores sueltos por módulo.
2. ❌ **Barra superior** (fila 0): **no se hizo así** — la identidad
   `pigmacs vN │ fichero │ f:c` vive en la **barra de estado** (derecha),
   para no restar una fila al editor. Si se quiere una cabecera de verdad
   hay que ajustar `layout()`, `draw_browser`/`draw_ai_pane` (pintan en la
   fila 0) y el resize del PTY del programa.
3. ✅ Bordes y separadores con acento; popup del menú con borde y sombra.
4. ✅ Barra de estado: píldora de modo con color de fondo, `●` de cambios
   y fichero/posición a la derecha; barra de ayuda con **teclas en color y
   descripciones dim**.
5. ✅ Editor: selección con **fondo de acento** (no `reverse`), números de
   línea dim, marcador de pliegue en color, cursor del modo en acento.
6. ✅ Tree: cabecera con banda de color e `★N`, directorios en color,
   selección con acento, ocultos dim.
7. ✅ Panel IA y menú Ctrl+P con la misma lengua visual.

**Parte B — funcional (informe §3):**

8. ✅ **Resaltado de sintaxis** en bloques `#+BEGIN_SRC` (`src_styles()`,
   `lang_of()`, `src_state_at()` en `editor.rs`, sólo líneas visibles):
   python, bash/sh, rust, c/cpp, js/ts y fallback (números, cadenas,
   comentarios). Ver §3 del informe.
9. ✅ **Menú difuso**: escribir filtra (`htp` → `htop`); Enter lanza lo
   tecleado tal cual si no hay coincidencia. Ver §3 del informe.
   (+ bell y banda de estado de la IA, §3 #C.)

**Definición de hecho:** `cargo build` 0 warnings + 115/115 checks en verde
antes de commit; la comprobación **en una consola VT real** (no en xterm)
está pendiente de que la haga el usuario (el agente no tiene acceso a
`/dev/ttyN`). `HELP_TEXT`, README e informe se actualizaron en v3.1.0.

## Estado (última actualización: 2026-10-17)

- **`experimental/lanzador-grafico` fusionada en `main`** (merge `2add39c`) —
  **`Ctrl+X` / `Alt+X` lanzador de aplicaciones gráficas**: `src/launcher.rs`
  lee los `.desktop` del sistema (`~/.local/share/applications` primero: pisan
  a las del sistema), popup difuso con el mismo dibujo que el menú `Ctrl+P`
  (`draw_popup()` se extrajo de `draw_menu` y lo comparten). Con
  `DISPLAY`/`WAYLAND_DISPLAY` el comando va al shell embebido
  (`run_in_shell()`). **Sin sesión gráfica en una tty real, pigmacs lanza
  `startx` él mismo heredando la consola** (`app.x = Some(XSesion{…})`; `x_poll()`
  al principio del bucle no pinta nada mientras X tenga la consola y, al
  terminar, restaura pantalla alterna + raw mode + `force_full`):
  así `tty` reporta `/dev/ttyN` y Xorg recibe `-keeptty`. **Corrección tras
  probarlo en consola real**: la primera versión mandaba `startx` al shell
  embebido y no funcionaba («connection refused» / bloqueado) porque ese pty
  vive en un `/dev/pts`; sin tty real el fallback sigue siendo el shell
  embebido. `Terminal=true` se abre por `launch_program` (ventana principal,
  como el menú). Se resuelven los códigos de campo (`%c`/`%k`/`%i`/`%%`;
  `%f`/`%u` se descartan) y, si la app pide argumentos, se genera un wrapper
  temporal (`/tmp/pigmacs-launch-<pid>.sh`) porque `startx` sólo admite un
  cliente sin argumentos. Se ocultan `Hidden`/`NoDisplay`, las que no son
  `Type=Application`, las de otros escritorios y las de `TryExec` ausente.
  Cero dependencias nuevas. Tests nuevos en `test_fase2.py`: 7 checks de
  popup/filtro (no se puede asumir ninguna app instalada) + 13 del camino de
  consola (un `startx` falso y `.desktop` propios; el gancho
  `PIGMACS_FAKE_VT`, sólo para las suites, simula la tty porque ese camino no
  existe en una PTY): hereda la tty, wrapper, vuelta a pigmacs al terminar y
  **Ctrl+G mata el árbol de la sesión X** (el `startx` falso duerme 3 s y
  deja una segunda marca si llega a su final: sin Ctrl+G la marca aparece).
- **Ronda de detalles «irritables» (2026-10-17)**: `Ctrl+G`/`Alt+G` mata la
  app gráfica cuando X tiene la consola (`arbol_de`/`matar_arbol` por `/proc`
  → `SIGTERM`, y `SIGKILL` a la segunda pulsación) · el **cursor del shell**
  se pinta como bloque en la celda donde escribe el prompt (antes el cursor
  real se quedaba clavado en la esquina inferior izquierda y había que
  teclear a ciegas; `ptylib.py` aprendió a recordar el SGR de cada celda
  para poder comprobarlo) · el **scroll del editor** cuenta filas *visibles*:
  pasar de una cabecera org plegada a otra ya no sube el texto ni deja el
  cursor arriba (`ensure_visible`). Informe §1.7, §1.8 y §3.
  *No se hicieron* (se explicaron al usuario): ventana a pantalla completa
  (haría falta un gestor de ventanas: X sin WM abre al tamaño que pide la
  app) y el popup de ayuda al mantener `Ctrl` (en una tty el kernel no
  entrega la pulsación/suelta de un modificador solo: sólo con el protocolo
  de teclado kitty, que la consola no implementa).
- **v3.1.0 — ronda de elegancia + pendientes #A/#C del informe**:
  `theme.rs` (paleta semántica de 16 colores, todo el pintado pasa por
  ella) · status con píldora de modo + `●` de cambios + identidad/fichero/
  posición a la derecha · barra de ayuda con teclas en color · ayuda F1
  coloreada (secciones cyan, teclas amarillo) · bordes con acento · tree
  con banda de cabecera, ★ visible en los marcados y contador `★N` ·
  selecciones con fondo de acento (no `reverse`) · **resaltado de sintaxis
  en bloques `#+BEGIN_SRC`** (python/bash/rust/c/js + fallback, con
  cadenas multilínea y `src_state_at()`) · **menú difuso** (`htp`→`htop`;
  Enter lanza lo tecleado si no hay matches) · **bell + banda de estado**
  en el panel IA. Ojo: en modo debug las restas de índices de pantalla sin
  `saturating_sub` son panics — `draw_keybar` ya la padeció tras un resize.
- **v3.0.1**: cabecera estándar en los 14 ficheros de `src/` y `tests/`
  (proyecto · autor `kidandromeda` · licencia MIT · versión · fecha) +
  fichero `LICENSE` (MIT) + `authors`/`license` en `Cargo.toml`.
- **v3.0.0** en local (bump desde 2.0.0): el cwd del proceso sigue al tree
  (`Browser::cd` hace `chdir` y sincroniza `$PWD`, en la que confían programas
  como ranger), `Ctrl+T` de ida y vuelta, `F1` de ayuda en todos los paneles
  (incluido el shell), `MENU` con argv (neovim→nvim, lynx/w3m con
  www.duckduckgo.com) y 15 programas (con mc como «Midnight Commander»).
- Tag `v2.0.0` + commit `52facf6` **sólo locales**: git sólo local, sin push.
- 115/115 checks en verde, 0 warnings.
