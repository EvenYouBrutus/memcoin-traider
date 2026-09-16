use std::collections::HashMap;
use std::time::Instant;

use solana_sdk::pubkey::Pubkey;
use tokio::sync::Mutex;

use crate::metrics::Metrics;
use crate::config::Config;

/// Position tracking for active trades
#[derive(Debug, Clone)]
pub struct Position {
    /// Token mint address
    pub mint: Pubkey,
    /// Amount of SOL invested
    pub sol_invested: f64,
    /// Buy price in USD (or relative units)
    pub buy_price: f64,
    /// Buy timestamp
    pub buy_time: Instant,
    /// Current price (updated via Geyser)
    pub current_price: f64,
    /// Highest price reached (for trailing stop)
    pub peak_price: f64,
    /// Position status: open, exited, stopped_out
    pub status: PositionStatus,
    /// Take profit levels
    pub take_profit_1_triggered: bool,
    pub take_profit_2_triggered: bool,
    pub take_profit_3_triggered: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PositionStatus {
    Open,
    TakeProfit1,
    TakeProfit2,
    TakeProfit3,
    StopLoss,
    Exited,
}

/// Position manager: tracks up to max_concurrent_positions
pub struct PositionManager {
    /// Active positions keyed by mint
    positions: dashmap::DashMap<Pubkey, Position>,
    /// Max concurrent positions
    max_positions: u32,
    /// Total daily PnL
    daily_pnl: f64,
    /// Starting daily balance
    starting_balance: f64,
    /// Daily loss limit tracker
    daily_loss_start: f64,
}

impl PositionManager {
    /// Create new position manager
    pub fn new(max_concurrent_positions: u32) -> Self {
        PositionManager {
            positions: dashmap::DashMap::new(),
            max_positions: max_concurrent_positions,
            daily_pnl: 0.0,
            starting_balance: 0.0,
            daily_loss_start: 0.0,
        }
    }
    
    /// Check if can buy (under position limit)
    pub fn can_buy(&self) -> bool {
        self.positions.len() < self.max_positions
    }
    
    /// Check if can buy considering daily loss limit
    pub fn can_buy_with_daily_limit(&self, trade_amount: f64, wallet_balance: f64) -> bool {
        // Check position limit
        if !self.can_buy() {
            return false;
        }
        
        // Check SOL balance minimum
        if wallet_balance < 0.5 {
            return false;
        }
        
        // Check daily loss limit
        let loss_pct = if self.daily_loss_start > 0.0 {
            (self.daily_pnl.abs() / self.daily_loss_start) * 100.0
        } else {
            0.0
        };
        
        // If daily loss limit hit (-15%), stop trading
        if loss_pct >= 15.0 {
            return false;
        }
        
        true
    }
    
    /// Open a new position
    pub fn open_position(&self, mint: Pubkey, sol_invested: f64, buy_price: f64) -> Result<(), String> {
        // Check if already holding this position
        if self.positions.get(&mint).is_some() {
            return Err("Already holding position for this mint".to_string());
        }
        
        // Check concurrent position limit
        if !self.can_buy() {
            return Err("Max concurrent positions reached".to_string());
        }
        
        let pos = Position {
            mint,
            sol_invested,
            buy_price,
            buy_time: Instant::now(),
            current_price: buy_price,
            peak_price: buy_price,
            status: PositionStatus::Open,
            take_profit_1_triggered: false,
            take_profit_2_triggered: false,
            take_profit_3_triggered: false,
        };
        
        self.positions.insert(mint, pos);
        
        Ok(())
    }
    
