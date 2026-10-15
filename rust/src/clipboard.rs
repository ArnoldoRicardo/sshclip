//! Portapapeles X11 via xclip/xsel, con verificacion y reintentos.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// (comando de escritura, comando de lectura)
pub type Cmds = (Vec<OsString>, Vec<OsString>);

const RETRY_WAITS: [Duration; 4] = [
    Duration::from_millis(80),
    Duration::from_millis(150),
    Duration::from_millis(250),
    Duration::from_millis(400),
];

const CMD_TIMEOUT: Duration = Duration::from_secs(5);

/// Como `shutil.which`: primer ejecutable con ese nombre en el PATH.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        let candidate = dir.join(name);
        let meta = std::fs::metadata(&candidate).ok()?;
        (meta.is_file() && meta.permissions().mode() & 0o111 != 0).then_some(candidate)
    })
}

/// Comandos de escritura y lectura de CLIPBOARD, o `None` si no hay ninguno.
pub fn clipboard_cmds() -> Option<Cmds> {
    let os = |parts: &[&str]| parts.iter().map(OsString::from).collect::<Vec<_>>();
    if let Some(xclip) = which("xclip") {
        let x = xclip.into_os_string();
        let mut w = vec![x.clone()];
        w.extend(os(&["-selection", "clipboard", "-i"]));
        let mut r = vec![x];
        r.extend(os(&["-selection", "clipboard", "-o"]));
        return Some((w, r));
    }
    if let Some(xsel) = which("xsel") {
        let x = xsel.into_os_string();
        let mut w = vec![x.clone()];
        w.extend(os(&["--input", "--clipboard"]));
        let mut r = vec![x];
        r.extend(os(&["--output", "--clipboard"]));
        return Some((w, r));
    }
    None
}

pub fn dbg(msg: &str) {
    let Some(path) = std::env::var_os("SSHCLIP_DEBUG").filter(|p| !p.is_empty()) else {
        return;
    };
    if let Ok(mut fh) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
    {
        let _ = writeln!(fh, "{msg}");
    }
}

/// Corre `argv` con un plazo. `input` va por stdin; con `capture` devuelve
/// stdout. Al vencer el plazo mata al hijo y devuelve error.
fn run_timeout(
    argv: &[OsString],
    input: Option<&[u8]>,
    capture: bool,
    new_session: bool,
) -> io::Result<Vec<u8>> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null());
    if new_session {
        // SAFETY: setsid es async-signal-safe y no toca memoria del padre.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let mut child = cmd.spawn()?;

    let writer = match (input, child.stdin.take()) {
        (Some(data), Some(mut stdin)) => {
            let data = data.to_vec();
            Some(thread::spawn(move || {
                let _ = stdin.write_all(&data);
            }))
        }
        _ => None,
    };
    let reader = child.stdout.take().map(|mut stdout| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = Instant::now() + CMD_TIMEOUT;
    let timed_out = loop {
        if child.try_wait()?.is_some() {
            break false;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break true;
        }
        thread::sleep(Duration::from_millis(10));
    };

    if let Some(w) = writer {
        let _ = w.join();
    }
    let out = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    if timed_out {
        return Err(io::Error::from(io::ErrorKind::TimedOut));
    }
    Ok(out)
}

/// Escribe en CLIPBOARD y verifica, reintentando si hace falta.
///
/// xfce4-clipman (el gestor de portapapeles de XFCE) compite por la propiedad
/// de la seleccion y se come la primera escritura mas o menos la mitad de las
/// veces; pasa igual con `xsel -ib` a pelo, sin este wrapper de por medio. Por
/// eso releemos el portapapeles y reintentamos.
///
/// La relectura es puramente local: nada vuelve nunca al lado remoto.
pub fn copy(cmds: Option<&Cmds>, payload: &[u8]) {
    dbg(&format!("copy: {} bytes", payload.len()));
    let Some((write_cmd, read_cmd)) = cmds else {
        return;
    };
    for wait in RETRY_WAITS {
        if run_timeout(write_cmd, Some(payload), false, true).is_err() {
            return; // el portapapeles nunca debe tumbar la sesion
        }
        thread::sleep(wait);
        let Ok(got) = run_timeout(read_cmd, None, true, false) else {
            return;
        };
        // got != payload tambien cuando xsel corta en un NUL: si lo leido es
        // prefijo de lo enviado, damos la copia por buena.
        if !got.is_empty() && payload.starts_with(&got) {
            return;
        }
    }
    dbg(&format!(
        "aviso: el portapapeles no confirmo la copia tras {} intentos",
        RETRY_WAITS.len()
    ));
}
