// shader_hash_shared.rs — SHARED hashing logic, `include!`d by BOTH build.rs and
// the src/shader_artifacts.rs test module so the build pipeline and the freshness
// check compute byte-identical hashes (no divergence possible).
//
// Do NOT add `use` statements here: fully-qualify std paths so this compiles
// cleanly at either include site regardless of their imports.

/// Transitive `#include` closure of a shader (excluding the shader itself).
/// Follows only real line-start `#include "name"` directives — commented example
/// includes (`// #include "..."`) are ignored, matching the GLSL preprocessor.
/// Each include is resolved relative to the including file's directory.
fn collect_shader_includes(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let mut visited: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    while let Some(file) = stack.pop() {
        let dir = file.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let text = match std::fs::read_to_string(&file) { Ok(t) => t, Err(_) => continue };
        for line in text.lines() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("#include") {
                if let (Some(a), Some(b)) = (rest.find('"'), rest.rfind('"')) {
                    if b > a + 0 && b > a {
                        let inc = dir.join(&rest[a + 1..b]);
                        if !visited.iter().any(|v| v == &inc) {
                            visited.push(inc.clone());
                            out.push(inc.clone());
                            stack.push(inc);
                        }
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// SHA-256 over a shader's transitive source closure: the shader's own bytes
/// followed by each included file's bytes in sorted-path order. A shader with no
/// includes hashes to exactly sha256 of its own file (so non-include sidecars are
/// unaffected by this scheme; only `#include`-bearing shaders gain include bytes).
fn transitive_source_hash(root: &std::path::Path) -> String {
    let mut data = std::fs::read(root).unwrap_or_default();
    for inc in collect_shader_includes(root) {
        if let Ok(bytes) = std::fs::read(&inc) {
            data.extend_from_slice(&bytes);
        }
    }
    sha256_hex(&data)
}

fn sha256_hex(data: &[u8]) -> String {
    // FIPS 180-4 SHA-256.
    const K: [u32; 64] = [
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,
        0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19,
    ];
    let bitlen = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&bitlen.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let mj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(mj);
            hh = g; g = f; f = e; e = d.wrapping_add(t1);
            d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for word in h {
        out.push_str(&format!("{:08x}", word));
    }
    out
}
