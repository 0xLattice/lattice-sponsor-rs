use crate::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
pub const LARGEST_VOLUME: i64 = (1 << 20) + 1024;
const LARGEST_PIN: usize = 2 << 20;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoreVolume {
    pub cid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreStatus {
    pub root: String,
    pub bytes: i64,
    pub needed: BTreeMap<String, u64>,
    pub credit: i64,
    pub stored: bool,
}
#[derive(Debug, Clone)]
pub struct Upload {
    pub volumes: Vec<StoreVolume>,
    pub bytes: i64,
    created: i64,
    paid: UInt256,
    pinning: bool,
}
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error(transparent)]
    Refusal(#[from] Refusal),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
#[derive(Debug, Serialize, Deserialize)]
struct Record {
    root: String,
    bytes: i64,
}
pub struct StorageLedger {
    pub budget: i64,
    pub rates: BTreeMap<Vec<String>, i64>,
    pub pending_cap: i64,
    pub lifetime: i64,
    file: PathBuf,
    pending: BTreeMap<String, Upload>,
    stored: BTreeMap<String, i64>,
}
impl StorageLedger {
    pub fn open(
        budget: i64,
        rates: BTreeMap<Vec<String>, i64>,
        file: PathBuf,
        pending_cap: i64,
        lifetime: i64,
    ) -> Result<Self, StorageError> {
        if budget < 0 || pending_cap < 0 {
            return Err(Refusal::Draft("negative storage budget".into()).into());
        }
        let mut stored = BTreeMap::new();
        if file.exists() {
            for r in serde_json::from_slice::<Vec<Record>>(&std::fs::read(&file)?)? {
                if r.bytes < 0 {
                    return Err(Refusal::Draft("negative stored byte count".into()).into());
                }
                stored.insert(r.root, r.bytes);
            }
        }
        Ok(Self {
            budget,
            rates,
            pending_cap,
            lifetime,
            file,
            pending: BTreeMap::new(),
            stored,
        })
    }
    pub fn stored(&self) -> &BTreeMap<String, i64> {
        &self.stored
    }
    pub fn rate(&self, path: &[String]) -> Result<i64, Refusal> {
        self.rates
            .get(path)
            .copied()
            .filter(|r| *r > 0)
            .ok_or(Refusal::NoStorageRate)
    }
    pub fn upload(
        &mut self,
        root: String,
        volumes: Vec<StoreVolume>,
        now: i64,
    ) -> Result<StoreStatus, Refusal> {
        self.sweep(now);
        if self.pending.contains_key(&root) || self.stored.contains_key(&root) {
            return Err(Refusal::Draft("root already uploaded".into()));
        }
        if !volumes.iter().any(|v| v.cid == root) {
            return Err(Refusal::Draft("the root is not listed".into()));
        }
        let mut bytes = 0i64;
        for volume in &volumes {
            let size = if let Some(entries) = &volume.entries {
                if !entries.contains_key(&volume.cid) {
                    return Err(Refusal::Draft("a Volume's entries lack its root".into()));
                }
                let mut size = 0i64;
                for value in entries.values() {
                    let data = STANDARD
                        .decode(value)
                        .map_err(|_| Refusal::Draft("entries are not base64".into()))?;
                    size = size
                        .checked_add(i64::try_from(data.len()).map_err(|_| Refusal::OverBudget)?)
                        .ok_or(Refusal::OverBudget)?;
                }
                size
            } else {
                volume.bytes.filter(|b| *b > 0).ok_or_else(|| {
                    Refusal::Draft("a Volume without entries declares its bytes".into())
                })?
            };
            let encoded = serde_json::to_vec(volume).expect("string/integer JSON");
            let encoded_len = encoded.len() + encoded.iter().filter(|b| **b == b'/').count();
            if size > LARGEST_VOLUME || encoded_len >= LARGEST_PIN - 1024 {
                return Err(Refusal::Draft("a Volume is over 1 MiB and 1 KiB".into()));
            }
            bytes = bytes.checked_add(size).ok_or(Refusal::OverBudget)?;
        }
        let held: i128 = self.pending.values().map(|u| i128::from(u.bytes)).sum();
        let stored: i128 = self.stored.values().copied().map(i128::from).sum();
        if i128::from(bytes) > i128::from(self.budget) - stored - held {
            return Err(Refusal::OverBudget);
        }
        if i128::from(bytes) > i128::from(self.pending_cap) - held {
            return Err(Refusal::Busy);
        }
        self.pending.insert(
            root.clone(),
            Upload {
                volumes,
                bytes,
                created: now,
                paid: UInt256::default(),
                pinning: false,
            },
        );
        Ok(self.status(&root).expect("inserted"))
    }
    pub fn check(&self, root: &str, path: &[String], now: i64) -> Result<(), Refusal> {
        self.rate(path)?;
        self.pending
            .get(root)
            .filter(|u| i128::from(now) - i128::from(u.created) < i128::from(self.lifetime))
            .map(|_| ())
            .ok_or(Refusal::UnknownUpload)
    }
    pub fn credit(
        &mut self,
        root: &str,
        value: UInt256,
        path: &[String],
        now: i64,
    ) -> Result<(), Refusal> {
        self.check(root, path, now)?;
        let rate = self.rate(path)?;
        let upload = self.pending.get_mut(root).expect("checked");
        upload.paid = saturated(big(upload.paid) + big(saturated(big(value) * rate as u64)));
        Ok(())
    }
    pub fn take_paid(&mut self, root: &str) -> Option<Upload> {
        let u = self.pending.get_mut(root)?;
        if u.pinning || big(u.paid) < BigUint::from(u.bytes as u64) * unit() {
            return None;
        }
        let taken = u.clone();
        u.pinning = true;
        Some(taken)
    }
    /// Write the durable reservation before publishing stored state. On a write
    /// failure the paid upload remains reserved and can retry; it is never lost.
    pub fn finish(&mut self, root: &str, pinned: bool) -> Result<(), StorageError> {
        if !pinned {
            if let Some(u) = self.pending.get_mut(root) {
                u.pinning = false;
            }
            return Ok(());
        }
        let Some(upload) = self.pending.get(root) else {
            return Ok(());
        };
        let mut next = self.stored.clone();
        next.insert(root.into(), upload.bytes);
        let records: Vec<_> = next
            .iter()
            .map(|(root, bytes)| Record {
                root: root.clone(),
                bytes: *bytes,
            })
            .collect();
        if let Err(error) = atomic_write(&self.file, &serde_json::to_vec(&records)?) {
            if let Some(upload) = self.pending.get_mut(root) {
                upload.pinning = false;
            }
            return Err(error.into());
        }
        self.stored = next;
        self.pending.remove(root);
        Ok(())
    }
    pub fn status(&self, root: &str) -> Option<StoreStatus> {
        if let Some(bytes) = self.stored.get(root) {
            return Some(StoreStatus {
                root: root.into(),
                bytes: *bytes,
                needed: BTreeMap::new(),
                credit: *bytes,
                stored: true,
            });
        }
        let u = self.pending.get(root)?;
        let cost = BigUint::from(u.bytes as u64) * unit();
        let paid = big(u.paid);
        let owed = if paid >= cost {
            BigUint::default()
        } else {
            cost - &paid
        };
        let mut needed = BTreeMap::new();
        for (path, rate) in &self.rates {
            if *rate > 0 {
                let divisor = BigUint::from(*rate as u64) * unit();
                let coins = (&owed + &divisor - 1u8) / divisor;
                needed.insert(
                    path.join("/"),
                    coins.to_u64_digits().first().copied().unwrap_or(0),
                );
            }
        }
        let credit = (paid / unit())
            .min(BigUint::from(i64::MAX as u64))
            .to_u64_digits()
            .first()
            .copied()
            .unwrap_or(0) as i64;
        Some(StoreStatus {
            root: root.into(),
            bytes: u.bytes,
            needed,
            credit,
            stored: false,
        })
    }
    fn sweep(&mut self, now: i64) {
        self.pending.retain(|_, u| {
            big(u.paid) >= BigUint::from(u.bytes as u64) * unit()
                || i128::from(now) - i128::from(u.created) < i128::from(self.lifetime)
        });
    }
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
