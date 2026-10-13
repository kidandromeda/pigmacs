# pigmacs — pigmeo + emacs

Siempre he trabajado en la terminal de linux y me ha resultado una incomodidad la introducción directa de los comandos. Lo lógico es tener una lista de ellos y copiarlos, pegarlos o modificarlos para, posteriormente, lanzarlos a la terminal.

En mis máquinas tengo instalado emacs, y este problema lo tengo cubierto de sobra, pero para sistemas nuevos o de fuera de casa siempre he querido tener un programa sencillo ( **un sólo archivo ejecutable** ) que permitiera navegar por los ficheros fácilmente y **lanzar** lineas o texto seleccionado **directamente a terminal**. Para eso empecé haciendo este programa.

Esta es su principal función: cargo el programa en una terminal, voy a un archivo mío con una relación de los comandos que uso en terminal mediante su **navegador de archivos** integrado, **edito** los comandos de ese archivo y los **lanzo a una terminal**. Ese era el principal objetivo de pigmacs.
Como segundo objetivo, que el programa fuera un lector directo de archivos **org**, con **plegado** de encabezados, coloreado de texto y edición.
Posteriormente he ido añadiendo funcionalidades útiles que me resultaban incómodas en las tty (entorno para el que se ha diseñado el programa, más que para entornos de escritorio), como un lanzador de aplicaciones con TUI más conocidas en el mismo programa (tmux, htop, ncdu... que tienen que estar instalados previamente), un panel para hacer sencillas preguntas a la IA (si se tiene instalado Pi Coding Agent) y algunas más.

El programa es sólo un archivo ejecutable único. En un ordenador con un SO básico, sin entorno gráfico, requiere la copia del programa y de un archivo con los comandos que suelas usar (yo lo tengo en formato org). Si el ordenador tiene conexión a internet, solo hace falta descargarlo del repo y ya tienes un mini sistema operativo cómodo funcionando en una tty.

 **Nota del autor:** Aviso de que este proyecto ha sido creado en su mayor parte mediante *vibecoding* (programación asistida por IA). La calidad del código puede no ser óptima. Mientras que los programas sean pequeños e independientes, no lo considero mala práctica.
 Este programa es fruto de una necesidad personal. Trataré de pasarme por el repositorio cada cierto tiempo. Sientánse libres de usarlo o forkearlo o lo que quieran.

## Instalación

**Requisitos:** Linux x86-64

```bash
curl -sL https://github.com/kidandromeda/pigmacs/releases/download/v3.0.0/pigmacs -o pigmacs && chmod +x pigmacs
./pigmacs
```

## Compilar desde el código fuente

**Requisitos:** Rust
```bash
git clone https://github.com/kidandromeda/pigmacs.git
cd pigmacs
cargo build --release
./target/release/pigmacs
```

## Uso

```bash
./target/release/pigmacs fichero.org
```

## Modos

**Normal** — movimiento y comandos:
- `h j k l` / flechas — mover
- `gg` / `G` — primera / última línea visible
- `0` / `$` — inicio / fin de línea
- `i` escribir · `a` tras el cursor · `A` al final · `o`/`O` línea nueva abajo/arriba
- `x` borrar carácter · `dd` borrar línea
- `v` selección visual · `x` borra la selección
- `/` buscar (resalta; `Enter` salta y revela si está plegado) · `n`/`N` siguiente/anterior

**Org (plegado de cabeceras)**:
- `Tab` — plegar/desplegar la cabecera bajo el cursor
- `za` alternar · `zc` cerrar · `zo` abrir · `zr` abrir todas · `zm` cerrar todas
- el movimiento salta las regiones plegadas; la búsqueda las revela

**Insert** — escribir: teclear inserta, `Enter` nueva línea, `Esc` vuelve a Normal.

**Shell** — terminal PTY real:
- `Shift+Tab` / `Ctrl+T` — alternar terminal/editor (`Tab` es autocompletar)
- `Ctrl+J` — volver al editor **también en tty reales** (allí el teclado del
  kernel no genera `Shift+Tab`; el byte `0x0A` de `Ctrl+J` sí llega siempre)
- `Ctrl+E` o `Ctrl+Enter` — enviar la selección visual al shell
- dentro del shell se respetan las teclas del programa de dentro:
  `Ctrl+B` (prefijo de tmux), `Ctrl+A`/`Ctrl+E`/`Ctrl+K`/`Ctrl+R` (readline/emacs),
  `Ctrl+C`, `Ctrl+D`, `Ctrl+L`, `Ctrl+U`, `Ctrl+W`… — solo `Ctrl+Q`, `Ctrl+S`,
  `Ctrl+Enter`, `Ctrl+J` y `Shift+Tab` siguen siendo de pigmacs

**IA** (fase 3):
- `Ctrl+A` / `Alt+A` — alternar editor/panel IA (en el shell: `Shift+Tab` antes)
- escribe la pregunta y pulsa `Enter` · `Esc` cancela la consulta en curso
- usa `opencode run` (debe estar en el PATH)

## Atajos globales (dos grupos)

Cada acción tiene dos atajos: uno **garantizado en tty** (`Ctrl+letra`, bytes de
control ASCII que llegan siempre en una consola de texto) y otro para
**terminales gráficas** (`Alt+letra`, ESC+letra). Misma letra = misma acción.

