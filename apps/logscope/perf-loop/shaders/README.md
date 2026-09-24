Drop edited kernels here (same file name as in `crates/gpu-gemm/src/kernels/`), then
`./loop.sh shaders <tag>`. The runner sets LOGSCOPE_SHADER_DIR to the synced copy of this folder and the
engine hot-loads `<name>.wgsl` from it, announcing each override on stderr. Empty folder = no overrides.
