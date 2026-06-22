# Web2QML Native Browser Architecture
## Vision: A JIT-Free, Pattern-Based Native Web Renderer

### Core Philosophy

Instead of interpreting arbitrary JavaScript, we **statically analyze** page JS to identify
behavioral patterns, then map those patterns to pre-built native Rust/QML handlers.
This gives us:

- **No JIT attack surface** — no eval, no code generation at runtime
- **Battery efficiency** — native compiled handlers vs. interpreted JS
- **~10x memory reduction** — no JS heap, no parallel DOM/layout/paint trees, lazy image decode
- **Auditability** — every behavior is a known, vetted pattern the user can inspect
- **Incremental coverage** — start with 5 patterns that cover 80% of real pages, expand over time

---

## Layer Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    QML UI Layer                         │
│  ChatView / PageView / FormView / ShoppingView          │
│  Native components, copy buttons, image gallery, etc.   │
├─────────────────────────────────────────────────────────┤
│                 Behavior Binding Layer                   │
│  Maps analyzed JS patterns → native QML signal/slots    │
│  e.g. "form.submit → POST /api/chat" becomes           │
│       Button.onClicked → backend.http_post(url, body)   │
├─────────────────────────────────────────────────────────┤
│              JS Pattern Analyzer (NEW)                  │
│  Static analysis of <script> tags and inline handlers   │
│  Outputs: Vec<PageBehavior> — typed action descriptors  │
│  No execution — pure parsing + pattern matching         │
├─────────────────────────────────────────────────────────┤
│          CSS → QML Converter (EXISTING)                 │
│  Inline styles, <style> blocks, CSS variables → QML    │
│  Layout: flexbox → Row/Column, grid → GridLayout       │
├─────────────────────────────────────────────────────────┤
│           HTML → QML Converter (EXISTING)               │
│  DOM tree → QML component tree                          │
│  Tags → native QML types, attributes → properties       │
├─────────────────────────────────────────────────────────┤
│              Network / Fetch Layer                       │
│  reqwest for HTTP, cookie jar, redirect following       │
│  SSE/streaming support for chat APIs                    │
│  Image/file download with progress                      │
└─────────────────────────────────────────────────────────┘
```

---

## JS Pattern Catalog

These are the behaviors we recognize and convert to native handlers.
Each pattern has: a detection heuristic, a native replacement, and a safety rating.

### Tier 1 — High Coverage (covers ~70% of interactive pages)

| Pattern | Detection | Native Handler |
|---------|-----------|----------------|
| **Form Submit** | `<form action="...">`, `addEventListener('submit', ...)`, `fetch/XMLHttpRequest` in submit handler | `Backend::http_post(url, form_data)` → response updates QML model |
| **Link Navigation** | `<a href>`, `window.location =`, `history.pushState` | `Backend::load_page(url)` — already implemented |
| **Click Toggle** | `element.classList.toggle(...)`, `element.style.display = ...` | QML `visible` property binding, `MouseArea.onClicked` |
| **Input Binding** | `input.value`, `oninput`, `onchange`, two-way data binding | QML `TextField` with `onTextChanged` signal |
| **Fetch + Render** | `fetch(url).then(r => r.json()).then(data => renderList(data))` | `Backend::http_get(url)` → JSON → QML ListModel |

### Tier 2 — Interactive Sites (Amazon, chat interfaces)

| Pattern | Detection | Native Handler |
|---------|-----------|----------------|
| **Search + Results** | Input → fetch → DOM list update | `TextField` → `Backend::search(query)` → `ListView` model update |
| **Image Gallery** | `<img>` arrays, lazy loading, `IntersectionObserver` | QML `GridView` + `Image` with async source loading |
| **Add to Cart** | Button click → POST → badge update | `Button.onClicked` → `Backend::http_post` → counter property |
| **Infinite Scroll** | `scroll` listener → fetch more → append DOM | QML `ListView` with `onAtYEnd` → `Backend::fetch_next_page` |
| **Copy to Clipboard** | `navigator.clipboard.writeText(...)`, `document.execCommand('copy')` | QML `Clipboard` API, native copy button |

### Tier 3 — Chat/AI Interfaces (Claude, ChatGPT)

| Pattern | Detection | Native Handler |
|---------|-----------|----------------|
| **SSE Streaming** | `EventSource`, `fetch` with `ReadableStream` | Rust `reqwest` SSE → channel → QML text append |
| **Markdown Render** | Content inserted as parsed markdown/HTML | `pulldown-cmark` in Rust → QML rich text components |
| **File Upload** | `<input type="file">`, FormData with file | Native file picker → multipart POST |
| **Message History** | Array of messages rendered as chat bubbles | QML `ListView` + `ChatMessageModel` |
| **Code Block + Copy** | `<pre><code>` with copy button | QML `TextArea` (read-only, monospace) + copy `Button` |

---

## Module Breakdown

### `src/js_analyzer.rs` — Static JS Pattern Analyzer

```rust
/// A recognized behavior extracted from page JavaScript.
/// No JS is ever executed — these are pure data descriptions
/// of what the JS *would do*, mapped to native handlers.
#[derive(Debug, Clone)]
pub enum PageBehavior {
    FormSubmit {
        form_selector: String,
        action_url: String,
        method: HttpMethod,
        fields: Vec<FormField>,
    },
    FetchOnEvent {
        trigger: EventTrigger,
        url: String,
        method: HttpMethod,
        body_template: Option<String>,
        response_target: String, // CSS selector where response renders
    },
    ClickToggle {
        trigger_selector: String,
        target_selector: String,
        toggle_type: ToggleType, // Visibility, ClassAdd, ClassRemove
    },
    ClipboardCopy {
        trigger_selector: String,
        source_selector: String,
    },
    InputBinding {
        input_selector: String,
        target_property: String,
    },
    SSEStream {
        url: String,
        event_types: Vec<String>,
        render_target: String,
    },
    Navigation {
        trigger_selector: String,
        url: String,
    },
    ScrollPagination {
        fetch_url_template: String,
        container_selector: String,
        item_template: String,
    },
}

