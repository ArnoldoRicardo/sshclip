//! sshclip — wrapper de ssh que traduce OSC 52 al portapapeles X11 local.
//!
//! Port en Rust del script de Python del repo. Corre ssh dentro de una PTY,
//! intercepta las secuencias OSC 52 de escritura, las quita del flujo antes de
//! pintarlo y manda el contenido decodificado a CLIPBOARD con xclip/xsel.
//!
//! Uso: igual que ssh.
//!     sshclip cyxpc-b
//!     sshclip -p 2222 ar@192.168.100.35
//!
//! Solo remoto -> local: las consultas OSC 52 (lectura del portapapeles) se
//! descartan sin responder.

mod clipboard;
mod filter;
mod session;
mod vectors;

use std::ffi::OsString;
use std::path::Path;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let code = if args.first().is_some_and(|a| a == "--selftest") {
        if vectors::run_all("selftest") { 0 } else { 1 }
    } else if !Path::new(session::SSH).exists() {
        eprintln!("sshclip: no existe {}", session::SSH);
        127
    } else {
        session::run(&args)
    };
    std::process::exit(code);
}
