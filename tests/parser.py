#!/usr/bin/env python3
"""Vectores del parser OSC 52: la especificacion compartida con el port.

    python3 tests/parser.py

Lee tests/vectors.json y pasa cada caso por Osc52Filter. Cualquier otra
implementacion (la de Rust) debe pasar los mismos vectores byte a byte.

Un "segmento" describe bytes en JSON:
    "texto"                       latin-1: cada code point 0-255 es un byte
    {"utf8": "日本語"}             los bytes UTF-8 del texto
    {"b64": seg}                  el base64 de los bytes de seg
    {"repeat": seg, "times": N}   seg repetido N veces
    [seg, seg, ...]               concatenacion

Cada caso: "chunks" (lista de segmentos, uno por feed()), "split": "bytewise"
opcional (parte la entrada byte a byte), "out" (todo lo emitido, flush()
incluido) y "payloads" (lista de segmentos, en orden).
"""

import base64
import importlib.machinery
import importlib.util
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SSHCLIP = os.path.join(os.path.dirname(HERE), "sshclip")
VECTORS = os.path.join(HERE, "vectors.json")

sys.dont_write_bytecode = True  # sin __pycache__ junto al script
_loader = importlib.machinery.SourceFileLoader("sshclip", SSHCLIP)
_spec = importlib.util.spec_from_loader("sshclip", _loader)
sshclip = importlib.util.module_from_spec(_spec)
_loader.exec_module(sshclip)

ok = True


def check(name, cond, extra=""):
    global ok
    if not cond:
        ok = False
    print("%s  %s%s" % ("ok   " if cond else "FALLA", name, ("  <- " + extra) if extra and not cond else ""))


def expand(seg):
    if isinstance(seg, str):
        return seg.encode("latin-1")
    if isinstance(seg, list):
        return b"".join(expand(s) for s in seg)
    if "utf8" in seg:
        return seg["utf8"].encode("utf-8")
    if "b64" in seg:
        return base64.b64encode(expand(seg["b64"]))
    if "repeat" in seg:
        return expand(seg["repeat"]) * seg["times"]
    raise ValueError("segmento desconocido: %r" % (seg,))


def short(data):
    return repr(data if len(data) <= 80 else data[:80] + b"...(%d bytes)" % len(data))


def run_case(case):
    chunks = [expand(c) for c in case["chunks"]]
    if case.get("split") == "bytewise":
        whole = b"".join(chunks)
        chunks = [whole[i : i + 1] for i in range(len(whole))]
    f = sshclip.Osc52Filter()
    out = bytearray()
    pays = []
    for c in chunks:
        o, p = f.feed(c)
        out += o
        pays += p
    out += f.flush()
    return bytes(out), pays


with open(VECTORS, encoding="utf-8") as fh:
    cases = json.load(fh)["cases"]

for case in cases:
    out, pays = run_case(case)
    want_out = expand(case["out"])
    want_pays = [expand(p) for p in case["payloads"]]
    extra = ""
    if out != want_out:
        extra = "out %s, esperaba %s" % (short(out), short(want_out))
    elif pays != want_pays:
        extra = "payloads %s, esperaba %s" % (
            [short(p) for p in pays],
            [short(p) for p in want_pays],
        )
    check(case["name"], not extra, extra)

# Lo unico que no cabe en JSON: el overflow tiene que acotar la memoria, no
# solo rechazar al final. 2 MiB de base64 sin terminador no pasan de MAX_B64.
f = sshclip.Osc52Filter()
f.feed(b"\x1b]52;c;" + b"A" * (2 << 20))
check(
    "overflow acota el buffer a MAX_B64",
    f.overflow and len(f.data) <= sshclip.MAX_B64,
    "overflow=%r len=%d" % (f.overflow, len(f.data)),
)

print()
print("parser: %d vectores," % len(cases), "todo verde" if ok else "hay fallos")
sys.exit(0 if ok else 1)
