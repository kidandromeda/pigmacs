#!/usr/bin/env python3
# ————— pigmacs · tests · parte A: limpieza al arrancar y Delete/Ctrl+U —————
# Autor:      kidandromeda
# Licencia:   MIT
# Versión:    3.0.1
# Fecha:      2026-10-13
"""Parte A: limpieza al arrancar + fantasmas del Delete/Ctrl+U en el shell.

Reconstruye la pantalla con un mini-emulador VT100 (CUP/EL/CR/LF/SGR) a
partir de la salida cruda de pigmacs, y compara la línea pintada con la que
bash ejecuta de verdad (la verdad es la salida de `echo`).
"""
import os, pty, time, select, fcntl, termios, struct, re, sys, tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ptylib import BIN

WORK = tempfile.mkdtemp(prefix="pigmacs_partA_")
with open(os.path.join(WORK, "test.org"), "w") as f:
    f.write("* prueba\nlinea uno\n")

fails = []


def check(name, cond, extra=""):
    print(("OK    " if cond else "FALLA ") + name + (("  :: " + str(extra)[:300]) if extra and not cond else ""))
    if not cond:
        fails.append(name)


class Screen:
    """mini VT100: CUP, CR, LF, BS, ED(2), caracteres imprimibles"""

    def __init__(self, w=110, h=30):
        self.w, self.h = w, h
        self.clear()
        self.x = self.y = 0
        self.alt = False

    def clear(self):
        self.rows = [[" "] * self.w for _ in range(self.h)]

    def put(self, c):
        if self.x >= self.w:
            self.x = 0
            self.y = min(self.y + 1, self.h - 1)
        self.rows[self.y][self.x] = c
        self.x += 1

    def feed(self, data):
        s = data.decode("utf-8", "replace")
        i = 0
        while i < len(s):
            ch = s[i]
            if ch == "\x1b":
                if i + 1 < len(s) and s[i + 1] == "[":
                    j = i + 2
                    while j < len(s) and not ("@" <= s[j] <= "~"):
                        j += 1
                    if j >= len(s):
                        break
                    params, fin = s[i + 2:j], s[j]
                    if fin == "H":
                        ps = [int(p) if p else 1 for p in params.split(";")[:2]] if params else [1, 1]
                        while len(ps) < 2:
                            ps.append(1)
                        self.y = min(max(ps[0] - 1, 0), self.h - 1)
                        self.x = min(max(ps[1] - 1, 0), self.w - 1)
                    elif fin == "J" and params in ("2", "3", ""):
                        self.clear()
                    elif fin == "K" and params in ("", "0", "1", "2"):
                        if params == "2" or params == "":
                            if params == "2":
                                self.rows[self.y] = [" "] * self.w
                        elif params == "1":
                            for k in range(min(self.x + 1, self.w)):
                                self.rows[self.y][k] = " "
                        else:
                            for k in range(self.x, self.w):
                                self.rows[self.y][k] = " "
                    # el resto (SGR, show/hide cursor, DCH de pigmacs…) se ignora
                    i = j + 1
                    continue
                elif i + 1 < len(s) and s[i + 1] == "]":
                    # OSC: saltar hasta BEL o ST
                    j = i + 2
                    while j < len(s) and s[j] not in ("\x07",):
                        if s[j] == "\x1b" and j + 1 < len(s) and s[j + 1] == "\\":
                            j += 1
                            break
                        j += 1
                    else:
                        i = j + 1
                        continue
                    i = j + 1
                    continue
                else:
                    i += 2
                    continue
            elif ch == "\r":
                self.x = 0
            elif ch == "\n":
                self.y = min(self.y + 1, self.h - 1)
                self.x = 0
            elif ch == "\x08":
                self.x = max(self.x - 1, 0)
            elif ch in ("\x07", "\x0b", "\x0c"):
                pass
            elif ch >= " ":
                self.put(ch)
            i += 1

    def lines(self):
        return ["".join(r).rstrip() for r in self.rows]


def spawn():
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.chdir(WORK)
        os.execv(BIN, ["pigmacs", "test.org"])
        os._exit(1)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 110, 0, 0))
    return pid, fd


