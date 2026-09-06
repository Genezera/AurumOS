use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// Log bruto de estado continuo por modulo (spread, funding rate, book) —
/// diferente do `EventBus`, que so persiste o que ja cruzou o limiar de
/// decisao. Sem isso, recalibrar um limiar (como o achado de 0 sinais do
/// Pump Exhaustion em 60 dias, ver backtests/) exige buscar dado historico
/// de novo em vez de reusar o que o proprio sistema ja observou ao vivo.
/// Mesma escrita atomica de uma chamada so usada em `events.rs` (ver o bug
/// de linhas coladas corrigido la) pra nao corromper o arquivo quando
/// varias tasks gravam ao mesmo tempo.
pub fn append(path: &str, value: serde_json::Value) {
    let Ok(mut line) = serde_json::to_string(&value) else {
        return;
    };
    line.push('\n');
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
