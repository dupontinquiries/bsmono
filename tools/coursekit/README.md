# coursekit

Turn `.docx` / `.pptx` / `.pdf` lecture files into markdown with a `.assets/`
sidecar -- the layout voiceterm resolves -- with headings recovered from run size,
images deduplicated by content hash, and speaker notes kept.

    python3 coursekit.py probe   <in>              what would happen, no writes
    python3 coursekit.py convert <in> -o notes/
