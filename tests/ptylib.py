#!/usr/bin/env python3
"""Utilidades comunes para los tests PTY de pigmacs.

Screen = mini-emulador VT100 (CUP, CR/LF/BS, EL, ED, SGR ignorado) que
reconstruye la pantalla a partir de la salida cruda, tolerando el renderer
por diff de pigmacs (que sólo emite las celdas cambiadas).
"""
import os, pty, time, select, fcntl, termios, struct, re, tempfile, shutil

# binario: relativo al repo (o PIGMACS_BIN para probar otra compilación)
BIN = os.environ.get(
    "PIGMACS_BIN",
    os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "target", "debug", "pigmacs",
    ),
)


class Screen:
    def __init__(self, w=110, h=30):
        self.w, self.h = w, h
        self.clear()
        self.x = self.y = 0

    def clear(self):
        self.rows = [[" "] * self.w for _ in range(self.h)]

    def put(self, c):
        if self.x >= self.w:
            self.x = 0
            self.y = min(self.y + 1, self.h - 1)
        if self.y < self.h and self.x < self.w:
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
                    elif fin == "J" and params in ("2", "3"):
                        self.clear()
                    elif fin == "K" and params in ("0", "1", "2"):
                        if params == "2":
                            self.rows[self.y] = [" "] * self.w
                        elif params == "1":
                            for k in range(min(self.x + 1, self.w)):
                                self.rows[self.y][k] = " "
                        else:
                            for k in range(self.x, self.w):
                                self.rows[self.y][k] = " "
                    i = j + 1
                    continue
                elif i + 1 < len(s) and s[i + 1] == "]":
                    j = i + 2
                    while j < len(s) and s[j] != "\x07":
                        if s[j] == "\x1b" and j + 1 < len(s) and s[j + 1] == "\\":
                            j += 1
                            break
                        j += 1
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

    def text(self):
        return "\n".join(self.lines())

    def flat(self):
        return re.sub(r"\s+", "", self.text())


class Session:
    def __init__(self, files=None, w=110, h=30, dirs=None):
        self.w, self.h = w, h
        self.scr = Screen(w, h)
        self.work = tempfile.mkdtemp(prefix="pigmacs_test_")
        os.makedirs(self.work, exist_ok=True)
        for d in dirs or []:
            os.makedirs(os.path.join(self.work, d), exist_ok=True)
        files = files if files is not None else {"test.org": "* prueba\nlinea uno\n"}
        for name, content in files.items():
            with open(os.path.join(self.work, name), "w") as f:
                f.write(content)
        main = next(iter(files))
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ["TERM"] = "xterm-256color"
            os.chdir(self.work)
            os.execv(BIN, ["pigmacs", main])
            os._exit(1)
        self.resize()

    def resize(self, row=None, col=None):
        fcntl.ioctl(
            self.fd, termios.TIOCSWINSZ,
            struct.pack("HHHH", row or self.h, col or self.w, 0, 0),
        )

    def read(self, secs=0.7):
        out = b""
        end = time.time() + secs
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.1)
            if r:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    break
                if not data:
                    break
                out += data
        self.scr.feed(out)
        return out

    def send(self, data, delay=0.4):
        if isinstance(data, str):
            data = data.encode()
        os.write(self.fd, data)
        time.sleep(delay)

    def repaint(self):
        """TIOCSWINSZ de ida y vuelta fuerza el repaint completo del frame."""
        self.resize(col=self.w + 1)
        time.sleep(0.3)
        self.resize()
        time.sleep(0.4)
        return self.read(0.8)

    def alive(self):
        return os.waitpid(self.pid, os.WNOHANG) == (0, 0)

    def wait_exit(self, secs=4.0):
        end = time.time() + secs
        while time.time() < end:
            if not self.alive():
                return True
            time.sleep(0.2)
        return False

    def close(self):
        try:
            os.close(self.fd)
        except OSError:
            pass
        shutil.rmtree(self.work, ignore_errors=True)


def check_factory(fails):
    def check(name, cond, extra=""):
        print(("OK    " if cond else "FALLA ") + name
              + (("  :: " + str(extra)[:250]) if extra and not cond else ""))
        if not cond:
            fails.append(name)
    return check
