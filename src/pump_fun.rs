use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;

use crate::config::Config;

/// Discriminator for pump.fun BondingCurve account
const BONDING_CURVE_DISCRIMINATOR: [u8; 8] = [
    59, 211, 113, 114, 81, 155, 30, 34,
];

/// Bonding curve account structure as defined by pump.fun program
#[derive(Debug, Clone)]
pub struct BondingCurve {
    pub discriminator: [u8; 8],
    pub virtual_token_reserves: u64,
    pub virtual_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub token_total_supply: u64,
    pub complete: bool,
}

impl BondingCurve {
    /// Deserialize BondingCurve from raw account data (zero-copy, in-place)
    /// Expects data to start with 8-byte discriminator followed by fields
    pub fn deserialize(data: &[u8]) -> Result<Self, String> {
        if data.len() < 8 + 6 * 8 { // discriminator + 6 u64 fields
            return Err("Insufficient data for BondingCurve".to_string());
        }

        // Read discriminator (first 8 bytes)
        let mut discriminator = [0u8; 8];
        discriminator.copy_from_slice(&data[0..8]);
        
        // Parse fields in-place from raw bytes using bytemuck-style reading
        // real_sol_reserves is at offset 8 + 4*8 = 40
        let real_sol_reserves = u64::from_le_bytes(data[40..48].try_into().map_err(|e| format!("{}", e))?);
        
        // token_total_supply at offset 48
        let token_total_supply = u64::from_le_bytes(data[48..56].try_into().map_err(|e| format!("{}", e))?);
        
        // complete at offset 56 (1 byte boolean)
        let complete = data[56] != 0;
        
        // Also read virtual reserves at their offsets
        // virtual_token_reserves at offset 8
        let virtual_token_reserves = u64::from_le_bytes(data[8..16].try_into().map_err(|e| format!("{}", e))?);
        // virtual_sol_reserves at offset 16
        let virtual_sol_reserves = u64::from_le_bytes(data[16..24].try_into().map_err(|e| format!("{}", e))?);
        // real_token_reserves at offset 24
        let real_token_reserves = u64::from_le_bytes(data[24..32].try_into().map_err(|e| format!("{}", e))?);

        // Verify discriminator
        if discriminator != BONDING_CURVE_DISCRIMINATOR {
            return Err("Invalid BondingCurve discriminator".to_string());
        }

        Ok(BondingCurve {
            discriminator,
            virtual_token_reserves,
            virtual_sol_reserves,
            real_token_reserves,
            real_sol_reserves,
            token_total_supply,
            complete,
        })
    }

    /// Check if token has graduated (bonding curve complete)
    pub fn is_graduated(&self) -> bool {
        self.complete
    }

    /// Get real SOL reserves
    pub fn sol_reserves(&self) -> u64 {
        self.real_sol_reserves
    }

    /// Get total token supply
    pub fn total_supply(&self) -> u64 {
        self.token_total_supply
    }
}

/// Graduation event detection
#[derive(Debug, Clone)]
pub struct GraduationEvent {
    pub mint: Pubkey,
    pub timestamp: std::time::Instant,
    pub was_complete_before: bool,
    pub real_sol_reserves: u64,
}

#[derive(Clone)]
pub struct PumpFunProgram {
    pub program_id: Pubkey,
    /// Track previous complete states for graduation detection
    previous_complete: DashMap<Pubkey, bool>,
}

impl PumpFunProgram {
    pub fn new(program_id_str: &str) -> Result<Self, String> {
        let program_id = Pubkey::from_str_unchecked(program_id_str);
        Ok(PumpFunProgram {
            program_id,
            previous_complete: DashMap::new(),
        })
    }

    /// Process a geyser account update and check for graduation
    pub fn check_graduation(&mut self, mint: &Pubkey, bonding_curve: &BondingCurve) -> Option<GraduationEvent> {
        let was_complete = *self.previous_complete.get(&mint).unwrap_or(&false);
        
        // Graduation event: complete went from false -> true
        let is_graduating = !was_complete && bonding_curve.complete;
        
        // Also trigger if real_sol_reserves >= 83 SOL (pre-graduation threshold)
        let meets_reserve_threshold = bonding_curve.real_sol_reserves >= 83;
        
        if is_graduating || meets_reserve_threshold {
            self.previous_complete.insert(*mint, bonding_curve.complete);
            
            Some(GraduationEvent {
                mint: *mint,
                timestamp: std::time::Instant::now(),
                was_complete_before: was_complete,
                real_sol_reserves: bonding_curve.real_sol_reserves,
            })
        } else {
            // Just update state even if not a graduation event
            self.previous_complete.insert(*mint, bonding_curve.complete);
            None
        }
    }
}

/// Note: "cube" was a typo in the original, should be "curve"
impl BondingCurve {
    /// Alias for real_sol_reserves >= 83 (pre-graduation trigger)
    pub fn meets_reserve_threshold(&self) -> bool {
        self.real_sol_reserves >= 83
    }
}