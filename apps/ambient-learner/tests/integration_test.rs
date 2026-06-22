#[cfg(test)]
mod tests {
    #[test]
    fn test_basic() {
        assert_eq!(2 + 2, 4);
    }
    
    #[test]
    fn test_temperature_bounds() {
        let temp = 25.0;
        assert!(temp > -100.0 && temp < 150.0);
    }
}
