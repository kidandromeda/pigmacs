#!/usr/bin/env python3
# ————— pigmacs · tests · fase 2: menú Ctrl+P y programa integrado —————
# Autor:      kidandromeda
# Licencia:   MIT
# Versión:    3.1.0
# Fecha:      2026-10-14
"""Fase 2: menú Ctrl+P · programa en la ventana principal · Ctrl+G lo cierra ·
lanzador de aplicaciones gráficas Ctrl+X."""
import sys, time, os, subprocess, shutil, tempfile
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

# ——— Ctrl+X: lanzador de aplicaciones gráficas (.desktop) ———
s2.send("\x18", 0.6)                      # Ctrl+X
s2.read(0.8)
t = s2.scr.text()
check("Ctrl+X abre el lanzador", "Aplicaciones gráficas" in t, t[:400])
check("el lanzador filtra y avisa de las teclas", "> " in t and "Enter lanzar" in t, t[:400])
s2.send("zzznoexiste", 0.5)               # escribir filtra
s2.read(0.5)
check("escribir en el lanzador filtra", "sin coincidencias" in s2.scr.text())
s2.send("\x1b", 0.4)                      # Esc limpia el filtro
s2.read(0.4)
t = s2.scr.text()
check("Esc limpia el filtro", "sin coincidencias" not in t and "Aplicaciones gráficas" in t)
s2.send("\x1b", 0.4)                      # Esc de nuevo cierra
s2.read(0.4)
check("Esc cierra el lanzador", "Aplicaciones gráficas" not in s2.scr.text())
# en el shell cede a readline, igual que el menú Ctrl+P
s2.send("\x14", 0.6)                      # Ctrl+T → shell
s2.read(0.5)
s2.send("\x18", 0.6)                      # Ctrl+X dentro del shell
s2.read(0.5)
t = s2.scr.text()
check("Ctrl+X en shell NO abre el lanzador", "Aplicaciones gráficas" not in t and "SHELL" in t)
s2.send("\x14", 0.6)                      # Ctrl+T → vuelve al editor
s2.read(0.5)
check("Ctrl+T vuelve al editor", "NORMAL" in s2.scr.text())

s2.send("\x1bq", 0.7)                     # Alt+Q sale
check("Alt+Q sale", s2.wait_exit(5))
with open(os.path.join(s2.work, "test.org")) as f:
    intacto_s2 = f.read() == "* prueba\nlinea uno\n"
s2.close()

# ——— Ctrl+X en consola: startx lanzado de forma directa ———
# En una tty real el lanzador NO usa el shell embebido (un /dev/pts no sirve:
# `startx` no le pasa «-keeptty» a Xorg y el servidor no arranca). pigmacs
# lanza startx él mismo heredando la stdio, igual que si lo teclearas a mano.
# Ese camino sólo se da en consola, así que PIGMACS_FAKE_VT simula la tty y
# un startx falso deja constancia de qué recibió (tty y cliente).
faixa = tempfile.mkdtemp(prefix="pigmacs_fake_")
os.makedirs(os.path.join(faixa, "applications"), exist_ok=True)
os.makedirs(os.path.join(faixa, "bin"), exist_ok=True)
with open(os.path.join(faixa, "applications", "pigmacs-fake.desktop"), "w") as f:
    f.write("[Desktop Entry]\nType=Application\nName=PigmacsFakeApp\nExec=/bin/true\n")
with open(os.path.join(faixa, "applications", "pigmacs-fake-args.desktop"), "w") as f:
    f.write("[Desktop Entry]\nType=Application\nName=PigmacsFakeArgs\nExec=/bin/echo hola desde el wrapper\n")
marca = os.path.join(faixa, "marca.txt")
with open(os.path.join(faixa, "bin", "startx"), "w") as f:
    f.write("#!/bin/sh\n"
            "{ echo \"tty=$(tty)\"; echo \"args=$*\"; } > \"$PIGMACS_MARCA\"\n"
            "\"$1\"\nsleep 3\nexit 0\n")
os.chmod(os.path.join(faixa, "bin", "startx"), 0o755)
os.environ["PIGMACS_FAKE_VT"] = "1"
os.environ["PIGMACS_MARCA"] = marca
os.environ["PATH"] = os.path.join(faixa, "bin") + os.pathsep + os.environ["PATH"]
os.environ["XDG_DATA_HOME"] = faixa
os.environ["XDG_DATA_DIRS"] = faixa      # esconde los .desktop del sistema
os.environ.pop("DISPLAY", None)          # sin sesión gráfica: como en una tty
os.environ.pop("WAYLAND_DISPLAY", None)

s3 = Session()
s3.read(1.3)
s3.send("\x18", 0.6)                      # Ctrl+X
s3.read(0.8)
t = s3.scr.text()
check("CON tty: el lanzador lee los .desktop del usuario", "PigmacsFakeApp" in t, t[:400])

# ——— app sin argumentos: startx recibe el binario directamente ———
s3.send("fakeapp", 0.5)
s3.read(0.5)
check("CON tty: filtrar el .desktop de prueba", "▸ PigmacsFakeApp" in s3.scr.text(), s3.scr.text()[:400])
s3.send("\r", 0.3)                        # Enter → spawn directo de startx
check("CON tty: pigmacs sigue vivo con X en la consola", s3.alive())
for _ in range(30):                       # el startx falso tarda poco en marcar
    if os.path.exists(marca):
        break
    time.sleep(0.1)
marca_txt = open(marca).read() if os.path.exists(marca) else ""
check("CON tty: startx hereda la tty de pigmacs (para «-keeptty»)",
      marca_txt.startswith("tty=/dev/") and "args=/bin/true" in marca_txt, marca_txt)

# ——— app con argumentos: wrapper temporal (startx sólo admite el binario) ———
s3.read(3.5)                              # esperar a que el hijo termine
s3.repaint()
check("CON tty: al terminar vuelve a pigmacs", "terminó" in s3.scr.text(),
      [l[-70:] for l in s3.scr.lines()[-2:]])
s3.send("\x18", 0.6)                      # Ctrl+X otra vez
s3.read(0.8)
s3.send("fakeargs", 0.5)
s3.read(0.5)
check("CON tty: el segundo .desktop se puede filtrar", "▸ PigmacsFakeArgs" in s3.scr.text(), s3.scr.text()[:400])
os.remove(marca)                          # marca fresca para el 2º arranque
s3.send("\r", 0.4)                        # Enter → wrapper temporal
s3.read(1.0)
marca_txt = open(marca).read() if os.path.exists(marca) else ""
check("CON tty: con argumentos se manda un wrapper a startx",
      "args=/tmp/pigmacs-launch-" in marca_txt, marca_txt)
check("CON tty: el wrapper ejecuta la app", "hola desde el wrapper" in s3.scr.text(), s3.scr.text()[:300])
s3.read(2.5)
s3.repaint()
check("CON tty: la segunda app también vuelve a pigmacs", "terminó" in s3.scr.text(),
      [l[-70:] for l in s3.scr.lines()[-2:]])
s3.send("\x1bq", 0.7)                     # Alt+Q sale
check("CON tty: Alt+Q sale", s3.wait_exit(5))
s3.close()

# ——— el fichero de trabajo no se toca ———
check("test.org intacto", intacto_s and intacto_s2)

print("\n%d fallos" % len(fails))
sys.exit(1 if fails else 0)
