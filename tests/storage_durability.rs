use lattice_primitives::UInt256;
use lattice_sponsor::storage::{StorageLedger, StoreVolume};
use std::collections::BTreeMap;
#[test]
fn failed_record_write_keeps_paid_reservation_and_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    let file = missing.join("stored.json");
    let rates = BTreeMap::from([(vec!["Nexus".into()], 1)]);
    let mut ledger = StorageLedger::open(10, rates.clone(), file.clone(), 10, 100).unwrap();
    ledger
        .upload(
            "r".into(),
            vec![StoreVolume {
                cid: "r".into(),
                bytes: Some(10),
                entries: None,
            }],
            0,
        )
        .unwrap();
    let mut value = [0; 32];
    value[23] = 10;
    ledger
        .credit("r", UInt256(value), &["Nexus".into()], 0)
        .unwrap();
    assert!(ledger.take_paid("r").is_some());
    assert!(ledger.finish("r", true).is_err());
    assert!(!ledger.status("r").unwrap().stored);
    assert!(ledger.stored().is_empty());
    assert!(ledger.take_paid("r").is_some());
    std::fs::create_dir(&missing).unwrap();
    ledger.finish("r", true).unwrap();
    assert!(ledger.status("r").unwrap().stored);
    let reopened = StorageLedger::open(10, rates, file, 10, 100).unwrap();
    assert!(reopened.status("r").unwrap().stored);
}
