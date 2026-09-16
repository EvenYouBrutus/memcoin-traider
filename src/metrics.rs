use std::time::Instant;
use dashmap::DashMap;
use tracing::{info, warn, debug};

use crate::config::Config;

/// Metrics tracker for latency, win rate, and dashboard
#[derive(Debug, Default)]
pub struct Metrics {
    /// Geyser to signal latency (ms)
    pub geyser_to_signal_latency: Option<u64>,
    /// Signal to tx sent latency (ms)
    pub execution_latency: Option<u64>,
    /// RPC send latency (ms)
    pub rpc_send_latency: Option<u64>,
    /// Blockhash refresh latency (ms)
    pub blockhash_refresh_latency: Option<u64>,
    /// Total latency (ms)
    pub total_latency: Option<u64>,
    /// Win count
    pub win_count: u64,
    /// Loss count
    pub loss_count: u64,
    /// Total trade count
    pub total_trades: u64,
    /// Daily PnL
    pub daily_pnl: f64,
    /// Starting daily balance
    pub starting_balance: f64,
}

impl Metrics {
    /// Create new metrics tracker
    pub fn new() -> Self {
        Metrics {
            ..Default::default()
        }
    }
    
    /// Record a trade with its result
    pub fn record_trade(&mut self, pnl: f64, buy_price: f64, sell_price: f64) {
        self.total_trades += 1;
        self.daily_pnl += pnl;
        
        // Calculate return percentage
        if buy_price > 0.0 {
            let return_pct = (sell_price / buy_price - 1.0) * 100.0;
            
            if return_pct > 0.0 {
                self.win_count += 1;
            } else {
                self.loss_count += 1;
            }
        }
        
        // Log win rate
        let win_rate = if self.total_trades > 0 {
            (self.win_count as f64 / self.total_trades as f64) * 100.0
        } else {
            0.0
        };
        
        debug!("Trade recorded: PnL {:.4} SOL, Return {:.2}%, Win rate: {:.1}%", 
            pnl, return_pct, win_rate);
    }
    
    /// Record latency measurement
    pub fn record_latency(&mut self, stage: &str, duration: std::time::Duration) {
        let ms = duration.as_millis() as u64;
        match stage {
            "geyser_to_signal" => self.geyser_to_signal_latency = Some(ms),
            "signal_to_tx" => self.execution_latency = Some(ms),
            "rpc_send" => self.rpc_send_latency = Some(ms),
            "blockhash_refresh" => self.blockhash_refresh_latency = Some(ms),
            "total" => self.total_latency = Some(ms),
            _ => {}
        }
        
        debug!("{} latency: {}ms", stage, ms);
    }
    
    /// Get win rate percentage
    pub fn win_rate(&self) -> f64 {
        if self.total_trades > 0 {
            (self.win_count as f64 / self.total_trades as f64) * 100.0
        } else {
            0.0
        }
    }
    
    /// Check if daily loss limit hit
    pub fn is_daily_loss_limit_hit(&self) -> bool {
        if self.starting_balance > 0.0 {
            let loss_pct = (self.daily_pnl.abs() / self.starting_balance) * 100.0;
            loss_pct >= 15.0
        } else {
            false
        }
    }
    
    /// Display real-time dashboard (using ratatui)
    pub fn display_dashboard(&self) {
        // Dashboard rendered by TUI layer
        // This just logs the key metrics
        info!(
            "=== DASHBOARD ===",
        );
        info!(
            "Status: ACTIVE   Trades: {}   Win rate: {:.1}%",
            self.total_trades,
            self.win_rate()
        );
        info!(
            "Daily PnL: {:.4} SOL   Loss limit: {}%",
            self.daily_pnl,
            if self.is_daily_loss_limit_hit() { "HIT" } else { "OK" }
        );
    }
}

/// Latency timer helper
pub struct LatencyTimer {
    /// Start time
    start: Instant,
    /// Stage name
    stage: &'static str,
    /// Metrics reference
    metrics: &'metrics Mutex<Metrics>,
}

impl LatencyTimer {
    pub fn new(stage: &'static str, metrics: &'metrics Mutex<Metrics>) -> Self {
        LatencyTimer {
            start: Instant::now(),
            stage,
            metrics,
        }
    }
    
    /// Stop and record latency
    pub fn stop(self) {
        let duration = self.start.elapsed();
        let mut metrics = self.metrics.lock().unwrap();
        metrics.record_latency(self.stage, duration);
    }
}