#[derive(Debug, Clone)]
pub enum HttpMethod { Get, Post, Put, Delete }

#[derive(Debug, Clone)]
pub enum EventTrigger {
    Click(String),      // selector
    Submit(String),     // form selector
    Input(String),      // input selector
    Scroll(String),     // container selector
    Load,               // page load
}

#[derive(Debug, Clone)]
pub enum ToggleType {
    Visibility,
    ClassToggle(String),
    StyleProperty(String, String),
}

#[derive(Debug, Clone)]
pub struct FormField {
    pub name: String,
    pub field_type: String,  // text, password, file, hidden, etc.
    pub selector: String,
}
```

### `src/behavior_binding.rs` — Maps Behaviors to QML

Takes `Vec<PageBehavior>` and generates the QML signal/slot wiring:

```rust
pub fn bind_behaviors_to_qml(
    behaviors: &[PageBehavior],
    backend: &mut Backend,
    qml_tree: &mut QmlNode,
) {
    for behavior in behaviors {
        match behavior {
            PageBehavior::FormSubmit { form_selector, action_url, method, fields } => {
                // Find the form node in the QML tree
                // Replace its submit button with a native Button that calls
                // backend.http_post(action_url, collected_field_values)
            }
            PageBehavior::ClipboardCopy { trigger_selector, source_selector } => {
                // Find the trigger element, add MouseArea with:
                // onClicked: { Clipboard.setText(sourceElement.text) }
            }
            PageBehavior::SSEStream { url, event_types, render_target } => {
                // Wire up backend.start_sse_stream(url)
                // Connect streaming tokens to a TextArea in render_target
            }
            // ... etc
        }
    }
}
```

### `src/http_client.rs` — Network Layer

```rust
pub struct HttpClient {
    client: reqwest::blocking::Client,  // or async
    cookie_jar: Arc<reqwest::cookie::Jar>,
    user_agent: String,
}

impl HttpClient {
    /// Standard GET/POST with cookie persistence
    pub fn get(&self, url: &str) -> Result<Response>;
    pub fn post(&self, url: &str, body: &str) -> Result<Response>;
    
    /// Form submission with multipart support (file uploads)
    pub fn submit_form(&self, url: &str, fields: &FormData) -> Result<Response>;
    
    /// SSE stream — returns a channel receiver for streaming tokens
    pub fn sse_stream(&self, url: &str) -> Result<Receiver<SseEvent>>;
    
