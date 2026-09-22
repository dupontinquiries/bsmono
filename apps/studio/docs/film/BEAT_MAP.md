# BEAT_MAP — what is on the footage

Ground truth for every camera file, checked against picture (contact
sheets) and transcripts. Where this disagrees with the script, this wins.

## Beats

| beat | what | camera files | recorder files |
|---|---|---|---|
| P1 | opening applause | C0017 curtain call (reuse) | STE-051 |
| S1 | nun scene: Gloria leaves Max with Sister Frances | C0050–C0069 (plano 1 wides, 2 intro, 3 Max runs back, 4 Gloria close-ups) | STE-013…021, 099–102; church room tone STE-000, STE-109 |
| S2 | theatre jog, casting poster | C0038–C0041; poster inserts C0070–C0074 | STE-084–087 |
| S2b | teens outside the theatre (not placed in any order yet) | C0042–C0048 | STE-089–093 |
| S3 | home confrontation (father Nelson, "final notice") | **no picture in these files** | STE-110, 111, 113–117 |
| S4 | church steps | C0075 (one wide), maybe C0049 | STE-103–108 |
| S5 | audition for "Death of Innocence" | C0010–C0014 | STE-043–048 |
| N2/N3 | café — footage is Max with a girl (romance extra), not Jude | C0028–C0029; C0037 is the "rose de mai" exterior | STE-064, 065; café room tone STE-062 |
| N3 | extra girlfriend scene, theatre corridor | C0021–C0023 | unconfirmed |
| S6 | Max's apartment, Jude invites him out | C0000–C0007 | STE-025–037 |
| N7 | rooftop party | C0030–C0036 | STE-069–075 |
| N8 | death call (Max side C0008–09, mother side C0025–27) | C0008, C0009, C0025–C0027 | STE-038, 039, 059, 060 |
| P2 | the play on stage; C0017 bows | C0015–C0020, C0024 | STE-049–058; theatre room tone STE-057, STE-058 |

## Open questions for the director

