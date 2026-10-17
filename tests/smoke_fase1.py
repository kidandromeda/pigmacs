#!/usr/bin/env python3
# ————— pigmacs · tests · smoke de regresión (fases 1 y A) —————
# Autor:      kidandromeda
# Licencia:   MIT
# Versión:    3.1.0
# Fecha:      2026-10-14
"""Smoke de regresión (fase 1 + parte A): modos, editor, shell, tree, IA, ayuda."""
import sys, time, os, subprocess
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ptylib import Session, check_factory

fails = []
check = check_factory(fails)

# ——— sesión principal (grupo Ctrl) ———
s = Session(dirs=["sub"])
s.read(1.3)
check("arranca en NORMAL", "NORMAL" in s.scr.text())
check("el tree se ve al arrancar", "test.org" in s.scr.text())
check("pinta el árbol del fichero", "* prueba" in s.scr.text())
s.repaint()
check("repaint tras resize", "* prueba" in s.scr.text())

# shell
s.send("\x14", 0.6)                      # Ctrl+T
s.read(0.6)
check("Ctrl+T → modo SHELL", "SHELL" in s.scr.text())
check("prompt de bash visible", ":~$" in s.scr.flat() or "humano@" in s.scr.text())

# el cursor del shell es un bloque pintado justo donde escribe el prompt
# (antes el cursor real se quedaba quieto en la esquina inferior izquierda)
def cursor_shell():
    for y, l in enumerate(s.scr.lines()):
        if "~$" in l:
            return l.index("$") + 2, y
    return None, None

cx, cy = cursor_shell()
check("el cursor del shell está tras el prompt",
      cx is not None and s.scr.attr_at(cx, cy)[1] == 14,
      (cx, cy, s.scr.attr_at(cx, cy) if cx is not None else None))
s.send("ls", 0.5)
s.read(0.5)
cx, cy = cursor_shell()
check("el cursor del shell avanza al escribir",
      cx is not None and s.scr.attr_at(cx + 2, cy)[1] == 14,
      (cx, cy, s.scr.attr_at(cx + 2, cy) if cx is not None else None))
s.send("\x7f\x7f", 0.4)                  # borra el "ls" de prueba
s.read(0.4)

# Ctrl+J: escape al editor que funciona en tty reales (Shift+Tab no llega)
s.send(b"\x0a", 0.6)
s.read(0.5)
check("Ctrl+J vuelve al editor desde el shell", "NORMAL" in s.scr.text())
s.send("\x14", 0.6)                      # Ctrl+T → shell otra vez
s.read(0.6)
check("Ctrl+T al shell tras Ctrl+J", "SHELL" in s.scr.text())

# volver al editor con Shift+Tab
s.send("\x1b[Z", 0.6)
s.read(0.5)
check("Shift+Tab vuelve al editor", "NORMAL" in s.scr.text())

# insertar / escribir / borrar
s.send("i", 0.4)
s.read(0.4)
check("i → INSERT", "INSERT" in s.scr.text())
s.send("AYUDA123", 0.5)
s.read(0.3)
check("escribe en el editor", "AYUDA123" in s.scr.text())
s.send("\x1b", 0.4)
s.read(0.4)
check("Esc → NORMAL", "NORMAL" in s.scr.text())

# undo / redo
s.send("u", 0.6)
s.read(0.3)
check("u deshace", "AYUDA123" not in s.scr.text())
s.send("\x12", 0.6)                      # Ctrl+R
s.read(0.3)
check("Ctrl+R rehace", "AYUDA123" in s.scr.text())
s.send("u", 0.5)
s.read(0.3)
check("segundo u deja limpio", "AYUDA123" not in s.scr.text())

# visual
s.send("v", 0.5)
s.read(0.4)
check("v → VISUAL", "VISUAL" in s.scr.text())
s.send("\x1b", 0.4)
s.read(0.4)
check("Esc sale de VISUAL", "NORMAL" in s.scr.text())

