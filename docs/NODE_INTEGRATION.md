# Real node and sponsor integration

From the sibling repositories, build the actual daemons, then run:

```sh
(cd ../lattice-node-rs && cargo build --locked --bin lattice-node)
cargo build --locked --bin lattice-sponsor
python3.12 tools/node-integration.py
```

The test creates a disposable directory and starts both processes on ephemeral
loopback ports without seeds. It materializes authenticated volume boundaries
from the existing Swift genesis corpus using the protocol validator's
`genesis_volumes` example. This deliberately easy test chain is not Nexus mainnet.
The public RFC 8032 test identities used here must never hold real funds.

The scenario requests sponsor work, computes a share hash, submits it to the
sponsor, observes positive credit and actual node height advancement, and checks
that replay returns `duplicateShare`. It spends credit on a sponsored draft,
adds the client's real Ed25519 signature, submits through the node's transaction
API, and mines a second fixture block. It verifies that the transaction is
included at that block. Further shares buy storage and serving credit: an
uploaded CID is pinned in the node, and the peer's standing increases by exactly
the sponsor's reported byte credit. Fresh assignments wait for the next template
when the previous reward is sold out; the test does not bypass backing limits.

The test then funds its client identity with a fixture reward, submits a real
three-coin transfer to the sponsor, and signs the payment authorization. The
sponsor refuses it before the configured one-block depth, refuses a forged
signature without changing standing, then credits exactly three bytes for the
valid payment. It rejects a repeat claim before and after restart.

After seven fixture blocks, the test cleanly stops both daemons, restarts the node
without bootstrap content, and checks recovered tip, transaction inclusion,
operator pins and serving standing. It restarts the sponsor with the same storage
ledger and confirms that the paid upload remains stored.

The sponsor uses share work 1 and an explicit fee of 1. The fixture node itself
allows zero-fee transactions; without the explicit sponsor fee, its refusal to
sell unnecessary fee work is correct. No special production acceptance bypasses
are enabled. Requests use the fresh node cookie.

This integration exposed the missing `/hosted-chains` route: the sponsor needs
it even on a root-only host. The node now returns an empty hosted-child list
through its normal authenticated operator API.

Reports and process logs are saved under `target/node-integration.*`. The report
starts with `passed: false`. Each run copies both daemons and the fixture signer
to its temporary directory, so concurrent rebuilds cannot change the executable
between startup and restart. The report records these binary hashes, fixture hash,
credit, submission CID, inclusion block, stored CID, serving bytes and recovered
tip after success. Runtime
state and fixture private keys are deleted on exit. Builds use local sibling
checkouts; the test is not remote-release or cross-platform evidence.

This covers root mining, fee sponsorship, signed transaction admission, work-paid
storage/serving sales, coin-funded serving credit and clean restart. It does not
cover child hosting, coin-funded storage, abrupt power loss, mixed Swift/Rust
daemons, or throughput.
Those remain separate scenarios rather than implications of this test.
