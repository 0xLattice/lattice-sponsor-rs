//! Sponsor orchestration. Locks protect synchronous reservations; RPC calls
//! happen outside locks, with expiry checked again before committing payments.
use crate::{
    node::{NodeError, SponsorNode},
    payment::{CoinPayment, PaymentLedger},
    serving::ServingCredit,
    storage::{StorageError, StorageLedger, StoreStatus, StoreVolume},
    *,
};
use hmac::{Hmac, Mac};
use lattice_primitives::{Block, ChainSpec, TransactionBody, crypto};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Refusal(#[from] Refusal),
    #[error(transparent)]
    Node(#[from] NodeError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Content(#[from] lattice_cashew::Error),
    #[error("the node admits zero-fee transactions; no sponsor needed")]
    ZeroFee,
    #[error("signing failed")]
    Signing,
    #[error("invalid key file: {0}")]
    Key(String),
}
impl ServiceError {
    pub fn status(&self) -> u16 {
        let refusal = match self {
            Self::Refusal(r)
            | Self::Node(NodeError::Refusal(r))
            | Self::Storage(StorageError::Refusal(r)) => Some(r),
            _ => None,
        };
        match refusal {
            Some(
                Refusal::Busy
                | Refusal::Unpriceable
                | Refusal::InsufficientFloat
                | Refusal::SoldOut,
            ) => 503,
            Some(Refusal::InsufficientCredit) => 402,
            Some(Refusal::NotIncluded) => 404,
            Some(Refusal::NotDeepEnough) => 425,
            Some(Refusal::AlreadyCredited) => 409,
            Some(Refusal::ServingNotCredited) => 502,
            Some(_) => 400,
            None if matches!(self, Self::ZeroFee) => 409,
            None => 500,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiningTemplate {
    #[serde(rename = "workID")]
    pub work_id: String,
    pub block: Block,
    pub search_target: UInt256,
    pub expires_in_milliseconds: u64,
    pub chains: Option<Vec<TemplateChain>>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateChain {
    pub chain_path: Vec<String>,
    pub target: UInt256,
    #[serde(with = "decimal")]
    pub height: u64,
    pub reward_recipient: Option<String>,
}
pub mod decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let s = String::deserialize(d)?;
        let n = s.parse::<u64>().map_err(serde::de::Error::custom)?;
        if n.to_string() != s {
            return Err(serde::de::Error::custom("noncanonical decimal"));
        }
        Ok(n)
    }
}
mod optional_decimal {
    use serde::{Serialize, Serializer};
    pub fn serialize<S: Serializer>(v: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        v.map(|n| n.to_string()).serialize(s)
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkResponse {
    pub client: String,
    pub assignment: String,
    pub prefix_hex: String,
    pub share_target: String,
    #[serde(with = "decimal")]
    pub start_nonce: u64,
    #[serde(with = "decimal")]
    pub count: u64,
    #[serde(with = "decimal")]
    pub fee: u64,
    #[serde(with = "decimal")]
    pub pay: u64,
    #[serde(with = "decimal")]
    pub shares_for_fee: u64,
    #[serde(with = "decimal")]
    pub expires_in_milliseconds: u64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareResponse {
    #[serde(with = "decimal")]
    pub credit: u64,
    #[serde(
        serialize_with = "optional_decimal::serialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub serving: Option<u64>,
}
pub struct ShareOutcome {
    pub response: Result<ShareResponse, ServiceError>,
    pub block: Option<String>,
    pub assignment: Option<ShareAssignment>,
}
#[derive(Debug, Clone, Serialize)]
pub struct BoundTransaction {
    pub signatures: BTreeMap<String, String>,
    #[serde(serialize_with = "serialize_body")]
    pub body: TransactionBody,
}
#[derive(Debug, Clone, Serialize)]
pub struct SponsorResponse {
    pub transaction: BoundTransaction,
    #[serde(with = "decimal")]
    pub fee: u64,
    #[serde(with = "decimal")]
    pub pay: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct StoreResponse {
    pub root: String,
    #[serde(with = "decimal")]
    pub bytes: u64,
    pub needed: BTreeMap<String, String>,
    #[serde(with = "decimal")]
    pub credit: u64,
    pub stored: bool,
}
impl From<StoreStatus> for StoreResponse {
    fn from(s: StoreStatus) -> Self {
        Self {
            root: s.root,
            bytes: s.bytes as u64,
            needed: s
                .needed
                .into_iter()
                .map(|(k, v)| (k, v.to_string()))
                .collect(),
            credit: s.credit as u64,
            stored: s.stored,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct PayResponse {
    #[serde(with = "decimal")]
    pub bytes: u64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Terms {
    pub pay_to: Option<String>,
    #[serde(with = "decimal")]
    pub depth: u64,
    pub storage_rates: BTreeMap<String, i64>,
    pub serving_rates: BTreeMap<String, i64>,
}
#[derive(Clone)]
pub struct SponsorKey {
    pub address: String,
    pub public_key: String,
    private_key: String,
}
impl SponsorKey {
    pub fn from_hex(public_key: String, private_key: String) -> Result<Self, ServiceError> {
        let address =
            crypto::create_address(&public_key).map_err(|e| ServiceError::Key(e.to_string()))?;
        let signature = crypto::sign("lattice-sponsor-key-check", &private_key)
            .ok_or_else(|| ServiceError::Key("invalid private key".into()))?;
        if !crypto::verify("lattice-sponsor-key-check", &signature, &public_key) {
            return Err(ServiceError::Key("private/public key mismatch".into()));
        }
        Ok(Self {
            address,
            public_key,
            private_key,
        })
    }
    pub fn from_file(path: &Path) -> Result<Self, ServiceError> {
        let metadata = std::fs::metadata(path).map_err(|e| ServiceError::Key(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(ServiceError::Key("group/other-readable; chmod 600".into()));
            }
        }
        #[cfg(not(unix))]
        let _ = metadata;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Key {
            public_key: String,
            private_key: String,
        }
        let key: Key = serde_json::from_slice(
            &std::fs::read(path).map_err(|e| ServiceError::Key(e.to_string()))?,
        )
        .map_err(|e| ServiceError::Key(e.to_string()))?;
        Self::from_hex(key.public_key, key.private_key)
    }
}
pub struct ServiceConfig {
    pub ledger: SponsorLedger,
    pub key: SponsorKey,
    pub minimum_fee: u64,
    pub most_pay: Option<u64>,
    pub token_key: [u8; 32],
    pub storage: Option<StorageLedger>,
    pub serving: ServingCredit,
    pub pay_to: Option<String>,
    pub coin_depth: u64,
    pub payments: Option<PaymentLedger>,
}
struct State {
    ledger: SponsorLedger,
    specs: BTreeMap<Vec<String>, ChainSpec>,
    fees: BTreeMap<Vec<String>, u64>,
    balances: BTreeMap<Vec<String>, u64>,
    storage: Option<StorageLedger>,
    payments: Option<PaymentLedger>,
}
struct Inner {
    state: Mutex<State>,
    node: SponsorNode,
    key: SponsorKey,
    minimum_fee: u64,
    most_pay: Option<u64>,
    token_key: [u8; 32],
    serving: ServingCredit,
    pay_to: Option<String>,
    coin_depth: u64,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}
#[derive(Clone)]
pub struct SponsorService(Arc<Inner>);
fn whole(v: BigUint) -> u64 {
    if v > BigUint::from(u64::MAX) {
        u64::MAX
    } else {
        v.to_u64_digits().first().copied().unwrap_or(0)
    }
}
impl SponsorService {
    pub fn new(config: ServiceConfig, node: SponsorNode) -> Self {
        Self::with_clock(
            config,
            node,
            Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
                    .unwrap_or(0)
            }),
        )
    }
    pub fn with_clock(
        c: ServiceConfig,
        node: SponsorNode,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State {
                ledger: c.ledger,
                specs: BTreeMap::new(),
                fees: BTreeMap::new(),
                balances: BTreeMap::new(),
                storage: c.storage,
                payments: c.payments,
            }),
            node,
            key: c.key,
            minimum_fee: c.minimum_fee,
            most_pay: c.most_pay,
            token_key: c.token_key,
            serving: c.serving,
            pay_to: c.pay_to,
            coin_depth: c.coin_depth,
            clock,
        }))
    }
    fn now(&self) -> i64 {
        (self.0.clock)()
    }
    pub fn address(&self) -> &str {
        &self.0.key.address
    }
    pub async fn refresh(&self) -> Result<bool, ServiceError> {
        let mut chains = vec!["Nexus".to_owned()];
        chains.extend(self.0.node.hosted_chains().await?);
        let request = serde_json::json!({"recipients":chains.into_iter().map(|p|serde_json::json!({"chainPath":p.split('/').collect::<Vec<_>>(),"address":self.address()})).collect::<Vec<_>>()});
        let at = self.now();
        let Some(template) = self.0.node.template(request).await? else {
            return Ok(false);
        };
        self.install(template, at).await;
        Ok(true)
    }
    pub async fn install(&self, template: MiningTemplate, fetched_at: i64) {
        let mut prices = BTreeMap::new();
        for chain in template
            .chains
            .unwrap_or_default()
            .iter()
            .filter(|c| c.reward_recipient.as_deref() == Some(self.address()))
        {
            let cached = {
                self.0
                    .state
                    .lock()
                    .unwrap()
                    .specs
                    .get(&chain.chain_path)
                    .cloned()
            };
            let spec = match cached {
                Some(s) => s,
                None => {
                    let Ok(s) = self.0.node.spec(&chain.chain_path).await else {
                        continue;
                    };
                    self.0
                        .state
                        .lock()
                        .unwrap()
                        .specs
                        .insert(chain.chain_path.clone(), s.clone());
                    s
                }
            };
            let Ok(floor) = self.0.node.relay_floor(&chain.chain_path).await else {
                continue;
            };
            let Ok(balance) = self.0.node.balance(self.address(), &chain.chain_path).await else {
                continue;
            };
            {
                let mut state = self.0.state.lock().unwrap();
                state
                    .fees
                    .insert(chain.chain_path.clone(), self.0.minimum_fee.max(floor));
                state.balances.insert(chain.chain_path.clone(), balance);
            }
            prices.insert(
                chain.chain_path.clone(),
                ChainPrice {
                    target: chain.target.min(template.search_target),
                    height: chain.height,
                    reward: spec.reward_at_block(chain.height),
                },
            );
        }
        let prefix_hex = hex::encode(template.block.pow_prefix());
        self.0.state.lock().unwrap().ledger.install(
            SponsorTemplate {
                work_id: template.work_id,
                prefix_hex,
                prices,
                submit_target: template.search_target,
                expires_at: fetched_at
                    .saturating_add(template.expires_in_milliseconds.min(i64::MAX as u64) as i64),
            },
            self.now(),
        );
    }
    fn fee(&self, s: &State, path: &[String], now: i64) -> Result<u64, ServiceError> {
        let latest = s
            .ledger
            .latest()
            .filter(|t| now < t.expires_at && t.prices.contains_key(path))
            .ok_or(Refusal::Unpriceable)?;
        let _ = latest;
        let fee = *s.fees.get(path).ok_or(Refusal::Unpriceable)?;
        if fee == 0 {
            return Err(ServiceError::ZeroFee);
        }
        Ok(fee)
    }
    fn charge(&self, s: &State, fee: u64, pay: u64, path: &[String]) -> Result<u64, Refusal> {
        let most = self
            .0
            .most_pay
            .or_else(|| {
                s.ledger
                    .latest()
                    .and_then(|t| t.prices.get(path).map(|p| p.reward))
            })
            .unwrap_or(0);
        if pay > most {
            return Err(Refusal::Draft(format!(
                "this sponsor pays at most {most} within a transaction"
            )));
        }
        fee.checked_add(pay)
            .ok_or_else(|| Refusal::Draft("pay out of range".into()))
    }
    fn issued(&self, token: &str) -> bool {
        if token.len() != 96 {
            return false;
        }
        let Ok(bytes) = hex::decode(token) else {
            return false;
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0.token_key).expect("fixed key");
        mac.update(&bytes[..16]);
        mac.verify_slice(&bytes[16..]).is_ok()
    }
    fn issue(&self) -> String {
        let mut id = [0; 16];
        OsRng.fill_bytes(&mut id);
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0.token_key).expect("fixed key");
        mac.update(&id);
        format!(
            "{}{}",
            hex::encode(id),
            hex::encode(mac.finalize().into_bytes())
        )
    }
    pub fn work(
        &self,
        client: Option<&str>,
        path: &[String],
        pay: u64,
        store: Option<&str>,
        peer: Option<&str>,
    ) -> Result<WorkResponse, ServiceError> {
        let now = self.now();
        let mut s = self.0.state.lock().unwrap();
        ServingCredit::check(peer, store)?;
        let store = store.filter(|_| s.storage.is_some());
        let fee = if peer.is_some() {
            if pay != 0 {
                return Err(Refusal::Draft("serving work pays nothing else".into()).into());
            }
            self.0.serving.rate(path)?;
            0
        } else if let Some(root) = store {
            if pay != 0 {
                return Err(Refusal::Draft("storage work pays nothing else".into()).into());
            }
            s.storage.as_ref().unwrap().check(root, path, now)?;
            0
        } else {
            self.fee(&s, path, now)?
        };
        let total = self.charge(&s, fee, pay, path)?;
        let client = match client {
            Some(token) if self.issued(token) => token.to_owned(),
            Some(_) => return Err(Refusal::Draft("unknown client token".into()).into()),
            None => self.issue(),
        };
        let a = s.ledger.assign(
            client.clone(),
            path.into(),
            store.map(str::to_owned),
            peer.map(str::to_owned),
            now,
        )?;
        let shares = if store.is_none() && peer.is_none() {
            whole(big(s
                .ledger
                .shares_for(total, path)
                .unwrap_or(UInt256([255; 32]))))
        } else {
            0
        };
        Ok(WorkResponse {
            client,
            assignment: a.id,
            prefix_hex: a.prefix_hex,
            share_target: hex::encode(a.share_target.0),
            start_nonce: a.start_nonce,
            count: a.count,
            fee,
            pay,
            shares_for_fee: shares,
            expires_in_milliseconds: (i128::from(a.expires_at) - i128::from(now))
                .clamp(0, u64::MAX as i128) as u64,
        })
    }
    pub fn share(&self, id: &str, nonce: u64) -> ShareOutcome {
        let now = self.now();
        let mut s = self.0.state.lock().unwrap();
        let Some(held) = s.ledger.assignment(id).cloned() else {
            return ShareOutcome {
                response: Err(Refusal::UnknownAssignment.into()),
                block: None,
                assignment: None,
            };
        };
        let Ok(prefix) = hex::decode(&held.prefix_hex) else {
            return ShareOutcome {
                response: Err(Refusal::UnknownAssignment.into()),
                block: None,
                assignment: None,
            };
        };
        let hash = UInt256(lattice_pow::hash(&prefix, nonce));
        let response = (|| {
            if let Some(root) = &held.store {
                s.storage.as_ref().ok_or(Refusal::UnknownUpload)?.check(
                    root,
                    &held.chain_path,
                    now,
                )?;
            }
            let value = s.ledger.credit_hash(id, nonce, hash, now)?;
            if let Some(root) = &held.store {
                s.storage
                    .as_mut()
                    .unwrap()
                    .credit(root, value, &held.chain_path, now)?;
            }
            let credit = whole(big(s.ledger.credit(&held.client, &held.chain_path)) / unit());
            let serving = held
                .peer
                .as_ref()
                .map(|_| {
                    self.0
                        .serving
                        .bytes(value, &held.chain_path)
                        .map(|v| v as u64)
                })
                .transpose()?;
            Ok(ShareResponse { credit, serving })
        })();
        let block = s.ledger.block(id, nonce, hash, now);
        ShareOutcome {
            response,
            block,
            assignment: Some(held),
        }
    }
    pub fn upload(
        &self,
        root: String,
        volumes: Vec<StoreVolume>,
    ) -> Result<StoreResponse, ServiceError> {
        Ok(self
            .0
            .state
            .lock()
            .unwrap()
            .storage
            .as_mut()
            .ok_or(Refusal::UnknownUpload)?
            .upload(root, volumes, self.now())?
            .into())
    }
    pub fn store_status(&self, root: &str) -> Option<StoreResponse> {
        self.0
            .state
            .lock()
            .unwrap()
            .storage
            .as_ref()?
            .status(root)
            .map(Into::into)
    }
    pub fn sponsor(
        &self,
        client: &str,
        draft: &TransactionBody,
        pay: u64,
    ) -> Result<SponsorResponse, ServiceError> {
        let now = self.now();
        let mut s = self.0.state.lock().unwrap();
        let path = &draft.chain_path;
        let fee = self.fee(&s, path, now)?;
        let total = self.charge(&s, fee, pay, path)?;
        let body = sponsored_body(draft, self.address(), fee, pay, path)?;
        let balance = s.balances.get(path).copied().unwrap_or(0);
        let block_time = s
            .specs
            .get(path)
            .map(|s| s.target_block_time)
            .unwrap_or(0)
            .min(i64::MAX as u64) as i64;
        let hold = 600000.max(block_time.wrapping_mul(3));
        s.ledger
            .commit(client, total, path, balance, now.saturating_add(hold), now)?;
        let signature = crypto::sign(&body.signing_preimage()?, &self.0.key.private_key)
            .ok_or(ServiceError::Signing)?;
        Ok(SponsorResponse {
            transaction: BoundTransaction {
                signatures: BTreeMap::from([(self.0.key.public_key.clone(), signature)]),
                body,
            },
            fee,
            pay,
        })
    }
    pub fn terms(&self) -> Terms {
        let s = self.0.state.lock().unwrap();
        Terms {
            pay_to: self.0.pay_to.clone(),
            depth: self.0.coin_depth,
            storage_rates: s
                .storage
                .as_ref()
                .map(|s| s.rates.iter().map(|(p, r)| (p.join("/"), *r)).collect())
                .unwrap_or_default(),
            serving_rates: self
                .0
                .serving
                .rates
                .iter()
                .map(|(p, r)| (p.join("/"), *r))
                .collect(),
        }
    }
}
/// The submission result is diagnostic: a node submission failure does not
/// reverse a credited share, matching the sponsor's route behavior.
pub struct ProcessedShare {
    pub response: Result<ShareResponse, ServiceError>,
    pub submission: Option<Result<serde_json::Value, NodeError>>,
}
struct PinReservation {
    service: SponsorService,
    root: String,
    active: bool,
}
impl Drop for PinReservation {
    fn drop(&mut self) {
        if self.active
            && let Some(storage) = self.service.0.state.lock().unwrap().storage.as_mut()
        {
            let _ = storage.finish(&self.root, false);
        }
    }
}
struct PaymentReservation {
    service: SponsorService,
    cid: String,
    active: bool,
}
impl Drop for PaymentReservation {
    fn drop(&mut self) {
        if self.active
            && let Some(payments) = self.service.0.state.lock().unwrap().payments.as_mut()
        {
            let _ = payments.release(&self.cid, false);
        }
    }
}
impl SponsorService {
    pub async fn process_share(&self, id: &str, nonce: u64) -> ProcessedShare {
        let outcome = self.share(id, nonce);
        let submission = match outcome.block {
            Some(id) => Some(self.0.node.submit(&id, nonce).await),
            None => None,
        };
        if let Some(root) = outcome.assignment.as_ref().and_then(|a| a.store.clone()) {
            let service = self.clone();
            tokio::spawn(async move {
                let _ = service.pin_if_paid(&root).await;
            });
        }
        let response = match outcome.response {
            Ok(answer) => {
                if let Some(peer) = outcome.assignment.as_ref().and_then(|a| a.peer.as_deref()) {
                    match self
                        .0
                        .node
                        .credit_serving(
                            peer,
                            answer.serving.unwrap_or(0).min(i64::MAX as u64) as i64,
                        )
                        .await
                    {
                        Ok(()) => Ok(answer),
                        Err(e) => Err(e.into()),
                    }
                } else {
                    Ok(answer)
                }
            }
            Err(e) => Err(e),
        };
        ProcessedShare {
            response,
            submission,
        }
    }
    pub async fn pin_if_paid(&self, root: &str) -> Result<bool, ServiceError> {
        let upload = {
            self.0
                .state
                .lock()
                .unwrap()
                .storage
                .as_mut()
                .and_then(|s| s.take_paid(root))
        };
        let Some(upload) = upload else {
            return Ok(false);
        };
        let mut reservation = PinReservation {
            service: self.clone(),
            root: root.into(),
            active: true,
        };
        let result = self.0.node.pin(&upload).await;
        let finish = {
            self.0
                .state
                .lock()
                .unwrap()
                .storage
                .as_mut()
                .map(|s| s.finish(root, result.is_ok()))
                .transpose()
        };
        reservation.active = false;
        finish?;
        result?;
        Ok(true)
    }
    fn check_payment(&self, s: &State, payment: &CoinPayment, now: i64) -> Result<i64, Refusal> {
        let path = payment.chain_path();
        if payment.peer.is_some() {
            self.0.serving.rate(&path)
        } else {
            let storage = s.storage.as_ref().ok_or(Refusal::UnknownUpload)?;
            let root = payment.store.as_ref().ok_or(Refusal::UnknownUpload)?;
            storage.check(root, &path, now)?;
            storage.rate(&path)
        }
    }
    pub async fn pay(&self, payment: &CoinPayment) -> Result<PayResponse, ServiceError> {
        let pay_to = self.0.pay_to.as_deref().ok_or(Refusal::NoPayment)?;
        {
            let mut state = self.0.state.lock().unwrap();
            if state.payments.is_none() {
                return Err(Refusal::NoPayment.into());
            }
            self.check_payment(&state, payment, self.now())?;
            state
                .payments
                .as_mut()
                .unwrap()
                .claim(payment.tx_cid.clone())?;
        }
        let mut reservation = PaymentReservation {
            service: self.clone(),
            cid: payment.tx_cid.clone(),
            active: true,
        };
        let coins = self
            .0
            .node
            .verify_payment(payment, pay_to, self.0.coin_depth)
            .await?;
        let path = payment.chain_path();
        let (rate, value, serving_bytes) = {
            let mut s = self.0.state.lock().unwrap();
            let at = self.now();
            let rate = self.check_payment(&s, payment, at)?;
            s.payments
                .as_mut()
                .unwrap()
                .release(&payment.tx_cid, true)?;
            reservation.active = false;
            let value = fixed(BigUint::from(coins) * unit()).expect("u64 fixed point");
            let serving_bytes = if payment.peer.is_some() {
                Some(self.0.serving.bytes(value, &path)?)
            } else {
                let root = payment.store.as_deref().ok_or(Refusal::UnknownUpload)?;
                s.storage.as_mut().unwrap().credit(root, value, &path, at)?;
                None
            };
            (rate, value, serving_bytes)
        };
        let _ = value;
        if let Some(peer) = &payment.peer {
            let bytes = serving_bytes.expect("peer branch");
            self.0.node.credit_serving(peer, bytes).await?;
            return Ok(PayResponse {
                bytes: bytes as u64,
            });
        }
        let root = payment.store.clone().expect("checked store");
        let service = self.clone();
        tokio::spawn(async move {
            let _ = service.pin_if_paid(&root).await;
        });
        Ok(PayResponse {
            bytes: coins.saturating_mul(rate as u64),
        })
    }
}
fn serialize_body<S: serde::Serializer>(b: &TransactionBody, s: S) -> Result<S::Ok, S::Error> {
    use serde_json::json;
    json!({"accountActions":b.account_actions.iter().map(|a|json!({"owner":a.owner,"delta":a.delta.to_string()})).collect::<Vec<_>>(),"actions":b.actions,"depositActions":b.deposit_actions.iter().map(|a|json!({"nonce":a.nonce.to_string(),"demander":a.demander,"amountDemanded":a.amount_demanded.to_string(),"amountDeposited":a.amount_deposited.to_string()})).collect::<Vec<_>>(),"receiptActions":b.receipt_actions.iter().map(|a|json!({"withdrawer":a.withdrawer,"nonce":a.nonce.to_string(),"demander":a.demander,"amountDemanded":a.amount_demanded.to_string(),"directory":a.directory})).collect::<Vec<_>>(),"withdrawalActions":b.withdrawal_actions.iter().map(|a|json!({"withdrawer":a.withdrawer,"nonce":a.nonce.to_string(),"demander":a.demander,"amountDemanded":a.amount_demanded.to_string(),"amountWithdrawn":a.amount_withdrawn.to_string()})).collect::<Vec<_>>(),"signers":b.signers,"nonce":b.nonce.to_string(),"chainPath":b.chain_path}).serialize(s)
}
