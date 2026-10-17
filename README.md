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
curl -sL https://github.com/kidandromeda/pigmacs/releases/download/v3.1.0/pigmacs -o pigmacs && chmod +x pigmacs
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
- la cabecera del panel cambia de color según el estado (amarillo =
  procesando, verde = con respuesta) y al terminar suena un **bell** en la
  terminal: no hace falta mirar el panel
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
| lanzador de aplicaciones gráficas | `Ctrl+X` | `Alt+X` |
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

**El menú es difuso**: en cuanto escribes, filtra las entradas
(`htp` → `htop`); `Esc` limpia el filtro y `Esc` de nuevo cierra. Si no hay
coincidencias, `Enter` lanza **lo tecleado tal cual** como comando
(`watch`, `yazi`, `iotop`…). El menú no se abre en modo SHELL (cede a
readline). `neovim` se lanza con el comando `nvim`; `Midnight Commander` con
el comando `mc`; `lynx` y `w3m` arrancan en `www.duckduckgo.com`.

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

## Lanzador de aplicaciones gráficas (Ctrl+X)

`Ctrl+X` / `Alt+X` abre un popup con **las aplicaciones instaladas en el
sistema**, leídas de tus `.desktop` (`~/.local/share/applications`,
`/usr/local/share/applications` y `/usr/share/applications`; los tuyos pisan
a los del sistema, como manda la especificación XDG).

Funciona como el menú `Ctrl+P`: **escribir filtra** de forma difusa
(`brv` → Brave), `j`/`k` o flechas mueven, `Enter` lanza, `Esc` limpia el
filtro y `Esc` de nuevo cierra. La ventana sigue al cursor si hay muchas
aplicaciones (flechitas `↑`/`↓` con el recuento de lo que queda fuera).

Al elegir una app:

- **con sesión gráfica** (`DISPLAY` o `WAYLAND_DISPLAY`) el comando se manda
  tal cual al shell embebido y la app arranca en el escritorio en el que
  estés;
- **sin sesión gráfica** (tty real) pigmacs lanza `startx` **él mismo,
  heredando la consola**: `startx` ve `tty` = `/dev/ttyN` y le pasa
  `-keeptty` a Xorg, exactamente igual que si teclearas a mano
  `startx /usr/bin/brave-browser`. No va por el shell embebido, porque
  ese es un `/dev/pts` y con él el servidor no arranca. X ocupa la
  consola mientras dure la app; al cerrarla, pigmacs repinta la pantalla
  entera y vuelve el terminal. Los errores de `startx`/Xorg se ven
  directamente en la consola, como si lo hubieras ejecutado tú (el log
  completo de Xorg queda en `~/.local/share/xorg/`);
- si la `.desktop` lleva `Terminal=true`, la app se abre en la **ventana
  principal** sobre su propio PTY, igual que las del menú `Ctrl+P`.

Detalles: se resuelven los códigos de campo de la especificación (`%c` nombre,
`%k` fichero, `%i` icono, `%%` literal; los `%f`/`%u` se descartan porque no
hay fichero que abrir) y las comillas de la línea `Exec`; si la app pide
argumentos y no hay sesión gráfica se genera un *wrapper* temporal, porque
`startx` sólo admite un cliente ejecutable sin argumentos. Se ocultan las
entradas `Hidden`/`NoDisplay`, las que no son `Type=Application`, las de
otros escritorios (`OnlyShowIn`/`NotShowIn`) y las cuyo `TryExec` no está
instalado.

Si no hay `startx` (paquete `xinit`) ni sesión gráfica, el status lo dice.
No hace falta `dmenu` ni `dunst`: el selector es el propio popup de pigmacs
(en dunst no hay selector: es un demonio de notificaciones).

## Consola de texto (VT real)

Al arrancar en una consola de texto (`/dev/ttyN`) pigmacs:

- limpia la pantalla anterior (sólo `ESC[2J`, respeta el scrollback de los
  emuladores)
- baja el `kernel.printk` (`/proc/sys/kernel/printk`) si tiene permisos para
  que los avisos del kernel no se pinten encima; al salir lo restaura
- si no tiene permisos, cada 2 s reescribe todas las celdas del frame para
  borrar los avisos que el kernel haya pintado encima (sin parpadeo), y el
  status muestra el hint: `sudo dmesg -n 1`

## Navegador de ficheros

Columna fija a la izquierda tipo treemacs:
- `Ctrl+F` / `Alt+F` — mostrar/foco · otra vez oculta · `Esc` foco al editor (árbol sigue visible)
- `j`/`k` — bajar/subir · `l` — entrar en carpeta · `h` — carpeta anterior
- `Enter` — abrir fichero en el editor · `g`/`G` — principio/fin
- **marcadores**: `b` marca/desmarca el directorio actual (★★★ tus sitios
  de siempre), `B` salta al siguiente; los directorios marcados se ven con
  `★` en el árbol, la cabecera cuenta cuántos hay (`★3`) y el status dice
  en cuál estás (`★ Marcador 2/3 · /ruta`)
- los directorios se muestran en amarillo, los ficheros en blanco y los
  ocultos atenuados; la selección va con fondo de acento
- el cwd del proceso sigue al tree: los programas del menú (`gdu`, `ncdu`…)
  arrancan en el directorio que estás mirando y el status muestra su ruta
  (la terminal interna no: `bash` se lanza al arrancar pigmacs)

## Colores y aspecto (v3.1.0)

pigmacs se dibuja sólo con **los 16 colores ANSI** de una tty real (nada de
truecolor ni de entorno gráfico):

- **barra de estado**: el modo en una píldora de color (NORMAL azul, INSERT
  verde, VISUAL magenta, SHELL cyan…), el punto rojo `●` si hay cambios sin
  guardar, el aviso del programa, y a la derecha `pigmacs v3.1.0 │ fichero
  │ fila:col` siempre visible
- **barra de ayuda**: encima del status, los atajos del modo activo con las
  teclas en color y las descripciones en gris; la ayuda completa (`F1`/`?`)
  lleva las secciones en cyan y las teclas en amarillo
- bordes y separadores con acento cyan, cabeceras de panel con banda de
  color y selecciones con fondo de acento (no vídeo inverso)

## Colores org y resaltado de código (v3.1.0)

- cabeceras: nivel 1 cyan (negrita), 2 verde, 3 amarillo, más magenta
- `TODO` rojo · `DONE` verde
- metadatos `#+...` amarillo · delimitadores `#+BEGIN_SRC` magenta
- **resaltado de sintaxis dentro de los bloques `#+BEGIN_SRC`** para
  python, bash/sh, rust, c/cpp, js/ts (keywords en cyan, cadenas en verde,
  números, comentarios en gris, funciones en amarillo…); los demás
  lenguajes llevan el fallback (números, cadenas, comentarios)

## Licencia

MIT — véase [LICENSE](LICENSE). Autor: **kidandromeda**.

Todos los ficheros de `src/` y `tests/` llevan cabecera con autor, licencia,
versión global (`3.1.0`) y fecha. La versión del proyecto vive en
`Cargo.toml` y en los tags de git; los archivos no llevan versión propia.
