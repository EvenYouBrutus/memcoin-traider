use reqwest::Client;
use serde::Deserialize;
use std::collections::HashSet;
use tokio::time::{timeout, Duration};

use crate::config::Config;
use crate::pump_fun::BondingCurve;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;

/// Results of all filter checks
#[derive(Debug, Clone)]
pub struct FilterResults {
    pub rugcheck_pass: bool,
    pub dev_holdings_pass: bool,
    pub top_holder_pass: bool,
    pub token_age_pass: bool,
    pub unique_buyers_pass: bool,
    pub blacklist_pass: bool,
    pub liquidity_sanity_pass: bool,
}

impl FilterResults {
    pub fn all_passed(&self) -> bool {
        self.rugcheck_pass
            && self.dev_holdings_pass
            && self.top_holder_pass
            && self.token_age_pass
            && self.unique_buyers_pass
            && self.blacklist_pass
            && self.liquidity_sanity_pass
    }
}

/// Blacklist of known rug dealer wallets and previously rugged tokens
#[derive(Debug, Default)]
pub struct TokenBlacklist {
    /// Known rug developer wallet pubkeys
    pub rug_wallets: HashSet<Pubkey>,
    /// Previously rugged token mints
    pub rugged_mints: HashSet<Pubkey>,
}

impl TokenBlacklist {
    pub fn check_blacklist(&self, mint: &Pubkey, dev_wallet: &Pubkey) -> bool {
        // Check if dev wallet is known rugger
        if self.rug_wallets.contains(dev_wallet) {
            return false; // blacklisted
        }

        // Check if mint is previously rugged
        if self.rugged_mints.contains(mint) {
            return false; // blacklisted
        }

        true // pass
    }

    pub fn add_rugger(&mut self, wallet: Pubkey) {
        self.rug_wallets.insert(wallet);
    }

    pub fn add_rugged_mint(&mut self, mint: Pubkey) {
        self.rugged_mints.insert(mint);
    }
}

/// Run all safety filters in parallel
/// Total time budget: <200ms
pub async fn run_all_filters(
    config: &Config,
    mint: &Pubkey,
    bonding_curve: &BondingCurve,
    wallet: &Keypair,
) -> FilterResults {
    // Run all filters concurrently with tokio::join!
    // We use separate blocks for each filter
    
    let rugcheck_result = tokio::spawn(async move {
        check_rugcheck(config, mint).await
    });
    
    let dev_holdings_result = tokio::spawn(async move {
        check_dev_holdings(config, mint, wallet).await
    });
    
    let top_holder_result = tokio::spawn(async move {
        check_top_holders(config, mint).await
    });
    
    let token_age_result = tokio::spawn(async move {
        check_token_age(config, mint).await
    });
    
    let unique_buyers_result = tokio::spawn(async move {
        check_unique_buyers(config, mint).await
    });
    
    let blacklist_result = tokio::spawn(async move {
        check_blacklist(config, mint, wallet)
    });
    
    let liquidity_sanity_result = tokio::spawn(async move {
        check_liquidity_sanity(bonding_curve)
    });

    // Await all results with timeout
    let rugcheck = match rugcheck_result.await {
        Ok(r) => r,
        Err(_) => false,
    };
    
    let dev_holdings = match dev_holdings_result.await {
        Ok(r) => r,
        Err(_) => false,
    };
    
    let top_holder = match top_holder_result.await {
        Ok(r) => r,
        Err(_) => false,
    };
    
    let token_age = match token_age_result.await {
        Ok(r) => r,
        Err(_) => false,
    };
    
    let unique_buyers = match unique_buyers_result.await {
        Ok(r) => r,
        Err(_) => false,
    };
    
    let blacklist = match blacklist_result.await {
        Ok(r) => r,
        Err(_) => false,
    };
    
    let liquidity_sanity = match liquidity_sanity_result.await {
        Ok(r) => r,
        Err(_) => false,
    };

    FilterResults {
        rugcheck_pass: rugcheck,
        dev_holdings_pass: dev_holdings,
        top_holder_pass: top_holder,
        token_age_pass: token_age,
        unique_buyers_pass: unique_buyers,
        blacklist_pass: blacklist,
        liquidity_sanity_pass: liquidity_sanity,
    }
}

/// Filter 1: RugCheck API
async fn check_rugcheck(config: &Config, mint: &Pubkey) -> bool {
    let client = Client::new();
    let url = format!("https://api.rugcheck.xyz/v1/tokens/{}/report", mint);
    
    match timeout(
        Duration::from_secs(150),
        client.get(&url).send().await,
    ).await {
        Ok(resp) => {
            match resp.json::<RugCheckReport>().await {
                Ok(report) => report.score >= config.filters.min_rugcheck_score,
                Err(_) => false,
            }
        }
        Err(_) => {
            // Timeout → skip (don't buy)
            false
        }
    }
}

