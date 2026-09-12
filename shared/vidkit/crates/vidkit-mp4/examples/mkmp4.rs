fn main() {
    use vidkit_mp4::build::*;
    let sps: &[u8] = &[0x67,0x64,0x00,0x28,0xAA];
    let pps: &[u8] = &[0x68,0xEE,0x3C,0xB0];
    let n = 30u32;
    let mut payloads = Vec::new();
    for i in 0..n { let b = vec![0xA0 | (i as u8 & 0x0F); 12];
        let mut s=(b.len() as u32).to_be_bytes().to_vec(); s.extend_from_slice(&b); payloads.push(s); }
    let sizes: Vec<u32> = payloads.iter().map(|p| p.len() as u32).collect();
    let mut mdat_body=Vec::new(); let mut chunk_rel=Vec::new();
    for pair in payloads.chunks(2) { chunk_rel.push(mdat_body.len());
        for p in pair { mdat_body.extend_from_slice(p); } }
    let mdat = bx(b"mdat", &mdat_body);
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");
    let make = |at: u64| {
        let stco = table(b"stco", &chunk_rel.iter().map(|r| u32b((at + *r as u64) as u32)).collect::<Vec<_>>());
        let stbl = bx(b"stbl", &cat(&[
            stsd(visual_entry(b"avc1", 1280, 720, avcc(4, sps, pps))),
            table(b"stts", &[cat(&[u32b(n), u32b(1000)])]),
            table(b"stsc", &[cat(&[u32b(1), u32b(2), u32b(1)])]),
            stsz(&sizes), stco]));
        let minf = bx(b"minf", &cat(&[bx(b"vmhd", &[0u8;12]), stbl]));
        let mdia = bx(b"mdia", &cat(&[mdhd(30_000, n*1000), hdlr(b"vide"), minf]));
        let trak = bx(b"trak", &cat(&[tkhd(1, 1280, 720), mdia]));
        bx(b"moov", &cat(&[bx(b"mvhd", &[0u8;100]), trak]))
    };
    let probe = make(0);
    let at = (ftyp.len()+probe.len()+8) as u64;
    let moov = make(at);
    assert_eq!(moov.len(), probe.len());
    std::fs::write("/tmp/sample.mp4", cat(&[ftyp, moov, mdat])).unwrap();
    println!("wrote /tmp/sample.mp4");
}
