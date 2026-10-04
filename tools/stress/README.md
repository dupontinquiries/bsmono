# stress — crowd the phone on demand, no network

Simulates "a browser with lots of video tabs" with local files only, so you can feel the back of the phone
and the logs show what the governor and the shell do.

1. `sh make-clip.sh ~/Downloads/stress/clip.mp4` (ffmpeg; ~2 MB) and copy `stress.html` next to it.
2. On the phone, start sampling: `sh sample.sh 180 /tmp/stress.csv &`
3. Open it: `flatpak run org.mozilla.firefox file://$HOME/Downloads/stress/stress.html?v=10`
   (the Firefox flatpak sees `~/Downloads`; `?v=20&c=4` adds more videos and busy canvases).
4. Watch Tideshell while it runs: swipe gestures, the keyboard, the lock screen. `journalctl --user -u tideshell`
   and the stall log (when it exists) say what stalled.

The page shows fps and the worst frame in the last second, so a throttled browser is visible without a tool.
Compare runs with the governor and with `CPUWeight` off (`systemctl --user revert tideshell` removes the
drop-in) to see whether the weights matter. Nothing here has been run on the phone yet.