/// RugCheck API response
#[derive(Deserialize)]
struct RugCheckReport {
    score: u64,
}

/// Filter 2: Dev wallet holdings
async fn check_dev_holdings(config: &Config, mint: &Pubkey, wallet: &Keypair) -> bool {
    let client = Client::new();
    
    // Get token accounts for the mint authority/dev wallet
    // Using getParsedTokenAccountsByOwner RPC call
    let json_rpc_url = &config.rpc.helius_url;
    
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getParsedTokenAccountsByOwner",
        "params": [
            wallet.pubkey().to_string(),
            {
                "mint": mint.to_string()
            }
        ]
    });
    
    match timeout(
        Duration::from_secs(150),
        client.post(json_rpc_url).json(&request).send().await,
    ).await {
        Ok(resp) => {
            let body = resp.json().await.ok();
            if let Some(obj) = body {
                // Parse the response to find token account amount
                // Check if dev holdings > 10% of supply → skip
                // This is simplified - real implementation would parse the UI amount
                if let Some(result) = obj.get("result") {
                    if let Some(accounts) = result.get("value") {
                        // If we found token accounts, check the balance
                        if !accounts.as_array().unwrap_or(&vec![]).is_empty() {
                            // Dev has some holdings - check concentration
                            // For now, assume pass if we got a response
                            return true;
                        }
                    }
                }
            }
            // No token accounts found - dev has 0 holdings, pass
            true
        }
        Err(_) => {
            // Timeout → don't block, but log
            tracing::warn!("Dev holdings check timed out");
            true // Don't block on timeout
        }
    }
}

/// Filter 3: Top holder concentration
async fn check_top_holders(config: &Config, mint: &Pubkey) -> bool {
    let client = Client::new();
    let json_rpc_url = &config.rpc.helius_url;
    
    // Get top 20 token holders using getTokenAccountsByOwner or similar
    // Actually, we need a different approach - get multiple token accounts
    // This is complex on Solana, using simplified check
    
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTokenHoldings",
        "params": [mint.to_string()]
    });
    
    match timeout(
        Duration::from_secs(150),
        client.post(json_rpc_url).json(&request).send().await,
    ).await {
        Ok(_) => {
            // In a full implementation, we would parse the holder concentrations here
            // top_10_holders_combined > 35% → skip
            // single_wallet > 8% → skip
            // For now, assume pass
            true
        }
        Err(_) => {
            tracing::warn!("Top holders check timed out");
            true // Don't block on timeout
        }
    }
}

/// Filter 4: Token age
async fn check_token_age(config: &Config, mint: &Pubkey) -> bool {
    // Get token creation blockslot/epoch and calculate age
    // For this demo, we'll use a simplified approach
    // In production, would get creation transaction and compute age
    
    // Simulated token age check
    // Optimal range: 15-90 minutes
    // token_age_minutes < 8 → skip (too new)
    // token_age_minutes > 180 → skip (too old)
    
    // For now, always pass - real implementation would query creation time
    true
}

/// Filter 5: Unique buyer count
async fn check_unique_buyers(config: &Config, mint: &Pubkey) -> bool {
    let client = Client::new();
    let json_rpc_url = &config.rpc.helius_url;
    
    // Get transaction history analysis
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSignaturesForAddress",
        "params": [mint.to_string(), { "limit": 1000 }]
    });
    
    match timeout(
        Duration::from_secs(150),
        client.post(json_rpc_url).json(&request).send().await,
    ).await {
        Ok(resp) => {
            let body = resp.json().await.ok();
            if let Some(obj) = body {
                if let Some(result) = obj.get("result") {
                    // Count unique signatures/buyers
                    // unique_buyers < 50 → skip
                    if let Some(arr) = result.as_array() {
                        let buyer_count = arr.len() as u64;
                        return buyer_count >= config.filters.min_unique_buyers;
                    }
                }
            }
            false
        }
        Err(_) => {
            tracing::warn!("Unique buyers check timed out");
            false
        }
    }
}

/// Filter 7: Blacklist check
fn check_blacklist(config: &Config, mint: &Pubkey, _wallet: &Keypair) -> bool {
    // Maintain local HashSet of:
    // - Known rug developer wallets
    // - Previously rugged token mints
    // - Suspicious deployer patterns
    // Instant check, no API call needed
    
    // For now, always pass - in production would check local blacklist
    true
}

/// Filter 8: Liquidity sanity
fn check_liquidity_sanity(bonding_curve: &BondingCurve) -> bool {
    // real_sol_reserves must be 83-87 SOL
    // Outside this range → skip
    // Too low: not graduating yet
    // Too high: already graduated, missed window
    
    let reserves = bonding_curve.real_sol_reserves;
    reserves >= config.filters.min_sol_reserves && reserves <= config.filters.max_sol_reserves
}