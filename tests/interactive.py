#!/usr/bin/env python3
"""Pruebas interactivas de sshclip: se le da una PTY real y se habla con el.

    python3 tests/interactive.py [host]      (por defecto: cyxpc-b)

Necesitan un servidor accesible por ssh sin contrasena y una sesion X11 viva:
una de las pruebas copia de verdad al portapapeles y lo relee.
"""

import fcntl
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import time

HERE = os.path.dirname(os.path.abspath(__file__))
SSHCLIP = os.path.join(os.path.dirname(HERE), "sshclip")
HOST = sys.argv[1] if len(sys.argv) > 1 else "cyxpc-b"

ok = True


def check(name, cond, extra=""):
    global ok
    if not cond:
        ok = False
    print("%s  %s%s" % ("ok   " if cond else "FALLA", name, ("  <- " + extra) if extra and not cond else ""))


def setsize(fd, rows, cols):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def spawn(args, rows=24, cols=80):
    pid, fd = pty.fork()
    if pid == 0:
        try:
            os.execv(sys.executable, [sys.executable, SSHCLIP] + args)
        except OSError:
            os._exit(127)
    setsize(fd, rows, cols)
    return pid, fd


def read_until(fd, pattern, timeout=25.0):
    buf = b""
    end = time.time() + timeout
    rx = re.compile(pattern)
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.3)
        if r:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            buf += chunk
            if rx.search(buf):
                return buf, True
    return buf, False


def wait(pid, timeout=15.0):
    end = time.time() + timeout
    while time.time() < end:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            return status
        time.sleep(0.1)
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    return None


def clip_write(data):
    subprocess.run(["xsel", "--input", "--clipboard"], input=data)


def clip_read():
    return subprocess.run(["xsel", "--output", "--clipboard"], capture_output=True).stdout


# --- 1. tamano de ventana inicial ----------------------------------------
pid, fd = spawn(["-t", HOST, "stty size"], rows=41, cols=137)
buf, hit = read_until(fd, rb"41 137")
wait(pid)
os.close(fd)
check("tamano de ventana inicial llega al remoto (41x137)", hit, repr(buf[-120:]))

# --- 2. colores intactos --------------------------------------------------
pid, fd = spawn(["-t", HOST, "printf '\\033[1;31mROJO\\033[0m\\n'"])
buf, hit = read_until(fd, rb"ROJO")
wait(pid)
os.close(fd)
check("secuencias de color pasan sin tocar", b"\x1b[1;31mROJO\x1b[0m" in buf, repr(buf[-120:]))

# --- 3. shell interactivo + SIGWINCH --------------------------------------
pid, fd = spawn(["-t", HOST, "bash --noprofile --norc -i"], rows=24, cols=80)
buf, hit = read_until(fd, rb"\$|#", timeout=20)
os.write(fd, b"stty size\n")
buf, hit1 = read_until(fd, rb"24 80")
setsize(fd, 55, 173)
os.kill(pid, signal.SIGWINCH)
time.sleep(0.7)
os.write(fd, b"stty size\n")
buf2, hit2 = read_until(fd, rb"55 173")
check("shell interactivo responde", hit1, repr(buf[-160:]))
check("SIGWINCH propaga el nuevo tamano (55x173)", hit2, repr(buf2[-160:]))

# --- 4. Ctrl+C interrumpe el proceso remoto -------------------------------
os.write(fd, b"sleep 60; echo TRAS-CTRLC\n")
time.sleep(1.5)
t0 = time.time()
os.write(fd, b"\x03")
buf3, hit3 = read_until(fd, rb"TRAS-CTRLC", timeout=10)
check("Ctrl+C mata el sleep remoto (<10s)", hit3 and time.time() - t0 < 10, repr(buf3[-160:]))

# --- 5. copia OSC 52 desde sesion interactiva -----------------------------
clip_write(b"antes-de-la-copia")
time.sleep(0.3)
marca = "copiado-interactivo-%d n 日本語" % os.getpid()
os.write(fd, ("printf '\\033]52;c;%%s\\007' \"$(printf '%s' | base64 -w0)\"; echo COPIA-HECHA\n" % marca).encode())
buf4, hit4 = read_until(fd, rb"COPIA-HECHA", timeout=15)
time.sleep(1.2)
got = clip_read()
check("copia OSC 52 en sesion interactiva", got.decode() == marca, repr(got[:120]))
check("la secuencia OSC 52 no se pinta en pantalla", b"\x1b]52" not in buf4, repr(buf4[-160:]))

# --- 6. escape ~. cierra la sesion ----------------------------------------
os.write(fd, b"\n")
time.sleep(0.5)
os.write(fd, b"\r~.")
status = wait(pid, timeout=12)
os.close(fd)
check("el escape ~. cierra la sesion", status is not None, "sshclip no murio")

# --- 7. codigo de salida remoto -------------------------------------------
pid, fd = spawn([HOST, "exit 7"])
status = wait(pid)
os.close(fd)
check(
    "codigo de salida remoto (7)",
    status is not None and os.WIFEXITED(status) and os.WEXITSTATUS(status) == 7,
    str(status),
)

# --- 8. la terminal queda como estaba -------------------------------------
pid, fd = spawn([HOST, "true"])
before = termios.tcgetattr(fd)
wait(pid)
after = termios.tcgetattr(fd)
os.close(fd)
check("la terminal queda como estaba al salir", before == after)

print()
print("interactivo:", "todo verde" if ok else "hay fallos")
sys.exit(0 if ok else 1)
