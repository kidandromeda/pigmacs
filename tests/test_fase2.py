#!/usr/bin/env python3
# ————— pigmacs · tests · fase 2: menú Ctrl+P y programa integrado —————
# Autor:      kidandromeda
# Licencia:   MIT
# Versión:    3.0.1
# Fecha:      2026-10-13
"""Fase 2: menú Ctrl+P · programa en la ventana principal · Ctrl+G lo cierra."""
import sys, time, os, subprocess, shutil
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ptylib import Session, check_factory

fails = []
check = check_factory(fails)

# conteo base de htop en la máquina: sólo cuenta los que dejen SOBRA este test
BASE_HTOP = subprocess.run(["pgrep", "-x", "htop"], capture_output=True, text=True).stdout.strip()
BASE_HTOP = len(BASE_HTOP.split()) if BASE_HTOP else 0

s = Session()
s.read(1.3)
check("arranca en NORMAL", "NORMAL" in s.scr.text())

# ——— menú ———
s.send("\x10", 0.6)                       # Ctrl+P
s.read(0.5)
t = s.scr.text()
check("Ctrl+P abre el menú", "Lanzar programa" in t)
check("menú lista los programas", "htop" in t and "tmux" in t and "ranger" in t, t)
check("cursor en la 1ª entrada", "▸ htop" in t)
check("pie del menú", "Enter lanzar" in t)
# el menú sólo muestra lo del PATH: neovim se lanza como nvim y
# lynx/w3m necesitan URL de arranque (se comprueba que aparezcan)
check("menú muestra neovim si existe nvim", ("neovim" in t) == (shutil.which("nvim") is not None), t)
check("menú muestra lynx/w3m si existen", ("lynx" in t and "w3m" in t) == (shutil.which("lynx") is not None and shutil.which("w3m") is not None), t)

s.send("j", 0.3)                          # baja
s.read(0.3)
check("j mueve el cursor", "▸ btop" in s.scr.text())
s.send("k", 0.3)                          # sube
s.read(0.3)
check("k sube el cursor", "▸ htop" in s.scr.text())
s.send("\x1b", 0.4)                       # Esc
s.read(0.4)
check("Esc cierra el menú", "Lanzar programa" not in s.scr.text())

# ——— lanzar htop ———
s.send("\x10", 0.5)
s.read(0.4)
s.send("\r", 1.2)                         # Enter → htop
s.read(1.0)
s.repaint()
t = s.scr.text()
check("Enter lanza htop (modo PROG)", "PROG" in t, t[-10:])
check("status del programa", "htop" in t and "marcha" in t, [l[-70:] for l in s.scr.lines()[-2:]])
check("htop se dibuja", "Load average" in t or "Tasks" in t, t[:400])

# ——— las teclas ceden al programa ———
s.send("\x01", 0.6)                       # Ctrl+A → debe CEDER (no abrir IA)
s.read(0.5)
check("Ctrl+A cede al programa", "PROG" in s.scr.text() and "AI " not in s.scr.text())
s.send("\x06", 0.6)                       # Ctrl+F → debe CEDER (no abrir tree)
s.read(0.5)
check("Ctrl+F cede al programa", "BROWSE" not in s.scr.text())

# ——— menú sobre el programa ———
s.send("\x10", 0.5)                       # Ctrl+P funciona en PROG
s.read(0.4)
check("menú sobre el programa", "Lanzar programa" in s.scr.text())
s.send("\x1b", 0.4)
s.read(0.4)
check("Esc vuelve al programa", "PROG" in s.scr.text() and "Lanzar programa" not in s.scr.text())

# ——— Shift+Tab: ida y vuelta al shell ———
s.send("\x1b[Z", 0.6)
s.read(0.5)
check("Shift+Tab → shell", "SHELL" in s.scr.text())
s.send("\x1b[Z", 0.6)
s.read(0.6)
s.repaint()
check("Shift+Tab vuelve al programa", "PROG" in s.scr.text())

# ——— resize con el programa en marcha ———
s.resize()
s.repaint()
check("resize con htop no lo rompe", "PROG" in s.scr.text() and "Load average" in s.scr.text())

