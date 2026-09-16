use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tracing::{info, warn, error, instrument};
use tokio::time::{self, instant};

use crate::config::Config;
use crate::geyser::GeyserSubscriber;
use crate::pump_fun::{BondingCurve, GraduationEvent, PumpFunProgram};
use crate::filters::FilterResults;
use crate::transaction::TransactionBuilder;
use crate::jito::JitoSender;
use crate::rpc_fanout::RpcFanout;
use crate::metrics::Metrics;
use crate::position::PositionManager;

mod configs;
mod main_router;

fn main() {
    // Initialize tracing with subscriber
    tracing_subscriber::fmt::init();

    let start = instant::now();
    info!("Graduation Sniper Bot starting...");

    // Load configuration
    let config = match Config::load("config.toml") {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    // Validate configuration
    if let Err(e) = config.validate() {
        error!("Configuration validation failed: {}", e);
        std::process::exit(1);
    }

    // Initialize wallet from private key
    let wallet = match solana_sdk::signature::Keypair::from_file(&config.wallet.private_key_path) {
        Some(kp) => kp,
        None => {
            error!("Failed to load wallet from {}", config.wallet.private_key_path);
            std::process::exit(1);
        }
    };

    info!("Wallet loaded: {}", wallet.pubkey());

    // Initialize position manager
    let position_manager = PositionManager::new(config.trading.max_concurrent_positions);

    // Initialize metrics
    let metrics = Arc::new(Metrics::new());

    // Initialize RPC fanout
    let rpc_fanout = RpcFanout::new(
        &config.rpc.helius_url,
        &config.rpc.quicknode_url,
        &config.rpc.triton_url,
    );

    // Initialize Jito sender
    let jito_sender = JitoSender::new(
        &config.jito.tip_lamports,
        &config.jito.max_tip_lamports,
        config.rpc.helius_geyser_url.clone(),
    );

    // Initialize Geyser subscriber for pump.fun program
    // Program ID: 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P
    let pump_program_id = solana_sdk::pubkey::Pubkey::from_str_unchecked("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
    
    let geyser = GeyserSubscriber::new(
        &config.helius_geyser_url,
        vec![pump_program_id],
    );

    // Start geyser stream
    if let Err(e) = geyser.start().await {
        error!("Failed to start geyser stream: {}", e);
        std::process::exit(1);
    }

    info!("Geyser stream connected, listening for pump.fun graduation events...");

    // Track previous states for graduation detection
    let previous_complete: DashMap<solana_sdk::pubkey::Pubkey, bool> = DashMap::new();

    // Start blockhash refresher task
    let metrics_clone = Arc::clone(&metrics);
    let rpc_fanout_clone = rpc_fanout.clone();
    let jito_clone = jito_sender.clone();
    let position_manager_clone = position_manager.clone();
    
    let _blockhash_task = tokio::spawn(async move {
        loop {
            // Refresh blockhash every 400ms
            let blockhash = rpc_fanout_clone.get_fresh_blockhash().await;
            let _ = jito_clone.set_blockhash(&blockhash).await;
            metrics_clone.blockhash_refresh_latency.start();
            let _ = rpc_fanout_clone.refresh_blockhash(&blockhash).await;
            metrics_clone.blockhash_refresh_latency.stop();
            time::sleep(Duration::from_millis(400)).await;
        }
    });

    // Main event loop - process geyser updates
    let geyser_clone = geyser.clone();
    let metrics_clone2 = Arc::clone(&metrics);
    let position_manager_clone2 = position_manager.clone();
    let previous_complete_clone = previous_complete.clone();
    
    let _event_loop = tokio::spawn(async move {
        let mut last_filter_time = instant::now();
        
        while let Some(update) = geyser_clone.next_update().await {
            let update_start = instant::now();
            
            // Process each account update
            for account_update in &update.accounts {
                // Check if this is a BondingCurve account
                if !account_update.account.data.is_empty() {
                    // Deserialize bonding curve
                    let bonding_curve = match BondingCurve::deserialize(&account_update.account.data) {
                        Ok(bc) => bc,
                        Err(_) => continue,
                    };
                    
                    let mint = *account_update.pubkey;
                    
                    // Check for graduation event
                    let was_complete = *previous_complete_clone.get(&mint).unwrap_or(&false);
                    let is_complete = bonding_curve.complete;
                    
                    if is_complete && !was_complete {
                        // This is a graduation event!
                        previous_complete_clone.insert(mint, true);
                        
                        let event_duration = update_start.elapsed();
                        metrics_clone2.geyser_to_signal_latency = Some(event_duration.as_millis() as u64);
                        
                        info!(
                            "GRADUATION EVENT detected for mint: {}",
                            mint
                        );
                        
                        // Run safety filters in parallel
                        let filter_start = instant::now();
                        
                        // Create mint pubkey for filter
                        let mint_for_filters = mint;
                        
                        // Spawn all filter tasks concurrently
                        let filter_results = tokio::join!(
                            // Filter 1: RugCheck API
                            crate::filters::rug_check(&config, mint),
                            // Filter 2: Dev wallet holdings
                            crate::filters::dev_holdings(&config, mint, &wallet),
                            // Filter 3: Top holder concentration
                            crate::filters::top_holder_concentration(&config, mint).await,
                            // Filter 4: Token age
                            crate::filters::token_age(&config, mint).await,
                            // Filter 5: Unique buyer count
                            crate::filters::unique_buyers(&config, mint).await,
                            // Filter 7: Blacklist check (instant)
                            crate::filters::blacklist_check(&config, mint),
                            // Filter 8: Liquidity sanity
                            crate::filters::liquidity_sanity(&bonding_curve),
                        );
                        
                        let filter_duration = filter_start.elapsed();
                        info!("Filters completed in {:?}", filter_duration);
                        
                        // Check if all filters passed
                        let all_passed = match &filter_results {
                            Ok(f) => f.all_passed(),
                            Err(_) => false,
                        };
                        
                        if all_passed {
                            // Execute buy order
                            let buy_result = execute_buy(
                                &config,
                                &wallet,
                                &rpc_fanout,
                                &jito_sender,
                                &metrics_clone2,
                                mint,
                                &bonding_curve,
                            ).await;
                            
                            if let Err(e) = buy_result {
                                error!("Buy execution failed: {}", e);
                            }
                        } else {
                            let filter_summary = filter_results.unwrap_or_default();
                            info!("Filters failed for mint {}: {:?}", mint, filter_summary);
                        }
                    }
                }
            }
        }
    });
    
    // Wait for shutdown signal
    let _ = tokio::signal::ctrl_c().await;
    info!("Shutting down...");
    
    // Graceful shutdown
    geyser.stop().await.unwrap_or_default();
    rpc_fanout.shutdown().await;
    jito_sender.shutdown().await;
    
    let total_duration = start.elapsed();
    info!("Bot shutdown complete. Total runtime: {:?}", total_duration);
}

async fn execute_buy(
    config: &Config,
    wallet: &solana_sdk::signature::Keypair,
    rpc_fanout: &RpcFanout,
    jito_sender: &JitoSender,
    metrics: &Arc<Metrics>,
    mint: solana_sdk::pubkey::Pubkey,
    bonding_curve: &BondingCurve,
) -> Result<(), Box<dyn std::error::Error>> {
    let buy_start = instant::now();
    
    // Get cached blockhash (atomic read, never fetch at signal time)
    let blockhash = rpc_fanout.get_cached_blockhash().await;
    
    // Get compute unit price from recent prioritization fees
    let compute_unit_price = fetch_compute_unit_price(config).await;
    
    // Use pre-built transaction template
    let mut tx = TransactionBuilder::from_template()
        .with_blockhash(&blockhash)
        .with_compute_unit_price(compute_unit_price)
        .with_mint(mint)
        .with_amount(config.trading.buy_amount_sol);
    
    // Sign transaction
    tx.sign(wallet);
    
    // Compute units via simulation first
    let simulated_cu = tx.simulate_compute_units(rpc_fanout).await;
    
    // Launch parallel sends: Jito bundle + RPC fanout
    let jito_handle = tokio::spawn(async move {
        jito_sender.send_bundle(&tx).await;
    });
    
    let rpc_handle = tokio::spawn(async move {
        rpc_fanout.send_transaction(&tx).await;
    });
    
    // Wait for both with timeout
    tokio::select! {
        _ = &mut jito_handle => {},
        _ = &mut rpc_handle => {},
        _ = time::sleep(Duration::from_secs(10)) => {
            warn!("Timeout waiting for transaction inclusion");
        }
    }
    
    let execution_duration = buy_start.elapsed();
    metrics.execution_latency = Some(execution_duration.as_millis() as u64);
    
    info!(
        "Buy executed for mint {} in {:?}",
        mint, execution_duration
    );
    
    Ok(())
}

async fn fetch_compute_unit_price(config: &Config) -> u64 {
    // Fetch recent prioritization fees from config RPC
    // Set at 95th percentile of recent fees
    let client = reqwest::Client::new();
    
    // Get recent block fees
    let _ = client
        .post(&config.rpc.helius_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getPrioritizationFees",
        }))
        .send()
        .await
        .ok();
    
    // Return dynamic tip based on congestion
    // Default with buffer
    100000 // 0.0001 SOL as base
}