def read_all(fd, secs=0.7):
    out = b""
    end = time.time() + secs
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            out += data
    return out


def send(fd, data, delay=0.35):
    os.write(fd, data)
    time.sleep(delay)


def repaint(fd, row=30, col=110):
    """TIOCSWINSZ de ida y vuelta fuerza el repaint completo del frame."""
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", row, col + 1, 0, 0))
    time.sleep(0.3)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", row, col, 0, 0))
    time.sleep(0.4)
    return read_all(fd, 0.8)


pid, fd = spawn()
scr = Screen()
first = read_all(fd, 1.2)
check("al arrancar limpia la pantalla (ESC[2J)", b"\x1b[2J" in first, repr(first[:120]))
scr.feed(first)
check("arranca en modo NORMAL", any("NORMAL" in l for l in scr.lines()))

# ——— Delete: la línea pintada debe coincidir con la que bash ejecuta ———
send(fd, b"\x14", 0.6)                    # Ctrl+T → shell
scr.feed(read_all(fd, 0.4))
send(fd, b"echo aabbcc", 0.5)
scr.feed(read_all(fd, 0.3))
send(fd, b"\x1b[D\x1b[D\x1b[D", 0.3)      # Left x3
scr.feed(read_all(fd, 0.3))
for _ in range(3):
    send(fd, b"\x1b[3~", 0.3)             # Delete x3
    scr.feed(read_all(fd, 0.3))
scr.feed(repaint(fd))

def input_line(scr):
    cand = [l for l in scr.lines() if "echo" in l]
    return cand[-1] if cand else ""

linea = input_line(scr)
m = re.search(r"echo([a-z]*)$", re.sub(r"\s+", "", linea))
linea_vista = "echo" + (m.group(1) if m else "?")
print("  línea en pantalla:", repr(linea), "->", linea_vista)

send(fd, b"\r", 1.0)                      # ejecutar
scr.feed(read_all(fd, 1.0))
scr.feed(repaint(fd))
ls = scr.lines()
idx = None
for i, l in enumerate(ls):
    if re.sub(r"\s+", "", l).endswith("$" + linea_vista):
        idx = i
salida = None
if idx is not None:
    for l in ls[idx + 1:]:
        f = re.sub(r"\s+", "", l.replace("│", ""))  # saltar el borde del panel
        if re.fullmatch(r"[a-z]+", f):
            salida = f
            break
print("  bash ejecutó:", salida)
check("sin caracteres fantasma (pantalla == ejecución)",
      salida is not None and linea_vista == "echo" + salida,
      f"pantalla={linea_vista} bash={salida}")

# ——— Ctrl+U (EL) debe limpiar la línea pintada ———
send(fd, b"echo zzzzzz", 0.5)
scr.feed(read_all(fd, 0.3))
send(fd, b"\x15", 0.6)                    # Ctrl+U → readline emite \r + CUF + EL
scr.feed(repaint(fd))
check("Ctrl+U limpia la línea pintada",
      not any("zzzzzz" in l for l in scr.lines()),
      [l for l in scr.lines() if "zz" in l])
send(fd, b"\x03", 0.4)                    # Ctrl+C por si acaso
scr.feed(read_all(fd, 0.4))

# ——— a la vuelta de Insertar/Backspace también ———
send(fd, b"echo QQ", 0.5)
scr.feed(read_all(fd, 0.3))
send(fd, b"\x7f\x7f", 0.5)                # Backspace x2
scr.feed(repaint(fd))
linea = input_line(scr)
check("Backspace no deja fantasmas",
      re.sub(r"\s+", "", linea).endswith("$echo"),
      repr(linea))
send(fd, b"\x03", 0.4)
scr.feed(read_all(fd, 0.4))

send(fd, b"\x11", 0.6)
r = os.waitpid(pid, os.WNOHANG)
check("Ctrl+Q sale", r[0] == pid, str(r))
try:
    os.close(fd)
except OSError:
    pass

print("\n%d fallos" % len(fails))
sys.exit(1 if fails else 0)