    /// Download file with progress callback
    pub fn download(&self, url: &str, dest: &Path, on_progress: impl Fn(u64, u64));
}
```

### `src/page_renderer.rs` — Unified Rendering Pipeline

```rust
pub fn render_page(url: &str, backend: &mut Backend) {
    // 1. Fetch HTML
    let html = backend.http_client.get(url)?;
    
    // 2. Parse DOM
    let document = Html::parse_document(&html);
    
    // 3. Extract and analyze JS (NO EXECUTION)
    let scripts = extract_scripts(&document);
    let behaviors = js_analyzer::analyze(&scripts, &document);
    
    // 4. Convert HTML+CSS to QML tree
    let qml_tree = html_to_qml(&document);
    
    // 5. Bind behaviors to QML components
    behavior_binding::bind_behaviors_to_qml(&behaviors, backend, &mut qml_tree);
    
    // 6. Render
    backend.set_qml_tree(qml_tree);
}
```

---

## Vetting / Safety UI

One of the key advantages: every behavior is inspectable before activation.

```
┌─────────────────────────────────────────────┐
│  Page Behaviors Detected              [⚙]  │
├─────────────────────────────────────────────┤
│  ✅ Form Submit → POST /api/search          │
│     Fields: query (text)                    │
│     [Allow] [Block] [Inspect]               │
│                                             │
│  ✅ Click Toggle → .dropdown visibility     │
│     [Allow] [Block] [Inspect]               │
│                                             │
│  ⚠️  Fetch on Load → GET /api/tracking      │
│     [Allow] [Block] [Inspect]               │
│                                             │
│  🔒 Unknown Pattern (lines 45-67)           │
│     [View Source] [Skip]                    │
└─────────────────────────────────────────────┘
```

Users can:
- See every detected behavior before it runs
- Allow/block individual behaviors
- Inspect the source JS that produced the detection
- Unknown patterns are flagged and blocked by default

---

## Implementation Roadmap

### Phase 1: JS Analyzer Foundation (weeks 1-3)
- `swc_ecma_parser` for JS AST parsing (no eval, just the tree)
- Pattern matchers for Tier 1 behaviors
- CLI tool: `web2qml analyze https://example.com` → prints detected behaviors
- Safety: everything is blocked by default until user allows

### Phase 2: Behavior Binding (weeks 4-6)
- Form submission with native HTTP
- Click toggles (show/hide elements)
- Input bindings (search boxes, text fields)
- Cookie persistence across page loads

### Phase 3: Interactive Sites (weeks 7-10)
- Image loading and gallery views
- Search → results flow (Amazon, Google)
- Infinite scroll / pagination
- File upload dialogs

### Phase 4: Chat Interfaces (weeks 11-14)
- SSE streaming for AI chat APIs
- Markdown → QML rendering
- Code blocks with syntax highlighting + copy
- Message history as native ListView
- File attachment upload

### Phase 5: Polish & Site-Specific (ongoing)
- Site profiles: pre-configured behavior mappings for popular sites
- User scripts: let users define custom pattern → handler mappings
- Performance: lazy QML component loading, image caching
- Accessibility: keyboard nav, screen reader support

---

## Key Dependencies to Add

```toml
[dependencies]
# JS parsing (AST only, no execution)
swc_ecma_parser = "0.149"
swc_ecma_ast = "0.118"
swc_common = "0.37"

# Markdown rendering
pulldown-cmark = "0.11"

# HTTP with cookies and streaming
reqwest = { version = "0.12", features = ["cookies", "stream", "multipart"] }

# SSE parsing
eventsource-client = "0.13"

# Image handling
image = "0.25"
```

---

## Memory Optimization Strategy

### Why We Win by Default

A Chromium tab keeping a page idle typically holds:

| Component | Chromium | Web2QML | Savings |
|-----------|----------|---------|---------|
| JS heap (V8 isolate) | 50-200 MB | 0 MB | 100% — no JS engine |
| DOM tree | 10-40 MB | 0 MB | 100% — we produce QML directly |
| Layout tree | 5-15 MB | 0 MB | QML handles its own layout |
| Paint/compositing layers | 10-50 MB | ~2-5 MB | QML scene graph is leaner |
| Decoded image textures | 20-200 MB | 5-30 MB | Lazy decode + eviction |
| CSS style resolution cache | 5-20 MB | ~1 MB | One-shot conversion |
| **Total per tab** | **100-525 MB** | **8-36 MB** | **~10x reduction** |

But we still need to be deliberate about the areas where we *do* allocate.

### 1. String Interning for DOM Content

Tailwind-heavy pages (ChatGPT, modern SPAs) repeat the same class tokens
thousands of times. Your ChatGPT export had class names like
`hljs_keyword` appearing 30+ times and monstrous composite names like
`flex_items_center_justify_center_touch_w_10_h_8_w_8` repeated across
every button. Without interning, each copy is a separate heap String.

