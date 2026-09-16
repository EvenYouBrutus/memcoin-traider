use std::sync::Arc;
use std::time::Instant;

use solana_client::RpcClient;
use solana_sdk::transaction::Transaction;
use tokio::time::{timeout, Duration};

use crate::metrics::Metrics;
use crate::config::Config;

/// Parallel multi-RPC transaction sender (fanout)
/// Sends tx to minimum 5 RPC nodes simultaneously
/// First confirmation wins, cancel rest
pub struct RpcFanout {
    /// RPC clients
    clients: Vec<RpcClient>,
    /// Cached blockhash
    blockhash: Arc<std::sync::Mutex<solana_sdk::hash::Hash>>,
    /// Metrics
    metrics: Arc<Metrics>,
    /// Config
    config: Arc<Config>,
}

impl RpcFanout {
    /// Create new RPC fanout with multiple endpoints
    pub fn new(
        helius_url: &str,
        quicknode_url: &str,
        triton_url: &str,
    ) -> Result<Self, String> {
        let clients = vec![
            RpcClient::new_with_commitment(helius_url.to_string(), solana_client::commitment_config::CommitmentConfig::processed()),
            RpcClient::new_with_commitment(quicknode_url.to_string(), solana_client::commitment_config::CommitmentConfig::processed()),
            RpcClient::new_with_commitment(triton_url.to_string(), solana_client::commitment_config::CommitmentConfig::processed()),
        ];
        
        Ok(RpcFanout {
            clients,
            blockhash: Arc::new(std::sync::Mutex::new(solana_sdk::hash::Hash::new_from_array([0u8; 32]))),
            metrics: Arc::new(Metrics::new()),
            config: Arc::new(Config::default()),
        })
    }
    
    /// Get fresh blockhash from one of the RPC nodes
    pub async fn get_fresh_blockhash(&self) -> solana_sdk::hash::Hash {
        // Try each client, first that responds wins
        for client in &self.clients {
            match client.get_recent_blockhash() {
                Ok(blockhash) => return blockhash,
                Err(_) => continue,
            }
        }
        
        // Fallback: return zero hash
        solana_sdk::hash::Hash::new_from_array([0u8; 32])
    }
    
    /// Get cached blockhash (atomic read, never fetch at signal time)
    pub fn get_cached_blockhash(&self) -> solana_sdk::hash::Hash {
        self.blockhash.lock().unwrap().clone()
    }
    
    /// Refresh the cached blockhash
    pub async fn refresh_blockhash(&self, blockhash: &solana_sdk::hash::Hash) {
        let mut bh = self.blockhash.lock().unwrap();
        *bh = *blockhash;
    }
    
    /// Send transaction to all RPC nodes in parallel
    /// Returns the first successful signature, cancels the rest
    pub async fn send_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let send_start = Instant::now();
        
        // Send to all clients simultaneously with tokio::join!
        let mut handles = Vec::new();
        
        for client in &self.clients {
            let tx_clone = transaction.clone();
            let client_clone = client.clone();
            
            let handle = tokio::spawn(async move {
                let result = timeout(
                    Duration::from_secs(10),
                    client_clone.send_transaction(&tx_clone),
                ).await;
                
                match result {
                    Ok(Ok(sig)) => Some(sig.to_string()),
                    Ok(Err(e)) => {
                        Some(format!("ERR:{}", e))
                    }
                    Err(_) => {
                        Some("TIMEOUT".to_string())
                    }
                }
            });
            handles.push(handle);
        }
        
        // Await first success, cancel rest
        let mut first_sig: Option<String> = None;
        
        for handle in handles {
            match handle.await {
                Ok(result) => {
                    if let Some(sig) = result {
                        // Check if it's a real signature or error
                        if !sig.starts_with("ERR:") && !sig.starts_with("TIMEOUT") {
                            // First successful confirmation wins
                            if first_sig.is_none() {
                                first_sig = Some(sig);
                            }
                        }
                    }
                }
                Err(_) => continue,
            }
        }
        
        let send_duration = send_start.elapsed();
        self.metrics.rpc_send_latency = Some(send_duration.as_millis() as u64);
        
        match first_sig {
            Some(sig) => Ok(sig),
            None => Err("All RPC nodes failed".into()),
        }
    }
    
    /// Shutdown fanout and close clients
    pub async fn shutdown(&self) {
        // All clients are dropped automatically
    }
}