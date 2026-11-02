use lattice_primitives::*;
use lattice_sponsor::{
    Refusal, ledger::SponsorLedger, node::SponsorNode, service::*, serving::ServingCredit,
    storage::*,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
struct Reply {
    path: String,
    status: u16,
    body: Value,
}
async fn server(replies: Vec<Reply>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    server_hook(replies, None).await
}
async fn server_hook(
    replies: Vec<Reply>,
    clock: Option<Arc<AtomicI64>>,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
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
            if let Some(clock) = &clock {
                clock.store(10, Ordering::SeqCst);
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

fn key() -> SponsorKey {
    SponsorKey::from_hex(
        "ed01d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60".into(),
    )
    .unwrap()
}
fn block() -> Block {
    let rows: Value = serde_json::from_str(include_str!(
        "../../lattice-rs/crates/lattice-validation/tests/genesis-swift.json"
    ))
    .unwrap();
    let bytes: Vec<u8> =
        serde_json::from_value(rows[0]["entries"][rows[0]["cid"].as_str().unwrap()].clone())
            .unwrap();
    lattice_cashew::cbor::decode(&bytes).unwrap()
}
fn config(storage: Option<StorageLedger>) -> ServiceConfig {
    ServiceConfig {
        ledger: SponsorLedger::new(UInt256([255; 32]), 8, 100, 1000).unwrap(),
        key: key(),
        minimum_fee: 1,
        most_pay: None,
        token_key: [42; 32],
        storage,
        serving: ServingCredit {
            rates: BTreeMap::from([(vec!["Nexus".into()], 3)]),
        },
        pay_to: None,
        coin_depth: 6,
        payments: None,
    }
}
fn reads(spec: bool, balance: u64) -> Vec<Reply> {
    let mut r = vec![];
    if spec {
        r.push(Reply{path:"/api/chain/spec".into(),status:200,body:json!({"targetBlockTime":"1000","initialReward":"8","halvingInterval":"100","maxBlockSize":100000,"maxNumberOfTransactionsPerBlock":"10","premine":"0","halfLife":"100"})});
    }
    r.push(Reply {
        path: "/api/fees".into(),
        status: 200,
        body: json!({"minRelayFee":"2","blocks":[]}),
    });
    r.push(Reply {
        path: format!("/api/state/account/{}", key().address),
        status: 200,
        body: json!({"owner":key().address,"balance":balance.to_string(),"nonce":"0"}),
    });
    r
}
fn template(id: &str) -> MiningTemplate {
    MiningTemplate {
        work_id: id.into(),
        block: block(),
        search_target: UInt256([255; 32]),
        expires_in_milliseconds: 1000,
        chains: Some(vec![TemplateChain {
            chain_path: vec!["Nexus".into()],
            target: UInt256([255; 32]),
            height: 1,
            reward_recipient: Some(key().address),
        }]),
    }
}
fn service(c: ServiceConfig, url: &str, now: Arc<AtomicI64>) -> SponsorService {
    SponsorService::with_clock(
        c,
        SponsorNode::new(url, Arc::new(|| None)).unwrap(),
        Arc::new(move || now.load(Ordering::SeqCst)),
    )
}
fn draft() -> TransactionBody {
    TransactionBody {
        account_actions: vec![],
        actions: vec![],
        deposit_actions: vec![],
        receipt_actions: vec![],
        withdrawal_actions: vec![],
        signers: vec!["client".into()],
        nonce: u64::MAX,
        chain_path: vec!["Nexus".into()],
    }
}
#[tokio::test]
async fn token_reuse_credit_cosign_float_and_expiry() {
    let mut replies = reads(true, 3);
    replies.extend(reads(false, 100));
    let (url, server) = server(replies).await;
    let now = Arc::new(AtomicI64::new(0));
    let s = service(config(None), &url, now.clone());
    s.install(template("w"), 0).await;
    let p = vec!["Nexus".into()];
    let work = s.work(None, &p, 0, None, None).unwrap();
    assert_eq!(work.client.len(), 96);
    assert_eq!(work.fee, 2);
    let again = s.work(Some(&work.client), &p, 0, None, None).unwrap();
    assert_eq!(again.start_nonce, 8);
    assert!(s.work(Some(&"a".repeat(96)), &p, 0, None, None).is_err());
    let answer = s.share(&work.assignment, 0);
    assert_eq!(answer.response.unwrap().credit, 8);
    assert_eq!(answer.block.as_deref(), Some("w"));
    assert!(matches!(
        s.share(&work.assignment, 0).response,
        Err(ServiceError::Refusal(Refusal::DuplicateShare))
    ));
    let signed = s.sponsor(&work.client, &draft(), 0).unwrap();
    let signature = &signed.transaction.signatures[&key().public_key];
    assert!(crypto::verify(
        &signed.transaction.body.signing_preimage().unwrap(),
        signature,
        &key().public_key
    ));
    let wire = serde_json::to_value(&signed).unwrap();
    assert_eq!(wire["transaction"]["body"]["nonce"], u64::MAX.to_string());
    assert_eq!(
        wire["transaction"]["body"]["accountActions"][0]["delta"],
        "-2"
    );
    assert!(matches!(
        s.sponsor(&work.client, &draft(), 0),
        Err(ServiceError::Refusal(Refusal::InsufficientFloat))
    ));
    s.install(template("new"), 0).await;
    s.sponsor(&work.client, &draft(), 0).unwrap();
    now.store(1000, Ordering::SeqCst);
    assert!(matches!(
        s.work(Some(&work.client), &p, 0, None, None),
        Err(ServiceError::Refusal(Refusal::Unpriceable))
    ));
    assert_eq!(server.await.unwrap().len(), 5);
}
#[tokio::test]
async fn external_credit_is_spent_even_if_serving_delivery_fails() {
    let mut replies = reads(true, 100);
    replies.push(Reply {
        path: "/mining/work".into(),
        status: 500,
        body: json!({}),
    });
    replies.push(Reply {
        path: "/serving-credit".into(),
        status: 500,
        body: json!({}),
    });
    let (url, server) = server(replies).await;
    let s = service(config(None), &url, Arc::new(AtomicI64::new(0)));
    s.install(template("w"), 0).await;
    let work = s
        .work(None, &["Nexus".into()], 0, None, Some(&"a".repeat(64)))
        .unwrap();
    assert_eq!(work.fee, 0);
    assert_eq!(work.shares_for_fee, 0);
    let answer = s.process_share(&work.assignment, 0).await;
    assert!(answer.submission.unwrap().is_err());
    assert!(matches!(
        answer.response,
        Err(ServiceError::Node(
            lattice_sponsor::node::NodeError::Refusal(Refusal::ServingNotCredited)
        ))
    ));
    assert!(matches!(
        s.share(&work.assignment, 0).response,
        Err(ServiceError::Refusal(Refusal::DuplicateShare))
    ));
    server.await.unwrap();
}
#[tokio::test]
async fn paid_upload_pins_once_and_persists() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("stored.json");
    let storage = StorageLedger::open(
        100,
        BTreeMap::from([(vec!["Nexus".into()], 1)]),
        file.clone(),
        100,
        1000,
    )
    .unwrap();
    let mut replies = reads(true, 100);
    replies.push(Reply {
        path: "/pins".into(),
        status: 200,
        body: json!({}),
    });
    let (url, server) = server(replies).await;
    let s = service(config(Some(storage)), &url, Arc::new(AtomicI64::new(0)));
    s.install(template("w"), 0).await;
    s.upload(
        "root".into(),
        vec![StoreVolume {
            cid: "root".into(),
            entries: Some(BTreeMap::from([("root".into(), "YWJj".into())])),
            bytes: None,
        }],
    )
    .unwrap();
    let work = s
        .work(None, &["Nexus".into()], 0, Some("root"), None)
        .unwrap();
    assert_eq!(s.share(&work.assignment, 0).response.unwrap().credit, 0);
    assert!(s.pin_if_paid("root").await.unwrap());
    assert!(!s.pin_if_paid("root").await.unwrap());
    assert!(s.store_status("root").unwrap().stored);
    assert!(file.exists());
    server.await.unwrap();
}
fn payment(peer: bool) -> (lattice_sponsor::payment::CoinPayment, Vec<Reply>) {
    let k = key();
    let mut payment = lattice_sponsor::payment::CoinPayment {
        chain: "Nexus".into(),
        tx_cid: "tx1".into(),
        signer: k.address.clone(),
        signature: String::new(),
        store: (!peer).then(|| "root".into()),
        peer: peer.then(|| "a".repeat(64)),
    };
    payment.signature = crypto::sign(
        &payment.message(),
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    )
    .unwrap();
    let described = json!({"txCID":"tx1","blockHeight":"10","blockHash":"b10","nonce":"0","signers":[k.address],"chainPath":["Nexus"],"chain":["Nexus"],"accountActions":[{"owner":"payTo","delta":"4"}],"depositActions":[],"receiptActions":[],"withdrawalActions":[]});
    let raw = json!({"transaction":{"signatures":[{"key":k.public_key,"value":"00"}],"body":{"rawCID":"bafyreif4y6jnr52hsw2gvpz2rgkch2tcmb2mwu6q7xeqg7lurjntzur5hy"}}});
    let replies = vec![
        Reply {
            path: "/api/transaction/tx1".into(),
            status: 200,
            body: described,
        },
        Reply {
            path: "/api/block/latest".into(),
            status: 200,
            body: json!({"height":"16","hash":"b16","transactionCount":0,"timestamp":"0"}),
        },
        Reply {
            path: "/api/block/10".into(),
            status: 200,
            body: json!({"hash":"b10"}),
        },
        Reply {
            path: "/transactions/tx1".into(),
            status: 200,
            body: raw,
        },
    ];
    (payment, replies)
}
#[tokio::test]
async fn payment_consumed_before_failed_serving_post_and_persisted() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("payments.json");
    let (payment, mut replies) = payment(true);
    replies.push(Reply {
        path: "/serving-credit".into(),
        status: 500,
        body: json!({}),
    });
    let (url, server) = server(replies).await;
    let mut c = config(None);
    c.pay_to = Some("payTo".into());
    c.payments = Some(lattice_sponsor::payment::PaymentLedger::open(file.clone()).unwrap());
    let s = service(c, &url, Arc::new(AtomicI64::new(0)));
    assert!(matches!(
        s.pay(&payment).await,
        Err(ServiceError::Node(
            lattice_sponsor::node::NodeError::Refusal(Refusal::ServingNotCredited)
        ))
    ));
    assert!(matches!(
        s.pay(&payment).await,
        Err(ServiceError::Refusal(Refusal::AlreadyCredited))
    ));
    let reopened = lattice_sponsor::payment::PaymentLedger::open(file).unwrap();
    assert!(reopened.credited().contains("tx1"));
    server.await.unwrap();
}
#[tokio::test]
async fn cancellation_releases_pending_payment_claim() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("payments.json");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let started = Arc::new(tokio::sync::Notify::new());
    let barrier = started.clone();
    let server = tokio::spawn(async move {
        let mut held = vec![];
        for _ in 0..2 {
            let (mut connection, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            assert!(connection.read(&mut buffer).await.unwrap() > 0);
            held.push(connection);
            barrier.notify_one();
        }
        std::future::pending::<()>().await;
    });
    let mut c = config(None);
    c.pay_to = Some("payTo".into());
    c.payments = Some(lattice_sponsor::payment::PaymentLedger::open(file).unwrap());
    let s = service(c, &url, Arc::new(AtomicI64::new(0)));
    let (payment, _) = payment(true);
    let task = {
        let s = s.clone();
        let p = payment.clone();
        tokio::spawn(async move { s.pay(&p).await })
    };
    started.notified().await;
    assert!(matches!(
        s.pay(&payment).await,
        Err(ServiceError::Refusal(Refusal::AlreadyCredited))
    ));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let retry = {
        let s = s.clone();
        tokio::spawn(async move { s.pay(&payment).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .expect("released claim must let retry reach RPC");
    retry.abort();
    let _ = retry.await;
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn public_http_routes_decimal_wire_cors_limits_and_optional_capabilities() {
    let (url, node) = server(reads(true, 100)).await;
    let s = service(config(None), &url, Arc::new(AtomicI64::new(0)));
    s.install(template("w"), 0).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let host = tokio::spawn(async move {
        axum::serve(listener, lattice_sponsor::http::router(s, false, false))
            .await
            .unwrap();
    });
    let client = reqwest::Client::new();
    let terms: Value = client
        .get(format!("{base}/terms"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(terms["depth"], "6");
    assert_eq!(
        client
            .post(format!("{base}/work"))
            .json(&json!({"pay":1}))
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    let work: Value = client
        .post(format!("{base}/work"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(work["startNonce"], "0");
    assert_eq!(work["fee"], "2");
    let reused: Value = client
        .post(format!("{base}/work"))
        .json(&json!({"client":work["client"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reused["startNonce"], "8");
    assert_eq!(
        client
            .post(format!("{base}/shares"))
            .json(&json!({"assignment":work["assignment"],"nonce":0}))
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    assert_eq!(
        client
            .get(format!("{base}/store/root"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let options = client
        .request(reqwest::Method::OPTIONS, format!("{base}/work"))
        .header("origin", "https://wallet.invalid")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(options.headers()["access-control-allow-origin"], "*");
    assert_eq!(
        client
            .post(format!("{base}/work"))
            .header("content-type", "application/json")
            .body(" ".repeat((1 << 20) + 1))
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    host.abort();
    let _ = host.await;
    node.await.unwrap();
}
#[test]
fn wire_decoder_keeps_full_width_numbers_and_rejects_noncanonical_forms() {
    let value = json!({"accountActions":[{"owner":"x","delta":i64::MIN.to_string()}],"actions":[],"depositActions":[{"nonce":u128::MAX.to_string(),"demander":"x","amountDemanded":u64::MAX.to_string(),"amountDeposited":"0"}],"receiptActions":[],"withdrawalActions":[],"signers":["x"],"nonce":u64::MAX.to_string(),"chainPath":["Nexus"]});
    let body = lattice_sponsor::http::decode_body(&value).unwrap();
    assert_eq!(body.deposit_actions[0].nonce, u128::MAX);
    assert_eq!(body.account_actions[0].delta, i64::MIN);
    let encoded = serde_json::to_value(BoundTransaction {
        signatures: BTreeMap::new(),
        body,
    })
    .unwrap();
    assert_eq!(encoded["body"], value);
    for invalid in [
        json!(1),
        json!("01"),
        json!("+1"),
        json!("-0"),
        json!("18446744073709551616"),
    ] {
        let mut value = value.clone();
        value["nonce"] = invalid;
        assert!(lattice_sponsor::http::decode_body(&value).is_err());
    }
}

#[tokio::test]
async fn payment_rechecks_upload_expiry_after_node_verification() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("payments.json");
    let now = Arc::new(AtomicI64::new(0));
    let (payment, replies) = payment(false);
    let (url, node) = server_hook(replies, Some(now.clone())).await;
    let storage = StorageLedger::open(
        100,
        BTreeMap::from([(vec!["Nexus".into()], 3)]),
        temp.path().join("stored.json"),
        100,
        10,
    )
    .unwrap();
    let mut c = config(Some(storage));
    c.pay_to = Some("payTo".into());
    c.payments = Some(lattice_sponsor::payment::PaymentLedger::open(file.clone()).unwrap());
    let s = service(c, &url, now);
    s.upload(
        "root".into(),
        vec![StoreVolume {
            cid: "root".into(),
            bytes: Some(3),
            entries: None,
        }],
    )
    .unwrap();
    assert!(matches!(
        s.pay(&payment).await,
        Err(ServiceError::Refusal(Refusal::UnknownUpload))
    ));
    assert!(
        lattice_sponsor::payment::PaymentLedger::open(file)
            .unwrap()
            .credited()
            .is_empty()
    );
    assert_eq!(s.store_status("root").unwrap().credit, 0);
    assert_eq!(node.await.unwrap().len(), 4);
}
