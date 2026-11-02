# Sponsor accounting evidence

`cargo test --test sponsor_replay` replays persisted observations from actual pinned Swift production implementations. Regenerate with `python3 tools/generate-sponsor-vectors.py`, using the prepared full-Lattice cache at `/tmp/lattice-genesis-oracle`. The generator checks original Lattice sources, dependency lock, miner-core sources and the sponsor source revision before building. It writes input, Swift output and hash evidence under `tests/`.

Reference: `0xLattice/lattice-sponsor` `24d8c4def3bd47c45b4cd10115762894e5185ea5`, linked against pinned full Lattice `dda2d852429a27be6043599b63bde81530cd9514`. The complete original `SponsorLedger.swift` is compiled unchanged. The complete declarations of ServingCredit, StoreVolume, StoreStatus, StorageLedger, CoinPayment and PaymentLedger are extracted unchanged from their source files. Network extensions and API terms models are excluded. This is actual Swift ledger execution, not a reimplementation of expected behavior.

Coverage:

- 336 share-value and shares-needed cases: fixed-point rounding, zero reward/fee, maximum u64 values, varying target difficulties, intermediate UInt256 overflow.
- 12 sponsor action traces, 107 steps: nonoverlapping nonwrapping ranges, capacity, work-ID reuse, assignment metadata, expiry, pruning, higher live heights and reorgs, retained sold backing when a chain disappears, partial last shares, duplicate/sold-out ordering, block submission independent of credit, client/chain separation, storage/serving assignments, fee reservation and release by time.
- 23 sponsored-body cases: fee/pay bounds, signer constraints, chain mismatch, sponsor references in accounts/actions/deposits/receipts/withdrawals, conservation, nonce preservation and first-signer order. Exact resulting JSON bodies and exact refusal names/messages are compared.
- 60 serving arithmetic cases, 16 peer validation cases and 12 coin-payment message/path cases.
- 7 storage traces, 54 steps: reservation/capacity, declared and inline entry sizes, base64 rejection, missing roots, free empty inline volume, fractional and saturating credit, rate errors, expiry, retention of paid uploads, pinning retry, stored persistence and loss of volatile pending uploads after restart.
- 13 payment-ledger steps: claim deduplication, unsuccessful release, successful persisted release, reloading, volatile claim loss and credited-record persistence.

Successful writes use isolated local temporary directories; no live service, pin request, mining work or transaction submission occurs. The credit hash seam takes synthetic hashes to exercise accounting branches. It does not establish that a client performed work; the serving runtime must compute the actual hash from its issued prefix and nonce.

`storage_durability.rs` separately checks the documented intentional Rust hardening: a failed stored-record write retains the paid reservation, releases the in-flight pin marker, and permits retry. Swift removes the pending reservation before the write; this error-path behavior is deliberately not claimed as parity. Rust also rejects negative storage configuration/stored counts and uses wider checked elapsed-time/budget arithmetic rather than reproducing Swift traps at extreme inputs.

These are finite traces, not proofs of arbitrary schedules or complete runtime behavior. They do not test HTTP request bounds/authentication, signature verification of coin claims, canonical inclusion/depth checks, actual remote pinning, serving-credit delivery failure, crash/power-loss durability, or concurrent refresh/request ordering. Those remain separate runtime concerns. Source extraction evidence should not be described as a complete Swift sponsor executable build.
