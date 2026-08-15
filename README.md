# pigmacs — pigmeo + emacs

Editor multimodal estilo vim para ficheros **org** con shell PTY integrado y
panel de **inteligencia artificial** (opencode). Fase 1 y 2 completadas.

## Compilar

```bash
cargo build --release
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
- `Tab`/`Shift+Tab` — alternar editor/shell
- `Ctrl+E` o `Ctrl+Enter` — enviar la selección visual al shell

**IA** (fase 3):
- `Ctrl+A` — alternar editor/panel IA
- escribe la pregunta y pulsa `Enter` · `Esc` cancela la consulta en curso
- usa `opencode run` (debe estar en el PATH)

## Atajos globales

- `Ctrl+S` guardar · `Ctrl+Q` salir (avisa si hay cambios sin guardar)
- `Ctrl+K` limpiar buffer del shell

Cursor bloque estilo vim. La barra de estado muestra el modo activo.
