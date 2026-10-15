//! La sesion: ssh dentro de una PTY, salida filtrada, senales y terminal.

use std::ffi::{CString, OsString};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicI32, Ordering};

use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::sys::signal::{Signal, kill};
use nix::sys::termios::{SetArg, Termios, cfmakeraw, tcgetattr, tcsetattr};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::Pid;

use crate::clipboard;
use crate::filter::Osc52Filter;

/// Fijado en compilacion; `SSHCLIP_SSH_PATH` solo existe para probar la suite
/// interactiva contra un ssh falso. No hay forma de cambiarlo en ejecucion.
pub const SSH: &str = match option_env!("SSHCLIP_SSH_PATH") {
    Some(p) => p,
    None => "/usr/bin/ssh",
};

const STDIN: RawFd = 0;
const STDOUT: RawFd = 1;
const BUF: usize = 65536;

// -- io -------------------------------------------------------------------

fn read_fd(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: buf es valido para buf.len() bytes.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

fn write_all(fd: RawFd, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        // SAFETY: data es valido para data.len() bytes.
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n < 0 {
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EAGAIN) => {
                    let mut p = libc::pollfd {
                        fd,
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    // SAFETY: un pollfd valido.
                    unsafe { libc::poll(&mut p, 1, -1) };
                    continue;
                }
                _ => return Err(err),
            }
        }
        data = &data[n as usize..];
    }
    Ok(())
}

fn set_cloexec(fd: RawFd) {
    // SAFETY: fcntl sobre un fd propio.
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, fl | libc::FD_CLOEXEC);
    }
}

fn stdin_winsize() -> Winsize {
    let mut ws = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let mut got = ws;
    // SAFETY: TIOCGWINSZ escribe un winsize en got.
    if unsafe { libc::ioctl(STDIN, libc::TIOCGWINSZ, &mut got) } == 0 {
        ws = got;
    }
    ws
}

fn copy_winsize(master: RawFd) {
    let ws = stdin_winsize();
    // SAFETY: TIOCSWINSZ lee un winsize valido.
    unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &ws) };
}

// -- senales --------------------------------------------------------------

/// Extremo de escritura de la tuberia de despertar (-1 si no hay).
static WAKE_FD: AtomicI32 = AtomicI32::new(-1);

/// Solo despierta el poll escribiendo el numero de senal; la accion va en el
/// bucle. Async-signal-safe: un write y errno preservado.
extern "C" fn on_signal(signum: libc::c_int) {
    let fd = WAKE_FD.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    // SAFETY: errno es thread-local; write es async-signal-safe.
    unsafe {
        let errno = *libc::__errno_location();
        let b = signum as u8;
        libc::write(fd, (&b as *const u8).cast(), 1);
        *libc::__errno_location() = errno;
    }
}

const HANDLED: [libc::c_int; 4] = [libc::SIGWINCH, libc::SIGTERM, libc::SIGHUP, libc::SIGINT];

