use std::collections::VecDeque;
use std::fs;
use std::io::BufRead;
use std::path::Path;
use std::time::Instant;

use chrono::Utc;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;
use solana_sdk::hash::Hash;

use crate::config::Config;
use crate::metrics::Metrics;
use crate::pump_fun::{BondingCurve, GraduationEvent, PumpFunProgram};
use crate::position::PositionManager;
use crate::exit::ExitStrategy;

/// A single historical graduation event for replay
#[derive(Debug, Clone)]
pub struct ReplayEvent {
    /// Mint address of the graduating token
    pub mint: Pubkey,
    /// Timestamp when graduation was detected (unix sec)
    pub timestamp: i64,
    /// SOL reserves at graduation moment
    pub sol_reserves: u64,
    /// Token total supply at graduation
    pub total_supply: u64,
}

/// Read graduation events from a CSV file
/// Expected format: mint,timestamp_unix_secs,sol_reserves,total_supply
/// Example: HYa27pj...,1700000000,83000000,5000000
pub fn read_events_from_csv<P: AsRef<Path>>(path: P) -> Vec<ReplayEvent> {
    let mut events = Vec::new();
    let file = fs::File::open(path).expect("Failed to open replay events CSV");
    let reader = std::io::BufReader::new(file);

    for line in reader.lines() {
        let line = line.expect("Failed to read replay event line");
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 3 {
            eprintln!("Skipping malformed replay line: {} (need at least 3 fields)", line);
            continue;
        }

        let mint_str = parts[0].trim();
        let timestamp = parts[1].trim().parse::<i64>().unwrap_or(0);
        let sol_reserves = parts[2].trim().parse::<u64>().unwrap_or(0);
        let total_supply = if parts.len() > 3 {
            parts[3].trim().parse::<u64>().unwrap_or(0)
        } else {
            0
        };

        if let Ok(mint) = Pubkey::from_str(mint_str) {
            events.push(ReplayEvent {
                mint,
                timestamp,
                sol_reserves,
                total_supply,
            });
        } else {
            eprintln!("Invalid pubkey in replay line: {}", line);
        }
    }

    events
}

/// Check baseline filters using only locally-available data.
/// This is suitable for replay/backtest mode where no RPC/HTTP calls should be made.
/// 
/// Checks:
/// 1. Liquidity sanity: sol_reserves must be in [min_sol, max_sol] range
/// 2. Token age: token must be within age range (based on graduation timestamp)
/// 3. Minimum unique buyers: simplified check (always pass in baseline)
/// 4. Dev holdings concentration: simplified check (always pass in baseline)
/// 5. Top holder concentration: simplified check (always pass in baseline)
/// 6. RugCheck score: simplified check (always pass in baseline)
/// 
/// Returns true if the event passes baseline filters.
pub fn check_baseline_filters(
    sol_reserves: u64,
    total_supply: u64,
    graduation_timestamp: i64,
    min_sol_reserves: f64,
    max_sol_reserves: f64,
    min_token_age_minutes: u64,
    max_token_age_minutes: u64,
) -> bool {
    // Filter 8: Liquidity sanity - real_sol_reserves must be in range
    let reserves_f64 = sol_reserves as f64;
    if reserves_f64 < min_sol_reserves || reserves_f64 > max_sol_reserves {
        // eprintln!("LIQUIDITY FAIL: reserves={:.1} SOL, range [{}%, {}%]", 
        //     reserves_f64 / 1_000_000_000.0, min_sol_reserves, max_sol_reserves);
        return false;
    }

    // Filter 4: Token age check
    // In replay, we consider token age from graduation timestamp.
    // For baseline: just check that the event timestamp is reasonable.
    // A newly graduated token should have been in development for some time.
    // We'll accept if graduation timestamp is after a reasonable token creation time.
    // - Too new (less than min_token_age_minutes old) → skip
    // - Too old (more than max_token_age_minutes old) → skip  
    // For replay of historical events, we just verify the timestamp is within a 
    // reasonable range. Since we don't know the exact creation time, we check that
    // the graduation timestamp is not absurdly recent or distant.
    // 
    // In a full implementation, would need token creation block/slot.
    // For baseline: pass (the graduation event itself implies some minimum age)
    // TODO: Implement proper token age check with creation time
    let _age_check_pass = true; // Baseline: pass for now

    // All other baseline filters pass (no API calls available in replay baseline)
    true
}

/// Replay mode: process historical graduation events through the strategy
/// Returns metrics and trade list
pub struct ReplayRunner {
    /// Configuration
    config: Config,
    /// Graduation events sorted by timestamp
    events: Vec<ReplayEvent>,
    /// Position tracking
    position_manager: PositionManager,
    /// Exit strategy
    exit_strategy: ExitStrategy,
    /// Metrics
    metrics: Metrics,
    /// Whether we're in replay mode
    _replay_mode: bool,
}

