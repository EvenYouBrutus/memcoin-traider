use std::time::Duration;

use crate::position::PositionManager;
use crate::config::Config;
use tracing::{info, warn};

/// Exit strategy logic: take profit, stop loss, time-based exits
pub struct ExitStrategy {
    /// Position manager
    position_manager: PositionManager,
    /// Config
    config: Config,
    /// Buy price reference
    buy_price: f64,
}

impl ExitStrategy {
    /// Create new exit strategy
    pub fn new(position_manager: PositionManager, config: Config, buy_price: f64) -> Self {
        ExitStrategy {
            position_manager,
            config,
            buy_price,
        }
    }
    
    /// Check exit conditions for a position at current price
    /// Returns: (sell_amount_sol, reason) or None if no exit
    pub fn check_exit(
        &self,
        mint: &solana_sdk::pubkey::Pubkey,
        current_price: f64,
    ) -> Option<(f64, &'static str)> {
        let pos = self.position_manager.positions.get(mint)?;
        
        let buy_price = pos.buy_price;
        let pct_change = (current_price / buy_price - 1.0) * 100.0;
        let hold_duration = pos.buy_time.elapsed();
        let hold_minutes = hold_duration.as_secs_f64() / 60.0;
        
        // Stop loss: -25% from buy price → sell 100% immediately
        // No exceptions, no "waiting for recovery"
        if pct_change <= -25.0 {
            info!(
                "STOP LOSS triggered for {:?}: {:.2}% decline from buy price",
                mint, pct_change
            );
            return Some((pos.sol_invested, "StopLoss"));
        }
        
        // Time-based exit: if position open > 20 minutes → sell 100%
        // Meme token hype dies fast, don't hold bags
        if hold_minutes >= self.config.exit.max_hold_minutes as f64 {
            info!(
                "TIME-BASED EXIT for {:?}: held {:.1} minutes, selling all",
                mint, hold_minutes
            );
            return Some((pos.sol_invested, "TimeExit"));
        }
        
        // Take profit levels (sell in tranches)
        // +80%  → sell 40% of position
        if pct_change >= 80.0 && !pos.take_profit_1_triggered {
            self.position_manager.positions.get_mut(mint)?.take_profit_1_triggered = true;
            let sell_amount = pos.sol_invested * 0.40;
            info!("TAKE PROFIT 1 for {:?}: +{:.1}%, selling 40%", mint, pct_change);
            return Some((sell_amount, "TP1"));
        }
        
        // +150% → sell 30% of position
        if pct_change >= 150.0 && !pos.take_profit_2_triggered {
            self.position_manager.positions.get_mut(mint)?.take_profit_2_triggered = true;
            let sell_amount = pos.sol_invested * 0.30;
            info!("TAKE PROFIT 2 for {:?}: +{:.1}%, selling 30%", mint, pct_change);
            return Some((sell_amount, "TP2"));
        }
        
        // +300% → sell 20% of position
        if pct_change >= 300.0 && !pos.take_profit_3_triggered {
            self.position_manager.positions.get_mut(mint)?.take_profit_3_triggered = true;
            let sell_amount = pos.sol_invested * 0.20;
            info!("TAKE PROFIT 3 for {:?}: +{:.1}%, selling 20%", mint, pct_change);
            return Some((sell_amount, "TP3"));
        }
        
        // Trailing stop (after +80%):
        // Track highest price reached
        // If price drops 40% from peak → sell everything
        // Example: peaked at +200%, dropped to +120% → exit
        if pos.position_manager.positions.get(mint).unwrap().peak_price > buy_price * 1.8 {
            let peak_entry = self.position_manager.positions.get(mint);
            if let Some(peak_pos) = peak_entry {
                let peak_price = peak_pos.peak_price;
                let drop_from_peak = (peak_price - current_price) / peak_price * 100.0;
                
                if drop_from_peak >= self.config.exit.trailing_stop_from_peak_percent {
                    info!(
                        "TRAILING STOP for {:?}: peaked +{:.1}%, dropped {:.1}% from peak",
                        mint, 
                        (peak_price / buy_price - 1.0) * 100.0,
                        drop_from_peak
                    );
                    return Some((pos.sol_invested, "TrailingStop"));
                }
            }
        }
        
        None
    }
    
    /// Process exit result: reduce position size
    pub fn process_exit(
        &self,
        mint: &solana_sdk::pubkey::Pubkey,
        sell_amount: f64,
        reason: &'static str,
    ) {
        if let Some(pos) = self.position_manager.positions.get_mut(mint) {
            pos.sol_invested -= sell_amount;
            info!(
                "Position reduced for {:?}: sold {:.4} SOL ({}%), remaining {:.4} SOL, reason: {}",
                mint, sell_amount, sell_amount / (pos.sol_invested + sell_amount) * 100.0, pos.sol_invested, reason
            );
        }
    }
}