#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
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
