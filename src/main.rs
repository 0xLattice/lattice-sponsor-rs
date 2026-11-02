use base64::{Engine, engine::general_purpose::STANDARD};
use clap::Parser;
use lattice_primitives::UInt256;
use lattice_sponsor::{
    ledger::SponsorLedger,
    node::SponsorNode,
    payment::PaymentLedger,
    service::{ServiceConfig, SponsorKey, SponsorService},
    serving::ServingCredit,
    storage::StorageLedger,
};
use num_bigint::BigUint;
use rand_core::{OsRng, RngCore};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
#[derive(Parser)]
#[command(
    name = "lattice-sponsor",
    about = "Pay transaction fees for clients who pay in proof of work"
)]
struct Args {
    #[arg(long)]
    node: String,
    #[arg(long)]
    cookie_file: PathBuf,
    #[arg(long)]
    key: PathBuf,
    #[arg(long, default_value = "2^24")]
    share_work: String,
    #[arg(long, default_value_t = 0)]
    fee: u64,
    #[arg(long)]
    most_pay: Option<u64>,
    #[arg(long)]
    sell_storage: bool,
    #[arg(long,default_value_t=1<<30)]
    storage_budget: i64,
    #[arg(long)]
    storage_rate: Vec<String>,
    #[arg(long, default_value = "lattice-sponsor-storage.json")]
    storage_file: PathBuf,
    #[arg(long)]
    serving_rate: Vec<String>,
    #[arg(long)]
    pay_to: Option<String>,
    #[arg(long, default_value_t = 6)]
    coin_depth: u64,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 8650)]
    port: u16,
}
fn rates(values: Vec<String>) -> Result<BTreeMap<Vec<String>, i64>, String> {
    let mut result = BTreeMap::new();
    for value in values {
        let (path, rate) = value
            .split_once('=')
            .ok_or("rate requires chain-path=positive-bytes")?;
        let rate = rate.parse::<i64>().map_err(|_| "invalid rate")?;
        if rate <= 0 {
            return Err("rate must be positive".into());
        }
        let path = path.split('/').map(str::to_owned).collect();
        if result.insert(path, rate).is_some() {
            return Err("duplicate rate chain".into());
        }
    }
    Ok(result)
}
fn target(text: &str) -> Result<UInt256, String> {
    let work = if let Some(exponent) = text.strip_prefix("2^") {
        let n = exponent
            .parse::<usize>()
            .map_err(|_| "invalid work exponent")?;
        if n > 255 {
            return Err("work exceeds 2^255".into());
        }
        BigUint::from(1u8) << n
    } else {
        BigUint::from(text.parse::<u64>().map_err(|_| "invalid decimal work")?)
    };
    if work == BigUint::default() {
        return Err("work must be positive".into());
    }
    let number = ((BigUint::from(1u8) << 256usize) / work) - 1u8;
    let bytes = number.to_bytes_be();
    let mut result = [0; 32];
    result[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(UInt256(result))
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = Args::parse();
    let key = SponsorKey::from_file(&a.key)?;
    let share_target = target(&a.share_work)?;
    let serving = ServingCredit {
        rates: rates(a.serving_rate)?,
    };
    let storage = if a.sell_storage {
        Some(StorageLedger::open(
            a.storage_budget,
            rates(a.storage_rate)?,
            a.storage_file.clone(),
            256 << 20,
            3600000,
        )?)
    } else {
        None
    };
    let payments = if let Some(address) = &a.pay_to {
        if !a.sell_storage && serving.rates.is_empty() {
            return Err("--pay-to requires storage or serving".into());
        }
        if !lattice_primitives::standard_cid(address) {
            return Err("--pay-to is not an address".into());
        }
        Some(PaymentLedger::open(
            a.storage_file
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("lattice-sponsor-payments.json"),
        )?)
    } else {
        None
    };
    let take_coin = payments.is_some();
    let cookie = a.cookie_file.clone();
    let node = SponsorNode::new(
        &a.node,
        Arc::new(move || {
            std::fs::read_to_string(&cookie)
                .ok()
                .map(|s| format!("Basic {}", STANDARD.encode(s.trim())))
        }),
    )?;
    let mut token_key = [0; 32];
    OsRng.fill_bytes(&mut token_key);
    let service = SponsorService::new(
        ServiceConfig {
            ledger: SponsorLedger::new(share_target, 1 << 32, 100000, 3600000)?,
            key,
            minimum_fee: a.fee,
            most_pay: a.most_pay,
            token_key,
            storage,
            serving,
            pay_to: a.pay_to,
            coin_depth: a.coin_depth,
            payments,
        },
        node,
    );
    let listener = tokio::net::TcpListener::bind((a.host.as_str(), a.port)).await?;
    eprintln!(
        "sponsor {} listening on {}",
        service.address(),
        listener.local_addr()?
    );
    let refresh_service = service.clone();
    let refresh = tokio::spawn(async move {
        loop {
            if let Err(e) = refresh_service.refresh().await {
                eprintln!("template fetch failed: {e}");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
    let result = axum::serve(
        listener,
        lattice_sponsor::http::router(service, a.sell_storage, take_coin),
    )
    .with_graceful_shutdown(shutdown())
    .await;
    refresh.abort();
    let _ = refresh.await;
    result?;
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=signal.recv()=>{}}
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
