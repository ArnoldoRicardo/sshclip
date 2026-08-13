#!/bin/sh
# Instala sshclip en esta maquina, que debe ser la CLIENTE: la que tiene X11 y
# el portapapeles. En el servidor no hay nada que instalar.
set -eu

src="$(cd "$(dirname "$0")" && pwd)/sshclip"
dest="${1:-$HOME/.local/bin/sshclip}"

[ -f "$src" ] || { echo "install.sh: no encuentro $src" >&2; exit 1; }
command -v python3 >/dev/null || { echo "install.sh: hace falta python3" >&2; exit 1; }

mkdir -p "$(dirname "$dest")"
cp "$src" "$dest"
chmod 755 "$dest"
echo "instalado en $dest"

if ! command -v xclip >/dev/null && ! command -v xsel >/dev/null; then
    echo "aviso: no hay xclip ni xsel; sin uno de los dos no hay portapapeles." >&2
fi
[ -n "${DISPLAY:-}" ] || echo "aviso: DISPLAY vacio; esto solo sirve en la maquina con X11." >&2

case ":$PATH:" in
    *":$(dirname "$dest"):"*) ;;
    *) echo "aviso: $(dirname "$dest") no esta en el PATH." >&2 ;;
esac

python3 "$dest" --selftest
