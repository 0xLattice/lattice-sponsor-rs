#[path = "sponsor_extras.rs"]
mod sponsor_extras;
use lattice_primitives::{TransactionBody, UInt256};
use lattice_sponsor::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
fn u(v: &Value) -> UInt256 {
    serde_json::from_value(v.clone()).unwrap()
}
fn path(v: &Value) -> Vec<String> {
    serde_json::from_value(v.clone()).unwrap()
}
fn string(v: &Value) -> String {
    v.as_str().unwrap().to_owned()
}
fn optional(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}
fn result<T: serde::Serialize>(v: Result<T, Refusal>) -> Value {
    match v {
        Ok(value) => json!({"ok":value}),
        Err(Refusal::Draft(message)) => json!({"error":"draft","message":message}),
        Err(error) => json!({"error":error.to_string()}),
    }
}
fn assignment(a: &ShareAssignment) -> Value {
    json!({"id":a.id,"client":a.client,"chainPath":a.chain_path,"height":a.height,"workID":a.work_id,"prefixHex":a.prefix_hex,"shareTarget":a.share_target,"shareValue":a.share_value,"blockValue":a.block_value,"startNonce":a.start_nonce,"count":a.count,"expiresAt":a.expires_at,"store":a.store,"peer":a.peer})
}
pub fn replay(input: &Value) -> Value {
    let arithmetic:Vec<_>=input["arithmetic"].as_array().unwrap().iter().map(|v|{let price=ChainPrice{target:u(&v["target"]),height:1,reward:v["reward"].as_u64().unwrap()};let share=u(&v["shareTarget"]);let mut ledger=SponsorLedger::new(share,1,1,100).unwrap();ledger.install(SponsorTemplate{work_id:"w".into(),prefix_hex:"00".into(),prices:BTreeMap::from([(vec!["Nexus".into()],price.clone())]),submit_target:price.target,expires_at:100},0);json!({"value":SponsorLedger::share_value(share,&price),"shares":ledger.shares_for(v["fee"].as_u64().unwrap(),&["Nexus".into()])})}).collect();
    let mut traces = BTreeMap::new();
    for trace in input["traces"].as_array().unwrap() {
        let mut ledger = SponsorLedger::new(
            u(&trace["shareTarget"]),
            trace["rangeSize"].as_u64().unwrap(),
            trace["maxAssignments"].as_u64().unwrap() as usize,
            trace["creditLifetime"].as_i64().unwrap(),
        )
        .unwrap();
        let mut outcomes = Vec::new();
        for a in trace["actions"].as_array().unwrap() {
            let now = a["now"].as_i64().unwrap_or(0);
            let outcome = match a["op"].as_str().unwrap() {
                "install" => {
                    let prices = a["prices"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| {
                            (
                                path(&p["path"]),
                                ChainPrice {
                                    target: u(&p["target"]),
                                    height: p["height"].as_u64().unwrap(),
                                    reward: p["reward"].as_u64().unwrap(),
                                },
                            )
                        })
                        .collect();
                    ledger.install(
                        SponsorTemplate {
                            work_id: string(&a["workID"]),
                            prefix_hex: string(&a["prefixHex"]),
                            prices,
                            submit_target: u(&a["submitTarget"]),
                            expires_at: a["expiresAt"].as_i64().unwrap(),
                        },
                        now,
                    );
                    json!(null)
                }
                "assign" => result(
                    ledger
                        .assign(
                            string(&a["client"]),
                            path(&a["path"]),
                            optional(&a["store"]),
                            optional(&a["peer"]),
                            now,
                        )
                        .map(|a| assignment(&a)),
                ),
                "credit" => result(ledger.credit_hash(
                    a["id"].as_str().unwrap(),
                    a["nonce"].as_u64().unwrap(),
                    u(&a["hash"]),
                    now,
                )),
                "block" => json!(ledger.block(
                    a["id"].as_str().unwrap(),
                    a["nonce"].as_u64().unwrap(),
                    u(&a["hash"]),
                    now
                )),
                "commit" => result(ledger.commit(
                    a["client"].as_str().unwrap(),
                    a["fee"].as_u64().unwrap(),
                    &path(&a["path"]),
                    a["balance"].as_u64().unwrap(),
                    a["holdUntil"].as_i64().unwrap(),
                    now,
                )),
                "balance" => json!(ledger.credit(a["client"].as_str().unwrap(), &path(&a["path"]))),
                "sharesFor" => {
                    json!(ledger.shares_for(a["fee"].as_u64().unwrap(), &path(&a["path"])))
                }
                "assignment" => ledger
                    .assignment(a["id"].as_str().unwrap())
                    .map(assignment)
                    .unwrap_or(Value::Null),
                _ => panic!("unknown action"),
            };
            outcomes.push(outcome);
        }
        traces.insert(string(&trace["name"]), outcomes);
    }
    let drafts: Vec<_> = input["drafts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let body: TransactionBody = serde_json::from_value(d["body"].clone()).unwrap();
            result(sponsored_body(
                &body,
                d["sponsor"].as_str().unwrap(),
                d["fee"].as_u64().unwrap(),
                d["pay"].as_u64().unwrap(),
                &path(&d["path"]),
            ))
        })
        .collect();
    json!({"arithmetic":arithmetic,"traces":traces,"drafts":drafts,"extras":sponsor_extras::extras(input)})
}
#[test]
fn pinned_swift_sponsor_ledger() {
    let input: Value = serde_json::from_str(include_str!("sponsor-input.json")).unwrap();
    let expected: Value = serde_json::from_str(include_str!("sponsor-swift.json")).unwrap();
    assert_eq!(replay(&input), expected);
    assert_eq!(expected["traces"]["soldout"][3]["error"], "soldOut");
    assert_eq!(expected["traces"]["soldout"][4]["error"], "duplicateShare");
    assert!(expected["drafts"][0]["ok"].is_object());
}
