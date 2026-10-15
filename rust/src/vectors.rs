//! Runner de `tests/vectors.json`, la especificacion compartida con Python.
//!
//! El formato de los segmentos esta documentado en `tests/parser.py`. El JSON
//! se compila dentro del binario para que `--selftest` pruebe la copia
//! instalada con la especificacion completa.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

use crate::filter::{MAX_B64, Osc52Filter};

const VECTORS: &str = include_str!("../../tests/vectors.json");

fn expand(seg: &Value) -> Vec<u8> {
    match seg {
        // latin-1: cada code point 0-255 es un byte
        Value::String(s) => s
            .chars()
            .map(|c| u8::try_from(u32::from(c)).expect("segmento latin-1 con code point > 0xFF"))
            .collect(),
        Value::Array(items) => items.iter().flat_map(expand).collect(),
        Value::Object(obj) => {
            if let Some(Value::String(s)) = obj.get("utf8") {
                s.as_bytes().to_vec()
            } else if let Some(inner) = obj.get("b64") {
                STANDARD.encode(expand(inner)).into_bytes()
            } else if let Some(inner) = obj.get("repeat") {
                let times = obj["times"].as_u64().expect("repeat sin times") as usize;
                expand(inner).repeat(times)
            } else {
                panic!("segmento desconocido: {seg}")
            }
        }
        _ => panic!("segmento desconocido: {seg}"),
    }
}

fn short(data: &[u8]) -> String {
    if data.len() <= 80 {
        format!("b'{}'", data.escape_ascii())
    } else {
        format!("b'{}...({} bytes)'", data[..80].escape_ascii(), data.len())
    }
}

fn run_case(case: &Value) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut chunks: Vec<Vec<u8>> = case["chunks"]
        .as_array()
        .expect("chunks")
        .iter()
        .map(expand)
        .collect();
    if case.get("split").and_then(Value::as_str) == Some("bytewise") {
        chunks = chunks.concat().into_iter().map(|b| vec![b]).collect();
    }
    let mut f = Osc52Filter::new();
    let mut out = Vec::new();
    let mut pays = Vec::new();
    for c in &chunks {
        let (o, p) = f.feed(c);
        out.extend(o);
        pays.extend(p);
    }
    out.extend(f.flush());
    (out, pays)
}

fn check(ok: &mut bool, name: &str, cond: bool, extra: &str) {
    if !cond {
        *ok = false;
    }
    let tail = if !cond && !extra.is_empty() {
        format!("  <- {extra}")
    } else {
        String::new()
    };
    println!("{}  {name}{tail}", if cond { "ok   " } else { "FALLA" });
}

/// Corre todos los vectores e imprime una linea por caso. `true` si todo pasa.
pub fn run_all(label: &str) -> bool {
    let spec: Value = serde_json::from_str(VECTORS).expect("vectors.json invalido");
    let cases = spec["cases"].as_array().expect("cases");
    let mut ok = true;

    for case in cases {
        let (out, pays) = run_case(case);
        let want_out = expand(&case["out"]);
        let want_pays: Vec<Vec<u8>> = case["payloads"]
            .as_array()
            .expect("payloads")
            .iter()
            .map(expand)
            .collect();
        let extra = if out != want_out {
            format!("out {}, esperaba {}", short(&out), short(&want_out))
        } else if pays != want_pays {
            let got: Vec<String> = pays.iter().map(|p| short(p)).collect();
            let want: Vec<String> = want_pays.iter().map(|p| short(p)).collect();
            format!("payloads {got:?}, esperaba {want:?}")
        } else {
            String::new()
        };
        let name = case["name"].as_str().unwrap_or("?");
        check(&mut ok, name, extra.is_empty(), &extra);
    }

    // Lo unico que no cabe en JSON: el overflow tiene que acotar la memoria,
    // no solo rechazar al final.
    let (overflow, len) = overflow_probe();
    check(
        &mut ok,
        "overflow acota el buffer a MAX_B64",
        overflow && len <= MAX_B64,
        &format!("overflow={overflow} len={len}"),
    );

    println!();
    println!(
        "{label}: {} vectores, {}",
        cases.len(),
        if ok { "todo verde" } else { "hay fallos" }
    );
    ok
}

/// 2 MiB de base64 sin terminador: (overflow, bytes retenidos).
fn overflow_probe() -> (bool, usize) {
    let mut f = Osc52Filter::new();
    let mut chunk = b"\x1b]52;c;".to_vec();
    chunk.resize(chunk.len() + (2 << 20), b'A');
    f.feed(&chunk);
    (f.overflow, f.data.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_pass() {
        assert!(run_all("parser"));
    }

    #[test]
    fn overflow_bounds_memory() {
        let (overflow, len) = overflow_probe();
        assert!(overflow);
        assert!(len <= MAX_B64);
    }
}
