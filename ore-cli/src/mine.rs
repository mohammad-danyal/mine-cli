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
    pubkey::Pubkey,
    signature::{Signature, Signer},
    system_instruction,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;

// Tip accounts provided for Jito engine
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

            let cu_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(CU_LIMIT_MINE);
            let cu_price_ix = ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee);
            let mine_ix = ore::instruction::mine(
                signer.pubkey(),
                BUS_ADDRESSES[rng.gen_range(0..BUS_COUNT) as usize],
                next_hash,
                nonce,
            );

            // Tip transaction
            let tip_index = rng.gen_range(0..TIP_ACCOUNTS.len());
            let tip_tx = system_instruction::transfer(&signer.pubkey(), &TIP_ACCOUNTS[tip_index], 1_000_000_000); // 1 SOL in lamports

            // Create and send the transaction bundle
            let tx = Transaction::new_signed_with_payer(
                &[cu_limit_ix, cu_price_ix, mine_ix, tip_tx],
                Some(&signer.pubkey()),
                &[&signer],
                self.rpc_client.get_latest_blockhash().await.unwrap(),
            );

            match self.rpc_client.send_and_confirm_transaction(&tx).await {
                Ok(signature) => {
                    println!("Transaction submitted successfully: {}", signature);
                },
                Err(e) => {
                    println!("Failed to submit transaction: {}", e);
                }
            }
        }
    }

    async fn find_bus_id(&self, reward_rate: u64) -> Bus {
        let mut rng = rand::thread_rng();
        loop {
            let bus_id = rng.gen_range(0..BUS_COUNT);
            let bus = self.get_bus(bus_id).await;
            if let Ok(bus) = bus {
                if bus.rewards > reward_rate.saturating_mul(4) {
                    return bus;
                }
            }
        }
    }

    fn find_next_hash_par(
        &self,
        hash: KeccakHash,
        difficulty: KeccakHash,
        threads: u64,
    ) -> (KeccakHash, u64) {
        let found_solution = Arc::new(AtomicBool::new(false));
        let solution = Arc::new(Mutex::new((KeccakHash::new(), 0)));
        let pubkey = self.signer().pubkey();

        let thread_handles: Vec<_> = (0..threads).map(|_| {
            let found_solution = Arc::clone(&found_solution);
            let solution = Arc::clone(&solution);
            thread::spawn(move || {
                let mut nonce = 0;
                while !found_solution.load(std::sync::atomic::Ordering::Relaxed) {
                    let potential_hash = hashv(&[hash.as_ref(), pubkey.as_ref(), &nonce.to_le_bytes()]);
                    if potential_hash <= difficulty {
                        let mut sol = solution.lock().unwrap();
                        *sol = (potential_hash, nonce);
                        found_solution.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    nonce += 1;
                }
            })
        }).collect();

        for handle in thread_handles {
            handle.join().unwrap();
        }

        let (final_hash, final_nonce) = *solution.lock().unwrap();
        (final_hash, final_nonce)
    }
}