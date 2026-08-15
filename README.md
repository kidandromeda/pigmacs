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

## Navegador de ficheros (v_03)

Columna fija a la izquierda tipo treemacs:
- `Ctrl+B` — mostrar/foco · otra `Ctrl+B` oculta · `Esc` foco al editor (árbol sigue visible)
- `j`/`k` — bajar/subir · `l` — entrar en carpeta · `h` — carpeta anterior
- `Enter` — abrir fichero en el editor · `g`/`G` — principio/fin
- los directorios se muestran en amarillo y los ficheros en gris

## Colores org (v_03)

- cabeceras: nivel 1 cyan (negrita), 2 verde, 3 amarillo, más magenta
- `TODO` rojo · `DONE` verde
- bloques `#+BEGIN_SRC`/`#+END_SRC` magenta, contenido gris
- metadatos `#+...` amarillo
