use std::{
    io::{stdout, Write},
    time::Duration,
};

use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    nonblocking::rpc_client::RpcClient,
    rpc_config::{RpcSendTransactionConfig, RpcSimulateTransactionConfig},
};
use solana_program::instruction::Instruction;
use solana_sdk::{
    commitment_config::{CommitmentConfig, CommitmentLevel},
    compute_budget::ComputeBudgetInstruction,
    signature::{Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::{TransactionConfirmationStatus, UiTransactionEncoding};

use crate::Miner;

const RPC_RETRIES: usize = 0;
const SIMULATION_RETRIES: usize = 4;
const GATEWAY_RETRIES: usize = 4;
const CONFIRM_RETRIES: usize = 4;

impl Miner {
    pub async fn send_and_confirm(
        &self,
        ixs: &[Instruction],
        dynamic_cus: bool,
        skip_confirm: bool,
    ) -> ClientResult<Signature> {
        let mut stdout = stdout();
        let signer = self.signer();
        let client = RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::confirmed());

        // Check the signer's balance before attempting to send the transaction
        let balance = client
            .get_balance_with_commitment(&signer.pubkey(), CommitmentConfig::confirmed())
            .await?;
        if balance.value <= 0 {
            return Err(ClientError {
                request: None,
                kind: ClientErrorKind::Custom("Insufficient SOL balance".into()),
            });
        }

        // Prepare the transaction
        let (mut hash, mut slot) = client
            .get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
            .await?;
        let mut send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(RPC_RETRIES),
            min_context_slot: Some(slot),
        };
        let mut tx = Transaction::new_with_payer(ixs, Some(&signer.pubkey()));

        // Optionally simulate the transaction
        if dynamic_cus {
            simulate_transaction(&client, &mut tx, &mut sim_attempts).await?;
        }

        // Submit the transaction and handle retries
        submit_transaction(&client, &mut tx, &mut send_cfg, &mut sigs, &mut attempts, skip_confirm).await
    }
}

async fn simulate_transaction(client: &RpcClient, tx: &mut Transaction, sim_attempts: &mut usize) -> ClientResult<()> {
    while *sim_attempts < SIMULATION_RETRIES {
        let sim_res = client
            .simulate_transaction_with_config(
                tx,
                RpcSimulateTransactionConfig {
                    sig_verify: false,
                    replace_recent_blockhash: true,
                    commitment: Some(CommitmentConfig::confirmed()),
                    encoding: Some(UiTransactionEncoding::Base64),
                    accounts: None,
                    min_context_slot: None,
                    inner_instructions: false,
                },
            )
            .await;

        match sim_res {
            Ok(sim_res) if sim_res.value.err.is_none() => {
                if let Some(units_consumed) = sim_res.value.units_consumed {
                    let cu_budget_ix = ComputeBudgetInstruction::set_compute_unit_limit(units_consumed as u32 + 1000);
                    tx.message.instructions.insert(0, cu_budget_ix);
                    return Ok(());
                }
            },
            Ok(sim_res) => {
                println!("Simulation error: {:?}", sim_res.value.err);
                *sim_attempts += 1;
            },
            Err(e) => {
                println!("Simulation error: {:?}", e);
                *sim_attempts += 1;
                if *sim_attempts >= SIMULATION_RETRIES {
                    return Err(ClientError {
                        request: None,
                        kind: ClientErrorKind::Custom("Simulation repeatedly failed".into()),
                    });
                }
            }
        }
    }
    Ok(())
}

async fn simulate_transaction(client: &RpcClient, tx: &mut Transaction, sim_attempts: &mut usize) -> ClientResult<()> {
    while *sim_attempts < SIMULATION_RETRIES {
        let sim_res = client
            .simulate_transaction_with_config(
                tx,
                RpcSimulateTransactionConfig {
                    sig_verify: false,
                    replace_recent_blockhash: true,
                    commitment: Some(CommitmentConfig::confirmed()),
                    encoding: Some(UiTransactionEncoding::Base64),
                    accounts: None,
                    min_context_slot: None,
                    inner_instructions: false,
                },
            )
            .await;

        match sim_res {
            Ok(sim_res) if sim_res.value.err.is_none() => {
                if let Some(units_consumed) = sim_res.value.units_consumed {
                    let cu_budget_ix = ComputeBudgetInstruction::set_compute_unit_limit(units_consumed as u32 + 1000);
                    tx.message.instructions.insert(0, cu_budget_ix);
                    return Ok(());
                }
            },
            Ok(sim_res) => {
                println!("Simulation error: {:?}", sim_res.value.err);
                *sim_attempts += 1;
            },
            Err(e) => {
                println!("Simulation error: {:?}", e);
                *sim_attempts += 1;
                if *sim_attempts >= SIMULATION_RETRIES {
                    return Err(ClientError {
                        request: None,
                        kind: ClientErrorKind::Custom("Simulation repeatedly failed".into()),
                    });
                }
            }
        }
    }
    Ok(())
}

async fn submit_transaction(client: &RpcClient, tx: &Transaction, send_cfg: &RpcSendTransactionConfig, skip_confirm: bool) -> ClientResult<Signature> {
    let mut attempts = 0;
    while attempts < GATEWAY_RETRIES {
        let response = client.send_transaction_with_config(tx, send_cfg.clone()).await;
        match response {
            Ok(sig) => {
                println!("Transaction sent with signature: {:?}", sig);
                if skip_confirm {
                    return Ok(sig);
                } else {
                    return confirm_transaction(client, &sig).await;
                }
            },
            Err(e) => {
                println!("Error sending transaction: {:?}", e);
                attempts += 1;
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        }
    }
    Err(ClientError {
        request: None,
        kind: ClientErrorKind::Custom("Exceeded maximum retries for sending transaction".into()),
    })
}

async fn confirm_transaction(client: &RpcClient, signature: &Signature) -> ClientResult<Signature> {
    let mut attempts = 0;
    while attempts < CONFIRM_RETRIES {
        thread::sleep(Duration::from_secs(2));
        let status = client.get_signature_statuses(&[signature.clone()]).await?;
        if let Some(status) = status.value.first().flatten() {
            match status.confirmation_status {
                Some(TransactionConfirmationStatus::Confirmed) |
                Some(TransactionConfirmationStatus::Finalized) => {
                    println!("Transaction confirmed!");
                    return Ok(*signature);
                },
                _ => {
                    attempts += 1;
                    continue;
                }
            }
        } else {
            println!("Transaction status not available");
            attempts += 1;
        }
    }
    Err(ClientError {
        request: None,
        kind: ClientErrorKind::Custom("Transaction confirmation failed after repeated attempts".into()),
    })
}