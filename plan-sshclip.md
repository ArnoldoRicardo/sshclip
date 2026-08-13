# Plan de implementación de `sshclip`

Este plan debe ejecutarse **exclusivamente en la máquina cliente**, donde corren Terminator, X11 y el portapapeles. No requiere realizar cambios en el servidor remoto.

## Arquitectura

```text
Terminator → sshclip local → /usr/bin/ssh → servidor
                    ↓
             xclip local (X11)
```

`sshclip` interceptará las escrituras OSC 52 de OpenCode, actualizará el portapapeles local y mantendrá intacta la sesión interactiva.

## Plan de ejecución

### 1. Verificar el cliente — complejidad baja (`runner`)

- Confirmar que la sesión gráfica usa X11.
- Comprobar las variables y herramientas necesarias: `DISPLAY`, Python 3 y `xclip` o `xsel`.
- No modificar el servidor remoto.

### 2. Crear `~/.local/bin/sshclip` — complejidad media-alta (Builder)

- Envolver `/usr/bin/ssh` mediante una pseudoterminal (PTY).
- Aceptar y reenviar todos los argumentos de SSH.
- Preservar colores, programas interactivos, `Ctrl+C`, redimensionamiento y código de salida.
- Procesar OSC 52 incrementalmente, incluso cuando la secuencia llegue fragmentada.
- Admitir los terminadores BEL y `ESC \\`.
- Retirar la secuencia OSC 52 antes de mostrar la salida en Terminator.
- Enviar el contenido decodificado a `xclip -selection clipboard` sin usar un shell.

### 3. Añadir controles de seguridad — complejidad media (Builder)

- Permitir únicamente escritura remoto → local.
- Rechazar consultas OSC 52 que intenten leer el portapapeles.
- Validar Base64 estrictamente.
- Limitar cada copia a 1 MiB.
- Activarse exclusivamente al ejecutar `sshclip`; no sustituir globalmente `ssh`.
- Restaurar siempre el estado de Terminator al salir o recibir señales.

### 4. Instalación local — complejidad baja (Builder)

- Crear el ejecutable en `~/.local/bin/sshclip`.
- No modificar `~/.ssh/config`.
- No crear servicios, sockets ni puertos.
- No usar `sudo` sin autorización.

### 5. Verificación — complejidad media (`runner`)

- Probar texto normal, colores, Unicode y secuencias fragmentadas.
- Probar redimensionamiento, `Ctrl+C`, `~.` y programas interactivos.
- Conectarse mediante:

  ```bash
  sshclip ar@192.168.100.35
  ```

- Abrir OpenCode en el servidor, copiar un mensaje y comprobar el portapapeles local con:

  ```bash
  xclip -selection clipboard -o
  ```

### 6. Dejar Mosh fuera de la primera fase

- Validar primero el funcionamiento sobre SSH.
- Evaluar Mosh posteriormente mediante un wrapper independiente.

## Riesgos

- Cualquier proceso remoto podrá sobrescribir el portapapeles mientras se use `sshclip`.
- Un error en el parser podría afectar la representación de la terminal.
- Si el script termina abruptamente, podría ser necesario ejecutar `reset`.
- El funcionamiento con tmux remoto deberá probarse por separado.

## Instrucciones para el agente de la máquina cliente

```text
Implementa el plan de sshclip exclusivamente en esta máquina cliente.

Objetivo:
Conservar Terminator y permitir que OpenCode ejecutándose en un servidor por
SSH copie mediante OSC 52 al portapapeles X11 local.

Crea ~/.local/bin/sshclip como wrapper interactivo de /usr/bin/ssh usando una
PTY. Debe pasar todos los argumentos de SSH, preservar colores, entrada,
Ctrl+C, SIGWINCH, redimensionamiento y código de salida.

Intercepta solamente OSC 52 de escritura, con terminación BEL o ESC-backslash,
aunque llegue dividido entre lecturas. Decodifica Base64 estrictamente y
escribe los bytes con xclip en CLIPBOARD, sin usar shell. Elimina la secuencia
OSC 52 de la salida antes de enviarla a Terminator.

Seguridad:
- Nunca permitir lectura del portapapeles.
- Rechazar payloads inválidos o mayores de 1 MiB.
- No abrir puertos ni crear servicios.
- No modificar ~/.ssh/config.
- No reemplazar ni crear un alias global para ssh.
- No modificar, instalar ni crear nada en el servidor remoto.
- Preguntar antes de usar sudo o instalar xclip.
- Restaurar siempre la configuración de la terminal.

Prueba el parser con secuencias completas, fragmentadas, Unicode, BEL,
ESC-backslash, Base64 inválido y tamaño excesivo. Después verifica una sesión
interactiva contra 192.168.100.35 y el copiado de OpenCode. No hagas commit ni
push.
```
