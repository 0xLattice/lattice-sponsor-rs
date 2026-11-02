use lattice_primitives::UInt256;
use lattice_sponsor::{Refusal, payment::*, serving::*, storage::*};
use serde_json::{Value, json};
use std::collections::BTreeMap;
fn result<T: serde::Serialize>(r: Result<T, Refusal>) -> Value {
    match r {
        Ok(x) => json!({"ok":x}),
        Err(Refusal::Draft(m)) => json!({"error":"draft","message":m}),
        Err(e) => json!({"error":e.to_string()}),
    }
}
fn status(s: StoreStatus) -> Value {
    json!({"root":s.root,"bytes":s.bytes,"needed":s.needed,"credit":s.credit,"stored":s.stored})
}
pub fn extras(input: &Value) -> Value {
    let serving: Vec<_> = input["serving"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let x = ServingCredit {
                rates: BTreeMap::from([(vec!["Nexus".into()], r["rate"].as_i64().unwrap())]),
            };
            result(x.bytes(
                serde_json::from_value(r["value"].clone()).unwrap(),
                &serde_json::from_value::<Vec<String>>(r["path"].clone()).unwrap(),
            ))
        })
        .collect();
    let peers: Vec<_> = input["peers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            result(ServingCredit::check(
                r["peer"].as_str(),
                r["store"].as_str(),
            ))
        })
        .collect();
    let payments: Vec<_> = input["payments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let p: CoinPayment = serde_json::from_value(r.clone()).unwrap();
            json!({"message":p.message(),"path":p.chain_path()})
        })
        .collect();
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("payments.json");
    let mut ledger = PaymentLedger::open(file.clone()).unwrap();
    let mut payment_trace = vec![];
    for a in input["paymentTrace"].as_array().unwrap() {
        let cid = a["cid"].as_str().unwrap_or("");
        let out = match a["op"].as_str().unwrap() {
            "claim" => result(ledger.claim(cid.into())),
            "release" => {
                ledger.release(cid, a["credit"].as_bool().unwrap()).unwrap();
                Value::Null
            }
            "restart" => {
                ledger = PaymentLedger::open(file.clone()).unwrap();
                Value::Null
            }
            "credited" => json!(ledger.credited()),
            _ => panic!(),
        };
        payment_trace.push(out);
    }
    let mut storage = BTreeMap::new();
    for t in input["storage"].as_array().unwrap() {
        let file = temp
            .path()
            .join(format!("{}.json", t["name"].as_str().unwrap()));
        let rates = BTreeMap::from([
            (vec!["Nexus".into()], 3),
            (vec!["Nexus".into(), "child".into()], 7),
            (vec!["Nexus".into(), "zero".into()], 0),
        ]);
        let mut ledger = StorageLedger::open(
            t["budget"].as_i64().unwrap(),
            rates.clone(),
            file.clone(),
            t["capacity"].as_i64().unwrap(),
            10,
        )
        .unwrap();
        let mut outputs = vec![];
        for a in t["actions"].as_array().unwrap() {
            let root = a["root"].as_str().unwrap_or("r");
            let now = a["now"].as_i64().unwrap_or(0);
            let path: Vec<String> = a
                .get("path")
                .map(|v| serde_json::from_value(v.clone()).unwrap())
                .unwrap_or_else(|| vec!["Nexus".into()]);
            let out = match a["op"].as_str().unwrap() {
                "upload" => result(
                    ledger
                        .upload(
                            root.into(),
                            serde_json::from_value(a["volumes"].clone()).unwrap(),
                            now,
                        )
                        .map(status),
                ),
                "credit" => result(ledger.credit(
                    root,
                    serde_json::from_value::<UInt256>(a["value"].clone()).unwrap(),
                    &path,
                    now,
                )),
                "check" => result(ledger.check(root, &path, now)),
                "status" => ledger.status(root).map(status).unwrap_or(Value::Null),
                "take" => ledger
                    .take_paid(root)
                    .map(|u| json!({"bytes":u.bytes,"volumes":u.volumes}))
                    .unwrap_or(Value::Null),
                "finish" => {
                    ledger.finish(root, a["pinned"].as_bool().unwrap()).unwrap();
                    Value::Null
                }
                "restart" => {
                    ledger = StorageLedger::open(
                        t["budget"].as_i64().unwrap(),
                        rates.clone(),
                        file.clone(),
                        t["capacity"].as_i64().unwrap(),
                        10,
                    )
                    .unwrap();
                    Value::Null
                }
                _ => panic!(),
            };
            outputs.push(out);
        }
        storage.insert(t["name"].as_str().unwrap(), outputs);
    }
    json!({"serving":serving,"peers":peers,"payments":payments,"paymentTrace":payment_trace,"storage":storage})
}
