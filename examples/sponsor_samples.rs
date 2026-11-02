#[path = "../tests/sponsor_replay.rs"]
mod replay;
fn main() {
    let bytes = std::fs::read(std::env::args().nth(1).expect("fixture path")).unwrap();
    let input = serde_json::from_slice(&bytes).unwrap();
    println!("{}", replay::replay(&input));
}