impl ReplayRunner {
    /// Create a new replay runner
    pub fn new(config: Config, events: Vec<ReplayEvent>) -> Result<Self, String> {
        let position_manager = PositionManager::new(config.trading.max_concurrent_positions);
        let exit_strategy = ExitStrategy::new(
            position_manager.clone(),
            config.exit.clone(),
            0.0, // buy_price will be set per trade
        );

        Ok(ReplayRunner {
            config,
            events,
            position_manager,
            exit_strategy,
            metrics: Metrics::new(),
            _replay_mode: true,
        })
    }

    /// Run the replay
    pub fn run(&mut self) -> (Metrics, Vec<TradeResult>) {
        let mut trades = Vec::new();

        // Sort events by timestamp for deterministic processing
        let mut sorted_events: Vec<&ReplayEvent> = self.events.iter().collect();
        sorted_events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        // Baseline filter parameters from config
        let min_sol = self.config.filters.min_sol_reserves;
        let max_sol = self.config.filters.max_sol_reserves;
        let min_age = self.filters.min_token_age_minutes();
        let max_age = self.config.filters.max_token_age_minutes();

        // Process each graduation event
        for event in &sorted_events {
            let _entry_start = Instant::now();

            // === BASIC ELIGIBILITY CHECKS (local, no API calls) ===
            
            // Check position limit
            if !self.position_manager.can_buy() {
                // eprintln!("SKIP: max positions reached ({})", self.position_manager.positions.len());
                continue;
            }

            // Check SOL balance minimum
            let wallet_balance = 10.0; // Simplified: assume sufficient SOL
            if wallet_balance < 0.5 {
                // eprintln!("SKIP: insufficient SOL balance");
                continue;
            }

            // Daily loss limit check
            let loss_pct = self.metrics.daily_pnl.abs() / self.metrics.starting_balance.max(1.0) * 100.0;
            if loss_pct >= 15.0 {
                // eprintln!("SKIP: daily loss limit hit ({:.1}%)", loss_pct);
                break;
            }

            // === BASELINE FILTER CHECK (local only, no API calls) ===
            // Check liquidity sanity: sol_reserves must be in [min_sol, max_sol] range
            let baseline_pass = event.sol_reserves as f64 >= min_sol && event.sol_reserves as f64 <= max_sol;
            
            // Also check graduation condition locally
            let is_graduating = event.sol_reserves >= 83;
            
            if !baseline_pass || !is_graduating {
                // eprintln!("BASELINE FILTERS FAILED for {}: sol_reserves={}, graduating={}", event.mint, event.sol_reserves, is_graduating);
                continue;
            }

            if !baseline_pass {
                // eprintln!("BASELINE FILTERS FAILED for {}: sol_reserves={}", event.mint, event.sol_reserves);
                continue;
            }

            // === ENTRY: Process graduation event ===
            // Create position at "entry price"
            let entry_price = self.simulate_entry_price(event);
            
            // Check core graduation conditions locally
            // Graduation: complete == true OR sol_reserves >= 83
            let is_graduating = event.sol_reserves >= 83; // Simple threshold check
            
            if !is_graduating {
                // eprintln!("NOT GRADUATING: sol_reserves={} < 83", event.sol_reserves);
                continue;
            }

            // === POSITION ENTRY ===
            let sol_invested = self.config.trading.buy_amount_sol;
            self.position_manager.open_position(
                event.mint.clone(),
                sol_invested,
                entry_price,
            );

            // eprintln!("BUY: {} at price {:.4}, amount {:.4} SOL", event.mint, entry_price, sol_invested);

            // Record trade start
            let trade = TradeResult {
                mint: event.mint,
                entry_price,
                entry_time: event.timestamp,
                sol_invested,
                ..Default::default()
            };

            // === SIMULATED PRICE EVOLUTION ===
            // Move to next events for price simulation
            let mut price = entry_price;
            let mut held_minutes = 0i64;

            // Simulate price over time using subsequent events
            let mut price_change_pct = 0.0f64;
            let mut exit_reason = String::new();
            let mut exit_price = price;

            // Process exit conditions based on time and price moves
            for later_event in &sorted_events {
                if later_event.timestamp <= event.timestamp {
                    continue; // No look-ahead: only use future data after entry
                }

                // Calculate holding time
                let hold_time_secs = later_event.timestamp - event.timestamp;
                held_minutes = hold_time_secs / 60;

                // Simulated price move based on holding time
                let hours_since_grad = hold_time_secs as f64 / 3600.0;
                
                // Simple mean-reverting price model for memecoin
                // Peak around 30-90 min, then decay
                if hours_since_grad < 2.0 {
                    // Initial hype burst
                    price_change_pct = 0.05 * hours_since_grad.sqrt();
                } else if hours_since_grad < 24.0 {
                    // Decay phase
                    let decay = (hours_since_grad - 2.0).abs() * 0.08;
                    price_change_pct = -decay;
                } else {
                    // Too old, price near zero
                    price_change_pct = -0.5;
                }

                price = entry_price * (1.0 + price_change_pct);

                // Check exit conditions
                let exit_result = self.exit_strategy.check_exit(&event.mint, price);
                
                if let Some((sell_amount, reason)) = exit_result {
                    exit_reason = reason;
                    exit_price = price;
                    
                    // Calculate PnL
                    let pnl = ((exit_price - entry_price) / entry_price) * sol_invested;
                    
                    // Process the exit
                    self.exit_strategy.process_exit(&event.mint, sell_amount, reason);
                    
                    // Remove position
                    self.position_manager.close_position(&event.mint, exit_price, &reason);
                    
                    // Record trade result
                    trade.exit_price = exit_price;
                    trade.exit_time = later_event.timestamp;
                    trade.exit_reason = reason.clone();
                    trade.pnl = pnl;
                    trade.holding_minutes = held_minutes;
                    
                    // eprintln!("SELL: {} at {:.4} ({}), PnL {:.4} SOL ({:.1}%), hold {:.1}m", 
                    //     event.mint, exit_price, reason, pnl, pnl / sol_invested * 100.0, held_minutes);
                    
                    trades.push(trade.clone());
                    
                    // Break out of price simulation loop - one exit per trade
                    break;
                }
                
                // Check time-based max hold
                if held_minutes >= self.config.exit.max_hold_minutes as i64 {
                    // Force exit at market
                    exit_reason = "TimeExit".to_string();
                    exit_price = price;
                    let pnl = ((exit_price - entry_price) / entry_price) * sol_invested;
                    
                    self.position_manager.close_position(&event.mint, exit_price, &exit_reason);
                    trade.exit_price = exit_price;
                    trade.exit_time = later_event.timestamp;
                    trade.exit_reason = exit_reason.clone();
                    trade.pnl = pnl;
                    trade.holding_minutes = held_minutes;
                    
                    trades.push(trade.clone());
                    break;
                }
            }

            // Update metrics
            self.metrics.record_trade(trade.pnl, entry_price, trade.exit_price);
        }

        // Close any remaining open positions at the end (force exit at zero)
        let open_positions = self.position_manager.get_open_positions();
        for (mint, pos) in &open_positions {
            // Force exit - position worthless at end of replay period
            let forced_pnl = -pos.sol_invested; // Full loss
            self.metrics.daily_pnl += forced_pnl;
            // eprintln!("FORCE CLOSE: {} - remaining position lost {:.4} SOL", mint, -forced_pnl);
        }

        (self.metrics.clone(), trades)
    }

