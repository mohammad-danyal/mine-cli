use std::{
    io::{stdout, Write},
    sync::{atomic::AtomicBool, Arc, Mutex},
    thread,
};

use ore::{self, state::Bus, BUS_ADDRESSES, BUS_COUNT, EPOCH_DURATION, TOKEN_DECIMALS};
use rand::Rng;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    keccak::{hashv, Hash as KeccakHash},
    signature::Signer,
    transaction::Transaction,
    pubkey::Pubkey,
    system_instruction,
};
use spl_associated_token_account::get_associated_token_address;

use crate::{
    cu_limits::{CU_LIMIT_MINE, CU_LIMIT_RESET},
    utils::{get_clock_account, get_proof, get_treasury},
    Miner,
};

// Define the tip accounts as constants in your module
const TIP_ACCOUNTS: &[Pubkey] = &[
    Pubkey::from_str("96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5").unwrap(),
    Pubkey::from_str("HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe").unwrap(),
    Pubkey::from_str("Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY").unwrap(),
    Pubkey::from_str("ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49").unwrap(),
    Pubkey::from_str("DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh").unwrap(),
    Pubkey::from_str("ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt").unwrap(),
    Pubkey::from_str("DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL").unwrap(),
    Pubkey::from_str("3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT").unwrap(),
];

// Odds of being selected to submit a reset tx
const RESET_ODDS: u64 = 20;

impl Miner {
    pub async fn mine(&self, threads: u64) {
        let signer = self.signer();
        self.register().await;
        let mut stdout = stdout();
        let mut rng = rand::thread_rng();

        loop {
            let balance = self.get_ore_display_balance().await;
            let treasury = get_treasury(self.cluster.clone()).await;
            let proof = get_proof(self.cluster.clone(), signer.pubkey()).await;
            let rewards = (proof.claimable_rewards as f64) / (10f64.powf(TOKEN_DECIMALS as f64));
            let reward_rate = (treasury.reward_rate as f64) / (10f64.powf(TOKEN_DECIMALS as f64));

            stdout.write_all(b"\x1b[2J\x1b[3J\x1b[H").ok();
            println!("Balance: {} ORE", balance);
            println!("Claimable: {} ORE", rewards);
            println!("Reward rate: {} ORE", reward_rate);

            println!("\nMining for a valid hash...");
            let (next_hash, nonce) = self.find_next_hash_par(proof.hash.into(), treasury.difficulty.into(), threads);

            println!("\n\nSubmitting hash for validation...");
            loop {
                let treasury = get_treasury(self.cluster.clone()).await;
                let clock = get_clock_account(self.cluster.clone()).await;
                let threshold = treasury.last_reset_at.saturating_add(EPOCH_DURATION);
                if clock.unix_timestamp >= threshold {
                    if rng.gen_range(0..RESET_ODDS) == 0 {
                        println!("Sending epoch reset transaction...");
                        self.send_reset_transaction(&signer).await.ok();
                    }
                }

                let bus = self.find_bus_id(treasury.reward_rate).await;
                println!("Sending on bus {} ({} ORE)", bus.id, bus.rewards);
                if let Ok(sig) = self.send_mining_transaction(&signer, &next_hash, nonce, bus.id).await {
                    println!("Success: {}", sig);
                    break;
                } else {
                    println!("Error submitting mining transaction");
                }
            }
        }
    }

    async fn send_reset_transaction(&self, signer: &dyn Signer) -> Result<(), Box<dyn std::error::Error>> {
        let reset_ix = ore::instruction::reset(signer.pubkey());
        let cu_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(CU_LIMIT_RESET);
        let cu_price_ix = ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee);
        let reset_tx = Transaction::new_signed_with_payer(
            &[cu_limit_ix, cu_price_ix, reset_ix],
            Some(&signer.pubkey()),
            &[signer],
            self.last_blockhash(),
        );
        self.rpc_client.send_and_confirm_transaction(&reset_tx).await.map_err(Into::into)
    }

    async fn send_mining_transaction(&self, signer: &dyn Signer, hash: &KeccakHash, nonce: u64, bus_id: usize) -> Result<String, Box<dyn std::error::Error>> {
        let cu_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(CU_LIMIT_MINE);
        let cu_price_ix = ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee);
        let mine_ix = ore::instruction::mine(
            signer.pubkey(),
            BUS_ADDRESSES[bus_id],
            hash.clone().into(),
            nonce,
        );
        let tip_ix = system_instruction::transfer(&signer.pubkey(), &TIP_ACCOUNTS, &10_000 /* assume 1 SOL tip amount in lamports */);

        // Bundle all instructions into one transaction
        let tx = Transaction::new_signed_with_payer(
            &[cu_limit_ix, cu_price_ix, mine_ix, tip_ix],
            Some(&signer.pubkey()),
            &[signer],
            self.last_blockhash(),
        );

        // Send the transaction and handle the result
        self.rpc_client.send_and_confirm_transaction(&tx).await.map_err(Into::into)
    }

    // Helper function to get the last valid blockhash
    fn last_blockhash(&self) -> KeccakHash {
        // This function would normally fetch the last valid blockhash from the network
        KeccakHash::new()
    }
}