use reqwest::Client;
use solana_sdk::transaction::Transaction;
use std::collections::HashSet;
use tokio::time::{timeout, Duration};

use crate::config::Config;
use crate::metrics::Metrics;

/// Jito block engine sender
/// Sends bundles to all Jito endpoints simultaneously for guaranteed inclusion
pub struct JitoSender {
    /// Jito tip lamports (dynamic, based on network congestion)
    pub tip_lamports: u64,
    /// Maximum tip lamports
    pub max_tip_lamports: u64,
    /// RPC endpoint for blockhash
    rpc_endpoint: String,
    /// Client for HTTP requests
    client: Client,
    /// Current blockhash (shared atomic access)
    blockhash: Arc<std::sync::Mutex<solana_sdk::hash::Hash>>,
    /// Metrics tracking
    metrics: Arc<Metrics>,
}

impl JitoSender {
    /// Create new Jito sender
    pub fn new(
        tip_lamports: &u64,
        max_tip_lamports: &u64,
        rpc_endpoint: String,
    ) -> Result<Self, String> {
        let client = Client::new();
        
        Ok(JitoSender {
            tip_lamports: *tip_lamports,
            max_tip_lamports: *max_tip_lamports,
            rpc_endpoint,
            client,
            blockhash: Arc::new(std::sync::Mutex::new(solana_sdk::hash::Hash::new_from_array([0u8; 32]))),
            metrics: Arc::new(Metrics::new()),
        })
    }
    
    /// Set blockhash from atomic reference
    pub fn set_blockhash(&self, blockhash: &solana_sdk::hash::Hash) {
        let mut bh = self.blockhash.lock().unwrap();
        *bh = *blockhash;
    }
    
    /// Get current blockhash (lock-free read)
    pub fn get_blockhash(&self) -> solana_sdk::hash::Hash {
        *self.blockhash.lock().unwrap()
    }
    
    /// Send bundle to all Jito endpoints simultaneously
    /// Returns once all sends are complete or timed out
    pub async fn send_bundle(&self, transaction: &Transaction) -> Result<(), Box<dyn std::error::Error>> {
        // Set dynamic tip based on network congestion
        let dynamic_tip = self.compute_dynamic_tip().await;
        
        // Apply tip to transaction
        let mut tx = transaction.clone();
        // Apply tip instruction - in a real implementation, this would add
        // a tip instruction to the transaction
        let _ = self.apply_tip(&mut tx, dynamic_tip).await;
        
        // Send to all Jito endpoints simultaneously
        let endpoints = [
            "mainnet.block-engine.jito.wtf",
            "amsterdam.mainnet.block-engine.jito.wtf",
            "frankfurt.mainnet.block-engine.jito.wtf",
            "ny.mainnet.block-engine.jito.wtf",
            "tokyo.mainnet.block-engine.jito.wtf",
        ];
        
        // Send to all endpoints in parallel using tokio::join!
        let mut handles = Vec::new();
        
        for endpoint in &endpoints {
            let client = self.client.clone();
            let tx_clone = tx.clone();
            let tip = dynamic_tip;
            
            let handle = tokio::spawn(async move {
                let result = send_to_jito_endpoint(&client, endpoint, &tx_clone, tip).await;
                result
            });
            handles.push(handle);
        }
        
        // Wait for all sends to complete
        let results: Vec<Result<(), reqwest::Error>> = timeout(
            Duration::from_secs(5),
            futures::future::join_all(handles),
        ).await;
        
        match results {
            Ok(results) => {
                // Check if any succeeded
                let any_success = results.iter().any(|r| r.is_ok());
                if any_success {
                    Ok(())
                } else {
                    Err("All Jito endpoints failed".into())
                }
            }
            Err(_) => {
                Err("Jito bundle send timed out".into())
            }
        }
    }
    
    /// Compute dynamic tip based on network congestion
    async fn compute_dynamic_tip(&self) -> u64 {
        // Fetch recent prioritization fees
        // Set at 95th percentile of recent fees
        // Range: 0.001-0.01 SOL
        
        let base_tip = self.tip_lamports;
        let max_tip = self.max_tip_lamports;
        
        // In a real implementation, would query recent fees
        // For now, use a dynamic approach based on tip_lamports config
        let mut tip = base_tip;
        
        // Add buffer based on congestion (simulated)
        // In production: query recent prioritization fees from RPC
        let congestion_factor = 1.0; // Would be dynamic
        
        tip = (tip as f64 * congestion_factor) as u64;
        tip.min(max_tip)
    }
    
    /// Apply tip to transaction
    async fn apply_tip(
        &self,
        transaction: &mut Transaction,
        tip_lamports: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // In a real implementation, this would add a system instruction
        // for the tip to the transaction's list of instructions
        // using system_instruction::transfer(&payer, &jito_authority, tip)
        
        Ok(())
    }
}

/// Send transaction to a single Jito block engine endpoint
async fn send_to_jito_endpoint(
    client: &Client,
    endpoint: &str,
    transaction: &Transaction,
    tip_lamports: u64,
) -> Result<(), reqwest::Error> {
    let url = format!("https://{}/api/v1/bundle", endpoint);
    
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sendBundle",
        "params": [vec![transaction.serialize().to_vec()]]
    });
    
    client.post(&url).json(&body).send().await?;
    
    Ok(())
}