```rust
// src/string_pool.rs
use std::collections::HashMap;

/// Intern pool: each unique string stored once, referenced by u32 handle.
/// Typical savings: 40-60% on class-heavy pages.
pub struct StringPool {
    strings: Vec<String>,
    lookup: HashMap<String, u32>,
}

impl StringPool {
    pub fn new() -> Self {
        Self { strings: Vec::new(), lookup: HashMap::new() }
    }

    /// Intern a string, returning a compact handle
    pub fn intern(&mut self, s: &str) -> StringId {
        if let Some(&id) = self.lookup.get(s) {
            return StringId(id);
        }
        let id = self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.lookup.insert(s.to_string(), id);
        StringId(id)
    }

    pub fn resolve(&self, id: StringId) -> &str {
        &self.strings[id.0 as usize]
    }

    /// Memory used by the pool itself
    pub fn memory_usage(&self) -> usize {
        self.strings.iter().map(|s| s.len() + std::mem::size_of::<String>()).sum::<usize>()
            + self.lookup.len() * (std::mem::size_of::<String>() + 4)
    }
}

/// 4 bytes instead of 24 bytes (String) + heap allocation per reference
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(u32);
```

Use `StringId` in your `Node` and QML tree structs instead of `String`
for tag names, class names, attribute keys, and repeated attribute values.

### 2. Image Memory Management

Images dominate memory on visual pages. Three strategies, layered:

**a) Lazy decode**: Don't decode images until they're about to enter the
viewport. Store the compressed bytes (JPEG = ~10% of decoded size) and
only decode to RGBA when the QML `Image` component requests pixels.

**b) Resolution capping**: If a `<img>` has CSS dimensions of 200x200,
don't decode a 4000x3000 source image at full resolution. Decode at the
display size. This alone saves 90%+ on product listing pages.

**c) LRU texture eviction**: Keep a budget (e.g., 64 MB of decoded
textures). When a new image needs decoding and the budget is full,
evict the least-recently-displayed image back to compressed bytes.

```rust
// src/image_cache.rs
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

pub struct ImageCache {
    /// Compressed image data (JPEG/PNG bytes)
    compressed: HashMap<String, Vec<u8>>,

    /// Decoded RGBA data, capped at budget_bytes
    decoded: HashMap<String, DecodedImage>,

    /// LRU order for eviction
    lru: VecDeque<String>,

    /// Maximum bytes for decoded textures
    budget_bytes: usize,
    current_bytes: usize,
}

struct DecodedImage {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl ImageCache {
    pub fn new(budget_mb: usize) -> Self {
        Self {
            compressed: HashMap::new(),
            decoded: HashMap::new(),
            lru: VecDeque::new(),
            budget_bytes: budget_mb * 1024 * 1024,
            current_bytes: 0,
        }
    }

    /// Store compressed bytes (cheap — ~10% of decoded size)
    pub fn store_compressed(&mut self, url: &str, data: Vec<u8>) {
        self.compressed.insert(url.to_string(), data);
    }

    /// Get decoded image, decoding on demand and evicting if needed
    pub fn get_decoded(&mut self, url: &str, max_width: u32, max_height: u32)
        -> Option<(&[u8], u32, u32)>
    {
        // Touch LRU
        self.lru.retain(|u| u != url);
        self.lru.push_back(url.to_string());

        if self.decoded.contains_key(url) {
            let img = &self.decoded[url];
            return Some((&img.rgba, img.width, img.height));
        }

        // Decode from compressed
        let compressed = self.compressed.get(url)?;
        let decoded = self.decode_capped(compressed, max_width, max_height)?;
        let size = decoded.rgba.len();

        // Evict until we have room
        while self.current_bytes + size > self.budget_bytes {
            if let Some(evict_url) = self.lru.pop_front() {
                if let Some(evicted) = self.decoded.remove(&evict_url) {
                    self.current_bytes -= evicted.rgba.len();
                }
            } else {
                break;
            }
        }

        self.current_bytes += size;
        self.decoded.insert(url.to_string(), decoded);
        let img = &self.decoded[url];
        Some((&img.rgba, img.width, img.height))
    }

    fn decode_capped(&self, data: &[u8], max_w: u32, max_h: u32)
        -> Option<DecodedImage>
    {
        // Use `image` crate with resize
        // image::load_from_memory(data)
        //     .ok()?
        //     .resize(max_w, max_h, image::imageops::FilterType::Triangle)
        //     .to_rgba8()
        todo!("implement with image crate")
    }

    pub fn memory_report(&self) -> String {
        format!(
            "Images: {} compressed ({} KB), {} decoded ({} MB / {} MB budget)",
            self.compressed.len(),
            self.compressed.values().map(|v| v.len()).sum::<usize>() / 1024,
            self.decoded.len(),
            self.current_bytes / (1024 * 1024),
            self.budget_bytes / (1024 * 1024),
        )
    }
}
```

### 3. QML Tree Compaction

