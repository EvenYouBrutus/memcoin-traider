use std::sync::Arc;
use std::time::Instant;

use tokio::time::{interval, IntervalTickError};
use tracing::{info, warn};

use crate::config::Config;
use crate::metrics::Metrics;
use crate::rpc_fanout::RpcFanout;

/// Background blockhash refresher
/// Refreshes blockhash every 400ms in background thread
/// Never fetches blockhash at signal time (already cached)
pub struct BlockhashRefresher {
    /// RPC fanout for fetching blockhashes
    rpc_fanout: Arc<RpcFanout>,
    /// Current blockhash (atomic pointer for lock-free access)
    blockhash: Arc<std::sync::Mutex<solana_sdk::hash::Hash>>,
    /// Metrics tracking
    metrics: Arc<Metrics>,
    /// Running flag
    running: Arc<tokio::sync::Mutex<bool>>,
    /// Tokio interval for ticks
    timer: Arc<interval>,
}

impl BlockhashRefresher {
    /// Create new blockhash refresher
    pub fn new(
        rpc_fanout: Arc<RpcFanout>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, IntervalTickError> {
        let timer = Arc::new(interval(std::time::Duration::from_millis(400)));
        
        Ok(BlockhashRefresher {
            rpc_fanout,
            blockhash: Arc::new(std::sync::Mutex::new(solana_sdk::hash::Hash::new_from_array([0u8; 32]))),
            metrics,
            running: Arc::new(tokio::sync::Mutex::new(true)),
            timer,
        })
    }
    
    /// Start the background blockhash refresh loop
    pub async fn start(self: Arc<Self>) {
        let running = Arc::clone(&self.running);
        let rpc_fanout = Arc::clone(&self.rpc_fanout);
        let blockhash = Arc::clone(&self.blockhash);
        let metrics = Arc::clone(&self.metrics);
        
        tokio::spawn(async move {
            loop {
                // Refresh blockhash every 400ms
                let fetch_start = Instant::now();
                
                // Fetch fresh blockhash from RPC fanout
                let new_blockhash = rpc_fanout.get_fresh_blockhash().await;
                
                // Update atomic blockhash
                let mut bh = blockhash.lock().unwrap();
                *bh = new_blockhash;
                
                let fetch_duration = fetch_start.elapsed();
                metrics.blockhash_refresh_latency = Some(fetch_duration.as_millis() as u64);
                
                // Log if refresh takes too long
                if fetch_duration.as_millis() > 400 {
                    warn!("Blockhash refresh took {:?} (target: 400ms)", fetch_duration);
                }
                
                // Wait for next tick - will be interrupted if stopped
                running.lock().await.wait_for().await;
            }
        });
    }
    
    /// Stop the refresher
    pub async fn stop(self: Arc<Self>) {
        *self.running.lock().await = false;
    }
    
    /// Get current blockhash (lock-free read)
    pub fn get_blockhash(&self) -> solana_sdk::hash::Hash {
        *self.blockhash.lock().unwrap()
    }
}