# guardar
s.send("\x13", 0.8)                      # Ctrl+S
s.read(0.5)
check("Ctrl+S guarda", "uardado" in s.scr.text(), s.scr.lines()[-4:])

# tree: 1ª pulsación enfoca, 2ª lo oculta
s.send("\x06", 0.6)                      # Ctrl+F
s.read(0.5)
check("Ctrl+F enfoca el tree (BROWSE)", "BROWSE" in s.scr.text())
s.send("\x06", 0.5)
s.read(0.5)
check("Ctrl+F 2ª vez vuelve al editor", "NORMAL" in s.scr.text())

# el cwd del proceso sigue al tree: un gdu/ncdu lanzado desde el menú
# ve el directorio que estás mirando
s.send("\x06", 0.6)                      # Ctrl+F → tree (cursor en «sub»)
s.read(0.4)
check("BROWSE con el tree enfocado", "BROWSE" in s.scr.text())
s.send("l", 0.5)                         # entra en «sub»
s.read(0.4)
check("entrar en un dir fija el cwd allí",
      "Directorio: %s/sub" % s.work in s.scr.text(), s.scr.lines()[-2:])
s.send("h", 0.5)                         # sube al padre
s.read(0.4)
check("subir vuelve al cwd del padre",
      "Directorio: %s" % s.work in s.scr.text(), s.scr.lines()[-2:])
s.send("l", 0.5)                         # otra vez a sub: se lanza desde ahí
s.read(0.4)
s.send("\x10", 0.6)                      # Ctrl+P → el menú abre desde BROWSE
s.read(0.5)
for _ in range(20):                      # bajar hasta ▸ ranger
    if any("▸ ranger" in l for l in s.scr.lines()):
        break
    s.send("j", 0.12)
    s.read(0.25)
check("menú navega hasta ranger", any("▸ ranger" in l for l in s.scr.lines()))
s.send("\r", 2.2)                        # Enter → ranger arranca en el cwd del tree
s.read(0.8)
# el cwd real del proceso lanzado: lo que vería un gdu/ncdu
pid = subprocess.run(["pgrep", "-n", "-x", "ranger"],
                     capture_output=True, text=True).stdout.strip()
cwd = os.readlink("/proc/%s/cwd" % pid) if pid else ""
check("el programa arranca en el cwd del tree",
      cwd == os.path.realpath(os.path.join(s.work, "sub")), cwd)
s.send("\x07", 0.8)                      # Ctrl+G → cierra ranger y vuelve al editor
s.read(0.6)
check("Ctrl+G cierra el programa", "Programa cerrado" in s.scr.text())
check("vuelve al editor tras cerrar", "NORMAL" in s.scr.text())

# IA
s.send("\x01", 0.7)                      # Ctrl+A
s.read(0.6)
check("Ctrl+A → panel IA", "AI" in s.scr.text())
s.send("\x01", 0.6)
s.read(0.6)
check("Ctrl+A vuelve al editor", "NORMAL" in s.scr.text())

# borde del shell
before = s.scr.text()
s.send("\x15", 0.5)                      # Ctrl+U sube el borde
s.repaint()
check("Ctrl+U sube el borde", s.scr.text() != before)
s.send("\x04", 0.5)                      # Ctrl+D baja
s.repaint()

# ayuda con F1 y con ?
s.send("\x1bOP", 0.6)                    # F1
s.read(0.6)
check("F1 abre la ayuda", "MANUAL" in s.scr.text())
s.send("\x1bOP", 0.5)                     # F1 la cierra
s.read(0.5)
check("F1 cierra la ayuda", "MANUAL" not in s.scr.text())
s.send("?", 0.5)
s.read(0.5)
check("? abre la ayuda", "MANUAL" in s.scr.text())
s.send("?", 0.5)
s.read(0.5)
check("? cierra la ayuda", "MANUAL" not in s.scr.text())
s.send("\x1b", 0.5)
s.read(0.5)
check("Esc no rompe nada tras cerrar", "NORMAL" in s.scr.text())

