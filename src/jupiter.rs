use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

use crate::config::Config;
use tracing::{info, warn};

/// Jupiter v6 quote response
#[derive(Debug, Deserialize)]
pub struct JupiterQuote {
    pub input_mint: String,
    pub output_mint: String,
    pub amount: u64,
    pub raw_amount: u64,
    pub min_out: u64,
    pub routes: Vec<JupiterRoute>,
    pub slipage_bps: u64,
}

/// Jupiter route response
#[derive(Debug, Deserialize)]
pub struct JupiterRoute {
    pub path: Vec<String>,
    pub programId: String,
    pub fee: u64,
    pub fees: Vec<JupiterFee>,
}

/// Jupiter fee structure
#[derive(Debug, Deserialize)]
pub struct JupiterFee {
    pub fee_amount: u64,
    pub fee_mint: String,
}

/// Jupiter swap transaction response
#[derive(Debug, Deserialize)]
pub struct JupiterSwapResponse {
    pub swapTransaction: String,
    pub keepAlive: bool,
}

/// Quote parameters for Jupiter
#[derive(Debug, Clone)]
pub struct QuoteParams {
    pub input_mint: String,
    pub output_mint: String,
    pub amount: u64,           // Amount in lamports (raw, not human readable)
    pub slippage_bps: u64,     // Slippage in basis points
    pub only_route: Option<String>,
}

/// Jupiter client with route caching for speed
pub struct JupiterClient {
    client: Client,
    pub config: Arc<Config>,
    /// Cache: mint -> (quote, timestamp)
    quote_cache: dashmap::DashMap<String, (JupiterQuote, std::time::Instant)>,
    /// Cache TTL in seconds
    cache_ttl: u64,
}

impl JupiterClient {
    /// Create new Jupiter client
    pub fn new(config: Arc<Config>) -> Result<Self, String> {
        let client = Client::new();
        
        Ok(JupiterClient {
            client,
            config,
            quote_cache: dashmap::DashMap::new(),
            cache_ttl: 30, // 30 second cache TTL
        })
    }
    
    /// Get a quote from Jupiter v6 API with caching
    pub async fn get_quote(&self, params: &QuoteParams) -> Result<JupiterQuote, Box<dyn std::error::Error>> {
        let cache_key = format!("{}:{}:{}", 
            params.input_mint, 
            params.output_mint, 
            params.amount
        );
        
        // Check cache first (zero-copy lookup)
        if let Some(entry) = self.quote_cache.get(&cache_key) {
            let (quote, timestamp) = entry.value();
            let age = self.config.logging.log_level.parse::<u64>().unwrap_or(0); // placeholder
            // Check if cache is still valid
            let now = std::time::Instant::now();
            let age_secs = now.duration_timestamp(*timestamp).as_secs(); // simplified
            
            // Simple age check - in production use precise timing
            if age_secs < self.cache_ttl {
                return Ok(quote.clone());
            }
        }
        
        // Fetch from API
        let quote = self.fetch_quote_from_api(params).await?;
        
        // Cache the result
        self.quote_cache.insert(cache_key, (quote.clone(), std::time::Instant::now()));
        
        Ok(quote)
    }
    
    /// Fetch quote from Jupiter API
    async fn fetch_quote_from_api(
        &self,
        params: &QuoteParams,
    ) -> Result<JupiterQuote, Box<dyn std::error::Error>> {
        let url = "https://quote-api.jup.ag/v6/quote";
        
        let request = serde_json::json!({
            "inputMint": params.input_mint,
            "outputMint": params.output_mint,
            "amount": params.amount,
            "slippageBps": params.slippage_bps,
        });
        
        match timeout(
            Duration::from_secs(500),
            self.client.post(url).json(&request).send().await,
        ).await {
            Ok(resp) => {
                let body = resp.json::<JupiterQuote>().await;
                match body {
                    Ok(quote) => Ok(quote),
                    Err(_) => Err("Failed to parse Jupiter quote".into()),
                }
            }
            Err(_) => {
                Err("Jupiter quote request timed out".into())
            }
        }
    }
    
    /// Get routes from Jupiter for a swap
    pub async fn get_routes(&self, input_mint: &str, output_mint: &str, amount: u64) -> Result<Vec<JupiterRoute>, Box<dyn std::error::Error>> {
        let url = "https://quote-api.jup.ag/v6/routes";
        
        let request = serde_json::json!({
            "inputMint": input_mint,
            "outputMint": output_mint,
            "amount": amount,
            "onlyDirectRoutes": "false",
        });
        
        match timeout(
            Duration::from_secs(500),
            self.client.post(url).json(&request).send().await,
        ).await {
            Ok(resp) => {
                let body = resp.json::<Vec<JupiterRoute>>().await;
                match body {
                    Ok(routes) => Ok(routes),
                    Err(_) => Err("Failed to parse Jupiter routes".into()),
                }
            }
            Err(_) => {
                Err("Jupiter routes request timed out".into())
            }
        }
    }
    
    /// Build swap transaction via Jupiter v6
    pub async fn build_swap_transaction(
        &self,
        input_mint: &str,
        output_mint: &str,
        amount: u64,
        slippage_bps: u64,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let quote = self.get_quote(&QuoteParams {
            input_mint: input_mint.to_string(),
            output_mint: output_mint.to_string(),
            amount,
            slippage_bps,
        }).await?;
        
        // Get best route
        let routes = self.get_routes(input_mint, output_mint, amount).await?;
        
        if routes.is_empty() {
            return Err("No routes available".into());
        }
        
        // Use first route
        let route = &routes[0];
        
        // Build swap transaction
        let url = "https://quote-api.jup.ag/v6/swap";
        
        let request = serde_json::json!({
            "quote": quote,
            "userPublicKey": "", // Will be filled in at send time
            "wrapAndUnwrapSol": true,
        });
        
        match timeout(
            Duration::from_secs(500),
            self.client.post(url).json(&request).send().await,
        ).await {
            Ok(resp) => {
                let body = resp.json::<JupiterSwapResponse>().await;
                match body {
                    Ok(swap) => Ok(swap.swapTransaction),
                    Err(_) => Err("Failed to parse Jupiter swap transaction".into()),
                }
            }
            Err(_) => {
                Err("Jupiter swap transaction build timed out".into())
            }
        }
    }
    
    /// Clear cache (called when market conditions change significantly)
    pub fn clear_cache(&self) {
        self.quote_cache.clear();
    }
    
    /// Get cache stats
    pub fn cache_stats(&self) -> (usize, usize) {
        (self.quote_cache.len(), self.cache_ttl)
    }
}