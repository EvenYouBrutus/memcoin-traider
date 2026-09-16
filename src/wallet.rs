use std::path::Path;
use std::sync::Arc;

use solana_sdk::{
    commitment_config::CommitmentConfig,
    signer::Signer,
    pubkey::Pubkey,
};
use solana_client::RpcClient;
use tokio::sync::OnceCell;

use crate::config::Config;

/// Wallet management: keypair, balance checks, account retrieval
pub struct Wallet {
    /// Keypair for signing transactions
    keypair: solana_sdk::signature::Keypair,
    /// Public key
    pubkey: Pubkey,
    /// RPC client for balance checks
    rpc_client: RpcClient,
    /// Config reference
    config: Arc<Config>,
}

impl Wallet {
    /// Load wallet from private key file
    pub fn load(config: &Config) -> Result<Self, Box<dyn std::error::Error>> {
        let path = &config.wallet.private_key_path;
        
        // Check file exists
        if !Path::new(path).exists() {
            return Err(format!("Wallet file not found: {:?}", path).into());
        }
        
        // Load keypair from file
        let keypair = solana_sdk::signature::Keypair::from_file(path)
            .map_err(|e| format!("Failed to load keypair: {}", e))?;
        
        let pubkey = keypair.pubkey();
        let commitment = CommitmentConfig::processed();
        let rpc_client = RpcClient::new_with_config(
            &config.rpc.helius_url,
            commitment,
        );
        
        // Check wallet balance
        let balance = rpc_client.get_balance(&pubkey)
            .map_err(|e| format!("Failed to get balance: {}", e))?;
        
        if balance < 0.001 * solana_sdk::lamports_per_sol as f64 {
            return Err(format!("Wallet has insufficient SOL: {} lamports ({} SOL)", 
                balance, balance / solana_sdk::lamports_per_sol as f64).into());
        }
        
        info!("Wallet loaded: {} with {} SOL", pubkey, balance / solana_sdk::lamports_per_sol as f64);
        
        Ok(Wallet {
            keypair,
            pubkey,
            rpc_client,
            config: Arc::new(config.clone()),
        })
    }
    
    /// Get public key
    pub pubkey(&self) -> &Pubkey {
        &self.pubkey
    }
    
    /// Get SOL balance
    pub fn sol_balance(&self) -> f64 {
        self.rpc_client.get_balance(&self.pubkey) as f64 / solana_sdk::lamports_per_sol as f64
    }
    
    /// Check if wallet has enough SOL for a trade (including fees)
    pub fn has_sufficient_balance_for_trade(&self, trade_amount_sol: f64) -> bool {
        let balance = self.sol_balance();
        // Need trade amount + fees (approx 0.001-0.005 SOL per tx)
        let minimum_required = trade_amount_sol + 0.005;
        balance >= minimum_required
    }
    
    /// Get nonce account info
    pub fn nonce_account(&self) -> Result<solana_sdk::account::Account, String> {
        self.rpc_client.get_account(&solana_sdk::system_program::ID)
            .map_err(|e| format!("Failed to get nonce: {}", e))
    }
    
    /// Get associated token account address
    pub fn associated_token_address(&self, mint: &Pubkey) -> Pubkey {
        // Simple derivation - in production would use AssociatedToken program
        let seed = mint.to_bytes();
        // This is a simplified version - real implementation uses the AssociatedToken program
        let mut hasher = solana_sdk::hash::Hash::default();
        hasher.copy_from_slice(seed.as_slice());
        // Just return a deterministic address for now
        Pubkey::new_from_array(hasher.compute_hash().0)
    }
    
    /// Get token account balance via RPC
    pub async fn get_token_balance(&self, token_mint: &Pubkey) -> Result<u64, String> {
        // Use getTokenAccountBalance RPC method
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTokenAccountBalance",
            "params": [self.associated_token_address(token_mint).to_string()]
        });
        
        match self.rpc_client.rpc_client().post(
            &self.rpc_client.url(),
            &request
        ).await {
            Ok(resp) => {
                let json: serde_json::Value = resp.json().await.map_err(|e| format!("{}", e))?;
                if let Some(amount) = json.get("result").and_then(|r| r.get("value")).and_then(|v| v.get("uiAmountString")) {
                    amount.parse::<u64>().map_err(|e| format!("{}", e))
                } else {
                    Err("No balance found in response".to_string())
                }
            }
            Err(e) => Err(format!("RPC error: {}", e)),
        }
    }
}

/// Arc-wrapped wallet
pub type ArcWallet = Arc<Wallet>;