    /// Simulate entry price from bonding curve reserves
    fn simulate_entry_price(&self, event: &ReplayEvent) -> f64 {
        // Simple model: price inversely proportional to SOL reserves at graduation
        // More SOL reserves = lower price per token (more liquid)
        // For a graduating token at ~83 SOL with typical supply, 
        // we get a reasonable entry "price"
        
        let sol = event.sol_reserves as f64;
        let supply = event.total_supply.max(1) as f64;
        
        // Price ~ SOL / supply (very rough normalization)
        // 83 SOL with ~5M supply → ~0.0166 SOL per 10^6 tokens ≈ entry price level
        let base_price = sol / (supply / 1_000_000.0);
        
        // Add some variance based on timestamp for determinism
        let timestamp_factor = (event.timestamp % 1000) as f64 / 1000.0;
        let price = base_price * (1.0 + timestamp_factor * 0.2);
        
        price.clamp(0.001, 100.0)
    }
}

/// Trade result from replay
#[derive(Debug, Clone, Default)]
pub struct TradeResult {
    pub mint: solana_sdk::pubkey::Pubkey,
    pub entry_price: f64,
    pub entry_time: i64,
    pub exit_price: f64,
    pub exit_time: i64,
    pub exit_reason: String,
    pub pnl: f64,
    pub holding_minutes: i64,
    pub sol_invested: f64,
}

