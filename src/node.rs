//! Bounded sponsor-to-node RPC. Chain inclusion is trusted to the configured
//! node, as in Swift; claim signatures bind the transfer to one destination.
use crate::{Refusal, payment::CoinPayment, serving::ServingCredit, storage::Upload};
use base64::{Engine, engine::general_purpose::STANDARD};
use lattice_primitives::crypto;
use reqwest::{Client, Method, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error(transparent)]
    Refusal(#[from] Refusal),
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    #[error("node HTTP status {0}")]
    Status(u16),
    #[error("node response exceeds byte limit")]
    ResponseLimit,
    #[error("invalid node response: {0}")]
    Decode(String),
    #[error("invalid node URL")]
    Url,
}
#[derive(Clone)]
pub struct SponsorNode {
    base: Url,
    client: Client,
    authorization: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub response_limit: usize,
    pub timeout: Duration,
}
impl SponsorNode {
    pub fn new(
        base: &str,
        authorization: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    ) -> Result<Self, NodeError> {
        let mut base = Url::parse(base).map_err(|_| NodeError::Url)?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(NodeError::Url);
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            base,
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            authorization,
            response_limit: 16 << 20,
            timeout: Duration::from_secs(10),
        })
    }
    fn url(&self, segments: &[&str], chain: Option<&[String]>) -> Result<Url, NodeError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| NodeError::Url)?
            .pop_if_empty()
            .extend(segments);
        if let Some(chain) = chain.filter(|c| *c != ["Nexus".to_owned()]) {
            url.query_pairs_mut()
                .append_pair("chainPath", &chain.join("/"));
        }
        Ok(url)
    }
    async fn call_response(
        &self,
        method: Method,
        segments: &[&str],
        chain: Option<&[String]>,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<(u16, Vec<u8>), NodeError> {
        let mut request = self
            .client
            .request(method, self.url(segments, chain)?)
            .timeout(timeout)
            .header("cache-control", "no-cache");
        if let Some(auth) = (self.authorization)() {
            request = request.header("authorization", auth);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await?;
        if !response.status().is_success() {
            return Err(NodeError::Status(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.response_limit as u64)
        {
            return Err(NodeError::ResponseLimit);
        }
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > self.response_limit.saturating_sub(bytes.len()) {
                return Err(NodeError::ResponseLimit);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, bytes))
    }
    async fn call(
        &self,
        method: Method,
        segments: &[&str],
        chain: Option<&[String]>,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<Vec<u8>, NodeError> {
        Ok(self
            .call_response(method, segments, chain, body, timeout)
            .await?
            .1)
    }
    async fn rpc_call(
        &self,
        method: Method,
        segments: &[&str],
        chain: Option<&[String]>,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<Vec<u8>, NodeError> {
        let (status, bytes) = self
            .call_response(method, segments, chain, body, timeout)
            .await?;
        if status != 200 {
            return Err(NodeError::Status(status));
        }
        Ok(bytes)
    }
    async fn read_rpc<T: serde::de::DeserializeOwned>(
        &self,
        segments: &[&str],
        chain: &[String],
    ) -> Result<T, NodeError> {
        let bytes = match self
            .rpc_call(Method::GET, segments, Some(chain), None, self.timeout)
            .await
        {
            Err(NodeError::Status(404)) => return Err(Refusal::NotIncluded.into()),
            result => result?,
        };
        serde_json::from_slice(&bytes).map_err(|e| NodeError::Decode(e.to_string()))
    }
    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        segments: &[&str],
        chain: &[String],
    ) -> Result<T, NodeError> {
        let bytes = match self
            .call(Method::GET, segments, Some(chain), None, self.timeout)
            .await
        {
            Err(NodeError::Status(404)) => return Err(Refusal::NotIncluded.into()),
            result => result?,
        };
        serde_json::from_slice(&bytes).map_err(|e| NodeError::Decode(e.to_string()))
    }
    pub async fn hosted_chains(&self) -> Result<Vec<String>, NodeError> {
        #[derive(Deserialize)]
        struct Hosted {
            chains: Vec<String>,
        }
        Ok(self
            .read_rpc::<Hosted>(&["hosted-chains"], &["Nexus".into()])
            .await?
            .chains)
    }
    pub async fn template<T: serde::de::DeserializeOwned>(
        &self,
        body: Value,
    ) -> Result<Option<T>, NodeError> {
        let bytes = match self
            .rpc_call(
                Method::POST,
                &["mining", "templates"],
                None,
                Some(body),
                self.timeout,
            )
            .await
        {
            Err(NodeError::Status(409 | 503)) => return Ok(None),
            other => other?,
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| NodeError::Decode(e.to_string()))
    }
    pub async fn spec(&self, path: &[String]) -> Result<lattice_primitives::ChainSpec, NodeError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Spec {
            target_block_time: Unsigned,
            initial_reward: Unsigned,
            halving_interval: Unsigned,
            max_block_size: i64,
            max_number_of_transactions_per_block: Unsigned,
            premine: Unsigned,
            half_life: Unsigned,
        }
        let spec: Spec = self.read_rpc(&["api", "chain", "spec"], path).await?;
        Ok(lattice_primitives::ChainSpec {
            max_number_of_transactions_per_block: spec.max_number_of_transactions_per_block.0,
            max_state_growth: 0,
            max_block_size: spec.max_block_size,
            premine: spec.premine.0,
            target_block_time: spec.target_block_time.0,
            initial_reward: spec.initial_reward.0,
            halving_interval: spec.halving_interval.0,
            half_life: spec.half_life.0,
            wasm_policies: vec![],
        })
    }
    pub async fn relay_floor(&self, path: &[String]) -> Result<u64, NodeError> {
        #[derive(Deserialize)]
        struct Fees {
            #[serde(rename = "minRelayFee")]
            floor: Unsigned,
            #[serde(rename = "blocks")]
            _blocks: Vec<FeeBlock>,
            #[serde(rename = "weight")]
            _weight: Option<FeeWeight>,
        }
        Ok(self.read_rpc::<Fees>(&["api", "fees"], path).await?.floor.0)
    }
    pub async fn balance(&self, address: &str, path: &[String]) -> Result<u64, NodeError> {
        #[derive(Deserialize)]
        struct Account {
            balance: Unsigned,
            #[serde(rename = "nonce")]
            _nonce: Unsigned,
            #[serde(rename = "owner")]
            _owner: String,
        }
        match self
            .read_rpc::<Account>(&["api", "state", "account", address], path)
            .await
        {
            Ok(account) => Ok(account.balance.0),
            Err(NodeError::Refusal(Refusal::NotIncluded)) => Ok(0),
            Err(e) => Err(e),
        }
    }
    pub async fn submit(&self, work_id: &str, nonce: u64) -> Result<Value, NodeError> {
        let bytes = self
            .call(
                Method::POST,
                &["mining", "work"],
                None,
                Some(json!({"workID":work_id,"nonce":nonce})),
                self.timeout,
            )
            .await?;
        serde_json::from_slice(&bytes).map_err(|e| NodeError::Decode(e.to_string()))
    }
    pub async fn credit_serving(&self, peer: &str, bytes: i64) -> Result<(), NodeError> {
        if bytes <= 0 {
            return Ok(());
        }
        self.call(
            Method::POST,
            &["serving-credit"],
            None,
            Some(json!({"peer":peer,"bytes":bytes})),
            self.timeout,
        )
        .await
        .map_err(|_| NodeError::Refusal(Refusal::ServingNotCredited))?;
        Ok(())
    }
    pub async fn pin(&self, upload: &Upload) -> Result<(), NodeError> {
        #[derive(Deserialize)]
        struct Pin {
            #[serde(rename = "cid")]
            _cid: String,
            entries: Option<BTreeMap<String, String>>,
        }
        for volume in &upload.volumes {
            let entries = if let Some(entries) = &volume.entries {
                entries.clone()
            } else {
                let raw = self
                    .call(
                        Method::GET,
                        &["fetch", &volume.cid],
                        None,
                        None,
                        Duration::from_secs(60),
                    )
                    .await?;
                let fetched: Pin =
                    serde_json::from_slice(&raw).map_err(|e| NodeError::Decode(e.to_string()))?;
                let entries = fetched.entries.unwrap_or_default();
                let size = entries
                    .values()
                    .try_fold(0usize, |sum, v| {
                        sum.checked_add(STANDARD.decode(v).map(|b| b.len()).unwrap_or(0))
                    })
                    .ok_or(Refusal::OverBudget)?;
                if i128::try_from(size).unwrap_or(i128::MAX) > i128::from(volume.bytes.unwrap_or(0))
                {
                    return Err(Refusal::OverBudget.into());
                }
                entries
            };
            self.call(
                Method::POST,
                &["pins"],
                None,
                Some(json!({"cid":volume.cid,"entries":entries})),
                Duration::from_secs(60),
            )
            .await?;
        }
        Ok(())
    }
    pub async fn verify_payment(
        &self,
        payment: &CoinPayment,
        pay_to: &str,
        depth: u64,
    ) -> Result<u64, NodeError> {
        ServingCredit::check(payment.peer.as_deref(), payment.store.as_deref())?;
        if payment.peer.is_none() == payment.store.is_none() {
            return Err(Refusal::Draft("pay names a store or a peer".into()).into());
        }
        let cid = &payment.tx_cid;
        if cid.is_empty() || cid.len() > 128 || !cid.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Refusal::Draft("txCID is not a CID".into()).into());
        }
        let path = payment.chain_path();
        let described: Projection = self.read(&["api", "transaction", cid], &path).await?;
        let (height, hash) = described
            .block_height
            .zip(described.block_hash)
            .ok_or(Refusal::NotIncluded)?;
        let tip: Tip = self.read(&["api", "block", "latest"], &path).await?;
        let block: BlockHash = self
            .read(&["api", "block", &height.0.to_string()], &path)
            .await?;
        if block.hash != hash || tip.height.0 < height.0 {
            return Err(Refusal::NotIncluded.into());
        }
        if tip.height.0 - height.0 < depth {
            return Err(Refusal::NotDeepEnough.into());
        }
        let raw: Raw = self.read(&["transactions", cid], &path).await?;
        if !described.signers.contains(&payment.signer) {
            return Err(Refusal::NotSigner.into());
        }
        let key = raw
            .transaction
            .signatures
            .keys()
            .find(|key| crypto::create_address(key).is_ok_and(|address| address == payment.signer))
            .ok_or(Refusal::NotSigner)?;
        if !crypto::verify(&payment.message(), &payment.signature, key) {
            return Err(Refusal::InvalidSignature.into());
        }
        let amount = described
            .account_actions
            .iter()
            .filter(|a| a.owner == pay_to)
            .try_fold(0i64, |sum, a| sum.checked_add(a.delta.0))
            .filter(|n| *n > 0)
            .ok_or(Refusal::NoPayment)?;
        Ok(amount as u64)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Projection {
    #[serde(rename = "txCID")]
    _cid: String,
    #[serde(rename = "timestamp")]
    _timestamp: Option<Signed>,
    #[serde(rename = "nonce")]
    _nonce: Unsigned,
    #[serde(rename = "chainPath")]
    _chain_path: Vec<String>,
    #[serde(rename = "chain")]
    _chain: Vec<String>,
    #[serde(rename = "depositActions")]
    _deposits: Vec<Deposit>,
    #[serde(rename = "receiptActions")]
    _receipts: Vec<Receipt>,
    #[serde(rename = "withdrawalActions")]
    _withdrawals: Vec<Withdrawal>,
    block_height: Option<Unsigned>,
    block_hash: Option<String>,
    signers: Vec<String>,
    account_actions: Vec<Account>,
}
#[derive(Deserialize)]
struct Account {
    owner: String,
    delta: Signed,
}
#[derive(Deserialize)]
struct Tip {
    #[serde(rename = "hash")]
    _hash: String,
    #[serde(rename = "transactionCount")]
    _count: i64,
    #[serde(rename = "timestamp")]
    _timestamp: Signed,
    #[serde(rename = "previousBlock")]
    _previous: Option<String>,
    #[serde(rename = "rewardRecipient")]
    _recipient: Option<String>,
    #[serde(rename = "rewardCredited")]
    _reward: Option<Unsigned>,
    height: Unsigned,
}
#[derive(Deserialize)]
struct BlockHash {
    hash: String,
}
#[derive(Deserialize)]
struct Raw {
    transaction: lattice_primitives::Transaction,
}
#[derive(Deserialize)]
struct Deposit {
    #[serde(rename = "nonce")]
    _nonce: String,
    #[serde(rename = "demander")]
    _demander: String,
    #[serde(rename = "amountDemanded")]
    _demanded: Unsigned,
    #[serde(rename = "amountDeposited")]
    _deposited: Unsigned,
}
#[derive(Deserialize)]
struct Receipt {
    #[serde(rename = "nonce")]
    _nonce: String,
    #[serde(rename = "demander")]
    _demander: String,
    #[serde(rename = "withdrawer")]
    _withdrawer: String,
    #[serde(rename = "amountDemanded")]
    _demanded: Unsigned,
    #[serde(rename = "directory")]
    _directory: String,
}
#[derive(Deserialize)]
struct Withdrawal {
    #[serde(rename = "nonce")]
    _nonce: String,
    #[serde(rename = "demander")]
    _demander: String,
    #[serde(rename = "withdrawer")]
    _withdrawer: String,
    #[serde(rename = "amountDemanded")]
    _demanded: Unsigned,
    #[serde(rename = "amountWithdrawn")]
    _withdrawn: Unsigned,
}
struct Unsigned(u64);
struct Signed(i64);
impl<'de> Deserialize<'de> for Unsigned {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let v = s.parse::<u64>().map_err(serde::de::Error::custom)?;
        if v.to_string() != s {
            return Err(serde::de::Error::custom("noncanonical decimal"));
        }
        Ok(Self(v))
    }
}
impl<'de> Deserialize<'de> for Signed {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let v = s.parse::<i64>().map_err(serde::de::Error::custom)?;
        if v.to_string() != s {
            return Err(serde::de::Error::custom("noncanonical decimal"));
        }
        Ok(Self(v))
    }
}

#[derive(Deserialize)]
struct FeeBlock {
    #[serde(rename = "transactionCount")]
    _count: i64,
    #[serde(rename = "bytes")]
    _bytes: i64,
    #[serde(rename = "lowestFee")]
    _fee: Unsigned,
    #[serde(rename = "lowestFeeBytes")]
    _fee_bytes: i64,
    #[serde(rename = "lowestFeeWeight")]
    _fee_weight: Option<i64>,
    #[serde(rename = "full")]
    _full: bool,
}
#[derive(Deserialize)]
struct FeeWeight {
    #[serde(rename = "blockRoom")]
    _block: i64,
    #[serde(rename = "growthRoom")]
    _growth: i64,
    #[serde(rename = "maxTransactions")]
    _max: i64,
    #[serde(rename = "transactionAllowance")]
    _allowance: i64,
}
