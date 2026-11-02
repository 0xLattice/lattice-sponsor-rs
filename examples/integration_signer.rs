//! Public RFC 8032 test identity; only for disposable integration chains.
use lattice_primitives::crypto;
use serde_json::{json, Value};
const PRIVATE: &str = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb";
const PUBLIC: &str = "ed013d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c";
fn main() {
    let Some(path) = std::env::args().nth(1) else {
        println!(
            "{}",
            json!({"accountActions":[],"actions":[],"depositActions":[],"receiptActions":[],"withdrawalActions":[],"signers":[crypto::create_address(PUBLIC).unwrap()],"nonce":"0","chainPath":["Nexus"]})
        );
        return;
    };
    if path == "--payment" {
        let path = std::env::args().nth(2).expect("payment JSON path");
        let mut payment: lattice_sponsor::payment::CoinPayment =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(payment.signer, crypto::create_address(PUBLIC).unwrap());
        payment.signature = crypto::sign(&payment.message(), PRIVATE).unwrap();
        println!("{}", serde_json::to_string(&payment).unwrap());
        return;
    }
    let mut response: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let tx = &mut response["transaction"];
    let body = lattice_sponsor::http::decode_body(&tx["body"]).unwrap();
    let message = body.signing_preimage().unwrap();
    let signature = crypto::sign(&message, PRIVATE).unwrap();
    assert!(crypto::verify(&message, &signature, PUBLIC));
    tx["signatures"][PUBLIC] = json!(signature);
    println!("{}", json!({"transaction":tx}));
}
