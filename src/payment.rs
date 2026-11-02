use crate::{
    Refusal,
    storage::{StorageError, atomic_write},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoinPayment {
    pub chain: String,
    #[serde(rename = "txCID")]
    pub tx_cid: String,
    pub signer: String,
    pub signature: String,
    pub store: Option<String>,
    pub peer: Option<String>,
}
impl CoinPayment {
    pub fn chain_path(&self) -> Vec<String> {
        self.chain.split('/').map(str::to_owned).collect()
    }
    pub fn message(&self) -> String {
        if let Some(peer) = &self.peer {
            format!(
                "lattice-sponsor-pay-serving:v1:{}:{}:{peer}",
                self.chain, self.tx_cid
            )
        } else {
            format!(
                "lattice-sponsor-pay:v1:{}:{}:{}",
                self.chain,
                self.tx_cid,
                self.store.as_deref().unwrap_or("")
            )
        }
    }
}
pub struct PaymentLedger {
    file: PathBuf,
    credited: BTreeSet<String>,
    claimed: BTreeSet<String>,
}
impl PaymentLedger {
    pub fn open(file: PathBuf) -> Result<Self, StorageError> {
        let credited = if file.exists() {
            serde_json::from_slice(&std::fs::read(&file)?)?
        } else {
            BTreeSet::new()
        };
        Ok(Self {
            file,
            credited,
            claimed: BTreeSet::new(),
        })
    }
    pub fn credited(&self) -> &BTreeSet<String> {
        &self.credited
    }
    pub fn claim(&mut self, cid: String) -> Result<(), Refusal> {
        if self.credited.contains(&cid) || !self.claimed.insert(cid) {
            return Err(Refusal::AlreadyCredited);
        }
        Ok(())
    }
    pub fn release(&mut self, cid: &str, credit: bool) -> Result<(), StorageError> {
        self.claimed.remove(cid);
        if !credit {
            return Ok(());
        }
        let mut next = self.credited.clone();
        next.insert(cid.into());
        atomic_write(&self.file, &serde_json::to_vec(&next)?)?;
        self.credited = next;
        Ok(())
    }
}
