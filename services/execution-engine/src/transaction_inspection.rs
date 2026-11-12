use agave_transaction_view::{
    transaction_version::TransactionVersion, transaction_view::TransactionView,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectionError {
    InvalidTransaction,
    AddressLookupTablesUnresolved,
    V1ResourceLimitsMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionInspection {
    pub format: &'static str,
    pub static_account_keys: Vec<String>,
    pub instructions: Vec<InstructionInspection>,
    pub address_lookup_table_count: u8,
    pub v1_resource_limits: Option<V1ResourceLimits>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct V1ResourceLimits {
    pub compute_unit_limit: u32,
    pub loaded_accounts_data_size_limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionInspection {
    pub program_id: String,
    pub accounts: Vec<String>,
    pub data_base64: String,
}

pub fn inspect_transaction(bytes: &[u8]) -> Result<TransactionInspection, InspectionError> {
    let view = TransactionView::<true, Vec<u8>>::try_new_sanitized(bytes.to_vec())
        .map_err(|_| InspectionError::InvalidTransaction)?;
    let address_lookup_table_count = view.num_address_table_lookups();
    if address_lookup_table_count != 0 {
        return Err(InspectionError::AddressLookupTablesUnresolved);
    }
    let v1_resource_limits = match view.version() {
        TransactionVersion::V1 => {
            let config = view
                .transaction_config()
                .ok_or(InspectionError::V1ResourceLimitsMissing)?;
            Some(V1ResourceLimits {
                compute_unit_limit: config
                    .compute_unit_limit()
                    .filter(|limit| *limit > 0)
                    .ok_or(InspectionError::V1ResourceLimitsMissing)?,
                loaded_accounts_data_size_limit: config
                    .loaded_accounts_data_size_limit()
                    .filter(|limit| *limit > 0)
                    .ok_or(InspectionError::V1ResourceLimitsMissing)?,
            })
        }
        TransactionVersion::Legacy | TransactionVersion::V0 => None,
    };

    let static_account_keys = view
        .static_account_keys()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let mut instructions = Vec::with_capacity(usize::from(view.num_instructions()));
    for (program_id, instruction) in view.program_instructions_iter() {
        let accounts = instruction
            .accounts
            .iter()
            .map(|index| {
                static_account_keys
                    .get(usize::from(*index))
                    .cloned()
                    .ok_or(InspectionError::InvalidTransaction)
            })
            .collect::<Result<Vec<_>, _>>()?;
        instructions.push(InstructionInspection {
            program_id: program_id.to_string(),
            accounts,
            data_base64: STANDARD.encode(instruction.data),
        });
    }

    Ok(TransactionInspection {
        format: match view.version() {
            TransactionVersion::Legacy => "legacy",
            TransactionVersion::V0 => "v0",
            TransactionVersion::V1 => "v1",
        },
        static_account_keys,
        instructions,
        address_lookup_table_count,
        v1_resource_limits,
    })
}

#[cfg(test)]
mod tests {
    use super::{InspectionError, inspect_transaction};

    fn legacy_transaction() -> Vec<u8> {
        let mut transaction = vec![1]; // One signature (compact-u16).
        transaction.extend([0; 64]);
        transaction.extend([1, 0, 1]); // Header: one writable signer, one readonly unsigned key.
        transaction.push(2); // Two static account keys.
        transaction.extend([0; 64]);
        transaction.extend([0; 32]); // Recent blockhash.
        transaction.push(1); // One compiled instruction.
        transaction.push(1); // Program id is static account key 1.
        transaction.push(1); // One account index.
        transaction.push(0); // Account index 0.
        transaction.push(2); // Two bytes of instruction data.
        transaction.extend([0xAA, 0x55]);
        transaction
    }

    fn v0_transaction_with_lookup() -> Vec<u8> {
        let mut transaction = vec![1]; // One signature (compact-u16).
        transaction.extend([0; 64]);
        transaction.push(0x80); // Versioned message, v0.
        transaction.extend([1, 0, 0]); // Header.
        transaction.push(1); // One static account key.
        transaction.extend([0; 32]);
        transaction.extend([0; 32]); // Recent blockhash.
        transaction.push(0); // No compiled instructions.
        transaction.push(1); // One address-table lookup.
        transaction.extend([0; 32]); // Lookup-table account key.
        transaction.extend([1, 0]); // One writable index: 0.
        transaction.extend([0]); // No readonly indices.
        transaction
    }

    fn v0_transaction_without_lookup() -> Vec<u8> {
        let mut transaction = vec![1];
        transaction.extend([0; 64]);
        transaction.push(0x80);
        transaction.extend([1, 0, 0]);
        transaction.push(1);
        transaction.extend([0; 32]);
        transaction.extend([0; 32]);
        transaction.extend([0, 0]); // No instructions and no lookup tables.
        transaction
    }

    fn v1_transaction_with_limits() -> Vec<u8> {
        let mut transaction = vec![0x81];
        transaction.extend([1, 0, 1]); // One writable signer and one readonly unsigned key.
        transaction.extend([0x0C, 0, 0, 0]); // Compute and loaded-account limits are present.
        transaction.extend([0; 32]); // Recent blockhash.
        transaction.push(1); // One instruction.
        transaction.push(2); // Two inline account addresses.
        transaction.extend([0; 64]);
        transaction.extend(200_000_u32.to_le_bytes());
        transaction.extend(65_536_u32.to_le_bytes());
        transaction.extend([1, 1, 2, 0]); // Program 1, one account, two data bytes.
        transaction.extend([0, 0xAA, 0x55]); // Account index 0, then instruction data.
        transaction.extend([0; 64]); // One signature at the end of a v1 transaction.
        transaction
    }

    fn v1_transaction_without_limits() -> Vec<u8> {
        let mut transaction = vec![0x81];
        transaction.extend([1, 0, 0]);
        transaction.extend([0; 4]); // Missing mandatory v1 resource limits.
        transaction.extend([0; 32]);
        transaction.extend([0, 1]); // No instructions; one inline address.
        transaction.extend([0; 32]);
        transaction.extend([0; 64]);
        transaction
    }

    #[test]
    fn parses_legacy_instruction_and_accounts() {
        let inspection = inspect_transaction(&legacy_transaction()).expect("valid legacy tx");
        assert_eq!(inspection.format, "legacy");
        assert_eq!(inspection.static_account_keys.len(), 2);
        assert_eq!(inspection.instructions.len(), 1);
        assert_eq!(
            inspection.instructions[0].accounts,
            vec![inspection.static_account_keys[0].clone()]
        );
        assert_eq!(inspection.instructions[0].data_base64, "qlU=");
        assert_eq!(inspection.address_lookup_table_count, 0);
    }

    #[test]
    fn rejects_malformed_transaction_bytes() {
        assert_eq!(
            inspect_transaction(&[1, 2, 3]),
            Err(InspectionError::InvalidTransaction)
        );
    }

    #[test]
    fn rejects_unknown_future_transaction_versions() {
        let mut transaction = vec![1];
        transaction.extend([0; 64]);
        transaction.push(0x82); // Version 2 is not supported by the pinned parser.
        assert_eq!(
            inspect_transaction(&transaction),
            Err(InspectionError::InvalidTransaction)
        );
    }

    #[test]
    fn refuses_to_simulate_unresolved_address_lookup_tables() {
        assert_eq!(
            inspect_transaction(&v0_transaction_with_lookup()),
            Err(InspectionError::AddressLookupTablesUnresolved)
        );
    }

    #[test]
    fn parses_v0_transactions_without_lookup_tables() {
        let inspection =
            inspect_transaction(&v0_transaction_without_lookup()).expect("valid v0 transaction");
        assert_eq!(inspection.format, "v0");
        assert_eq!(inspection.address_lookup_table_count, 0);
    }

    #[test]
    fn parses_v1_and_requires_explicit_resource_limits() {
        let inspection =
            inspect_transaction(&v1_transaction_with_limits()).expect("valid v1 transaction");
        assert_eq!(inspection.format, "v1");
        assert_eq!(
            inspection.v1_resource_limits,
            Some(super::V1ResourceLimits {
                compute_unit_limit: 200_000,
                loaded_accounts_data_size_limit: 65_536,
            })
        );
        assert_eq!(inspection.instructions.len(), 1);
        assert_eq!(
            inspection.instructions[0].accounts,
            vec![inspection.static_account_keys[0].clone()]
        );
        assert_eq!(
            inspection.instructions[0].program_id,
            inspection.static_account_keys[1]
        );
        assert_eq!(inspection.instructions[0].data_base64, "qlU=");
        assert_eq!(
            inspect_transaction(&v1_transaction_without_limits()),
            Err(InspectionError::V1ResourceLimitsMissing)
        );
    }
}
