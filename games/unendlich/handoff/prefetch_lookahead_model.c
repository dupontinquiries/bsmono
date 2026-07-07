/* prefetch_lookahead_model.c — does motion-predictive prefetch actually hide
 * the on-demand near-mesh stall, and what UNENDLICH_PREFETCH_* values are
 * needed? Pure arithmetic, no deps. gcc handoff/prefetch_lookahead_model.c -o /tmp/pm && /tmp/pm
 *
 * Model: a chunk becomes "needed" when the camera reaches it. To be resident
 * in time, we must REQUEST it at least L (mesh+upload latency) earlier. Our
 * prefetch reach (chunks) = lookahead(frames) * speed(chunks/frame).
 *   speed_chunks_per_frame = (v_blocks_s / CHUNK) / fps
 * Required reach to cover latency L: d = (v_blocks_s/CHUNK) * (L_ms/1000) chunks.
 * Solving reach >= d:  lookahead_frames >= L_ms * fps / 1000   (speed CANCELS).
 * COUNT must cover chunks crossed during L: count >= d.
 */
#include <stdio.h>
#define CHUNK 32.0
int main(void){
  double L_ms = 30.0;            /* observed worst near-mesh+upload */
  int fps_list[] = {60,120,240};
  double v_list[] = {32,128,512,1500}; /* blocks/s: walk .. fast dragon */
  printf("L=%.0fms mesh+upload latency; CHUNK=%.0f blocks\n\n", L_ms, CHUNK);
  printf("required LOOKAHEAD (frames) = L_ms*fps/1000, independent of speed:\n");
  for(int i=0;i<3;i++){
    int fps=fps_list[i];
    printf("  fps=%-4d -> lookahead >= %.1f frames  (default 16 -> covers L up to %.0fms)\n",
           fps, L_ms*fps/1000.0, 16.0*1000.0/fps);
  }
  printf("\nrequired COUNT (chunks crossed during L) by speed:\n");
  for(int j=0;j<4;j++){
    double v=v_list[j];
    double d = (v/CHUNK)*(L_ms/1000.0);
    printf("  v=%-6.0f blocks/s (%.2f chunks/s) -> count >= %.2f chunks  (default 16 -> ample)\n",
           v, v/CHUNK, d);
  }
  printf("\nVerdict: lookahead-in-FRAMES is the binding knob (not speed, not count).\n");
  printf("Defaults LOOKAHEAD=16, COUNT=16 hide L<=%.0fms at 120fps and cover\n", 16.0*1000.0/120.0);
  printf("fast-dragon speeds with margin. If L grows (denser near chunks),\n");
  printf("raise LOOKAHEAD to >= L_ms*fps/1000, not COUNT.\n");
  return 0;
}