# ——— Ctrl+J: escape al editor dejando el programa en marcha ———
s.send(b"\x0a", 0.7)
s.read(0.6)
t = s.scr.text()
check("Ctrl+J va al editor dejando el programa", "NORMAL" in t and "PROG" not in t)
alive = subprocess.run(["pgrep", "-x", "htop"], capture_output=True, text=True).stdout.strip()
check("el programa sigue vivo tras Ctrl+J", len(alive.split()) == BASE_HTOP + 1, alive)

# ——— Ctrl+P + Enter sobre el mismo programa recupera el foco ———
s.send("\x10", 0.5)                       # Ctrl+P
s.read(0.4)
check("menú con el programa en segundo plano", "Lanzar programa" in s.scr.text())
s.send("\r", 0.8)                         # Enter sobre htop (ya en marcha)
s.read(0.6)
s.repaint()
t = s.scr.text()
check("Enter sobre el mismo programa recupera el foco", "PROG" in t and "Load average" in t)

# ——— Ctrl+G cierra ———
s.send("\x07", 0.7)                       # Ctrl+G
s.read(0.6)
t = s.scr.text()
check("Ctrl+G cierra el programa", "NORMAL" in t and "cerrado" in t, [l[-70:] for l in s.scr.lines()[-2:]])
time.sleep(0.5)
left = subprocess.run(["pgrep", "-x", "htop"], capture_output=True, text=True).stdout.strip()
n = len(left.split()) if left else 0
check("htop no queda huérfano", n == BASE_HTOP, left)

# ——— Ctrl+G sin programa ———
s.send("\x07", 0.5)
s.read(0.4)
check("Ctrl+G sin programa avisa", "No hay programa" in s.scr.text())

# ——— en el shell el menú cede a readline ———
s.send("\x14", 0.6)                       # Ctrl+T → shell
s.read(0.5)
s.send("\x10", 0.6)                       # Ctrl+P → cede a bash (history)
s.read(0.5)
t = s.scr.text()
check("Ctrl+P en shell NO abre el menú", "Lanzar programa" not in t and "SHELL" in t)

# ——— salir con el programa en marcha no deja restos ———
s.send("\x1b[Z", 0.5)                     # Shift+Tab → editor
s.read(0.4)
s.send("\x10", 0.5)
s.read(0.4)
s.send("\r", 1.2)                         # lanza htop otra vez
s.read(0.8)
s.send("\x11", 0.8)                       # Ctrl+Q con htop dentro
check("Ctrl+Q sale con el programa en marcha", s.wait_exit(5))
time.sleep(0.6)
left = subprocess.run(["pgrep", "-x", "htop"], capture_output=True, text=True).stdout.strip()
n = len(left.split()) if left else 0
check("no queda htop tras salir", n == BASE_HTOP, left)
# comprobar la integridad del fichero ANTES de cerrar (close() borra el dir)
with open(os.path.join(s.work, "test.org")) as f:
    intacto_s = f.read() == "* prueba\nlinea uno\n"
s.close()

# ——— Alt+G (grupo gui) cierra ———
s2 = Session()
s2.read(1.3)
s2.send("\x1bp", 0.6)                     # Alt+P abre el menú
s2.read(0.5)
check("Alt+P abre el menú", "Lanzar programa" in s2.scr.text())
s2.send("\r", 1.2)
s2.read(1.0)
s2.repaint()
check("Alt+P + Enter lanza", "PROG" in s2.scr.text())
s2.send("\x1bg", 0.7)                     # Alt+G cierra
s2.read(0.6)
check("Alt+G cierra el programa", "NORMAL" in s2.scr.text() and "cerrado" in s2.scr.text())
s2.send("\x1bq", 0.7)                     # Alt+Q sale
check("Alt+Q sale", s2.wait_exit(5))
with open(os.path.join(s2.work, "test.org")) as f:
    intacto_s2 = f.read() == "* prueba\nlinea uno\n"
s2.close()

# ——— el fichero de trabajo no se toca ———
check("test.org intacto", intacto_s and intacto_s2)

print("\n%d fallos" % len(fails))
sys.exit(1 if fails else 0)
