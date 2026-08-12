pub mod arbitrage;
pub mod dex_launch_radar;
pub mod launch_radar;
pub mod liquidation_hunter;
pub mod macro_engine;
pub mod multi_asset;
pub mod news_reactor;
pub mod order_flow;
pub mod pump_exhaustion;
pub mod whale_watch;

use serde_json::Value;
use tokio::sync::mpsc::Sender;

use crate::types::Opportunity;

/// Contrato que todo módulo de dados (arbitragem, whale watch, news reactor,
/// launch radar, etc.) precisa implementar para participar do AurumOS. Um
/// módulo só emite `Opportunity`s no canal — nunca decide sozinho se executa.
#[async_trait::async_trait]
pub trait SignalSource: Send {
    /// Nome do módulo, usado em logs.
    fn name(&self) -> &'static str;

    /// Roda indefinidamente, enviando oportunidades ao orquestrador via `tx`.
    /// Deve retornar apenas se a fonte de dados encerrar (erro fatal, canal
    /// fechado, etc).
    async fn run(&mut self, tx: Sender<Opportunity>) -> anyhow::Result<()>;
}

/// Bybit e Bitget mandam níveis de book como `[["preco", "quantidade"], ...]`
/// em strings — pega só o melhor nível (primeiro do array). Compartilhado
/// entre os módulos que leem book público das duas exchanges.
pub fn best_level(levels: Option<&Value>) -> Option<(f64, f64)> {
    let arr = levels?.as_array()?;
    let first = arr.first()?.as_array()?;
    let price: f64 = first.first()?.as_str()?.parse().ok()?;
    let qty: f64 = first.get(1)?.as_str()?.parse().ok()?;
    Some((price, qty))
}
