use crate::*;
use lattice_primitives::{AccountAction, TransactionBody};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainPrice {
    pub target: UInt256,
    pub height: u64,
    pub reward: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorTemplate {
    pub work_id: String,
    pub prefix_hex: String,
    pub prices: BTreeMap<Vec<String>, ChainPrice>,
    pub submit_target: UInt256,
    pub expires_at: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareAssignment {
    pub id: String,
    pub client: String,
    pub chain_path: Vec<String>,
    pub height: u64,
    pub work_id: String,
    pub prefix_hex: String,
    pub share_target: UInt256,
    pub share_value: UInt256,
    pub block_value: UInt256,
    pub start_nonce: u64,
    pub count: u64,
    pub expires_at: i64,
    pub store: Option<String>,
    pub peer: Option<String>,
}
#[derive(Debug, Clone)]
pub struct SponsorLedger {
    pub share_target: UInt256,
    pub range_size: u64,
    pub max_assignments: usize,
    pub credit_lifetime: i64,
    templates: BTreeMap<String, SponsorTemplate>,
    next_start: BTreeMap<String, u64>,
    assignments: BTreeMap<String, ShareAssignment>,
    shares: BTreeSet<(String, u64)>,
    submitted: BTreeSet<(String, u64)>,
    credits: BTreeMap<(String, Vec<String>), (UInt256, i64)>,
    sold: BTreeMap<(Vec<String>, u64), UInt256>,
    commitments: Vec<(Vec<String>, u64, i64)>,
    last_id: u64,
    latest: Option<SponsorTemplate>,
}
impl SponsorLedger {
    pub fn new(
        share_target: UInt256,
        range_size: u64,
        max_assignments: usize,
        credit_lifetime: i64,
    ) -> Result<Self, Refusal> {
        if share_target == UInt256::default() || range_size == 0 || max_assignments == 0 {
            return Err(Refusal::Draft("invalid ledger configuration".into()));
        }
        Ok(Self {
            share_target,
            range_size,
            max_assignments,
            credit_lifetime,
            templates: BTreeMap::new(),
            next_start: BTreeMap::new(),
            assignments: BTreeMap::new(),
            shares: BTreeSet::new(),
            submitted: BTreeSet::new(),
            credits: BTreeMap::new(),
            sold: BTreeMap::new(),
            commitments: vec![],
            last_id: 0,
            latest: None,
        })
    }
    pub fn latest(&self) -> Option<&SponsorTemplate> {
        self.latest.as_ref()
    }
    pub fn install(&mut self, template: SponsorTemplate, now: i64) {
        self.templates
            .entry(template.work_id.clone())
            .or_insert_with(|| template.clone());
        self.latest = Some(template);
        self.prune(now);
    }
    fn live_height(&self, path: &[String]) -> Option<u64> {
        self.templates
            .values()
            .filter_map(|t| t.prices.get(path).map(|p| p.height))
            .max()
    }
    pub fn assign(
        &mut self,
        client: String,
        path: Vec<String>,
        store: Option<String>,
        peer: Option<String>,
        now: i64,
    ) -> Result<ShareAssignment, Refusal> {
        let t = self
            .latest
            .as_ref()
            .filter(|t| now < t.expires_at)
            .ok_or(Refusal::Unpriceable)?;
        let price = t.prices.get(&path).ok_or(Refusal::Unpriceable)?;
        let value = Self::share_value(self.share_target, price)
            .filter(|v| *v != UInt256::default())
            .ok_or(Refusal::Unpriceable)?;
        if self.live_height(&path).unwrap_or(price.height) > price.height {
            return Err(Refusal::Busy);
        }
        let block_value = fixed(BigUint::from(price.reward) * unit()).expect("u64 reward fits");
        if self
            .sold
            .get(&(path.clone(), price.height))
            .copied()
            .unwrap_or_default()
            >= block_value
        {
            return Err(Refusal::SoldOut);
        }
        if self.assignments.len() >= self.max_assignments {
            return Err(Refusal::Busy);
        }
        let start = self.next_start.get(&t.work_id).copied().unwrap_or(0);
        let end = start.checked_add(self.range_size).ok_or(Refusal::Busy)?;
        self.next_start.insert(t.work_id.clone(), end);
        self.last_id = self.last_id.wrapping_add(1);
        let a = ShareAssignment {
            id: format!("{}.{}", t.work_id, self.last_id),
            client,
            chain_path: path,
            height: price.height,
            work_id: t.work_id.clone(),
            prefix_hex: t.prefix_hex.clone(),
            share_target: self.share_target,
            share_value: value,
            block_value,
            start_nonce: start,
            count: self.range_size,
            expires_at: t.expires_at,
            store,
            peer,
        };
        self.assignments.insert(a.id.clone(), a.clone());
        Ok(a)
    }
    pub fn assignment(&self, id: &str) -> Option<&ShareAssignment> {
        self.assignments.get(id)
    }
    /// Trusted arithmetic seam matching Swift: hash must be computed from the
    /// assignment prefix and nonce, never accepted directly from an HTTP client.
    pub fn credit_hash(
        &mut self,
        id: &str,
        nonce: u64,
        hash: UInt256,
        now: i64,
    ) -> Result<UInt256, Refusal> {
        let a = self
            .assignments
            .get(id)
            .filter(|a| self.templates.contains_key(&a.work_id))
            .ok_or(Refusal::UnknownAssignment)?;
        if now >= a.expires_at || self.live_height(&a.chain_path).unwrap_or(a.height) > a.height {
            return Err(Refusal::Expired);
        }
        if nonce < a.start_nonce || nonce - a.start_nonce >= a.count {
            return Err(Refusal::OutsideRange);
        }
        if hash > a.share_target {
            return Err(Refusal::MissesShareTarget);
        }
        if !self.shares.insert((a.work_id.clone(), nonce)) {
            return Err(Refusal::DuplicateShare);
        }
        let block = (a.chain_path.clone(), a.height);
        let backed = self.sold.get(&block).copied().unwrap_or_default();
        if backed >= a.block_value {
            return Err(Refusal::SoldOut);
        }
        let value = big(a.share_value).min(big(a.block_value) - big(backed));
        let fixed_value = fixed(value.clone()).expect("bounded by reward");
        self.sold.insert(
            block,
            fixed(big(backed) + &value).expect("bounded by reward"),
        );
        if a.store.is_none() && a.peer.is_none() {
            let key = (a.client.clone(), a.chain_path.clone());
            let old = self.credits.get(&key).map(|v| v.0).unwrap_or_default();
            self.credits.insert(key, (saturated(big(old) + value), now));
        }
        Ok(fixed_value)
    }
    pub fn credit_nonce(&mut self, id: &str, nonce: u64, now: i64) -> Result<UInt256, Refusal> {
        let a = self.assignments.get(id).ok_or(Refusal::UnknownAssignment)?;
        let prefix = hex::decode(&a.prefix_hex).map_err(|_| Refusal::Unpriceable)?;
        self.credit_hash(id, nonce, UInt256(lattice_pow::hash(&prefix, nonce)), now)
    }
    /// Independent of credit/sold-out state, as a share can still win another
    /// merged chain. Caller must compute hash and validate request assignment.
    pub fn block(&mut self, id: &str, nonce: u64, hash: UInt256, now: i64) -> Option<String> {
        let a = self.assignments.get(id)?;
        let t = self.templates.get(&a.work_id)?;
        if now >= t.expires_at
            || hash > t.submit_target
            || !self.submitted.insert((t.work_id.clone(), nonce))
        {
            return None;
        }
        Some(t.work_id.clone())
    }
    pub fn credit(&self, client: &str, path: &[String]) -> UInt256 {
        self.credits
            .get(&(client.into(), path.into()))
            .map(|v| v.0)
            .unwrap_or_default()
    }
    pub fn share_value(share_target: UInt256, price: &ChainPrice) -> Option<UInt256> {
        let product = BigUint::from(price.reward)
            * unit()
            * BigUint::from_bytes_be(&lattice_pow::work_for_target(&share_target.0));
        let product = big(fixed(product)?);
        let divisor = BigUint::from_bytes_be(&lattice_pow::work_for_target(&price.target.0));
        if divisor == BigUint::default() {
            return None;
        }
        fixed(product / divisor)
    }
    pub fn shares_for(&self, fee: u64, path: &[String]) -> Option<UInt256> {
        let p = self.latest.as_ref()?.prices.get(path)?;
        let value = big(Self::share_value(self.share_target, p)?);
        if value == BigUint::default() {
            return None;
        }
        fixed((BigUint::from(fee) * unit() + &value - 1u8) / value)
    }
    pub fn commit(
        &mut self,
        client: &str,
        fee: u64,
        path: &[String],
        balance: u64,
        hold_until: i64,
        now: i64,
    ) -> Result<(), Refusal> {
        let key = (client.into(), path.into());
        let cost = BigUint::from(fee) * unit();
        let held = self
            .credits
            .get(&key)
            .filter(|v| big(v.0) >= cost)
            .ok_or(Refusal::InsufficientCredit)?;
        let committed = self
            .commitments
            .iter()
            .filter(|(p, _, t)| p == path && *t > now)
            .fold(BigUint::from(fee), |sum, (_, f, _)| sum + *f);
        if committed > BigUint::from(balance) {
            return Err(Refusal::InsufficientFloat);
        }
        self.credits.insert(
            key,
            (fixed(big(held.0) - cost).expect("subtraction"), held.1),
        );
        self.commitments.push((path.into(), fee, hold_until));
        Ok(())
    }
    fn prune(&mut self, now: i64) {
        let gone: BTreeSet<_> = self
            .templates
            .values()
            .filter(|t| now >= t.expires_at)
            .map(|t| t.work_id.clone())
            .collect();
        for id in &gone {
            self.templates.remove(id);
            self.next_start.remove(id);
        }
        self.assignments.retain(|_, a| !gone.contains(&a.work_id));
        self.shares.retain(|(id, _)| !gone.contains(id));
        self.submitted.retain(|(id, _)| !gone.contains(id));
        let heights: BTreeMap<_, _> = self
            .sold
            .keys()
            .map(|(p, _)| (p.clone(), self.live_height(p)))
            .collect();
        self.sold
            .retain(|(p, h), _| heights[p].is_none_or(|live| *h >= live));
        self.credits.retain(|_, (v, t)| {
            i128::from(now) - i128::from(*t) < i128::from(self.credit_lifetime)
                && *v != UInt256::default()
        });
        self.commitments.retain(|(_, _, t)| now < *t);
    }
}
pub fn sponsored_body(
    draft: &TransactionBody,
    sponsor: &str,
    fee: u64,
    pay: u64,
    path: &[String],
) -> Result<TransactionBody, Refusal> {
    let refuse = |s: &str| Refusal::Draft(s.into());
    if fee == 0 || fee > i64::MAX as u64 {
        return Err(refuse("fee out of range"));
    }
    let total = fee
        .checked_add(pay)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| refuse("pay out of range"))?;
    if draft.chain_path != path {
        return Err(refuse("wrong chain"));
    }
    if draft.signers.len() != 1 || draft.signers[0] == sponsor {
        return Err(refuse(
            "the draft must have exactly one signer, not the sponsor",
        ));
    }
    if draft.account_actions.iter().any(|a| a.owner == sponsor)
        || draft.deposit_actions.iter().any(|a| a.demander == sponsor)
        || draft
            .receipt_actions
            .iter()
            .any(|a| a.demander == sponsor || a.withdrawer == sponsor)
        || draft
            .withdrawal_actions
            .iter()
            .any(|a| a.demander == sponsor || a.withdrawer == sponsor)
        || draft.actions.iter().any(|a| {
            [
                Some(a.key.as_str()),
                a.old_value.as_deref(),
                a.new_value.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|s| s.contains(sponsor))
        })
    {
        return Err(refuse("the draft names the sponsor"));
    }
    if draft.miner_surplus().is_none() {
        return Err(refuse("the draft creates value"));
    }
    let mut body = draft.clone();
    if pay > 0 {
        body.account_actions.push(AccountAction {
            owner: draft.signers[0].clone(),
            delta: pay as i64,
        });
    }
    body.account_actions.push(AccountAction {
        owner: sponsor.into(),
        delta: -(total as i64),
    });
    body.signers.push(sponsor.into());
    Ok(body)
}
