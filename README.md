# lattice-sponsor (Rust)

Rust port of the platform sponsorship service in [0xLattice/lattice-sponsor](https://github.com/0xLattice/lattice-sponsor).

Implemented library and service:

- Disjoint share assignments, template expiry, per-height reward backing, separate fee credit and float commitments.
- Sponsored transaction body construction with sponsor-authority exclusion checks.
- Storage upload reservations, scaled credit, pinning retries and permanent records.
- Serving credit conversion and peer validation.
- Coin-payment claim messages and persistent single-credit bookkeeping.
- Bounded node RPC for template/pricing reads, work submission, pinning, serving credit and signed coin-payment verification.
- Concurrent service orchestration, template refresh, token-authenticated fee work, co-signing, storage and coin-payment workflows.
- Runnable HTTP/CLI with the work, shares, sponsor, store, terms and pay routes.

`credit_nonce` computes actual assignment proof of work. `credit_hash` and `block` are trusted arithmetic seams matching Swift; an HTTP server must never accept an asserted hash from a client. Storage entries are structurally budgeted here; the node validates their content addresses when pinned.

Run `cargo test --locked` and `cargo clippy --all-targets --locked -- -D warnings`. Sibling Rust checkouts are currently required. The runnable CLI is `cargo run -- --help`. It requires a node URL, RPC cookie file and a private key file. All verification so far used isolated local fixtures; live deployment and remote service interoperability have not been exercised.

See [PORTING.md](PORTING.md) for scope and differences.
