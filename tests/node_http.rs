use lattice_primitives::{UInt256, crypto};
use lattice_sponsor::{
    Refusal,
    node::{NodeError, SponsorNode},
    payment::CoinPayment,
    storage::*,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
struct Reply {
    path: String,
    status: u16,
    body: Value,
}
async fn server(replies: Vec<Reply>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = vec![];
        for reply in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let end = loop {
                let mut b = [0; 1024];
                let n = stream.read(&mut b).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&b[..n]);
                if let Some(i) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let headers = String::from_utf8(request[..end].to_vec()).unwrap();
            let length = headers
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|v| v.parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while request.len() < end + length {
                let mut b = [0; 1024];
                let n = stream.read(&mut b).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&b[..n]);
            }
            assert_eq!(headers.split_whitespace().nth(1).unwrap(), reply.path);
            requests.push(String::from_utf8(request).unwrap());
            let body = reply.body.to_string();
            let bytes = format!(
                "HTTP/1.1 {} Reply\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n{body}",
                reply.status,
                body.len()
            );
            stream.write_all(bytes.as_bytes()).await.unwrap();
        }
        requests
    });
    (url, task)
}
fn fixture() -> (CoinPayment, String, Value, Value, Value) {
    let (private, public) = crypto::generate_key_pair();
    let signer = crypto::create_address(&public).unwrap();
    let mut payment = CoinPayment {
        chain: "Nexus/Market".into(),
        tx_cid: "tx1".into(),
        signer: signer.clone(),
        signature: String::new(),
        store: Some("root".into()),
        peer: None,
    };
    payment.signature = crypto::sign(&payment.message(), &private).unwrap();
    let described = json!({"txCID":"tx1","blockHeight":"10","blockHash":"b10","nonce":"0","signers":[signer],"chainPath":["Nexus","Market"],"chain":["Nexus","Market"],"accountActions":[{"owner":"sponsor","delta":"7"},{"owner":"sponsor","delta":"-3"}],"depositActions":[],"receiptActions":[],"withdrawalActions":[]});
    let tip = json!({"height":"16","hash":"b16","transactionCount":0,"timestamp":"0"});
    let raw = json!({"transaction":{"signatures":[{"key":public.clone(),"value":"00"}],"body":{"rawCID":"bafyreif4y6jnr52hsw2gvpz2rgkch2tcmb2mwu6q7xeqg7lurjntzur5hy"}}});
    (payment, private, described, tip, raw)
}
fn reads(described: Value, tip: Value, raw: Value) -> Vec<Reply> {
    [
        ("/api/transaction/tx1", described),
        ("/api/block/latest", tip),
        ("/api/block/10", json!({"hash":"b10"})),
        ("/transactions/tx1", raw),
    ]
    .into_iter()
    .map(|(p, body)| Reply {
        path: format!("{p}?chainPath=Nexus%2FMarket"),
        status: 200,
        body,
    })
    .collect()
}
#[tokio::test]
async fn real_http_payment_signature_depth_reorg_and_value_checks() {
    for mode in [
        "ok",
        "signature",
        "depth",
        "reorg",
        "overflow",
        "signer",
        "missing",
    ] {
        let (mut payment, _, mut described, mut tip, raw) = fixture();
        let expected = match mode {
            "signature" => {
                payment.signature = "00".into();
                Some(Refusal::InvalidSignature)
            }
            "depth" => {
                tip["height"] = json!("15");
                Some(Refusal::NotDeepEnough)
            }
            "reorg" => {
                described["blockHash"] = json!("other");
                Some(Refusal::NotIncluded)
            }
            "overflow" => {
                described["accountActions"] = json!([{"owner":"sponsor","delta":i64::MAX.to_string()},{"owner":"sponsor","delta":"1"}]);
                Some(Refusal::NoPayment)
            }
            "signer" => {
                described["signers"] = json!([]);
                Some(Refusal::NotSigner)
            }
            "missing" => Some(Refusal::NotIncluded),
            _ => None,
        };
        let mut replies = reads(described, tip, raw);
        if mode == "depth" || mode == "reorg" {
            replies.truncate(3);
        }
        if mode == "missing" {
            replies.truncate(1);
            replies[0].status = 404;
        }
        let (url, task) = server(replies).await;
        let node = SponsorNode::new(&url, Arc::new(|| Some("Basic cookie".into()))).unwrap();
        let result = node.verify_payment(&payment, "sponsor", 6).await;
        if let Some(expected) = expected {
            assert!(
                matches!(result,Err(NodeError::Refusal(ref e))if *e==expected),
                "{mode}: {result:?}"
            );
        } else {
            assert_eq!(result.unwrap(), 4);
        }
        let requests = task.await.unwrap();
        assert!(requests.iter().all(|r| {
            r.to_ascii_lowercase()
                .contains("authorization: basic cookie")
        }));
    }
}
#[tokio::test]
async fn fetched_volume_budget_and_serving_failure_are_observable() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage = StorageLedger::open(
        100,
        BTreeMap::from([(vec!["Nexus".into()], 1)]),
        temp.path().join("store.json"),
        100,
        100,
    )
    .unwrap();
    storage
        .upload(
            "root".into(),
            vec![StoreVolume {
                cid: "root".into(),
                entries: None,
                bytes: Some(3),
            }],
            0,
        )
        .unwrap();
    storage
        .credit("root", UInt256([255; 32]), &["Nexus".into()], 1)
        .unwrap();
    let upload = storage.take_paid("root").unwrap();
    let (url, task) = server(vec![
        Reply {
            path: "/fetch/root".into(),
            status: 200,
            body: json!({"cid":"root","entries":{"root":"AQID"}}),
        },
        Reply {
            path: "/pins".into(),
            status: 200,
            body: json!({}),
        },
        Reply {
            path: "/serving-credit".into(),
            status: 503,
            body: json!({}),
        },
    ])
    .await;
    let auth = Arc::new(Mutex::new("Bearer first".to_owned()));
    let current = auth.clone();
    let node = SponsorNode::new(
        &url,
        Arc::new(move || Some(current.lock().unwrap().clone())),
    )
    .unwrap();
    node.pin(&upload).await.unwrap();
    *auth.lock().unwrap() = "Bearer rotated".into();
    assert!(matches!(
        node.credit_serving(&"a".repeat(64), 10).await,
        Err(NodeError::Refusal(Refusal::ServingNotCredited))
    ));
    let requests = task.await.unwrap();
    assert!(requests[1].contains("\"cid\":\"root\""));
    assert!(requests[2].contains("Bearer rotated"));
    let (url, task) = server(vec![Reply {
        path: "/fetch/root".into(),
        status: 200,
        body: json!({"cid":"root","entries":{"root":"AQIDBA=="}}),
    }])
    .await;
    let node = SponsorNode::new(&url, Arc::new(|| None)).unwrap();
    assert!(matches!(
        node.pin(&upload).await,
        Err(NodeError::Refusal(Refusal::OverBudget))
    ));
    task.await.unwrap();
}
#[tokio::test]
async fn bounded_reads_and_redirects_do_not_become_successful_payment() {
    for status in [200, 302] {
        let (payment, _, described, _, _) = fixture();
        let (url, task) = server(vec![Reply {
            path: "/api/transaction/tx1?chainPath=Nexus%2FMarket".into(),
            status,
            body: described,
        }])
        .await;
        let mut node = SponsorNode::new(&url, Arc::new(|| None)).unwrap();
        node.response_limit = 4;
        let result = node.verify_payment(&payment, "sponsor", 6).await;
        if status == 200 {
            assert!(matches!(result, Err(NodeError::ResponseLimit)));
        } else {
            assert!(matches!(result, Err(NodeError::Status(302))));
        }
        task.await.unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let mut node = SponsorNode::new(&url, Arc::new(|| None)).unwrap();
    node.timeout = Duration::from_millis(20);
    assert!(
        matches!(node.verify_payment(&fixture().0,"sponsor",6).await,Err(NodeError::Transport(e))if e.is_timeout())
    );
    task.abort();
    let _ = task.await;
}
#[tokio::test]
async fn pricing_template_and_submission_rpc_preserve_wire_shapes() {
    let replies = vec![
        Reply {
            path: "/hosted-chains".into(),
            status: 200,
            body: json!({"chains":["Nexus/Market"]}),
        },
        Reply {
            path: "/api/chain/spec".into(),
            status: 200,
            body: json!({"targetBlockTime":"1000","initialReward":"1024","halvingInterval":"100","maxBlockSize":1000000,"maxNumberOfTransactionsPerBlock":"100","premine":"0","halfLife":"120"}),
        },
        Reply {
            path: "/api/fees".into(),
            status: 200,
            body: json!({"minRelayFee":"7","blocks":[]}),
        },
        Reply {
            path: "/api/state/account/missing".into(),
            status: 404,
            body: json!({}),
        },
        Reply {
            path: "/api/state/account/owner?chainPath=Nexus%2FMarket".into(),
            status: 200,
            body: json!({"owner":"owner","nonce":"2","balance":"18446744073709551615"}),
        },
        Reply {
            path: "/mining/templates".into(),
            status: 503,
            body: json!({}),
        },
        Reply {
            path: "/mining/templates".into(),
            status: 200,
            body: json!({"workID":"work"}),
        },
        Reply {
            path: "/mining/work".into(),
            status: 200,
            body: json!({"disposition":"accepted"}),
        },
    ];
    let (url, task) = server(replies).await;
    let node = SponsorNode::new(&url, Arc::new(|| None)).unwrap();
    let root = vec!["Nexus".into()];
    assert_eq!(node.hosted_chains().await.unwrap(), ["Nexus/Market"]);
    let spec = node.spec(&root).await.unwrap();
    assert_eq!(spec.initial_reward, 1024);
    assert_eq!(spec.max_state_growth, 0);
    assert!(spec.wasm_policies.is_empty());
    assert_eq!(node.relay_floor(&root).await.unwrap(), 7);
    assert_eq!(node.balance("missing", &root).await.unwrap(), 0);
    assert_eq!(
        node.balance("owner", &["Nexus".into(), "Market".into()])
            .await
            .unwrap(),
        u64::MAX
    );
    let body = json!({"recipients":[{"chainPath":["Nexus"],"address":"owner"}]});
    assert!(
        node.template::<Value>(body.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        node.template::<Value>(body.clone()).await.unwrap().unwrap()["workID"],
        "work"
    );
    assert_eq!(
        node.submit("work", u64::MAX).await.unwrap()["disposition"],
        "accepted"
    );
    let requests = task.await.unwrap();
    let sent: Value = serde_json::from_str(requests[6].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(sent, body);
    let sent: Value = serde_json::from_str(requests[7].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(sent["nonce"].as_u64(), Some(u64::MAX));
}