1. Rev 8 or the 09-18 cut? (see AI_GUIDE §3)
2. Do camera files for S3 (home confrontation, Emmanuel's apartment) exist
   outside this upload?
3. Where does S2b (teens outside the theatre) go?
4. The canon order names "N2 café — Jude + Max"; the café footage is Max and
   a girl. Same scene, or is a Jude café scene missing?

## Every camera file

Generated from three independent readings:
- **beat:** picture plus transcript, by a person.
- **spoken slate:** `studio_video::spoken_slate` over the whisper transcript.
  Shot letter A = plano 1, B = plano 2. `-` = nothing readable.
- **recorder / offset:** `studio_video::dualsync` over all 77 × 114 files,
  "Sure" pairs only. Recorder position = camera position − offset. An empty
  cell means no confident pairing. That is expected for wides and inserts
  with no dialogue, and it is a job for a person's ear.

Offsets are measured on 16 kHz audio. Re-run `dualsync_pool` on the full-rate
audio before a final conform.

| clip | beat | spoken slate | recorder (Sure) | offset s | notes |
|---|---|---|---|---|---|
| 026-…C0038 | - | - | STE-084 | -9.98 | misnamed duplicate of C0038, exclude |
| C0000 | S6 | - | STE-025 | -3.65 |  |
| C0001 | S6 | 12A-1 | STE-026 | -3.29 |  |
| C0002 | S6 | 12A-2 | STE-031 | -5.24 |  |
| C0003 | S6 | 12B-1 | STE-032 | -5.99 |  |
| C0004 | S6 | - | STE-033 | -10.33 |  |
| C0005 | S6 | 12B-3 | STE-034 | -3.37 |  |
| C0006 | S6 | 12C-1 | STE-036 | -3.78 |  |
| C0007 | S6 | 12-2 | STE-037 | -3.46 |  |
| C0008 | N8 | 14A-1 | STE-038 | -0.74 |  |
| C0009 | N8 | 14A-2 | STE-039 | -3.58 |  |
| C0010 | S5 | - | STE-043 | -5.97 | whisper loop |
| C0011 | S5 | 10A-2 | STE-044 | -4.74 |  |
| C0012 | S5 | 10B-1 | STE-045 | -3.37 |  |
| C0013 | S5 | - | STE-046 | -5.04 |  |
| C0014 | S5 | - | STE-048 | -4.08 |  |
| C0015 | P2 | 11A-1 | STE-049 | -3.07 |  |
| C0016 | P2 | 11A-2 | STE-050 | -2.63 |  |
| C0017 | P2 | 11B-1 | STE-051 | -3.58 |  |
| C0018 | P2 | 11 |  |  |  |
| C0019 | P2 | 11-1 |  |  | whisper loop |
| C0020 | P2 | 11-2 |  |  |  |
| C0021 | N3 | - |  |  |  |
| C0022 | N3 | - |  |  |  |
| C0023 | N3 | - |  |  |  |
| C0024 | P2 | 11E-1 | STE-055 | -1.81 |  |
| C0025 | N8 | 14A-1 | STE-059 | -9.62 |  |
| C0026 | N8 | - |  |  |  |
| C0027 | N8 | 14A-2 | STE-060 | -3.19 |  |
| C0028 | N3? | EXTRA ROMANCEB-1 |  |  | whisper loop |
| C0029 | N3? | EXTRA ROMANCEB-2 | STE-065 | -4.24 |  |
| C0030 | N7 | 13A-1 | STE-069 | -2.28 |  |
| C0031 | N7 | 3A-1 | STE-070 | -2.34 | slate misheard: 13 |
| C0032 | N7 | 13A-2 | STE-071 | -2.81 |  |
| C0033 | N7 | 13B-1 | STE-072 | -2.85 |  |
| C0034 | N7 | 13B-2 | STE-073 | -3.41 |  |
| C0035 | N7 | 3C-1 | STE-074 | -2.85 | slate misheard: 13 |
| C0036 | N7 | 13C-2 | STE-075 | -3.40 |  |
| C0037 | N2? | - | STE-078 | -20.78 |  |
| C0038 | S2 | 2-1 | STE-084 | -9.98 |  |
| C0039 | S2 | 2-2 | STE-085 | -8.85 |  |
| C0040 | S2 | 2-1 | STE-087 | -5.22 |  |
| C0041 | S2 | - |  |  | whisper loop |
| C0042 | S2b | 2.1-1 | STE-089 | -4.34 | whisper loop |
| C0043 | S2b | 2.1-2 | STE-090 | -3.68 |  |
| C0044 | S2b | 2.1-1 |  |  |  |
| C0045 | S2b | - |  |  |  |
| C0046 | S2b | - |  |  | whisper stockphrase |
| C0047 | S2b | 2.1-3 |  |  |  |
| C0048 | S2b | - |  |  |  |
| C0049 | S1/S4 | - |  |  |  |
| C0050 | S1 | 1A-1 |  |  | whisper stockphrase |
| C0051 | S1 | 1A-2 |  |  | whisper loop |
| C0052 | S1 | 1A-3 |  |  |  |
| C0053 | S1 | 1A-4 |  |  |  |
| C0054 | S1 | 1A-5 |  |  |  |
| C0055 | S1 | 1A-6 |  |  |  |
| C0056 | S1 | 1A-7 |  |  |  |
| C0057 | S1 | 1B-1 | STE-013 | -3.58 |  |
| C0058 | S1 | 1B-2 | STE-015 | -1.84 |  |
| C0059 | S1 | 1C-1 | STE-016 | -6.82 |  |
| C0060 | S1 | 1C-2 | STE-017 | -2.19 |  |
| C0061 | S1 | 1C-3 | STE-018 | -1.75 |  |
| C0062 | S1 | 1C-4 | STE-019 | -1.57 |  |
| C0063 | S1 | - |  |  | whisper stockphrase |
| C0064 | S1 | 1D-1 | STE-020 | -1.70 |  |
| C0065 | S1 | 1D-2 |  |  |  |
| C0066 | S1 | 1D-3 | STE-099 | +3.20 |  |
| C0067 | S1 | 1D-4 | STE-100 | -0.62 |  |
| C0068 | S1 | 1D-5 | STE-101 | -0.88 |  |
| C0069 | S1 | 1D-6 | STE-102 | -3.70 |  |
| C0070 | S2 | - |  |  | whisper stockphrase |
| C0071 | S2 | - |  |  | whisper stockphrase |
| C0072 | S2 | - |  |  |  |
| C0073 | S2 | - |  |  |  |
| C0074 | S2 | - |  |  |  |
| C0075 | S4 | 7-1 |  |  |  |