| Acción | grupo TTY | grupo GUI |
|---|---|---|
| salir | `Ctrl+Q` | `Alt+Q` |
| guardar | `Ctrl+S` | `Alt+S` |
| panel IA | `Ctrl+A` | `Alt+A` |
| tree de ficheros | `Ctrl+F` | `Alt+F` |
| ir/salir del terminal | `Ctrl+T` | `Alt+T` |
| volver al editor | `Ctrl+V` | `Alt+V` |
| volver al editor desde el shell/programa | `Ctrl+J` | — |
| enviar selección al shell | `Ctrl+E` | `Ctrl+Enter` |
| rehacer | `Ctrl+R` | `Alt+R` |
| limpiar la terminal | `Ctrl+K` | `Alt+K` |
| nueva conversación IA | `Ctrl+N` | `Alt+N` |
| subir/bajar el borde | `Ctrl+U`/`Ctrl+D` | `Ctrl+Shift+↑/↓` |
| menú de programas | `Ctrl+P` | `Alt+P` |
| cerrar programa en marcha | `Ctrl+G` | `Alt+G` |
| ayuda | `?` | `F1` |

Navegación de paneles (grupo GUI): `Shift+←` tree · `Shift+→`/`↑` editor ·
`Shift+↓` terminal · `Shift+Tab` alterna con el terminal.

**Reservadas para el programa de dentro:** `Ctrl+B` se ha liberado porque es el
prefijo de tmux; en modo SHELL también pasan al programa `Ctrl+A`, `Ctrl+E`,
`Ctrl+K` y `Ctrl+R` (lectura de línea de comandos emacs), de modo que tmux y
readline funcionan dentro de la terminal de pigmacs.

Cursor bloque estilo vim. La barra de estado muestra el modo activo.

## Programas en la ventana principal (menú Ctrl+P)

`Ctrl+P` / `Alt+P` abre un menú con los programas instalados en el PATH
(`htop`, `btop`, `fastfetch`, `ncdu`, `gdu`, `nmtui`, `ranger`,
`Midnight Commander`, `gitui`, `tmux`, `neovim`, `bluetui`, `jeeves`,
`lynx`, `w3m`…): `j`/`k` o flechas mueven, `Enter` lanza, `Esc`/`q` cierra.
El menú no se abre en modo SHELL (cede a readline). `neovim` se lanza con el
comando `nvim`; `Midnight Commander` con el comando `mc`; `lynx` y `w3m`
arrancan en `www.duckduckgo.com`.

- el programa corre sobre **su propio PTY** ocupando el área del editor, con un
  emulador VT100 integrado (colores 256 / truecolor, pantalla alterna, región
  de scroll, respuestas DSR/DA, wrap diferido)
- mientras está activo, sus teclas le ceden: sólo `Ctrl/Alt+Q` (salir),
  `Ctrl/Alt+S` (guardar el fichero), `Ctrl/Alt+P` (menú), `Ctrl/Alt+G`
  (cerrar el programa), `Ctrl+J` (volver al editor dejándolo en segundo
  plano) y `Shift+Tab` (saltar al shell y volver) siguen siendo de pigmacs
- si dejas el programa con `Ctrl+J`, `Ctrl+P` + `Enter` sobre el mismo
  programa recupera su foco sin relanzarlo
- el PTY se redimensiona solo con la terminal, el borde editor/shell y el tree
- si el programa termina solo, pigmacs vuelve al editor y lo indica en el status
- `Ctrl+G` mata el programa y no deja zombies ni huérfanos

## Consola de texto (VT real)

Al arrancar en una consola de texto (`/dev/ttyN`) pigmacs:

- limpia la pantalla anterior (sólo `ESC[2J`, respeta el scrollback de los
  emuladores)
- baja el `kernel.printk` (`/proc/sys/kernel/printk`) si tiene permisos para
  que los avisos del kernel no se pinten encima; al salir lo restaura
- si no tiene permisos, cada 2 s reescribe todas las celdas del frame para
  borrar los avisos que el kernel haya pintado encima (sin parpadeo), y el
  status muestra el hint: `sudo dmesg -n 1`

## Navegador de ficheros (v1.0.1)

Columna fija a la izquierda tipo treemacs:
- `Ctrl+F` / `Alt+F` — mostrar/foco · otra vez oculta · `Esc` foco al editor (árbol sigue visible)
- `j`/`k` — bajar/subir · `l` — entrar en carpeta · `h` — carpeta anterior
- `Enter` — abrir fichero en el editor · `g`/`G` — principio/fin
- los directorios se muestran en amarillo y los ficheros en gris
- el cwd del proceso sigue al tree: los programas del menú (`gdu`, `ncdu`…)
  arrancan en el directorio que estás mirando y el status muestra su ruta
  (la terminal interna no: `bash` se lanza al arrancar pigmacs)

## Colores org (v1.0.1)

- cabeceras: nivel 1 cyan (negrita), 2 verde, 3 amarillo, más magenta
- `TODO` rojo · `DONE` verde
- bloques `#+BEGIN_SRC`/`#+END_SRC` magenta, contenido gris
- metadatos `#+...` amarillo

## Tests

Tres suites en Python (sólo stdlib) que arrancan el binario en un PTY y
reconstruyen la pantalla con un mini-emulador VT100:

```bash
cargo build
python3 tests/test_partA.py    # limpieza al arrancar · Delete/Ctrl+U sin fantasmas
python3 tests/smoke_fase1.py   # modos, editor, shell, tree, IA, ayuda, atajos
python3 tests/test_fase2.py    # menú Ctrl+P · programa · Ctrl+G · huérfanos
```

## Informe de mejoras (fase 3)

[[file:informe_ux_terminal.org][informe_ux_terminal.org]] — mejoras de UX en terminal
entregadas y propuestas (coloreado de código Python y otros lenguajes,
configs, entrada libre en el menú, bracketed paste, mouse, OSC 52…) y la
explicación completa de los avisos del kernel (`printk`) con lo que hace
pigmacs para evitarlos.
