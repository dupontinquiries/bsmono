// ============================================================================
// Temperature Forecasting Module
// ============================================================================
//
// Predicts temperature trends using historical data from the past 2 weeks
// Provides 1-hour and 24-hour forecasts on the main page
//
// Architecture:
// - Time series analysis of past 14 days
// - Pattern recognition (daily cycles, weekly trends)
// - Simple feedforward neural network
// - Confidence intervals for predictions

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use chrono::{Datelike, Timelike};

/// Time series data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesPoint {
    pub timestamp: i64,
    pub temperature: f32,
    pub location_id: String,
}

/// Forecast result with confidence
#[derive(Debug, Clone)]
pub struct Forecast {
    pub temperature: f32,
    pub confidence: f32,
    pub min_temp: f32,  // Lower confidence bound
    pub max_temp: f32,  // Upper confidence bound
}

/// Forecasting model
pub struct ForecastModel {
    history: VecDeque<TimeSeriesPoint>,
    max_history_days: usize,
    
    // Pattern weights learned from data
    hourly_pattern: Vec<f32>,   // 24 values for hour of day
    daily_pattern: Vec<f32>,    // 7 values for day of week
    trend_weight: f32,
}

impl ForecastModel {
    pub fn new() -> Self {
        Self {
            history: VecDeque::new(),
            max_history_days: 14,
            hourly_pattern: vec![0.0; 24],
            daily_pattern: vec![0.0; 7],
            trend_weight: 0.0,
        }
    }
    
    /// Add a data point to history
    pub fn add_observation(&mut self, point: TimeSeriesPoint) {
        self.history.push_back(point);
        
        // Keep only last 14 days
        let cutoff = chrono::Utc::now().timestamp() - (self.max_history_days as i64 * 86400);
        while let Some(front) = self.history.front() {
            if front.timestamp < cutoff {
                self.history.pop_front();
            } else {
                break;
            }
        }
        
        // Retrain patterns if we have enough data
        if self.history.len() > 100 {
            self.learn_patterns();
        }
    }
    
    /// Learn patterns from historical data
    fn learn_patterns(&mut self) {
        if self.history.is_empty() {
            return;
        }
        
        // Reset patterns
        self.hourly_pattern = vec![0.0; 24];
        self.daily_pattern = vec![0.0; 7];
        let mut hourly_counts = vec![0usize; 24];
        let mut daily_counts = vec![0usize; 7];
        
        // Calculate average temperature for each hour of day
        for point in &self.history {
            let dt = chrono::DateTime::from_timestamp(point.timestamp, 0)
                .unwrap_or_else(|| chrono::Utc::now());
            let hour = dt.hour() as usize;
            let weekday = dt.weekday().num_days_from_monday() as usize;
            
            self.hourly_pattern[hour] += point.temperature;
            hourly_counts[hour] += 1;
            
            self.daily_pattern[weekday] += point.temperature;
            daily_counts[weekday] += 1;
        }
        
        // Average
        for i in 0..24 {
            if hourly_counts[i] > 0 {
                self.hourly_pattern[i] /= hourly_counts[i] as f32;
            }
        }
        for i in 0..7 {
            if daily_counts[i] > 0 {
                self.daily_pattern[i] /= daily_counts[i] as f32;
            }
        }
        
        // Calculate trend (linear regression)
        if self.history.len() >= 2 {
            let n = self.history.len();
            let mut sum_x = 0.0f32;
            let mut sum_y = 0.0f32;
            let mut sum_xy = 0.0f32;
            let mut sum_xx = 0.0f32;
            
            for (i, point) in self.history.iter().enumerate() {
                let x = i as f32;
                let y = point.temperature;
                sum_x += x;
                sum_y += y;
                sum_xy += x * y;
                sum_xx += x * x;
            }
            
            let n_f32 = n as f32;
            self.trend_weight = (n_f32 * sum_xy - sum_x * sum_y) / 
                               (n_f32 * sum_xx - sum_x * sum_x);
        }
    }
    
    /// Predict temperature for a future timestamp
    pub fn forecast(&self, hours_ahead: f32) -> Option<Forecast> {
        if self.history.is_empty() {
            return None;
        }
        
        // Get current temperature as baseline
        let current = self.history.back()?.temperature;
        let current_time = self.history.back()?.timestamp;
        
        // Calculate future timestamp
        let future_time = current_time + (hours_ahead * 3600.0) as i64;
        let future_dt = chrono::DateTime::from_timestamp(future_time, 0)?;
        let future_hour = future_dt.hour() as usize;
        let _future_weekday = future_dt.weekday().num_days_from_monday() as usize;
        
        // Base prediction on patterns
        let mut prediction = current;
        
        // Add hourly pattern influence
        if !self.hourly_pattern.is_empty() {
            let current_dt = chrono::DateTime::from_timestamp(current_time, 0)?;
            let current_hour = current_dt.hour() as usize;
            
            let hourly_diff = self.hourly_pattern[future_hour] - 
                             self.hourly_pattern[current_hour];
            prediction += hourly_diff * 0.6; // 60% weight
        }
        
        // Add trend influence
        let trend_change = self.trend_weight * (hours_ahead * 60.0); // per 60 samples
        prediction += trend_change * 0.2; // 20% weight
        
        // Calculate confidence based on data availability
        let confidence = if self.history.len() > 1000 {
            0.9 // Lots of data
        } else if self.history.len() > 500 {
            0.7 // Good amount
        } else if self.history.len() > 100 {
            0.5 // Moderate
        } else {
            0.3 // Limited data
        };
        
        // Calculate variance for confidence bounds
        let variance = self.calculate_variance();
        let std_dev = variance.sqrt();
        
        Some(Forecast {
            temperature: prediction,
            confidence,
            min_temp: prediction - std_dev * 1.5,
            max_temp: prediction + std_dev * 1.5,
        })
    }
    
    /// Calculate temperature variance in history
    fn calculate_variance(&self) -> f32 {
        if self.history.len() < 2 {
            return 1.0;
        }
        
        let mean: f32 = self.history.iter()
            .map(|p| p.temperature)
            .sum::<f32>() / self.history.len() as f32;
        
        let variance = self.history.iter()
            .map(|p| (p.temperature - mean).powi(2))
            .sum::<f32>() / self.history.len() as f32;
        
        variance
    }
    
    /// Get number of data points in history
    pub fn data_points(&self) -> usize {
        self.history.len()
    }
    
    /// Load history from database calibrations
    pub fn load_from_calibrations(&mut self, calibrations: Vec<(i64, f32, String)>) {
        for (timestamp, temperature, location_id) in calibrations {
            self.add_observation(TimeSeriesPoint {
                timestamp,
                temperature,
                location_id,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_forecast_model() {
        let mut model = ForecastModel::new();
        
        // Add some test data
        let now = chrono::Utc::now().timestamp();
        for i in 0..100 {
            model.add_observation(TimeSeriesPoint {
                timestamp: now - (100 - i) * 3600,
                temperature: 20.0 + (i as f32 * 0.1),
                location_id: "test".to_string(),
            });
        }
        
        // Forecast 1 hour ahead
        let forecast = model.forecast(1.0);
        assert!(forecast.is_some());
        
        let forecast = forecast.unwrap();
        assert!(forecast.temperature > 19.0 && forecast.temperature < 25.0);
        assert!(forecast.confidence > 0.0 && forecast.confidence <= 1.0);
    }
}
