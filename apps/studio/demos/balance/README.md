# Balance — second import test (performance, unified mix + master)

The artist's own track (rights held by the artist), from an FL Studio 20.7
package: 160 BPM, 119 channels, 1844 playlist items, 18 Omnisphere, 35 mixer
inserts, a heavy Ozone / Neutron chain. Two references came with it: the MIX
(balance18, -13.5 LUFS) and a MASTER made separately (balance19, -13.3 LUFS)
because of an FL bug; both peak +2.6 dBFS true peak.

* `balance.import.txt` — what the import did and did not do.
* `balance.master-vs-19.txt`, `balance.mix-vs-18.txt` — ONE Studio project
  rendered twice (`render_project`, and `--dry-master` for the mix) and held up
  against each reference (`compare_reference --ours`). No second project, no
  re-export between mix and master.

Not in git: the media (the artist's recordings) and the .studio file.
