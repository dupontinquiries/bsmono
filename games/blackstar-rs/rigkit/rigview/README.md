# rigview

The rig viewer's shell dissolve drawn by a real GL pipeline, for X11 capture.
Built because a CPU copy of the shader answered the wrong question: its renders
looked clean while the GPU drew a hairy spray of spinning slivers.

    rigview <rig-viewer.html> [t1,t2,...]      # six timeline values, 3x2 grid
    RIGVIEW_ONLY_STREAM=1 rigview ...          # dissolving shell only, for measuring

It reads the built page itself -- RIG, CAR_META, CAR_B64, and the page's own
tessellate limits -- so it loads data the way the runtime does. The dissolve
GLSL is extracted from the page by evaluating its voxHook (`extract.sh`), and
read at run time, so iterating on the shader means: rebuild the page,
re-extract, relaunch. No recompile.

    src/bundle.rs    page -> groups, windows, stream flags (headless)
    src/geom.rs      tessellate / smooth normals / unshare with aCell (headless)
    src/timeline.rs  per-part dissolve progress, hide rule (headless, tested)
    src/main.rs      miniquad shell: 3x2 grid, the extracted shader

## Capturing (see docs/agents/HEADLESS_GUI_TESTING.md in the monorepo)

    apt-get install -y --no-install-recommends xvfb openbox xdotool imagemagick libgl1-mesa-dri
    export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1
    nohup setsid Xvfb :99 -screen 0 1500x800x24 >/tmp/xvfb.log 2>&1 </dev/null &
    nohup setsid openbox >/tmp/ob.log 2>&1 </dev/null &
    nohup setsid ./target/release/rigview page.html >/tmp/rv.log 2>&1 </dev/null &
    # wait ~20 s (llvmpipe), then in a NEW call:
    import -window root shot.png

Two traps hit while building this:
- `pkill -f rigview` matched the calling shell (its cwd contains "rigview")
  and killed the tool call. Use `pkill -x`.
- A coverage count included the window's bottom border line, which read as
  3.7% of the shell "reappearing" late. Find the client area from the image.

## Scene mode: the page's own frames

    node tools/dump.js page.html scene.bin 0.62,0.67,0.70,0.74,0.78,0.81
    rigview --scene scene.bin                  # RIGVIEW_FILTER=shell|rest to isolate

`tools/boot.js` runs the page's actual script in node with real three.js r128
(npm three@0.128.0), stubbing only the browser. `dump.js` sets the timeline,
calls the page's own refresh() with heft off, and writes every visible mesh:
geometry once, then per frame its world matrix, dissolve state, colour and the
per-bone skin motions. Nothing about the rig is re-implemented.

Two dump bugs worth remembering:
- The page reuses its per-panel bone-motion arrays every frame, and the file is
  written after all frames. Storing a REFERENCE drew every frame with the last
  frame's skeleton -- strands of fragments that did not exist. Copy at capture.
- The shader extractor needs every three.js type the hook constructs; a missing
  Vector4 left the previous shader on disk silently.
