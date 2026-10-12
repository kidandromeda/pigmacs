#!/usr/bin/env python3
"""Smoke de regresión (fase 1 + parte A): modos, editor, shell, tree, IA, ayuda."""
import sys, time, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ptylib import Session, check_factory

fails = []
check = check_factory(fails)

# ——— sesión principal (grupo Ctrl) ———
s = Session()
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

print("\n%d fallos" % len(fails))
sys.exit(1 if fails else 0)
