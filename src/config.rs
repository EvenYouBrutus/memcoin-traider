use std::path::PathBuf;
use serde::Deserialize;
use thiserror::Error;

use solana_sdk::pubkey::Pubkey;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    #[serde(rename = "rpc")]
    pub rpc: RpcConfig,

    #[serde(rename = "wallet")]
    pub wallet: WalletConfig,

    #[serde(rename = "trading")]
    pub trading: TradingConfig,

    #[serde(rename = "filters")]
    pub filters: FilterConfig,

    #[serde(rename = "exit")]
    pub exit: ExitConfig,

    #[serde(rename = "jito")]
    pub jito: JitoConfig,

    #[serde(rename = "logging")]
    pub logging: LoggingConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RpcConfig {
    pub helius_url: String,
    pub helius_geyser_url: String,
    pub quicknode_url: String,
    pub triton_url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WalletConfig {
    pub private_key_path: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TradingConfig {
    pub buy_amount_sol: f64,
    pub max_buy_amount_sol: f64,
    pub max_concurrent_positions: u32,
    pub daily_loss_limit_percent: f64,
    pub slippage_bps: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FilterConfig {
    pub min_rugcheck_score: u64,
    pub max_dev_holdings_percent: u64,
    pub max_top10_concentration_percent: u64,
    pub min_token_age_minutes: u64,
    pub max_token_age_minutes: u64,
    pub min_unique_buyers: u64,
    pub min_sol_reserves: f64,
    pub max_sol_reserves: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExitConfig {
    pub stop_loss_percent: f64,
    pub take_profit_1_percent: f64,
    pub take_profit_1_size: f64,
    pub take_profit_2_percent: f64,
    pub take_profit_2_size: f64,
    pub take_profit_3_percent: f64,
    pub take_profit_3_size: f64,
    pub max_hold_minutes: u64,
    pub trailing_stop_from_peak_percent: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct JitoConfig {
    pub tip_lamports: u64,
    pub max_tip_lamports: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    pub log_level: String,
    pub log_file: String,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to load config: {0}")]
    LoadError(String),

    #[error("Failed to parse config: {0}")]
    ParseError(String),

    #[error("Configuration validation failed: {0}")]
    ValidationError(String),
}

impl Config {
    pub fn load(path: &str) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::LoadError(format!("{}", e)))?;
        
        let config = serde_json::from_str(&content)
            .map_err(|e| ConfigError::ParseError(format!("{}", e)))?;
        
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate RPC URLs are not empty
        if self.rpc.helius_url.is_empty() {
            return Err(ConfigError::ValidationError("Helius RPC URL is empty".to_string()));
        }
        if self.rpc.helius_geyser_url.is_empty() {
            return Err(ConfigError::ValidationError("Helius Geyser URL is empty".to_string()));
        }
        
        // Validate wallet path exists
        if !PathBuf::new(&self.wallet.private_key_path).exists() {
            return Err(ConfigError::ValidationError(
                format!("Wallet private key file not found: {:?}", self.wallet.private_key_path)
            ));
        }
        
        // Validate trading parameters
        if self.trading.buy_amount_sol <= 0.0 {
            return Err(ConfigError::ValidationError("Buy amount must be positive".to_string()));
        }
        if self.trading.max_buy_amount_sol <= self.trading.buy_amount_sol {
            return Err(ConfigError::ValidationError("Max buy amount must be greater than buy amount".to_string()));
        }
        if self.trading.max_concurrent_positions < 1 {
            return Err(ConfigError::ValidationError("Max concurrent positions must be at least 1".to_string()));
        }
        if self.trading.daily_loss_limit_percent <= 0.0 || self.trading.daily_loss_limit_percent > 100.0 {
            return Err(ConfigError::ValidationError("Daily loss limit must be between 0 and 100".to_string()));
        }
        
        // Validate filter parameters
        if self.filters.min_rugcheck_score < 100 {
            return Err(ConfigError::ValidationError("Min rugcheck score too low".to_string()));
        }
        if self.filters.max_dev_holdings_percent > 50 {
            return Err(ConfigError::ValidationError("Max dev holdings percent too high".to_string()));
        }
        if self.filters.min_token_age_minutes >= self.filters.max_token_age_minutes {
            return Err(ConfigError::ValidationError("Min token age must be less than max token age".to_string()));
        }
        if self.filters.min_unique_buyers < 1 {
            return Err(ConfigError::ValidationError("Min unique buyers must be at least 1".to_string()));
        }
        if self.filters.min_sol_reserves >= self.filters.max_sol_reserves {
            return Err(ConfigError::ValidationError("Min SOL reserves must be less than max SOL reserves".to_string()));
        }
        
        // Validate exit parameters
        if self.exit.stop_loss_percent <= 0.0 || self.exit.stop_loss_percent > 100.0 {
            return Err(ConfigError::ValidationError("Stop loss percent must be between 0 and 100".to_string()));
        }
        if self.exit.max_hold_minutes < 1 {
            return Err(ConfigError::ValidationError("Max hold minutes must be at least 1".to_string()));
        }
        
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            rpc: RpcConfig {
                helius_url: "https://mainnet.helius-rpc.com".to_string(),
                helius_geyser_url: "https://mainnet.helius-rpc.com".to_string(),
                quicknode_url: "https://mainnet.quicknode.com".to_string(),
                triton_url: "https://rpc.triton.tg".to_string(),
            },
            wallet: WalletConfig {
                private_key_path: "./keypair.json".to_string(),
            },
            trading: TradingConfig {
                buy_amount_sol: 0.3,
                max_buy_amount_sol: 1.0,
                max_concurrent_positions: 3,
                daily_loss_limit_percent: 15.0,
                slippage_bps: 300,
            },
            filters: FilterConfig {
                min_rugcheck_score: 500,
                max_dev_holdings_percent: 10,
                max_top10_concentration_percent: 35,
                min_token_age_minutes: 8,
                max_token_age_minutes: 180,
                min_unique_buyers: 50,
                min_sol_reserves: 83.0,
                max_sol_reserves: 87.0,
            },
            exit: ExitConfig {
                stop_loss_percent: 25.0,
                take_profit_1_percent: 80.0,
                take_profit_1_size: 0.40,
                take_profit_2_percent: 150.0,
                take_profit_2_size: 0.30,
                take_profit_3_percent: 300.0,
                take_profit_3_size: 0.20,
                max_hold_minutes: 20,
                trailing_stop_from_peak_percent: 40.0,
            },
            jito: JitoConfig {
                tip_lamports: 100000,
                max_tip_lamports: 1000000,
            },
            logging: LoggingConfig {
                log_level: "info".to_string(),
                log_file: "./bot.log".to_string(),
            },
        }
    }
}

#[derive(Clone)]
pub struct ArcConfig(pub Arc<Config>);

impl std::fmt::Debug for ArcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArcConfig").finish()
    }
}