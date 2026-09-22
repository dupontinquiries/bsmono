//! elf_deps: what a dynamically linked binary needs at run time, read from
//! the file itself (no `readelf` on a phone or tablet to ask).
//!
//! Used for apps launched inside the Alpine root on a reMarkable: each
//! `DT_NEEDED` library becomes `so:<name>` for `apk add`, which installs the
//! package that provides it (Alpine indexes list `so:` provides). So a build
//! that links libasound brings alsa-lib into the root without anyone
//! writing a package list.
//!
//! Little-endian ELF32 (rM1/rM2, armv7) and ELF64 (Paper Pro family).

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElfInfo {
    /// PT_INTERP: the dynamic loader, e.g. `/lib/ld-musl-aarch64.so.1`.
    /// None for a static binary.
    pub interp: Option<String>,
    /// DT_NEEDED entries, in order.
    pub needed: Vec<String>,
}

impl ElfInfo {
    pub fn is_musl_dynamic(&self) -> bool {
        self.interp.as_deref().is_some_and(|i| i.contains("ld-musl-"))
    }

    /// `so:<lib>` for each needed library except musl's libc itself (it IS
    /// the loader; apk names it `so:libc.musl-<arch>.so.1`, already in any
    /// root).
    pub fn apk_provides(&self) -> Vec<String> {
        self.needed.iter().filter(|n| !n.starts_with("libc.musl-") && n.as_str() != "libc.so").map(|n| format!("so:{n}")).collect()
    }
}

pub fn elf_info(b: &[u8]) -> Result<ElfInfo, String> {
    if b.len() < 52 || &b[..4] != b"\x7fELF" {
        return Err("not an ELF file".into());
    }
    let is64 = match b[4] { 1 => false, 2 => true, _ => return Err("unknown ELF class".into()) };
    if b[5] != 1 {
        return Err("big-endian ELF".into());
    }
    let u16at = |o: usize| -> Result<u64, String> { Ok(u16::from_le_bytes(b.get(o..o + 2).ok_or("truncated ELF")?.try_into().unwrap()) as u64) };
    let u32at = |o: usize| -> Result<u64, String> { Ok(u32::from_le_bytes(b.get(o..o + 4).ok_or("truncated ELF")?.try_into().unwrap()) as u64) };
    let u64at = |o: usize| -> Result<u64, String> { Ok(u64::from_le_bytes(b.get(o..o + 8).ok_or("truncated ELF")?.try_into().unwrap())) };
    let word = |o: usize| if is64 { u64at(o) } else { u32at(o) };
    let (phoff, phentsize, phnum) = if is64 {
        (u64at(0x20)? as usize, u16at(0x36)? as usize, u16at(0x38)? as usize)
    } else {
        (u32at(0x1c)? as usize, u16at(0x2a)? as usize, u16at(0x2c)? as usize)
    };
    // Program header fields: (type, offset, vaddr, filesz)
    let ph = |i: usize| -> Result<(u64, u64, u64, u64), String> {
        let o = phoff + i * phentsize;
        if is64 {
            Ok((u32at(o)?, u64at(o + 8)?, u64at(o + 16)?, u64at(o + 32)?))
        } else {
            Ok((u32at(o)?, u32at(o + 4)?, u32at(o + 8)?, u32at(o + 16)?))
        }
    };
    let cstr = |off: usize| -> Result<String, String> {
        let s = b.get(off..).ok_or("string out of range")?;
        let end = s.iter().position(|c| *c == 0).ok_or("unterminated string")?;
        Ok(String::from_utf8_lossy(&s[..end]).into_owned())
    };
    let mut info = ElfInfo::default();
    let (mut loads, mut dynamic) = (Vec::new(), None);
    for i in 0..phnum {
        let (ty, off, vaddr, filesz) = ph(i)?;
        match ty {
            1 => loads.push((vaddr, off, filesz)),  // PT_LOAD
            2 => dynamic = Some((off, filesz)),     // PT_DYNAMIC
            3 => info.interp = Some(cstr(off as usize)?), // PT_INTERP
            _ => {}
        }
    }
    let Some((doff, dsize)) = dynamic else { return Ok(info) };
    // Dynamic entries: (tag, value) words. DT_NEEDED=1 (string table
    // offsets), DT_STRTAB=5 (a VIRTUAL address, mapped via PT_LOAD).
    let esz = if is64 { 16 } else { 8 };
    let (mut needed_offs, mut strtab) = (Vec::new(), None);
    let mut o = doff as usize;
    while o + esz <= (doff + dsize) as usize {
        let (tag, val) = (word(o)?, word(o + esz / 2)?);
        match tag {
            0 => break,
            1 => needed_offs.push(val),
            5 => strtab = Some(val),
            _ => {}
        }
        o += esz;
    }
    let strtab = strtab.ok_or("dynamic section without a string table")?;
    let file_off = loads.iter().find(|(v, _, sz)| strtab >= *v && strtab < v + sz)
        .map(|(v, off, _)| off + (strtab - v))
        .ok_or("string table outside any loaded segment")?;
    for n in needed_offs {
        info.needed.push(cstr((file_off + n) as usize)?);
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_dynamic_binary_names_its_loader_and_libraries() {
        let Ok(bytes) = std::fs::read("/bin/ls") else { return };
        let i = elf_info(&bytes).unwrap();
        assert!(i.interp.as_deref().is_some_and(|p| p.contains("ld-")), "{i:?}");
        assert!(i.needed.iter().any(|n| n.starts_with("libc.so")), "{i:?}");
        assert!(!i.is_musl_dynamic(), "this host is glibc");
    }

    #[test]
    fn musl_libc_is_not_asked_for_and_the_rest_become_so_provides() {
        let i = ElfInfo { interp: Some("/lib/ld-musl-aarch64.so.1".into()), needed: vec!["libasound.so.2".into(), "libgcc_s.so.1".into(), "libc.musl-aarch64.so.1".into()] };
        assert!(i.is_musl_dynamic());
        assert_eq!(i.apk_provides(), vec!["so:libasound.so.2", "so:libgcc_s.so.1"]);
    }

    #[test]
    fn garbage_is_refused() {
        assert!(elf_info(b"not an elf at all, just some bytes of text here ok").is_err());
        assert!(elf_info(&[0x7f, b'E', b'L', b'F', 2, 1]).is_err());
    }
}
