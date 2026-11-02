//! Sponsor-local accounting. None of these credits establish chain consensus.
#![forbid(unsafe_code)]
pub mod ledger;
use lattice_primitives::UInt256;
pub use ledger::*;
use num_bigint::BigUint;
fn big(v: UInt256) -> BigUint {
    BigUint::from_bytes_be(&v.0)
}
fn unit() -> BigUint {
    BigUint::from(1u8) << 64usize
}
fn fixed(v: BigUint) -> Option<UInt256> {
    let bytes = v.to_bytes_be();
    if bytes.len() > 32 {
        return None;
    }
    let mut out = [0; 32];
    out[32 - bytes.len()..].copy_from_slice(&bytes);
    Some(UInt256(out))
}
fn saturated(v: BigUint) -> UInt256 {
    fixed(v).unwrap_or(UInt256([255; 32]))
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("unknownAssignment")]
    UnknownAssignment,
    #[error("expired")]
    Expired,
    #[error("outsideRange")]
    OutsideRange,
    #[error("duplicateShare")]
    DuplicateShare,
    #[error("missesShareTarget")]
    MissesShareTarget,
    #[error("busy")]
    Busy,
    #[error("soldOut")]
    SoldOut,
    #[error("insufficientCredit")]
    InsufficientCredit,
    #[error("insufficientFloat")]
    InsufficientFloat,
    #[error("unpriceable")]
    Unpriceable,
    #[error("draft: {0}")]
    Draft(String),
    #[error("noStorageRate")]
    NoStorageRate,
    #[error("noServingRate")]
    NoServingRate,
    #[error("servingNotCredited")]
    ServingNotCredited,
    #[error("overBudget")]
    OverBudget,
    #[error("unknownUpload")]
    UnknownUpload,
    #[error("notIncluded")]
    NotIncluded,
    #[error("notDeepEnough")]
    NotDeepEnough,
    #[error("notSigner")]
    NotSigner,
    #[error("invalidSignature")]
    InvalidSignature,
    #[error("noPayment")]
    NoPayment,
    #[error("alreadyCredited")]
    AlreadyCredited,
}
pub mod node;
pub mod payment;
pub mod serving;
pub mod storage;

pub mod service;

pub mod http;
