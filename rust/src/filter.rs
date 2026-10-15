//! Maquina de estados incremental que separa OSC 52 del resto de la salida.
//!
//! Traduccion directa de `Osc52Filter` en el script de Python; la
//! especificacion compartida son los vectores de `tests/vectors.json`.

use base64::Engine;
use base64::alphabet;
use base64::engine::{GeneralPurpose, GeneralPurposeConfig};

use crate::clipboard::dbg;

pub const MAX_PAYLOAD: usize = 1 << 20; // 1 MiB de bytes decodificados
pub const MAX_B64: usize = (MAX_PAYLOAD / 3 + 2) * 4; // el base64 correspondiente, con margen
const MAX_PARAM: usize = 8; // digitos de parametro OSC antes de descartar la hipotesis "52"
const MAX_TARGET: usize = 16; // longitud del campo Pc de OSC 52

const BEL: u8 = 0x07;
const ESC: u8 = 0x1b;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const ST8: u8 = 0x9c; // ST de 8 bits

const VALID_TARGETS: &[u8] = b"cpqs01234567";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Text,
    Esc,
    Param,
    OscPass,
    Target,
    Data,
}

/// `feed(chunk) -> (bytes_a_pintar, payloads_para_el_portapapeles)`
///
/// Todo lo que no sea un OSC 52 de escritura pasa intacto, incluidas las
/// secuencias partidas entre lecturas: lo indeciso se retiene en `held`
/// hasta poder resolverlo.
pub struct Osc52Filter {
    state: State,
    held: Vec<u8>, // bytes retenidos mientras no sabemos si son OSC 52
    param: Vec<u8>,
    target: Vec<u8>,
    pub(crate) data: Vec<u8>,
    pub(crate) overflow: bool,
    esc_pending: bool, // dentro de OSC: vimos ESC, esperamos '\'
}

impl Default for Osc52Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Osc52Filter {
    pub fn new() -> Self {
        Osc52Filter {
            state: State::Text,
            held: Vec::new(),
            param: Vec::new(),
            target: Vec::new(),
            data: Vec::new(),
            overflow: false,
            esc_pending: false,
        }
    }

    // -- helpers ---------------------------------------------------------

    fn reset(&mut self) {
        self.state = State::Text;
        self.held.clear();
        self.param.clear();
        self.target.clear();
        self.data.clear();
        self.overflow = false;
        self.esc_pending = false;
    }

    /// Cierra un OSC 52 capturado y, si es valido, encola el payload.
    fn finish_52(&mut self, payloads: &mut Vec<Vec<u8>>) {
        let target = std::mem::take(&mut self.target);
        let data = std::mem::take(&mut self.data);
        let overflow = self.overflow;
        self.reset();

        if overflow {
            dbg("rechazo: overflow (>1 MiB)");
            return; // mas de 1 MiB: se descarta entero
        }
        if data == b"?" {
            dbg("rechazo: consulta de lectura");
            return; // consulta de lectura: nunca se responde
        }
        if !target.iter().all(|b| VALID_TARGETS.contains(b)) {
            let shown = &target[..target.len().min(32)];
            dbg(&format!("rechazo: Pc invalido b'{}'", shown.escape_ascii()));
            return;
        }
        let Some(payload) = b64_decode(&data) else {
            dbg(&format!(
                "rechazo: base64 invalido ({} bytes de b64)",
                data.len()
            ));
            return;
        };
        if payload.len() > MAX_PAYLOAD {
            dbg(&format!("rechazo: payload de {} bytes", payload.len()));
            return;
        }
        payloads.push(payload);
    }

    // -- parser ----------------------------------------------------------

