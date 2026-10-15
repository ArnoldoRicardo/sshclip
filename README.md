# sshclip

Wrapper de `ssh` que traduce las secuencias **OSC 52** del lado remoto al portapapeles
X11 local.

Terminator (VTE 0.70) no soporta OSC 52, así que las TUI que copian por esa vía —el modo
fullscreen de Claude Code y de opencode corriendo en un servidor por ssh— no llegaban
nunca al portapapeles de la laptop. `sshclip` arranca `ssh` dentro de una PTY, se queda
con los OSC 52 de escritura, los borra del flujo antes de que Terminator los pinte y
manda el contenido decodificado a `CLIPBOARD`.

```text
Terminator → sshclip → /usr/bin/ssh → servidor
                 ↓
          xclip/xsel (X11 local)
```

Todo ocurre en la máquina cliente. El servidor no se toca: ni configuración, ni
paquetes, ni servicios, ni puertos.

## Uso

Igual que `ssh`, con las mismas banderas:

```sh
sshclip cyxpc-b
sshclip azul
sshclip -p 2222 ar@192.168.100.35
```

Preserva colores, entrada interactiva, `Ctrl+C`, redimensionado (SIGWINCH), el escape
`~.` y el código de salida remoto.

## Instalación

En la **máquina cliente** (la que tiene X11 y el portapapeles):

```sh
./install.sh                 # copia a ~/.local/bin/sshclip y corre el selftest
./install.sh /otra/ruta      # o donde quieras
```

Sin clonar el repo, tirando de cyxpc-b:

```sh
scp cyxpc-b:~/dev/sshclip/sshclip ~/.local/bin/sshclip && chmod +x ~/.local/bin/sshclip
```

Requisitos: Python 3 (sin dependencias externas), X11, y `xclip` o `xsel`.

### Versión en Rust (`rust/`)

Port del script, byte a byte: un binario estático (musl, ~630 KB) que no necesita
Python. Por ahora se instala **al lado** del de Python, como `sshclip-rs`, hasta pasar la
prueba manual del portapapeles.

```sh
# en cyxpc-b (donde está el repo y cargo)
cd rust && cargo test && cargo build --release --target x86_64-unknown-linux-musl

# en la laptop
scp cyxpc-b:~/dev/sshclip/rust/target/x86_64-unknown-linux-musl/release/sshclip ~/.local/bin/sshclip-rs
sshclip-rs --selftest                                      # los vectores, embebidos
SSHCLIP_BIN=~/.local/bin/sshclip-rs python3 tests/interactive.py cyxpc-b
```

`cargo test` y `--selftest` corren el mismo `tests/vectors.json` que la versión de
Python: es la especificación de las dos. El decodificador base64 replica a Python, que
acepta bits sobrantes (`aGl=` → `hi`) y relleno faltante; el engine `STANDARD` del crate
`base64` los rechaza, y hay vectores que lo detectan.

`SSHCLIP_SSH_PATH`, si se define **al compilar**, cambia la ruta de ssh (por defecto
`/usr/bin/ssh`). Solo sirve para probar la suite interactiva contra un ssh falso; en
ejecución no hay forma de cambiarla.

## Pruebas

```sh
python3 sshclip --selftest      # 18 pruebas del parser, sin tocar la red
python3 tests/parser.py         # 67 vectores del parser (tests/vectors.json)
python3 tests/interactive.py cyxpc-b   # 14 pruebas con PTY real contra un servidor
```

`--selftest` va dentro del script para que `install.sh` pruebe la copia instalada.
`tests/vectors.json` es la especificación completa del parser, escrita como datos para
que cualquier implementación (el port de Rust) pase los mismos casos byte a byte. Cubre
todas las líneas y ramas alcanzables de `Osc52Filter` y `_b64_decode`, incluidos
comportamientos raros que se fijan tal cual (el byte que se traga un ESC abortado, el Pc
de más de 16 bytes, el OSC 52 vacío que copia una cadena vacía). El formato está
documentado en `tests/parser.py`.

Las interactivas comprueban tamaño de ventana inicial, colores intactos, shell
interactivo, SIGWINCH, `Ctrl+C`, copia OSC 52 real, que la secuencia no se pinte, el
escape `~.`, el código de salida, que la terminal quede restaurada, SIGTERM, que un EOF
en stdin no corte la sesión y el 128+n cuando ssh muere por señal. Para probar otro
binario:

```sh
SSHCLIP_BIN=./target/release/sshclip python3 tests/interactive.py cyxpc-b
```

Fuera de las automáticas, y por eso se prueban a mano, quedan los reintentos de `_copy()`
contra `xfce4-clipman` y la elección entre xclip y xsel.

## Seguridad

- **Solo remoto → local.** Las consultas OSC 52 (`ESC ] 52 ; c ; ? BEL`, con las que un
  proceso remoto pediría *leer* tu portapapeles) se descartan sin responder.
- Base64 validado contra su alfabeto antes de decodificar.
- Tope de 1 MiB por copia; lo que pase se descarta entero.
- Se activa solo al invocar `sshclip`. No reemplaza `ssh`, no crea alias globales, no
  toca `~/.ssh/config`, no abre puertos ni levanta servicios.
- La terminal se restaura siempre al salir, también por señal.

Ojo con lo obvio: mientras uses `sshclip`, **cualquier proceso del servidor puede
sobrescribir tu portapapeles local**. Es el precio de la función.

## Límites conocidos

- **`xfce4-clipman`** (y probablemente otros gestores de portapapeles) compite por la
  propiedad de la selección y se come la primera escritura más o menos la mitad de las
  veces. Pasa igual con `xsel -ib` a pelo, sin este wrapper de por medio. Por eso
  `_copy()` relee el portapapeles y reintenta hasta 4 veces; **no quitar ese reintento**.
  La relectura es puramente local: nada vuelve nunca al lado remoto.
- `xsel` corta el contenido en el primer byte NUL, así que copiar binario no es fiable.
  Texto sí: verificado hasta 600 KB con md5 coincidente.
- Mosh queda fuera: necesitaría un wrapper aparte.
- tmux remoto no está probado.

## Origen

`plan-sshclip.md` es el plan de implementación del que salió esto.
