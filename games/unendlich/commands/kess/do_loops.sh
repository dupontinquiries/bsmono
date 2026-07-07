mkdir -p logs

run() {
  local name="$1"; shift
  local stamp=$(date +%Y%m%d-%H%M%S)
  echo "=== $name ==="
  env "$@" \
    cargo run --release --features="render,gpu,hot-shaders" \
      -p unendlich-engine --bin unendlich-world \
    2>&1 | tee "logs/${name}-${stamp}.log" > /dev/null
  echo "--- summary for $name ---"
  awk '/lifetime summary/,/Pre-exit: dropping mesh_pool/' "logs/${name}-${stamp}.log" \
    | tee "logs/${name}-${stamp}.summary"
}

COMMON="UNENDLICH_THREAD_PROBE=1 UNENDLICH_VSYNC=0 UNENDLICH_MAX_DURATION_MS=30000"

run gpu-off-rd10   $COMMON UNENDLICH_GPU_MESHER=0 UNENDLICH_RENDER_DISTANCE=10
run gpu-on-rd10    $COMMON UNENDLICH_GPU_MESHER=1 UNENDLICH_RENDER_DISTANCE=10
run stride8-rd10   $COMMON UNENDLICH_SHADOW_CHUNK_STRIDE=8 UNENDLICH_RENDER_DISTANCE=10
run shadow1024     $COMMON UNENDLICH_SHADOW_CHUNK_STRIDE=8 UNENDLICH_SHADOW_MAP_SIZE=1024 UNENDLICH_RENDER_DISTANCE=10
run cache2048      $COMMON UNENDLICH_MESH_CACHE_SIZE=2048 UNENDLICH_RENDER_DISTANCE=10 UNENDLICH_MAX_DURATION_MS=60000
run everything     $COMMON UNENDLICH_GPU_MESHER=1 UNENDLICH_SHADOW_CHUNK_STRIDE=8 UNENDLICH_SHADOW_MAP_SIZE=1024 UNENDLICH_MESH_CACHE_SIZE=2048 UNENDLICH_RENDER_DISTANCE=16 UNENDLICH_MAX_UPLOADS_PER_FRAME=48 UNENDLICH_UPLOAD_BUDGET_MS=10.0
