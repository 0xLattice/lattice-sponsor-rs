use crate::*;
use std::collections::BTreeMap;
#[derive(Debug, Clone)]
pub struct ServingCredit {
    pub rates: BTreeMap<Vec<String>, i64>,
}
impl ServingCredit {
    pub fn rate(&self, path: &[String]) -> Result<i64, Refusal> {
        self.rates
            .get(path)
            .copied()
            .filter(|r| *r > 0)
            .ok_or(Refusal::NoServingRate)
    }
    pub fn bytes(&self, value: UInt256, path: &[String]) -> Result<i64, Refusal> {
        let product = big(value) * self.rate(path)? as u64;
        if product.bits() > 256 {
            return Ok(i64::MAX);
        }
        Ok((product / unit())
            .min(BigUint::from(i64::MAX as u64))
            .to_u64_digits()
            .first()
            .copied()
            .unwrap_or(0) as i64)
    }
    pub fn check(peer: Option<&str>, store: Option<&str>) -> Result<(), Refusal> {
        let Some(peer) = peer else {
            return Ok(());
        };
        if store.is_some() {
            return Err(Refusal::Draft("peer and store are exclusive".into()));
        }
        if peer.len() != 64
            || !peer
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Refusal::Draft("peer is not 64 lowercase hex".into()));
        }
        Ok(())
    }
}