fn install_handlers(wake_w: RawFd) {
    WAKE_FD.store(wake_w, Ordering::Relaxed);
    for sig in HANDLED {
        // SAFETY: sigaction con un handler async-signal-safe.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

fn uninstall_handlers() {
    WAKE_FD.store(-1, Ordering::Relaxed);
}

// -- terminal -------------------------------------------------------------

/// Pone stdin en raw y lo restaura al soltarse, tambien ante un panic.
struct RawGuard {
    saved: Option<Termios>,
}

impl RawGuard {
    fn new(saved: Option<Termios>) -> Self {
        if let Some(t) = &saved {
            let mut raw = t.clone();
            cfmakeraw(&mut raw);
            let _ = tcsetattr(stdin_fd(), SetArg::TCSAFLUSH, &raw);
        }
        RawGuard { saved }
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        if let Some(t) = self.saved.take() {
            let _ = tcsetattr(stdin_fd(), SetArg::TCSADRAIN, &t);
        }
    }
}

fn stdin_fd() -> BorrowedFd<'static> {
    // SAFETY: el fd 0 vive todo el proceso.
    unsafe { BorrowedFd::borrow_raw(STDIN) }
}

// -- sesion ---------------------------------------------------------------

enum End {
    /// El hijo cerro la pty (o fallo la escritura): esperar su codigo.
    Child,
    /// SIGINT: salir con 130 sin esperar, como el KeyboardInterrupt de Python.
    Interrupted,
}

pub fn run(args: &[OsString]) -> i32 {
    // SAFETY: isatty no tiene precondiciones.
    let stdin_is_tty = unsafe { libc::isatty(STDIN) } == 1;
    let saved = if stdin_is_tty {
        tcgetattr(stdin_fd()).ok()
    } else {
        None
    };

    let clip = clipboard::clipboard_cmds();
    if clip.is_none() {
        eprintln!("sshclip: no encuentro xclip ni xsel; la sesion sigue, sin portapapeles.");
    } else if std::env::var_os("DISPLAY").is_none_or(|d| d.is_empty()) {
        eprintln!("sshclip: DISPLAY vacio; el portapapeles no va a funcionar.");
    }

    // Todo lo que el hijo necesita se prepara antes del fork.
    let ssh = CString::new(SSH).expect("ruta de ssh con NUL");
    let mut argv = vec![ssh.clone()];
    for a in args {
        match CString::new(a.as_bytes()) {
            Ok(c) => argv.push(c),
            Err(_) => {
                eprintln!("sshclip: argumento con byte NUL");
                return 2;
            }
        }
    }

    // El tamano se pasa ya al crear la pty: ssh lo lee al arrancar.
    let ws = stdin_winsize();
    // SAFETY: el proceso es de un solo hilo en este punto; el hijo solo hace
    // execv o _exit.
    let fork = unsafe { forkpty(&ws, None) };
    let (child, master): (Pid, OwnedFd) = match fork {
        Ok(ForkptyResult::Parent { child, master }) => (child, master),
        Ok(ForkptyResult::Child) => {
            let err = nix::unistd::execv(&ssh, &argv).unwrap_err();
            let msg = format!("sshclip: no puedo ejecutar {SSH}: {err}\n");
            let _ = write_all(2, msg.as_bytes());
            // SAFETY: salir del hijo sin correr destructores del padre.
            unsafe { libc::_exit(126) }
        }
        Err(err) => {
            eprintln!("sshclip: no puedo crear la pty: {err}");
            return 1;
        }
    };
    let master_fd = master.as_raw_fd();
    set_cloexec(master_fd); // que xclip/xsel no hereden la pty

    // Tuberia de despertar: hace que poll() salga al recibir una senal.
    let mut fds = [0 as RawFd; 2];
    // SAFETY: pipe2 escribe dos fds en el arreglo.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        eprintln!(
            "sshclip: no puedo crear la tuberia: {}",
            io::Error::last_os_error()
        );
        return 1;
    }
    let (wake_r, wake_w) = (fds[0], fds[1]);
    install_handlers(wake_w);

    copy_winsize(master_fd);

    let end = {
        let _raw = RawGuard::new(saved);
        let end = pump(master_fd, wake_r, child, clip.as_ref());
        uninstall_handlers();
        end
        // _raw restaura la terminal aqui
    };
    drop(master);
    // SAFETY: fds propios, ya sin uso.
    unsafe {
        libc::close(wake_r);
        libc::close(wake_w);
    }

    if let End::Interrupted = end {
        return 130;
    }
    loop {
        match waitpid(child, None) {
            Ok(WaitStatus::Exited(_, code)) => return code,
            Ok(WaitStatus::Signaled(_, sig, _)) => return 128 + sig as i32,
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => return 1,
        }
    }
}

/// El bucle: salida del hijo filtrada a stdout, stdin al hijo, senales.
fn pump(master: RawFd, wake_r: RawFd, child: Pid, clip: Option<&clipboard::Cmds>) -> End {
    let mut filt = Osc52Filter::new();
    let mut buf = vec![0u8; BUF];
    let mut watch_stdin = true;
    let ready = |p: &libc::pollfd| {
        p.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
    };

    loop {
        let mut pfds = [
            libc::pollfd {
                fd: master,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake_r,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: STDIN,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let nfds = if watch_stdin { 3 } else { 2 };
        // SAFETY: pfds tiene al menos nfds entradas validas.
        if unsafe { libc::poll(pfds.as_mut_ptr(), nfds, -1) } < 0 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }

        if ready(&pfds[1]) {
            let mut sigs = [0u8; 1024];
            let n = read_fd(wake_r, &mut sigs).unwrap_or(0);
            for &signum in &sigs[..n] {
                match signum as libc::c_int {
                    libc::SIGWINCH => copy_winsize(master),
                    libc::SIGTERM => {
                        let _ = kill(child, Signal::SIGTERM);
                    }
                    libc::SIGHUP => {
                        let _ = kill(child, Signal::SIGHUP);
                    }
                    libc::SIGINT => return End::Interrupted,
                    _ => {}
                }
            }
        }

        if ready(&pfds[0]) {
            let n = match read_fd(master, &mut buf) {
                Ok(n) => n,
                Err(e) if e.raw_os_error() == Some(libc::EINTR) => continue,
                Err(_) => 0, // EIO = el hijo cerro la pty
            };
            if n == 0 {
                break;
            }
            let (clean, payloads) = filt.feed(&buf[..n]);
            if !clean.is_empty() && write_all(STDOUT, &clean).is_err() {
                break;
            }
            for payload in &payloads {
                clipboard::copy(clip, payload);
            }
        }

        if watch_stdin && ready(&pfds[2]) {
            let n = match read_fd(STDIN, &mut buf) {
                Ok(n) => n,
                Err(e) if e.raw_os_error() == Some(libc::EINTR) => continue,
                Err(_) => 0,
            };
            if n == 0 {
                watch_stdin = false; // EOF local: no cerramos la pty del hijo
            } else if write_all(master, &buf[..n]).is_err() {
                break;
            }
        }
    }

    let rest = filt.flush();
    if !rest.is_empty() {
        let _ = write_all(STDOUT, &rest);
    }
    End::Child
}