# ——— F1 funciona desde el shell y Ctrl+T es de ida y vuelta ———
s.send("\x14", 0.5)                      # Ctrl+T → shell
s.read(0.4)
check("Ctrl+T entra al shell", "SHELL" in s.scr.text())
s.send("\x1bOP", 0.5)                    # F1 en el shell
s.read(0.4)
check("F1 en el shell abre la ayuda", "MANUAL" in s.scr.text())
s.send("\x1bOP", 0.4)                    # F1 la cierra (sigues en el shell)
s.read(0.4)
check("cerrar la ayuda sigue en el shell", "SHELL" in s.scr.text())
s.send("\x14", 0.5)                      # Ctrl+T → vuelve al editor
s.read(0.4)
check("Ctrl+T del shell vuelve al editor", "SHELL" not in s.scr.text())

# limpiar el shell: escribir, ejecutar y Ctrl+K desde el editor
s.send("\x14", 0.5)                      # Ctrl+T
s.read(0.4)
s.send("echo basura123\r", 1.0)
s.read(0.5)
check("comando en el shell", "basura123" in s.scr.text())
s.send("\x1b[Z", 0.5)                    # Shift+Tab → editor
s.read(0.4)
s.send("\x0b", 0.8)                      # Ctrl+K limpia la pantalla del shell
s.read(0.3)
s.repaint()
check("Ctrl+K limpia el shell", "basura123" not in s.scr.text())

# salida
s.send("\x11", 0.8)                      # Ctrl+Q
check("Ctrl+Q sale", s.wait_exit(4))
s.close()

# ——— sesión 2: grupo Alt ———
s2 = Session()
s2.read(1.3)
s2.send("\x1bt", 0.7)                    # Alt+T → shell
s2.read(0.6)
check("Alt+T → modo SHELL", "SHELL" in s2.scr.text())
s2.send("\x1b[Z", 0.6)                   # Shift+Tab → editor (desde el shell
s2.read(0.5)                             #  el resto de Ctrl/Alt cede a readline)
check("Shift+Tab al editor", "NORMAL" in s2.scr.text())
s2.send("i", 0.4)
s2.read(0.4)
check("i → INSERT", "INSERT" in s2.scr.text())
s2.send("\x1bv", 0.7)                    # Alt+V → vuelve al editor
s2.read(0.6)
check("Alt+V → editor", "NORMAL" in s2.scr.text())
s2.send("\x1bq", 0.8)                    # Alt+Q → salir
check("Alt+Q sale", s2.wait_exit(4))
s2.close()

# ——— sesión 3: las cabeceras plegadas no deben hacer saltar el texto ———
# el scroll cuenta filas *visibles*: al pasar de una cabecera plegada a otra
# el texto se queda quieto (antes, con filas lógicas, subía y dejaba el
# cursor al principio de la ventana)
org = []
for i in range(1, 9):
    org.append(f"* h{i}")
    for c in range(1, 5):
        org.append(f"  contenido {i}.{c}")
    org.append("fin")
s3 = Session(files={"cabeceras.org": "\n".join(org) + "\n"})
s3.read(1.3)
check("cabeceras: arranca con todo plegado",
      "* h1" in s3.scr.text() and "* h8" in s3.scr.text())
for _ in range(3):                       # bajar 3 cabeceras (h1 → h4)
    s3.send("j", 0.25)
s3.read(0.4)
t = s3.scr.text()
check("cabeceras: el texto no salta al bajar cabeceras",
      "* h1" in t and "* h4" in t, t[:300])
check("cabeceras: el cursor avanzó a la línea 19", "19:1" in t, t[-120:])
s3.send("\x1bq", 0.7)                    # Alt+Q sale
check("cabeceras: Alt+Q sale", s3.wait_exit(4))
s3.close()

print("\n%d fallos" % len(fails))
sys.exit(1 if fails else 0)
