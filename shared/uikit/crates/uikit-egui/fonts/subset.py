from fontTools import subset
RANGES = [
 (0x2190,0x21FF),(0x2200,0x22FF),(0x2300,0x23FF),(0x2400,0x245F),(0x2460,0x24FF),
 (0x25A0,0x25FF),(0x2600,0x26FF),(0x2700,0x27BF),(0x27C0,0x27EF),(0x27F0,0x27FF),
 (0x2900,0x297F),(0x2980,0x29FF),(0x2A00,0x2AFF),(0x2B00,0x2BFF),
]
uni = ",".join(f"U+{a:04X}-{b:04X}" for a,b in RANGES)
for src,dst in [("NotoSansSymbols2-Regular.ttf","NotoSansSymbols2-uisubset.ttf"),
                ("NotoSansMath-Regular.ttf","NotoSansMath-uisubset.ttf")]:
    subset.main([src, f"--unicodes={uni}", f"--output-file={dst}",
                 "--layout-features=", "--no-hinting", "--desubroutinize",
                 "--drop-tables+=DSIG", "--name-IDs=*", "--notdef-outline"])