Your current approach generates QML as text strings, writes to a temp
file, and loads it. This means the full QML source text lives in memory
alongside the instantiated QML object tree. For large pages, that's
double the structural memory.

**Solution**: Component pooling + streaming generation.

Instead of generating one massive QML file, generate a **template** with
a `Repeater` or `ListView` backed by your `NodeModel`. This way:
- The model holds compact node data (~40 bytes per node)
- QML instantiates only the visible delegates (~20-30 at a time)
- Off-screen content uses zero QML object memory

```qml
// This replaces the "generate entire page as QML text" approach
ListView {
    id: pageView
    width: parent.width
    height: parent.height
    model: nodeModel
    cacheBuffer: 200  // pixels of off-screen buffer

    delegate: Loader {
        width: parent.width
        // Loader instantiates only the right component type
        source: {
            switch(model.kind) {
                case 0: return "qml/TextNode.qml"
                case 1: return "qml/LinkNode.qml"
                case 2: return "qml/ImageNode.qml"
                case 3: return "qml/FormNode.qml"
                default: return "qml/TextNode.qml"
            }
        }
    }
}
```

For a page with 2000 nodes, this means ~30 QML objects alive at any time
(the visible ones + cacheBuffer) instead of 2000.

### 4. Parse Tree Drop-After-Convert

The `scraper::Html` document tree is only needed during the HTML→QML
conversion pass. After conversion, drop it explicitly:

```rust
fn load_page(&mut self, url: String, recursion_amt: u32) {
    // ... fetch HTML ...

    let document = scraper::Html::parse_document(&html);

    // Extract behaviors (borrows document)
    let scripts = js_analyzer::extract_scripts(&document);
    let behaviors = js_analyzer::analyze(&scripts, &document);

    // Convert to QML nodes (borrows document)
    let nodes = html_to_nodes(&document, recursion_amt);

    // NOW drop the parse tree — we're done with it
    drop(document);
    // Also drop the raw HTML string
    drop(html);

    // Everything from here uses only our compact node model
    self.node_model.replace_nodes(nodes);
    self.behaviors = behaviors;
}
```

### 5. Memory Budget & Monitoring

Expose a memory dashboard so users can see exactly where memory goes:

```rust
pub struct MemoryReport {
    pub string_pool_bytes: usize,
    pub image_cache_compressed_bytes: usize,
    pub image_cache_decoded_bytes: usize,
    pub image_cache_budget_bytes: usize,
    pub node_model_bytes: usize,
    pub behavior_count: usize,
    pub qml_objects_alive: usize, // from QML engine stats
}

impl MemoryReport {
    pub fn total(&self) -> usize {
        self.string_pool_bytes
            + self.image_cache_compressed_bytes
            + self.image_cache_decoded_bytes
            + self.node_model_bytes
    }

    pub fn display(&self) -> String {
        format!(
            "Memory: {:.1} MB total\n  Strings: {} KB\n  Images: {:.1}/{:.1} MB decoded\n  Nodes: {} KB\n  Behaviors: {}",
            self.total() as f64 / (1024.0 * 1024.0),
            self.string_pool_bytes / 1024,
            self.image_cache_decoded_bytes as f64 / (1024.0 * 1024.0),
            self.image_cache_budget_bytes as f64 / (1024.0 * 1024.0),
            self.node_model_bytes / 1024,
            self.behavior_count,
        )
    }
}
```

### Configuration

Let users tune the budget based on their hardware:

```toml
# web2qml.toml
[memory]
image_budget_mb = 64       # decoded texture budget
max_nodes_in_memory = 5000 # drop excess nodes from very large pages
string_pool_max_mb = 8     # intern pool cap
prefetch_images = 3        # how many off-screen images to pre-decode
```

---

## Why This Works (and Where It Doesn't)

### Works well for:
- Content sites (news, blogs, documentation)
- Search engines (Google, DuckDuckGo)
- Shopping (product pages, search, cart)
- Chat interfaces (API-based interaction)
- Forms (login, signup, contact)
- Static apps with simple interactivity

### Won't work for:
- Heavy SPAs with complex state management (Gmail's full UI)
- WebGL/Canvas-based apps (games, maps)
- Sites that require specific browser APIs (WebRTC, WebAuthn)
- Highly dynamic DOM manipulation (drag-and-drop builders)

### The honest tradeoff:
This approach covers maybe 60-70% of what people actually DO on the web,
with 10x better battery life, full auditability, and no JIT exploits.
For the remaining 30%, the user falls back to a real browser.
The key insight: most web browsing is reading + forms + search + chat.
Those patterns are finite and enumerable.
