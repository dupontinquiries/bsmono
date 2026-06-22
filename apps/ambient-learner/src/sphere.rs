// ============================================================================
// Temperature Sphere Visualization
// ============================================================================
//
// Generates an SVG visualization of temperature distribution across device sensors
// Maps 53 sensors onto a 3D sphere surface with color gradients
//
// Color scheme: Blue (cold) → Cyan → Green → Yellow → Red (hot)

use std::f32::consts::PI;

/// Convert temperature to RGB color
fn temperature_to_rgb(temp: f32) -> (u8, u8, u8) {
    // Map temperature to 0-1 range
    // Typical range: 20°C (cool) to 80°C (hot)
    let normalized = (temp - 20.0) / 60.0;
    let clamped = normalized.clamp(0.0, 1.0);
    
    // Blue → Cyan → Green → Yellow → Red
    if clamped < 0.25 {
        // Blue → Cyan (0-15°C range)
        let t = clamped * 4.0;
        (0, (t * 255.0) as u8, 255)
    } else if clamped < 0.5 {
        // Cyan → Green (15-30°C range)
        let t = (clamped - 0.25) * 4.0;
        (0, 255, ((1.0 - t) * 255.0) as u8)
    } else if clamped < 0.75 {
        // Green → Yellow (30-50°C range)
        let t = (clamped - 0.5) * 4.0;
        ((t * 255.0) as u8, 255, 0)
    } else {
        // Yellow → Red (50-80°C range)
        let t = (clamped - 0.75) * 4.0;
        (255, ((1.0 - t) * 255.0) as u8, 0)
    }
}

/// Convert RGB to hex color string
fn rgb_to_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

/// Map sensor index to 3D sphere coordinates using Fibonacci sphere algorithm
/// Returns (x, y, z) in range [-1, 1]
fn fibonacci_sphere_point(index: usize, total: usize) -> (f32, f32, f32) {
    let golden_ratio = (1.0 + 5.0_f32.sqrt()) / 2.0;
    let theta = 2.0 * PI * (index as f32) / golden_ratio;
    let phi = ((index as f32 / total as f32) * 2.0 - 1.0).acos();
    
    let x = phi.sin() * theta.cos();
    let y = phi.sin() * theta.sin();
    let z = phi.cos();
    
    (x, y, z)
}

