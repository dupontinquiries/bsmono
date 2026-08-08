//! 4x6 bitmap glyphs for the HUD. Each glyph is six 4-bit rows packed
//! into a u32 (top row in the high nibble of the low 24 bits).

pub const GW: usize = 4;
pub const GH: usize = 6;

pub fn glyph(c: char) -> u32 {
    match c.to_ascii_uppercase() {
        '0' => 0x_6999_96,
        '1' => 0x_2622_27,
        '2' => 0x_6912_4F,
        '3' => 0x_E161_1E,
        '4' => 0x_99F1_11,
        '5' => 0x_F8E1_1E,
        '6' => 0x_78E9_96,
        '7' => 0x_F122_44,
        '8' => 0x_6969_96,
        '9' => 0x_6997_1E,
        'A' => 0x_699F_99,
        'B' => 0x_E9E9_9E,
        'C' => 0x_6988_96,
        'D' => 0x_E999_9E,
        'E' => 0x_F8E8_8F,
        'F' => 0x_F8E8_88,
        'G' => 0x_698B_97,
        'H' => 0x_99F9_99,
        'I' => 0x_7222_27,
        'J' => 0x_7111_96,
        'K' => 0x_9ACA_A9,
        'L' => 0x_8888_8F,
        'M' => 0x_9FF9_99,
        'N' => 0x_9DDB_B9,
        'O' => 0x_6999_96,
        'P' => 0x_E99E_88,
        'Q' => 0x_6999_B7,
        'R' => 0x_E99E_A9,
        'S' => 0x_7861_1E,
        'T' => 0x_F222_22,
        'U' => 0x_9999_96,
        'V' => 0x_9999_66,
        'W' => 0x_999F_F9,
        'X' => 0x_9966_99,
        'Y' => 0x_9962_22,
        'Z' => 0x_F124_8F,
        ':' => 0x_0202_00,
        '.' => 0x_0000_02,
        '/' => 0x_1122_48,
        '-' => 0x_000F_00,
        '+' => 0x_0027_20,
        '%' => 0x_9124_89,
        _ => 0,
    }
}

/// draw text into an RGB buffer; returns width used
pub fn draw(
    buf: &mut [u8],
    w: usize,
    x: usize,
    y: usize,
    text: &str,
    color: [u8; 3],
    scale: usize,
) -> usize {
    let mut cx = x;
    for c in text.chars() {
        if c != ' ' {
            let g = glyph(c);
            for row in 0..GH {
                let bits = (g >> ((GH - 1 - row) * 4)) & 0xF;
                for col in 0..GW {
                    if bits & (0x8 >> col) != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let px = cx + col * scale + sx;
                                let py = y + row * scale + sy;
                                let i = (py * w + px) * 3;
                                if i + 2 < buf.len() {
                                    buf[i] = color[0];
                                    buf[i + 1] = color[1];
                                    buf[i + 2] = color[2];
                                }
                            }
                        }
                    }
                }
            }
        }
        cx += (GW + 1) * scale;
    }
    cx - x
}