    /// Close a position and calculate PnL
    pub fn close_position(&self, mint: &Pubkey, sell_price: f64, reason: &str) -> Result<f64, String> {
        let pos = self.positions.get(mint)
            .ok_or_else(|| format!("Position not found for mint {:?}", mint))?
            .clone();
        
        // Calculate PnL
        let sol_invested = pos.sol_invested;
        let buy_price = pos.buy_price;
        let pnl = ((sell_price - buy_price) / buy_price) * sol_invested;
        
        // Remove position
        self.positions.remove(mint);
        
        // Update daily PnL
        self.daily_pnl += pnl;
        
        info!("Position closed for {:?}: PnL {:.4} SOL ({}%), reason: {}", 
            mint, pnl, pnl / sol_invested * 100.0, reason);
        
        Ok(pnl)
    }
    
    /// Update current price for a position
    pub fn update_price(&self, mint: &Pubkey, new_price: f64) -> Option<Position> {
        let mut pos = self.positions.get_mut(mint)?;
        
        let old_price = pos.current_price;
        pos.current_price = new_price;
        
        // Update peak price for trailing stop
        if new_price > pos.peak_price {
            pos.peak_price = new_price;
        }
        
        // Check take profit levels
        let tp1_pct = (new_price / buy_price - 1.0) * 100.0;
        let tp2_pct = ((new_price / pos.buy_price) - 1.0) * 100.0; // simplified
        
        // This is simplified - real implementation would check exact TP levels
        
        Some(pos.clone())
    }
    
    /// Check exit conditions for a position
    pub fn check_exits(&self, mint: &Pubkey, current_price: f64) -> Option<(f64, String)> {
        let pos = self.positions.get(mint)?;
        
        let buy_price = pos.buy_price;
        let pct_change = (current_price / buy_price - 1.0) * 100.0;
        let hold_duration = pos.buy_time.elapsed();
        let hold_minutes = hold_duration.as_secs_f64() / 60.0;
        
        // Stop loss: -25% from buy price
        if pct_change <= -25.0 {
            return Some((-0.25 * pos.sol_invested, "StopLoss".to_string()));
        }
        
        // Time-based exit: if position open > 20 minutes → sell 100%
        if hold_minutes > Config::default().exit.max_hold_minutes as f64 {
            return Some((pos.sol_invested, "TimeExit".to_string()));
        }
        
        // Take profit levels
        // +80% → sell 40%
        if pct_change >= 80.0 && !pos.take_profit_1_triggered {
            pos.take_profit_1_triggered = true;
            let sell_amount = pos.sol_invested * 0.40;
            return Some((sell_amount, "TP1".to_string()));
        }
        
        // +150% → sell 30%
        if pct_change >= 150.0 && !pos.take_profit_2_triggered {
            pos.take_profit_2_triggered = true;
            let sell_amount = pos.sol_invested * 0.30;
            return Some((sell_amount, "TP2".to_string()));
        }
        
        // +300% → sell 20%
        if pct_change >= 300.0 && !pos.take_profit_3_triggered {
            pos.take_profit_3_triggered = true;
            let sell_amount = pos.sol_invested * 0.20;
            return Some((sell_amount, "TP3".to_string()));
        }
        
        // Trailing stop: after +80%, if price drops 40% from peak
        if pos.peak_price > buy_price * 1.8 {
            let peak_pct = (pos.peak_price / buy_price - 1.0) * 100.0;
            let drop_from_peak = (pos.peak_price - current_price) / pos.peak_price * 100.0;
            
            if drop_from_peak >= 40.0 {
                return Some((pos.sol_invested, "TrailingStop".to_string()));
            }
        }
        
        None
    }
    
    /// Get all open positions
    pub fn get_open_positions(&self) -> Vec<(Pubkey, Position)> {
        self.positions
            .iter()
            .filter(|entry| *entry.value().status == PositionStatus::Open)
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect()
    }
    
    /// Get daily PnL
    pub fn daily_pnl(&self) -> f64 {
        self.daily_pnl
    }
    
    /// Set daily loss start
    pub fn set_daily_loss_start(&mut self, balance: f64) {
        self.daily_loss_start = balance;
    }
}

impl PositionStatus {
    fn status(&self) -> &PositionStatus {
        self
    }
}