# Texture and hollow-body survey

Measured before building anything, because the plan as stated cannot work and it
is cheaper to find that out now.

## Brawl has no texture to donate

`imported_brawl_tank` and `imported_brawl_robot` both carry a `texcoord`
attribute, and **every UV in both is (0, 0)** — one distinct UV across 8,938 and
21,310 vertices. Their material is `plate_blue.png`, a placeholder.

So "take Brawl's texture and map it to parts" has nothing to take. Brawl is a
*geometry* reference, which is what the anatomy work uses him for, and a
*texture* recipient rather than a donor.

## But 586 models do have real UVs

| | models | vertices |
|---|---|---|
| real UVs (span > 0.01) | 586 | 4,895,223 |
| all-zero UVs | 547 | 6,927,157 |

41% of the corpus's vertices are textured. The largest donors:

| verts | model |
|---|---|
| 308,037 | `xfill00_part0` |
| 241,530 | `scorponoktrnasformrib_part0` |
| 115,524 | `jazzplus_part0` |
| 83,544 | `jazzevolved_part0` |

**Scorponok is a donor**, which matters: he is also the model with the clearest
body plan (85% paired). A model that is both well-textured and well-structured is
the right template.

Materials tell the same story: `plate_blue.png` covers 256 meshes, while real
names like `fallofcybertrongrimlocklmb_RB_Grimlock_Legs_TEXSET_D_baseColor.jpg`
appear on a handful. Most of the corpus was stripped to a placeholder at import.

## The hollow-body observation is correct

Counting edges shared by exactly one triangle:

| model | open edges | |
|---|---|---|
| brawl tank | 7,266 / 18,603 | **39% open** |
| brawl robot | 16,796 / 43,666 | 38% open |
| hovertank | 3,578 / 49,737 | 7% open |
| scorponok rib | 241,530 / 241,530 | 100% — *unwelded*, not hollow |

These are hollow shells, not solids. Two consequences: a mirrored part copied
across the centreline has its **normals inverted** and renders inside-out, and a
UV transfer onto a shell needs the mirror flag or the texture reads backwards.
`model-verify`'s determinant check already catches the first; the second is the
new work.

Note scorponok's 100%: that is the unwelded case again — every triangle carrying
private vertices, so no edge is ever shared. Weld before measuring openness, or
every unwelded model looks maximally hollow.

## What this changes

The pipeline is not "Brawl's texture onto everything". It is:

1. **Pick a donor per shape class** from the 586 textured models, matched by the
   anatomy pass — a textured tread donates to an untextured tread.
2. **Transfer by projection**, not by vertex correspondence, since donor and
   recipient are different meshes. A shell's UV can be projected from its own
   principal axes; that is generic and needs no matching.
3. **Mirror-aware transfer**: flip the U coordinate for the mirrored twin, or the
   texture reads backwards on one side of every vehicle.
4. **The human skeleton** (`dude.iqm`, 20 joints) remains the generic base for
   models with no usable body plan — the hovertank, which is 65% one welded hull
   with 2% vertex symmetry, is exactly that case.
