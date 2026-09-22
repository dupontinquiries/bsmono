# chunkup

Split a large tree into upload-sized units. `exclude-ip.txt` lists the units left
out entirely -- one case-insensitive regex per line. It names licensed marks on
purpose, because it is the filter for them (the same exemption `vocab.rs` has).

    python3 chunkup.py --help