/// Generate SVG for temperature sphere visualization
pub fn generate_temperature_sphere(
    sensors: &[f32],
    width: u32,
    height: u32,
) -> String {
    if sensors.is_empty() {
        return String::from(r#"<svg></svg>"#);
    }
    
    let center_x = width as f32 / 2.0;
    let center_y = height as f32 / 2.0;
    let radius = (width.min(height) as f32 / 2.0) * 0.85;
    
    let mut svg = format!(
        r#"<svg width="{}" height="{}" viewBox="0 0 {} {}" xmlns="http://www.w3.org/2000/svg">"#,
        width, height, width, height
    );
    
    // Background circle (average temperature)
    let avg_temp: f32 = sensors.iter().sum::<f32>() / sensors.len() as f32;
    let (r, g, b) = temperature_to_rgb(avg_temp);
    let bg_color = rgb_to_hex(r, g, b);
    
    svg.push_str(&format!(
        r#"<circle cx="{:.1}" cy="{:.1}" r="{:.1}" fill="{}" opacity="0.15"/>"#,
        center_x, center_y, radius, bg_color
    ));
    
    // Sort sensors by z-coordinate (back to front)
    let mut sensor_data: Vec<(usize, f32, f32, f32, f32)> = sensors
        .iter()
        .enumerate()
        .map(|(i, &temp)| {
            let (x, y, z) = fibonacci_sphere_point(i, sensors.len());
            (i, x, y, z, temp)
        })
        .collect();
    
    sensor_data.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap());
    
    // Draw sensor points (back to front for proper layering)
    for (_i, x, y, z, temp) in sensor_data {
        // Project 3D → 2D (orthographic projection)
        let px = center_x + x * radius;
        let py = center_y - y * radius; // Flip Y for screen coordinates
        
        // Size based on z-depth (larger = closer to viewer)
        let base_size = 4.0;
        let size = base_size + z * 3.0;
        
        // Opacity based on depth (more opaque = closer)
        let opacity = 0.5 + z * 0.5;
        
        let (r, g, b) = temperature_to_rgb(temp);
        let color = rgb_to_hex(r, g, b);
        
        // Glow effect for hot sensors
        if temp > 60.0 {
            svg.push_str(&format!(
                r#"<circle cx="{:.1}" cy="{:.1}" r="{:.1}" fill="{}" opacity="0.3" filter="blur(2)"/>"#,
                px, py, size * 2.0, color
            ));
        }
        
        // Main sensor point
        svg.push_str(&format!(
            r#"<circle cx="{:.1}" cy="{:.1}" r="{:.1}" fill="{}" opacity="{:.2}" stroke="black" stroke-width="0.5"/>"#,
            px, py, size, color, opacity
        ));
    }
    
    // NSEW cardinal labels
    svg.push_str(&format!(
        r#"<text x="{:.1}" y="15" text-anchor="middle" font-size="14" font-weight="bold" fill="gray">N</text>"#,
        center_x
    ));
    svg.push_str(&format!(
        r#"<text x="{:.1}" y="{}" text-anchor="middle" font-size="14" font-weight="bold" fill="gray">S</text>"#,
        center_x, height - 5
    ));
    svg.push_str(&format!(
        r#"<text x="15" y="{:.1}" text-anchor="start" font-size="14" font-weight="bold" fill="gray">W</text>"#,
        center_y
    ));
    svg.push_str(&format!(
        r#"<text x="{}" y="{:.1}" text-anchor="end" font-size="14" font-weight="bold" fill="gray">E</text>"#,
        width - 15, center_y
    ));
    
    // Temperature legend
    let legend_x = 20.0;
    let legend_y = height as f32 - 80.0;
    let legend_height = 60.0;
    let legend_width = 20.0;
    
    // Gradient definition
    svg.push_str(r#"<defs><linearGradient id="tempGrad" x1="0%" y1="100%" x2="0%" y2="0%">"#);
    for i in 0..=10 {
        let temp = 20.0 + (60.0 * i as f32 / 10.0);
        let (r, g, b) = temperature_to_rgb(temp);
        let color = rgb_to_hex(r, g, b);
        let offset = i * 10;
        svg.push_str(&format!(
            r#"<stop offset="{}%" stop-color="{}"/>"#,
            offset, color
        ));
    }
    svg.push_str(r#"</linearGradient></defs>"#);
    
    // Legend rectangle
    svg.push_str(&format!(
        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="url(#tempGrad)" stroke="black" stroke-width="1"/>"#,
        legend_x, legend_y, legend_width, legend_height
    ));
    
    // Legend labels
    svg.push_str(&format!(
        r#"<text x="{}" y="{}" font-size="10" fill="gray">80°C</text>"#,
        legend_x + legend_width + 5.0, legend_y + 5.0
    ));
    svg.push_str(&format!(
        r#"<text x="{}" y="{}" font-size="10" fill="gray">50°C</text>"#,
        legend_x + legend_width + 5.0, legend_y + legend_height / 2.0
    ));
    svg.push_str(&format!(
        r#"<text x="{}" y="{}" font-size="10" fill="gray">20°C</text>"#,
        legend_x + legend_width + 5.0, legend_y + legend_height
    ));
    
    svg.push_str("</svg>");
    svg
}

/// Generate a cached version of the sphere (only regenerate if temps changed significantly)
pub struct SphereCache {
    last_sensors: Vec<f32>,
    last_svg: String,
    threshold: f32,  // Minimum change to trigger regeneration
}

impl SphereCache {
    pub fn new() -> Self {
        Self {
            last_sensors: Vec::new(),
            last_svg: String::new(),
            threshold: 0.5,  // Regenerate if any sensor changes by >0.5°C
        }
    }
    
    pub fn get_sphere(&mut self, sensors: &[f32], width: u32, height: u32) -> &str {
        // Check if regeneration needed
        let needs_update = if self.last_sensors.len() != sensors.len() {
            true
        } else {
            sensors.iter()
                .zip(self.last_sensors.iter())
                .any(|(a, b)| (a - b).abs() > self.threshold)
        };
        
        if needs_update {
            self.last_svg = generate_temperature_sphere(sensors, width, height);
            self.last_sensors = sensors.to_vec();
        }
        
        &self.last_svg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_temperature_to_rgb() {
        // Cold (blue)
        let (r, g, b) = temperature_to_rgb(20.0);
        assert_eq!(r, 0);
        assert_eq!(b, 255);
        
        // Hot (red)
        let (r, g, b) = temperature_to_rgb(80.0);
        assert_eq!(r, 255);
        assert_eq!(b, 0);
    }
    
    #[test]
    fn test_fibonacci_sphere() {
        let (x, y, z) = fibonacci_sphere_point(0, 10);
        let len = (x*x + y*y + z*z).sqrt();
        assert!((len - 1.0).abs() < 0.01); // Should be on unit sphere
    }
    
    #[test]
    fn test_generate_sphere() {
        let sensors = vec![25.0, 30.0, 35.0, 40.0];
        let svg = generate_temperature_sphere(&sensors, 400, 400);
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("circle"));
    }
}