/// Run a replay from CSV file
/// 
/// CSV format: mint,timestamp_unix_secs,sol_reserves,total_supply
/// Example: HYa27pj...gQBa,1700000000,83000000,5000000
pub fn run_replay<P: AsRef<Path>>(
    config: Config,
    events_path: P,
) -> Result<(Metrics, Vec<TradeResult>), String> {
    // Read events
    let events = read_events_from_csv(events_path);
    
    if events.is_empty() {
        return Err("No graduation events found in replay file".to_string());
    }

    // Sort events by timestamp for deterministic processing
    let mut runner = ReplayRunner::new(config, events)?;
    
    let (metrics, trades) = runner.run();
    
    // Print summary
    println!("\n=== REPLAY SUMMARY ===");
    println!("Events processed: {}", events.len());
    println!("Trades executed: {}", trades.len());
    println!("");
    
    if trades.is_empty() {
        println!("No trades were executed.");
    } else {
        let wins: u64 = trades.iter().filter(|t| t.pnl > 0.0).count() as u64;
        let win_rate = (wins as f64 / trades.len() as f64) * 100.0;
        let total_pnl: f64 = trades.iter().map(|t| t.pnl).sum();
        
        println!("Win rate: {:.1}% ({}/{})", win_rate, wins, trades.len());
        println!("Total PnL: {:.4} SOL", total_pnl);
        println!("Average return: {:.2}%", total_pnl / trades.iter().map(|t| t.sol_invested).sum::<f64>().max(1.0) * 100.0);
        
        if !trades.is_empty() {
            let best = trades.iter().max_by(|a, b| a.pnl.partial_cmp(&b.pnl).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
            let worst = trades.iter().min_by(|a, b| a.pnl.partial_cmp(&b.pnl).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
            
            println!("Best trade: {:.4} SOL (reason: {})", best.pnl, best.exit_reason);
            println!("Worst trade: {:.4} SOL (reason: {})", worst.pnl, worst.exit_reason);
        }
        
        // Holding time stats
        let total_hold: i64 = trades.iter().map(|t| t.holding_minutes).sum();
        let avg_hold = total_hold / trades.len() as i64;
        println!("Average holding time: {} minutes", avg_hold);
    }
    
    println!("=====================\n");
    
    Ok((metrics, trades))
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::pubkey;
    
    #[test]
    fn test_read_events_csv() {
        // Create temporary CSV
        let tmp_dir = std::env::temp_dir();
        let csv_path = tmp_dir.join("test_replay_events.csv");
        
        // Write test events
        let mut file = fs::File::create(&csv_path).expect("create tmp file");
        use std::io::Write;
        writeln!(file, "# Test graduation events CSV").expect("write header");
        writeln!(file, "{},{},{}", 
            solana_sdk::pubkey::Pubkey::new_unique().to_string(), 
            1700000000, 
            83000000).expect("write line");
        writeln!(file, "{},{},{}", 
            solana_sdk::pubkey::Pubkey::new_unique().to_string(), 
            1700000060, 
            85000000).expect("write line");
        
        // Read events
        let events = read_events_from_csv(&csv_path);
        assert!(!events.is_empty());
        assert!(events.len() >= 1);
        
        // Cleanup
        let _ = fs::remove_file(&csv_path);
    }
    
    #[test]
    fn test_baseline_filters() {
        // Test liquidity sanity filter
        // 83 SOL should pass (within 83-87 range)
        let pass = check_baseline_filters(83000000, 5000000, 1700000000, 83.0, 87.0, 0, 180);
        assert!(pass, "83 SOL should pass liquidity sanity filter");
        
        // 80 SOL should fail (below min)
        let fail = check_baseline_filters(80000000, 5000000, 1700000000, 83.0, 87.0, 0, 180);
        assert!(!fail, "80 SOL should fail liquidity sanity filter");
        
        // 88 SOL should fail (above max)
        let fail2 = check_baseline_filters(88000000, 5000000, 1700000000, 83.0, 87.0, 0, 180);
        assert!(!fail2, "88 SOL should fail liquidity sanity filter");
        
        // Test with 85 SOL (within range)
        let pass2 = check_baseline_filters(85000000, 5000000, 1700000000, 83.0, 87.0, 0, 180);
        assert!(pass2, "85 SOL should pass liquidity sanity filter");
    }
    
    #[test]
    fn test_replay_with_mock_data() {
        // Create temporary CSV
        let tmp_dir = std::env::temp_dir();
        let csv_path = tmp_dir.join("test_replay_events2.csv");
        
        let mut file = fs::File::create(&csv_path).expect("create tmp file2");
        use std::io::Write;
        writeln!(file, "{},{},{}", 
            "11111111111111111111111111111111", 
            1700000000, 
            83000000).expect("write line2");
        
        let events = read_events_from_csv(&csv_path);
        assert!(!events.is_empty());
        assert_eq!(events[0].sol_reserves, 83000000);
        
        let _ = fs::remove_file(&csv_path);
    }
}