# HANDOFF — foliage texture art (r336-textures bonus, coordinator)
Leaves & bushes (OakLeaves/BirchLeaves/DeadBush/Vine) were unregistered in `block_texture`,
so they rendered the untextured material-colour PLACEHOLDER. They now resolve to a tinted
existing tile (T_DIRT tinted foliage greens/brown) — the placeholder is gone, but the pixels
are borrowed. ACTION (asset step): paint dedicated leaf/foliage tiles into BLOCK_ATLAS free
slots (indices 4, 8, 9 are unused in the 4×3 atlas) and repoint these four arms at them; the
cross-billboard plants (Fern/TallGrass) render via the subvoxel path, not block_texture.
