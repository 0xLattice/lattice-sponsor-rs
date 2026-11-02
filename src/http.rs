//! Bounded public sponsor routes. Decimal fields use the pinned JSON wire spelling.
use crate::{payment::CoinPayment, service::*, storage::StoreVolume};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use lattice_primitives::*;
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::cors::{Any, CorsLayer};
#[derive(Debug)]
pub struct HttpError(pub StatusCode, pub String);
impl From<ServiceError> for HttpError {
    fn from(e: ServiceError) -> Self {
        Self(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.to_string(),
        )
    }
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
fn bad() -> HttpError {
    HttpError(
        StatusCode::BAD_REQUEST,
        "invalid canonical transaction body".into(),
    )
}
fn number<T: std::str::FromStr + ToString>(v: &Value) -> Result<T, HttpError> {
    let s = v.as_str().ok_or_else(bad)?;
    let n: T = s.parse().map_err(|_| bad())?;
    if n.to_string() != s {
        return Err(bad());
    }
    Ok(n)
}
fn text(v: &Value) -> Result<String, HttpError> {
    v.as_str().map(str::to_owned).ok_or_else(bad)
}
fn array(v: &Value) -> Result<&Vec<Value>, HttpError> {
    v.as_array().ok_or_else(bad)
}
/// Decode the concrete ContentBoundTransaction body without passing large
/// integer strings through a lossy JSON number representation.
pub fn decode_body(v: &Value) -> Result<TransactionBody, HttpError> {
    Ok(TransactionBody {
        account_actions: array(&v["accountActions"])?
            .iter()
            .map(|a| {
                Ok(AccountAction {
                    owner: text(&a["owner"])?,
                    delta: number(&a["delta"])?,
                })
            })
            .collect::<Result<_, HttpError>>()?,
        actions: serde_json::from_value(v["actions"].clone()).map_err(|_| bad())?,
        deposit_actions: array(&v["depositActions"])?
            .iter()
            .map(|a| {
                Ok(DepositAction {
                    nonce: number(&a["nonce"])?,
                    demander: text(&a["demander"])?,
                    amount_demanded: number(&a["amountDemanded"])?,
                    amount_deposited: number(&a["amountDeposited"])?,
                })
            })
            .collect::<Result<_, HttpError>>()?,
        receipt_actions: array(&v["receiptActions"])?
            .iter()
            .map(|a| {
                Ok(ReceiptAction {
                    withdrawer: text(&a["withdrawer"])?,
                    nonce: number(&a["nonce"])?,
                    demander: text(&a["demander"])?,
                    amount_demanded: number(&a["amountDemanded"])?,
                    directory: text(&a["directory"])?,
                })
            })
            .collect::<Result<_, HttpError>>()?,
        withdrawal_actions: array(&v["withdrawalActions"])?
            .iter()
            .map(|a| {
                Ok(WithdrawalAction {
                    withdrawer: text(&a["withdrawer"])?,
                    nonce: number(&a["nonce"])?,
                    demander: text(&a["demander"])?,
                    amount_demanded: number(&a["amountDemanded"])?,
                    amount_withdrawn: number(&a["amountWithdrawn"])?,
                })
            })
            .collect::<Result<_, HttpError>>()?,
        signers: serde_json::from_value(v["signers"].clone()).map_err(|_| bad())?,
        nonce: number(&v["nonce"])?,
        chain_path: serde_json::from_value(v["chainPath"].clone()).map_err(|_| bad())?,
    })
}
#[derive(Deserialize)]
struct WorkInput {
    client: Option<String>,
    chain: Option<String>,
    pay: Option<String>,
    store: Option<String>,
    peer: Option<String>,
}
fn pay(v: Option<String>) -> Result<u64, HttpError> {
    v.map(|s| number(&Value::String(s)))
        .transpose()
        .map(|n| n.unwrap_or(0))
}
async fn work(
    State(s): State<SponsorService>,
    Json(i): Json<WorkInput>,
) -> Result<Json<WorkResponse>, HttpError> {
    let path = i
        .chain
        .unwrap_or_else(|| "Nexus".into())
        .split('/')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Ok(Json(s.work(
        i.client.as_deref(),
        &path,
        pay(i.pay)?,
        i.store.as_deref(),
        i.peer.as_deref(),
    )?))
}
#[derive(Deserialize)]
struct ShareInput {
    assignment: String,
    #[serde(with = "decimal")]
    nonce: u64,
}
async fn shares(
    State(s): State<SponsorService>,
    Json(i): Json<ShareInput>,
) -> Result<Json<ShareResponse>, HttpError> {
    let out = s.process_share(&i.assignment, i.nonce).await;
    if let Some(Err(e)) = out.submission {
        eprintln!("block submission failed: {e}");
    }
    Ok(Json(out.response?))
}
#[derive(Deserialize)]
struct SponsorInput {
    client: String,
    transaction: Value,
    pay: Option<String>,
}
async fn sponsor(
    State(s): State<SponsorService>,
    Json(i): Json<SponsorInput>,
) -> Result<Json<SponsorResponse>, HttpError> {
    let _: std::collections::BTreeMap<String, String> =
        serde_json::from_value(i.transaction["signatures"].clone()).map_err(|_| bad())?;
    let body = decode_body(&i.transaction["body"])?;
    Ok(Json(s.sponsor(&i.client, &body, pay(i.pay)?)?))
}
#[derive(Deserialize)]
struct StoreInput {
    root: String,
    volumes: Vec<StoreVolume>,
}
async fn store(
    State(s): State<SponsorService>,
    Json(i): Json<StoreInput>,
) -> Result<Json<StoreResponse>, HttpError> {
    Ok(Json(s.upload(i.root, i.volumes)?))
}
async fn status(
    State(s): State<SponsorService>,
    Path(root): Path<String>,
) -> Result<Json<StoreResponse>, HttpError> {
    if let Err(e) = s.pin_if_paid(&root).await {
        eprintln!("pinning {root} failed: {e}");
    }
    Ok(Json(s.store_status(&root).ok_or_else(|| {
        HttpError(StatusCode::NOT_FOUND, "unknownUpload".into())
    })?))
}
async fn terms(State(s): State<SponsorService>) -> Json<Terms> {
    Json(s.terms())
}
async fn payment(
    State(s): State<SponsorService>,
    Json(p): Json<CoinPayment>,
) -> Result<Json<PayResponse>, HttpError> {
    Ok(Json(s.pay(&p).await?))
}
/// Optional routes are absent unless their capabilities are configured.
pub fn router(service: SponsorService, sell_storage: bool, take_coin: bool) -> Router {
    let mut router = Router::new()
        .route("/work", post(work))
        .route("/shares", post(shares))
        .route("/sponsor", post(sponsor))
        .route("/terms", get(terms))
        .layer(DefaultBodyLimit::max(1 << 20));
    if sell_storage {
        router = router
            .route("/store", post(store).layer(DefaultBodyLimit::max(64 << 20)))
            .route("/store/{root}", get(status));
    }
    if take_coin {
        router = router.route("/pay", post(payment));
    }
    router.with_state(service).layer(
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers(Any),
    )
}
