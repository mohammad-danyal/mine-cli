use std::{
    io::{stdout, Write},
    sync::{atomic::AtomicBool, Arc, Mutex},
    thread,
};

use ore::{self, state::Bus, BUS_ADDRESSES, BUS_COUNT, EPOCH_DURATION};
use rand::Rng;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    keccak::{hashv, Hash as KeccakHash},
    pubkey::Pubkey,
    signature::{Signer, Keypair},
    system_instruction,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;

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
            stdout.write_all(b"\x1b[2J\x1b[3J\x1b[H").ok();

            let (next_hash, nonce) = self.find_next_hash_par(proof.hash.into(), treasury.difficulty.into(), threads);

            let mut instructions = vec![
                ComputeBudgetInstruction::set_compute_unit_limit(CU_LIMIT_MINE),
                ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee),
                ore::instruction::mine(signer.pubkey(), BUS_ADDRESSES[rng.gen_range(0..BUS_COUNT)], next_hash, nonce),
            ];

            // Add a tip transaction to the instructions
            let tip_pubkey = Pubkey::from_str(TIP_ACCOUNTS[rng.gen_range(0..TIP_ACCOUNTS.len())]).unwrap();
            instructions.push(system_instruction::transfer(&signer.pubkey(), &tip_pubkey, 1_000_000_000)); // Tip 1 SOL

            let transaction = Transaction::new_signed_with_payer(
                &instructions,
                Some(&signer.pubkey()),
                &[&signer],
                self.rpc_client.get_latest_blockhash().await.unwrap(),
            );

            match self.rpc_client.send_and_confirm_transaction(&transaction).await {
                Ok(signature) => println!("Transaction submitted successfully: {}", signature),
                Err(e) => println!("Failed to submit transaction: {}", e),
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
        (final_hash, final_nonce).unwrap();
        (final_hash, final_nonce)
    }

    pub async fn get_ore_display_balance(&self) -> String {
        let client = RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::confirmed());
        let signer = self.signer();
        let token_account_address = get_associated_token_address(&signer.pubkey(), &ore::MINT_ADDRESS);
        
        match client.get_token_account(&token_account_address).await {
            Ok(token_account) => token_account.token_amount.ui_amount_string,
            Err(_) => "Error fetching balance".to_string(),
        }
    }
}
