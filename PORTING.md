# Sponsor port evidence

Source: 0xLattice/lattice-sponsor `24d8c4def3bd47c45b4cd10115762894e5185ea5`.
Its Package.swift pins lattice-node `9abd15b1bb05a0d4305e83f64c4676a6e296071f`,
Lattice 54.2.1 and cashew 5.0.3; these differ from the main platform research
pins. Dependency substitutions in a Swift oracle must be recorded, not treated
as proof of compatibility with that original resolved dependency graph.

The Rust ledger follows the source's mutation order: a valid-range/hash share
is marked seen before the sold-out check, so retrying a sold-out share is a
duplicate. Block submission deduplication is independent of credit, and absent
chains do not erase previously sold height backing. Credit is in 2^64-scaled
units; products are checked against UInt256 before division, not silently
widened to create credit Swift would refuse. Fee reservations do not consume
account nonce. Storage/serving assignments do not increase fee credit.

Native integration tests cover real PoW credit, duplicate/sold-out ordering,
float commitments, disappearing/reorganized chain backing, storage reservation
expiry and disk-write failure, and payment claim/restart behavior. Actual Swift
differential corpora are being added independently. Finite tests do not prove
full service correctness. Lattice-formal currently has no sponsor ledger bridge.

Intentional robustness differences:

- Invalid ledger configuration returns an error instead of a precondition trap.
  Zero target pricing returns no price instead of dividing by zero.
- Expiry differences and byte-budget totals use wide checked arithmetic rather
  than trapping on Int64 subtraction/addition at pathological input bounds.
- Negative storage budgets/stored byte counts are rejected on construction.
- Storage completion writes its permanent record before removing the paid
  pending upload. A failed write retains its reservation and resets pinning for
  retry; Swift mutates its in-memory stored set before writing. Records are
  written through a private temporary file, synced and renamed.

The ledgers are single-owner mutable state. They do not lock separate processes
sharing ledger files or promise permanent availability by themselves. The node
adapter below supplies payment verification and pinning; service orchestration and the local HTTP/CLI are implemented. Distribution packaging and live deployment remain outside the current evidence.

## Node RPC adapter

`src/node.rs` implements sponsor-local node calls, with fresh authorization
per request, chain-path query encoding, template unavailability (409/503),
missing-account zero balance, reward-schedule projection, pinning, serving
credit, and payment verification. A claim checks reported canonical inclusion
and depth, membership in reported signers, a matching raw transaction key,
the actual Lattice-domain signature, and checked positive net payment.
As in Swift, this trusts the configured node's reported chain; it is not
a light-client inclusion proof or a guarantee against a later reorg.

The client refuses redirects and caps decoded response bytes at 16 MiB by
default, with 10-second ordinary request deadlines and 60-second pin/fetch
deadlines. These explicit resource limits are Rust hardening beyond the
reference adapter. Whole-body tasks can be cancelled by dropping their future.
Fee responses retain the typed projection/weight field checks even though
pricing only consumes minRelayFee. SponsorRPC reads/templates require status
200; SponsorNode payment/pinning/serving calls retain the reference 2xx rule.

`tests/node_http.rs` uses local TCP HTTP fixtures, not a live service, for
valid/invalid signatures, depth/reorg failures, amount overflow, missing
transactions, auth refresh, fetched-byte declarations, serving failures,
response limits, redirects, timeouts, pricing/template requests and full-width
nonce submission. Original Swift ledger comparisons remain separate evidence.


## Service orchestration and HTTP

`src/service.rs` ports the Sponsor actor's refresh/pricing, work issuance, share
hashing, fee co-signing, storage, serving and payment flows. Its shared state is
locked only for synchronous operations. RPC awaits release the lock, retaining
payment claims or in-flight pin reservations, and payments recheck upload expiry
after verification. Dropped futures release unfinished claims/pin markers.
Credits already persisted before a failed serving post remain spent.

`src/http.rs` exposes the source routes, enabling store/pay routes only when
configured. The share route computes the hash from the issued assignment and
nonce, attempts block submission independently of share-credit acceptance,
and preserves the response despite submission failure. Pinning occurs in a
background task; status reads also retry paid pins. Public JSON uses canonical
decimal strings for transaction integers and the documented response fields,
including UInt128 draft nonces without a JSON-number conversion. Requests are
bounded at 1 MiB for ordinary routes and 64 MiB for storage uploads, with
browser CORS support. HTTP error responses use a JSON error string and the
source refusal status mapping; framework-generated malformed-JSON responses
can differ from Hummingbird's formatting.

`src/main.rs` provides the source options, validates key permissions and
private/public key agreement, reads fresh cookie authorization, refreshes hosted
chain templates every five seconds, and handles Ctrl-C/SIGTERM shutdown.
The library also exposes an injectable clock for deterministic tests.

Intentional source fixes/hardening:

- Swift issues 16 random bytes plus a 32-byte HMAC (96 hexadecimal characters),
  but its `isIssued` guard requires 64 characters. Rust accepts and verifies the
  actual 96-character token. Reuse and forgery tests cover this correction.
- Private/public key agreement is checked on load; the reference checks file
  permissions but does not verify the pair before serving.
- Expiry addition/subtraction uses saturation/wider arithmetic rather than
  reproducing overflow traps. The source's wrapping three-block hold calculation
  and minimum ten-minute hold are retained.
- Cancellation releases outstanding reservations; successful payment persistence
  remains final even when delivery fails. Storage write-failure retention is
  documented separately.

`tests/service.rs` exercises real localhost node calls and public HTTP routes:
pricing/spec cache, token reuse, computed share credit, signature verification,
fee-float commitments, template expiry, failed serving delivery, pin persistence,
payment deduplication and cancellation, decimal wire bounds, optional routes,
CORS and body limits. `tests/service_cli.rs` starts the actual binary against a
local unavailable-work node and reads its terms endpoint; no external node is
contacted. The service actor itself has not yet received a complete actual-Swift
differential harness. The extensive ledger differential is narrower evidence,
not a proof of all asynchronous service schedules or deployment behavior.

## Real Rust node integration

The [disposable node/sponsor scenario](docs/NODE_INTEGRATION.md) now exercises
actual binaries, work credit, replay refusal, fee co-signing, client signing,
transaction submission, mining, work-paid storage pins and serving credit, and
coin-funded serving credit with depth/signature checks and replay rejection,
and durable restart with inclusion, pin, standing and paid-upload checks.
This closes the earlier mock-only integration gap for that root fee flow; it
is not deployment or full asynchronous service parity evidence.