    pub fn feed(&mut self, chunk: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut out = Vec::with_capacity(chunk.len());
        let mut payloads = Vec::new();
        let mut i = 0;
        let n = chunk.len();

        while i < n {
            let b = chunk[i];

            match self.state {
                State::Text => match chunk[i..].iter().position(|&c| c == ESC) {
                    None => {
                        out.extend_from_slice(&chunk[i..]);
                        i = n;
                    }
                    Some(off) => {
                        out.extend_from_slice(&chunk[i..i + off]);
                        self.held.push(ESC);
                        self.state = State::Esc;
                        i += off + 1;
                    }
                },

                State::Esc => {
                    if b == b']' {
                        self.held.push(b']');
                        self.param.clear();
                        self.state = State::Param;
                    } else if b == ESC {
                        out.append(&mut self.held); // ESC ESC: el primero es literal
                        self.held.push(ESC);
                    } else {
                        out.append(&mut self.held);
                        out.push(b);
                        self.state = State::Text;
                    }
                    i += 1;
                }

                State::Param => {
                    if b == b';' {
                        if self.param == b"52" {
                            self.held.clear(); // a partir de aqui se traga todo
                            self.target.clear();
                            self.data.clear();
                            self.overflow = false;
                            self.esc_pending = false;
                            self.state = State::Target;
                        } else {
                            out.append(&mut self.held);
                            out.push(b);
                            self.state = State::OscPass;
                        }
                        i += 1;
                    } else if b.is_ascii_digit() && self.param.len() < MAX_PARAM {
                        self.param.push(b);
                        self.held.push(b);
                        i += 1;
                    } else {
                        // No es un OSC 52: soltamos lo retenido y reprocesamos el byte
                        out.append(&mut self.held);
                        self.state = State::OscPass;
                    }
                }

                State::OscPass => {
                    // OSC ajeno: se pinta tal cual, solo rastreamos donde termina
                    out.push(b);
                    i += 1;
                    if self.esc_pending {
                        self.esc_pending = b == ESC;
                        if !self.esc_pending {
                            self.state = State::Text;
                        }
                    } else if b == BEL || b == ST8 {
                        self.state = State::Text;
                    } else if b == ESC {
                        self.esc_pending = true;
                    } else if b == CAN || b == SUB {
                        self.state = State::Text;
                    }
                }

                State::Target => {
                    i += 1;
                    if self.esc_pending {
                        // ESC '\' cierra un OSC 52 sin datos; ESC otra-cosa lo aborta.
                        // En los dos casos se descarta, y el byte se traga.
                        self.reset();
                    } else if b == b';' {
                        self.state = State::Data;
                    } else if b == ESC {
                        self.esc_pending = true;
                    } else if matches!(b, BEL | ST8 | CAN | SUB) {
                        self.reset();
                    } else if self.target.len() < MAX_TARGET {
                        self.target.push(b);
                    } else {
                        self.reset(); // Pc absurdamente largo: malformado
                    }
                }

                State::Data => {
                    i += 1;
                    if self.esc_pending {
                        self.esc_pending = false;
                        if b == b'\\' {
                            self.finish_52(&mut payloads);
                        } else {
                            self.reset(); // ESC dentro del payload: aborta
                        }
                    } else if b == BEL || b == ST8 {
                        self.finish_52(&mut payloads);
                    } else if b == ESC {
                        self.esc_pending = true;
                    } else if b == CAN || b == SUB {
                        self.reset();
                    } else if self.data.len() < MAX_B64 {
                        self.data.push(b);
                    } else {
                        self.overflow = true; // seguimos consumiendo hasta el final
                    }
                }
            }
        }

        (out, payloads)
    }

    /// Bytes retenidos al cerrar el flujo (secuencia incompleta).
    pub fn flush(&mut self) -> Vec<u8> {
        let held = if matches!(self.state, State::Esc | State::Param) {
            std::mem::take(&mut self.held)
        } else {
            Vec::new()
        };
        self.reset();
        held
    }
}

/// Base64 estricto: alfabeto validado, relleno tolerado si falta.
///
/// Replica `base64.b64decode(..., validate=True)` de Python: acepta bits
/// sobrantes distintos de cero en el ultimo simbolo (`aGl=` -> `hi`), cosa que
/// el engine `STANDARD` del crate rechaza.
pub fn b64_decode(data: &[u8]) -> Option<Vec<u8>> {
    const ENGINE: GeneralPurpose = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
    );
    if data.is_empty() {
        return Some(Vec::new());
    }
    if let Ok(v) = ENGINE.decode(data) {
        return Some(v);
    }
    let pad = (4 - data.len() % 4) % 4;
    if pad == 0 {
        return None;
    }
    let mut padded = data.to_vec();
    padded.resize(data.len() + pad, b'=');
    ENGINE.decode(&padded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_decode_matches_python() {
        assert_eq!(b64_decode(b""), Some(vec![]));
        assert_eq!(b64_decode(b"aGk=").as_deref(), Some(&b"hi"[..]));
        assert_eq!(b64_decode(b"aGl=").as_deref(), Some(&b"hi"[..]));
        assert_eq!(b64_decode(b"aGk").as_deref(), Some(&b"hi"[..]));
        assert_eq!(b64_decode(b"aGk=="), None);
        assert_eq!(b64_decode(b"a"), None);
        assert_eq!(b64_decode(b"!!!!"), None);
    }
}
