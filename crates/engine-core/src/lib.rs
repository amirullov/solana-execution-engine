#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionIntent {
    pub id: Uuid,
    pub idempotency_key: String,
    pub input_mint: String,
    pub output_mint: String,
    pub amount_atomic: u64,
    pub max_slippage_bps: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentState {
    Received,
    Validated,
    Simulated,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Commitment {
    Processed,
    Confirmed,
    Finalized,
    Dead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotObservation {
    pub slot: u64,
    pub blockhash: String,
    pub parent_slot: Option<u64>,
    pub commitment: Commitment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerAction {
    Inserted,
    Duplicate,
    CommitmentAdvanced,
    GapDetected { parent_slot: u64 },
    ReorgObserved { old_hash: String, new_hash: String },
    SlotOrphaned { blockhash: String },
    FinalizedConflict { old_hash: String, new_hash: String },
}

#[derive(Debug, Default)]
pub struct SlotLedger {
    canonical: BTreeMap<u64, SlotObservation>,
    finalized_slot: Option<u64>,
}

impl SlotLedger {
    /// Records an idempotent observation and detects parent gaps or conflicting forks.
    pub fn observe(&mut self, observation: SlotObservation) -> LedgerAction {
        if observation.commitment == Commitment::Dead {
            if self
                .canonical
                .get(&observation.slot)
                .is_some_and(|existing| existing.blockhash == observation.blockhash)
            {
                self.canonical.remove(&observation.slot);
                return LedgerAction::SlotOrphaned {
                    blockhash: observation.blockhash,
                };
            }
            return LedgerAction::Duplicate;
        }

        if let Some(existing) = self.canonical.get_mut(&observation.slot) {
            if existing.blockhash == observation.blockhash {
                if commitment_rank(observation.commitment) <= commitment_rank(existing.commitment) {
                    return LedgerAction::Duplicate;
                }
                existing.commitment = observation.commitment;
                self.advance_finalized_checkpoint(observation.slot, observation.commitment);
                return LedgerAction::CommitmentAdvanced;
            }

            if existing.commitment == Commitment::Finalized {
                return LedgerAction::FinalizedConflict {
                    old_hash: existing.blockhash.clone(),
                    new_hash: observation.blockhash,
                };
            }

            let old_hash = existing.blockhash.clone();
            if commitment_rank(observation.commitment) >= commitment_rank(existing.commitment) {
                *existing = observation.clone();
                self.advance_finalized_checkpoint(observation.slot, observation.commitment);
            }
            return LedgerAction::ReorgObserved {
                old_hash,
                new_hash: observation.blockhash,
            };
        }

        let missing_parent = observation
            .parent_slot
            .filter(|parent| *parent > 0 && !self.canonical.contains_key(parent));
        let slot = observation.slot;
        let commitment = observation.commitment;
        self.canonical.insert(slot, observation);
        self.advance_finalized_checkpoint(slot, commitment);
        missing_parent.map_or(LedgerAction::Inserted, |parent_slot| {
            LedgerAction::GapDetected { parent_slot }
        })
    }

    #[must_use]
    pub const fn finalized_slot(&self) -> Option<u64> {
        self.finalized_slot
    }

    fn advance_finalized_checkpoint(&mut self, slot: u64, commitment: Commitment) {
        if commitment == Commitment::Finalized {
            self.finalized_slot = Some(
                self.finalized_slot
                    .map_or(slot, |current| current.max(slot)),
            );
        }
    }
}

const fn commitment_rank(commitment: Commitment) -> u8 {
    match commitment {
        Commitment::Processed | Commitment::Dead => 0,
        Commitment::Confirmed => 1,
        Commitment::Finalized => 2,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPolicy {
    pub mode: ExecutionMode,
    pub max_slippage_bps: u16,
    pub max_amount_atomic: u64,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            mode: ExecutionMode::Disabled,
            max_slippage_bps: 100,
            max_amount_atomic: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PolicyDenial {
    #[error("transaction execution is disabled in Foundation")]
    ExecutionDisabled,
    #[error("amount must be greater than zero")]
    InvalidAmount,
    #[error("requested slippage exceeds policy")]
    SlippageExceeded,
}

impl ExecutionPolicy {
    /// Foundation is fail-closed: even a valid intent cannot reach signing or broadcast.
    ///
    /// # Errors
    ///
    /// Returns a policy denial when the intent violates a limit or when execution is disabled.
    pub fn evaluate(&self, intent: &ExecutionIntent) -> Result<(), PolicyDenial> {
        if intent.amount_atomic == 0 {
            return Err(PolicyDenial::InvalidAmount);
        }
        if intent.max_slippage_bps > self.max_slippage_bps {
            return Err(PolicyDenial::SlippageExceeded);
        }
        match self.mode {
            ExecutionMode::Disabled => Err(PolicyDenial::ExecutionDisabled),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_intent() -> ExecutionIntent {
        ExecutionIntent {
            id: Uuid::nil(),
            idempotency_key: "test-1".to_owned(),
            input_mint: "So11111111111111111111111111111111111111112".to_owned(),
            output_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_owned(),
            amount_atomic: 1_000_000,
            max_slippage_bps: 50,
        }
    }

    fn slot(slot: u64, hash: &str, parent: u64, commitment: Commitment) -> SlotObservation {
        SlotObservation {
            slot,
            blockhash: hash.to_owned(),
            parent_slot: Some(parent),
            commitment,
        }
    }

    #[test]
    fn duplicate_stream_replay_is_idempotent() {
        let mut ledger = SlotLedger::default();
        let observation = slot(10, "a", 9, Commitment::Confirmed);
        assert_eq!(
            ledger.observe(observation.clone()),
            LedgerAction::GapDetected { parent_slot: 9 }
        );
        assert_eq!(ledger.observe(observation), LedgerAction::Duplicate);
    }

    #[test]
    fn commitment_advances_without_duplicate_business_fact() {
        let mut ledger = SlotLedger::default();
        ledger.observe(slot(9, "parent", 8, Commitment::Confirmed));
        ledger.observe(slot(10, "block", 9, Commitment::Confirmed));
        assert_eq!(
            ledger.observe(slot(10, "block", 9, Commitment::Finalized)),
            LedgerAction::CommitmentAdvanced
        );
        assert_eq!(ledger.finalized_slot(), Some(10));
    }

    #[test]
    fn conflicting_hash_is_recorded_as_reorg_and_not_silently_overwritten_at_finality() {
        let mut ledger = SlotLedger::default();
        ledger.observe(slot(9, "parent", 8, Commitment::Confirmed));
        ledger.observe(slot(10, "old", 9, Commitment::Confirmed));
        assert_eq!(
            ledger.observe(slot(10, "new", 9, Commitment::Confirmed)),
            LedgerAction::ReorgObserved {
                old_hash: "old".to_owned(),
                new_hash: "new".to_owned()
            }
        );
        ledger.observe(slot(10, "new", 9, Commitment::Finalized));
        assert_eq!(
            ledger.observe(slot(10, "impossible", 9, Commitment::Confirmed)),
            LedgerAction::FinalizedConflict {
                old_hash: "new".to_owned(),
                new_hash: "impossible".to_owned()
            }
        );
    }

    #[test]
    fn default_policy_denies_execution() {
        assert_eq!(
            ExecutionPolicy::default().evaluate(&valid_intent()),
            Err(PolicyDenial::ExecutionDisabled)
        );
    }

    #[test]
    fn invalid_amount_is_rejected_before_execution_gate() {
        let mut intent = valid_intent();
        intent.amount_atomic = 0;
        assert_eq!(
            ExecutionPolicy::default().evaluate(&intent),
            Err(PolicyDenial::InvalidAmount)
        );
    }

    #[test]
    fn excessive_slippage_is_rejected_before_execution_gate() {
        let mut intent = valid_intent();
        intent.max_slippage_bps = 101;
        assert_eq!(
            ExecutionPolicy::default().evaluate(&intent),
            Err(PolicyDenial::SlippageExceeded)
        );
    }
}
