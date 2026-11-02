use lattice_primitives::UInt256;
use lattice_sponsor::{payment::*, serving::ServingCredit, storage::*, *};
use std::collections::BTreeMap;
fn path() -> Vec<String> {
    vec!["Nexus".into()]
}
fn coins(n: u64) -> UInt256 {
    let mut b = [0; 32];
    b[16..24].copy_from_slice(&n.to_be_bytes());
    UInt256(b)
}
fn template(id: &str, height: u64, reward: u64, expires: i64) -> SponsorTemplate {
    SponsorTemplate {
        work_id: id.into(),
        prefix_hex: "0123".into(),
        prices: BTreeMap::from([(
            path(),
            ChainPrice {
                target: UInt256([255; 32]),
                height,
                reward,
            },
        )]),
        submit_target: UInt256([255; 32]),
        expires_at: expires,
    }
}
#[test]
fn real_hash_credit_global_cap_and_fee_float_are_separate() {
    let mut ledger = SponsorLedger::new(UInt256([255; 32]), 100, 10, 1000).unwrap();
    ledger.install(template("work", 1, 10, 100), 0);
    let first = ledger
        .assign("alice".into(), path(), None, None, 0)
        .unwrap();
    let other = ledger.assign("bob".into(), path(), None, None, 0).unwrap();
    assert_eq!(other.start_nonce, 100);
    assert_eq!(ledger.credit_nonce(&first.id, 0, 1).unwrap(), coins(10));
    assert_eq!(
        ledger.credit_nonce(&other.id, 100, 2),
        Err(Refusal::SoldOut)
    );
    assert_eq!(
        ledger.credit_nonce(&other.id, 100, 2),
        Err(Refusal::DuplicateShare)
    );
    assert_eq!(
        ledger.commit("alice", 6, &path(), 5, 50, 3),
        Err(Refusal::InsufficientFloat)
    );
    assert_eq!(ledger.credit("alice", &path()), coins(10));
    ledger.commit("alice", 6, &path(), 10, 50, 3).unwrap();
    assert_eq!(
        ledger.commit("alice", 4, &path(), 9, 50, 4),
        Err(Refusal::InsufficientFloat)
    );
    ledger.commit("alice", 4, &path(), 4, 60, 50).unwrap();
    assert_eq!(ledger.credit("alice", &path()), UInt256::default());
    // A height without remaining customer credit can still yield a submitted block.
    assert_eq!(
        ledger.block(&other.id, 100, UInt256::default(), 3),
        Some("work".into())
    );
    assert_eq!(ledger.block(&other.id, 100, UInt256::default(), 3), None);
}
#[test]
fn absent_chain_retains_sold_height_and_reorg_rejects_old_work() {
    let mut l = SponsorLedger::new(UInt256([255; 32]), 1, 10, 1000).unwrap();
    l.install(template("a", 1, 2, 10), 0);
    let a = l.assign("c".into(), path(), None, None, 0).unwrap();
    l.credit_nonce(&a.id, 0, 1).unwrap();
    let mut missing = template("missing", 2, 2, 30);
    missing.prices.clear();
    l.install(missing, 10);
    l.install(template("same-height", 1, 2, 30), 11);
    assert_eq!(
        l.assign("c".into(), path(), None, None, 11),
        Err(Refusal::SoldOut)
    );
    l.install(template("higher", 2, 2, 40), 12);
    let higher = l
        .assign("c".into(), path(), Some("upload".into()), None, 12)
        .unwrap();
    l.credit_nonce(&higher.id, 0, 12).unwrap();
    assert_eq!(l.credit("c", &path()), coins(2));
    l.install(template("reorg", 1, 2, 50), 13);
    assert_eq!(
        l.assign("c".into(), path(), None, None, 13),
        Err(Refusal::Busy)
    );
    assert_eq!(
        ServingCredit {
            rates: BTreeMap::from([(path(), 3)])
        }
        .bytes(coins(2), &path())
        .unwrap(),
        6
    );
}
fn declared(cid: &str, n: i64) -> StoreVolume {
    StoreVolume {
        cid: cid.into(),
        entries: None,
        bytes: Some(n),
    }
}
#[test]
fn paid_storage_survives_expiry_failed_persistence_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("missing/stored.json");
    let rates = BTreeMap::from([(path(), 3)]);
    let mut ledger = StorageLedger::open(20, rates.clone(), file.clone(), 20, 10).unwrap();
    let status = ledger
        .upload("root".into(), vec![declared("root", 10)], 0)
        .unwrap();
    assert_eq!(status.needed["Nexus"], 4);
    ledger.credit("root", coins(4), &path(), 1).unwrap();
    assert_eq!(ledger.status("root").unwrap().credit, 12);
    assert!(ledger.take_paid("root").is_some());
    assert!(ledger.take_paid("root").is_none());
    assert!(ledger.finish("root", true).is_err());
    assert!(!ledger.status("root").unwrap().stored);
    assert!(ledger.take_paid("root").is_some());
    // Sweep cannot discard a paid upload even after its work window closes.
    assert_eq!(
        ledger.check("root", &path(), 10),
        Err(Refusal::UnknownUpload)
    );
    ledger
        .upload("other".into(), vec![declared("other", 10)], 20)
        .unwrap();
    assert_eq!(
        ledger.upload("too-much".into(), vec![declared("too-much", 1)], 20),
        Err(Refusal::OverBudget)
    );
    std::fs::create_dir(file.parent().unwrap()).unwrap();
    ledger.finish("root", true).unwrap();
    let restored = StorageLedger::open(20, rates, file, 20, 10).unwrap();
    assert!(restored.status("root").unwrap().stored);
    assert!(restored.status("other").is_none());
}
#[test]
fn payment_claims_persist_once_and_failed_writes_do_not_create_credit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("missing/pay.json");
    let mut ledger = PaymentLedger::open(path.clone()).unwrap();
    ledger.claim("tx".into()).unwrap();
    assert_eq!(ledger.claim("tx".into()), Err(Refusal::AlreadyCredited));
    assert!(ledger.release("tx", true).is_err());
    assert!(!ledger.credited().contains("tx"));
    ledger.claim("tx".into()).unwrap();
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    ledger.release("tx", true).unwrap();
    assert_eq!(
        PaymentLedger::open(path).unwrap().claim("tx".into()),
        Err(Refusal::AlreadyCredited)
    );
    let mut p = CoinPayment {
        chain: "Nexus/Child".into(),
        tx_cid: "tx".into(),
        signer: "a".into(),
        signature: "b".into(),
        store: Some("root".into()),
        peer: None,
    };
    assert_eq!(p.message(), "lattice-sponsor-pay:v1:Nexus/Child:tx:root");
    p.peer = Some("peer".into());
    assert_eq!(
        p.message(),
        "lattice-sponsor-pay-serving:v1:Nexus/Child:tx:peer"
    );
    assert!(ServingCredit::check(p.peer.as_deref(), p.store.as_deref()).is_err());
}
