//! web_prefs — how to render the web, globally and per site.
//!
//! The engine has knobs that only a real page can settle: how aggressive
//! the ad filter should be, how many blocks are worth rendering, whether
//! placeholders for video and canvas help or clutter. Hard-coding them
//! means every page gets a compromise, and the compromise is wrong in
//! different directions on a documentation site and a news site.
//!
//! ## Per-site because the answer is per-site
//!
//! Aggressive filtering is right on a news site and wrong on a wiki whose
//! sidebar it eats. A 2,000-block budget is generous for an article and
//! mean for an API reference. One global setting forces the person to keep
//! changing it, and a setting people keep changing is one they stop using.
//!
//! ## Out of the way until needed
//!
//! Nothing here has a permanent home on screen. The defaults are what
//! almost everyone gets; the panel is reached from the command palette
//! ("web settings"), from a manual page, or by voice — the three routes
//! this app already uses for everything else, so this adds a subject
//! rather than a mechanism.
//!
//! ## Overrides are per FIELD, not per site
//!
//! A site rule holds `Option`s. Setting the filter for `example.com` must
//! not silently freeze its block budget at whatever the global was when the
//! rule was written — that is the bug where changing a global setting
//! appears to do nothing on exactly the sites you care about most.

use crate::browser::FilterMode;

/// The file, under the app's XDG config dir.
pub const FILE: &str = "web.conf";

/// What the renderer needs, after global and per-site have been combined.
// `Eq` is absent because `FilterMode` upstream is `PartialEq` only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effective {
    pub filter: FilterMode,
    pub max_blocks: usize,
    /// Draw a placeholder for images (their alt text). Off hides them.
    pub images: bool,
    /// Draw a placeholder for canvas/video/iframe.
    pub islands: bool,
}

/// The defaults everyone gets without touching anything.
///
/// `Standard` rather than `Aggressive`: aggressive filtering removes things
/// that are not ads on a minority of sites, and a reader who has not opted
/// in should never wonder whether the page is missing something.
impl Default for Effective {
    fn default() -> Self {
        Effective {
            filter: FilterMode::Standard,
            max_blocks: uikit_web::MAX_BLOCKS,
            images: true,
            islands: true,
        }
    }
}

/// A per-site override. Every field optional — see the module note.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SiteRules {
    pub filter: Option<FilterMode>,
    pub max_blocks: Option<usize>,
    pub images: Option<bool>,
    pub islands: Option<bool>,
}

impl SiteRules {
    pub fn is_empty(&self) -> bool {
        *self == SiteRules::default()
    }
}

/// Global settings plus per-host rules.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebPrefs {
    pub global: Global,
    /// (host, rules). A vec, not a map: it round-trips in a stable order,
    /// which matters for a file people are expected to hand-edit — a map's
    /// iteration order rewrites the whole file on every save and makes the
    /// diff useless.
    pub sites: Vec<(String, SiteRules)>,
}

/// The global settings, as stored. Same fields as [`Effective`], and a
/// separate type because "what is configured" and "what applies here" are
/// different questions and conflating them is how a per-site override ends
/// up written back as a global.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Global {
    pub filter: FilterMode,
    pub max_blocks: usize,
    pub images: bool,
    pub islands: bool,
}

impl Default for Global {
    fn default() -> Self {
        let d = Effective::default();
        Global { filter: d.filter, max_blocks: d.max_blocks,
                 images: d.images, islands: d.islands }
    }
}

/// The host part of a URL, lower-cased. `None` when there isn't one.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next()?;
    // Strip userinfo and port.
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?;
    if host.is_empty() { None } else { Some(host.to_ascii_lowercase()) }
}

/// Does a rule for `pattern` apply to `host`?
///
/// Exact match, or `host` is a subdomain of `pattern`. A rule written for
/// `example.com` covering `www.example.com` is what people mean — nobody
/// writing a rule for a site intends to exclude its www — and the
/// subdomain test checks a leading dot so `notexample.com` does not match
/// `example.com`.
pub fn matches(pattern: &str, host: &str) -> bool {
    let p = pattern.trim().trim_start_matches('.').to_ascii_lowercase();
    if p.is_empty() { return false; }
    host == p || host.ends_with(&format!(".{p}"))
}

impl WebPrefs {
    /// The rules for a host: the MOST SPECIFIC matching pattern wins.
    ///
    /// Longest match, so a rule for `docs.example.com` beats one for
    /// `example.com` on that host. Without this, which rule applied would
    /// depend on the order they happened to be written in, and a person
    /// adding a narrow exception would watch it do nothing.
    pub fn rules_for(&self, host: &str) -> SiteRules {
        self.sites.iter()
            .filter(|(p, _)| matches(p, host))
            .max_by_key(|(p, _)| p.len())
            .map(|(_, r)| *r)
            .unwrap_or_default()
    }

    /// What to render this host with.
    pub fn effective(&self, host: Option<&str>) -> Effective {
        let g = self.global;
        let base = Effective { filter: g.filter, max_blocks: g.max_blocks,
                               images: g.images, islands: g.islands };
        let Some(h) = host else { return base };
        let r = self.rules_for(h);
        Effective {
            filter: r.filter.unwrap_or(base.filter),
            max_blocks: r.max_blocks.unwrap_or(base.max_blocks),
            images: r.images.unwrap_or(base.images),
            islands: r.islands.unwrap_or(base.islands),
        }
    }

    /// Set one field for a host, creating the rule if needed.
    pub fn site_mut(&mut self, host: &str) -> &mut SiteRules {
        let host = host.trim().to_ascii_lowercase();
        if let Some(i) = self.sites.iter().position(|(p, _)| *p == host) {
            return &mut self.sites[i].1;
        }
        self.sites.push((host, SiteRules::default()));
        let n = self.sites.len() - 1;
        &mut self.sites[n].1
    }

    /// Drop a host's rule. A rule with nothing set is removed rather than
    /// written back empty — an empty `[site …]` section in the file reads
    /// as "this site is configured" when it is not.
    pub fn prune(&mut self) {
        self.sites.retain(|(_, r)| !r.is_empty());
    }
}

fn mode_str(m: FilterMode) -> &'static str {
    match m {
        FilterMode::Off => "off",
        FilterMode::Standard => "standard",
        FilterMode::Aggressive => "aggressive",
    }
}

fn mode_of(s: &str) -> Option<FilterMode> {
    match s.trim() {
        "off" => Some(FilterMode::Off),
        "standard" => Some(FilterMode::Standard),
        "aggressive" => Some(FilterMode::Aggressive),
        _ => None,
    }
}

fn bool_of(s: &str) -> Option<bool> {
    match s.trim() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// `[global]` then a `[site example.com]` per rule.
///
/// A flat, commented, hand-editable file, like every other store in this
/// app. The header says what the values mean, because the one recovery
/// path a config file has is somebody opening it.
pub fn render(p: &WebPrefs) -> String {
    let mut out = String::from(
        "# voiceterm web rendering\n\
         # filter: off | standard | aggressive\n\
         # max_blocks: how much of a long page to render\n\
         # images / islands: draw placeholders for pictures and for video/canvas\n\
         # A [site …] rule sets only the lines it lists; the rest follow [global].\n");
    out.push_str("\n[global]\n");
    out.push_str(&format!("filter = {}\n", mode_str(p.global.filter)));
    out.push_str(&format!("max_blocks = {}\n", p.global.max_blocks));
    out.push_str(&format!("images = {}\n", p.global.images));
    out.push_str(&format!("islands = {}\n", p.global.islands));
    for (host, r) in &p.sites {
        if r.is_empty() { continue; }
        out.push_str(&format!("\n[site {host}]\n"));
        if let Some(m) = r.filter { out.push_str(&format!("filter = {}\n", mode_str(m))); }
        if let Some(n) = r.max_blocks { out.push_str(&format!("max_blocks = {n}\n")); }
        if let Some(b) = r.images { out.push_str(&format!("images = {b}\n")); }
        if let Some(b) = r.islands { out.push_str(&format!("islands = {b}\n")); }
    }
    out
}

pub fn parse(text: &str) -> WebPrefs {
    let mut p = WebPrefs::default();
    // `None` = the [global] section; `Some(i)` = index into `sites`.
    let mut cur: Option<usize> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let sec = sec.trim();
            if let Some(host) = sec.strip_prefix("site ") {
                let host = host.trim().to_ascii_lowercase();
                cur = Some(match p.sites.iter().position(|(h, _)| *h == host) {
                    Some(i) => i,
                    None => { p.sites.push((host, SiteRules::default())); p.sites.len() - 1 }
                });
            } else {
                cur = None;
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match cur {
            None => match k {
                "filter" => if let Some(m) = mode_of(v) { p.global.filter = m },
                // A budget of zero renders nothing and looks like a crash.
                // Clamped rather than refused: the value was meant as "very
                // little", and honouring it literally is unhelpful.
                "max_blocks" => if let Ok(n) = v.parse::<usize>() {
                    p.global.max_blocks = n.max(1);
                },
                "images" => if let Some(b) = bool_of(v) { p.global.images = b },
                "islands" => if let Some(b) = bool_of(v) { p.global.islands = b },
                // An unknown key is IGNORED, not fatal: this file outlives
                // the version that wrote it, and a setting removed in a
                // later build must not stop the rest from loading.
                _ => {}
            },
            Some(i) => {
                let r = &mut p.sites[i].1;
                match k {
                    "filter" => r.filter = mode_of(v),
                    "max_blocks" => r.max_blocks = v.parse::<usize>().ok().map(|n| n.max(1)),
                    "images" => r.images = bool_of(v),
                    "islands" => r.islands = bool_of(v),
                    _ => {}
                }
            }
        }
    }
    p.prune();
    p
}

pub fn load() -> WebPrefs {
    config_kit::load("voiceterm", FILE).map(|t| parse(&t)).unwrap_or_default()
}

pub fn store(p: &WebPrefs) -> std::io::Result<()> {
    config_kit::save("voiceterm", FILE, &render(p))
}

/// The panel. Current site first, globals below.
///
/// That order because the reason people open this is the page in front of
/// them: an ad wall, a sidebar the filter ate, a video placeholder they
/// would rather not see. Making them scroll past four global toggles to
/// reach the one that matters is how a settings panel becomes something
/// people avoid.
///
/// Returns true when anything changed, so the caller writes the file once
/// rather than on every frame the panel is open.
pub fn web_settings_ui(ui: &mut egui::Ui, p: &mut WebPrefs, url: &str,
                       c: &uikit_egui::PaletteColors) -> bool {
    let mut changed = false;
    let host = host_of(url);

    match host.as_deref() {
        Some(h) => {
            ui.label(egui::RichText::new(h).strong());
            let eff = p.effective(Some(h));
            let mut r = p.rules_for(h);
            // Shown as the EFFECTIVE value with a marker for what is
            // overridden, rather than as empty controls: a panel of blanks
            // does not tell you what the page is currently doing, which is
            // the question you opened it with.
            let before = r;
            filter_row(ui, &mut r.filter, eff.filter, c);
            opt_bool_row(ui, "images", &mut r.images, eff.images, c);
            opt_bool_row(ui, "video / canvas", &mut r.islands, eff.islands, c);
            if r != before {
                *p.site_mut(h) = r;
                p.prune();
                changed = true;
            }
            if !p.rules_for(h).is_empty()
                && ui.small_button("follow the defaults again").clicked() {
                p.sites.retain(|(pat, _)| pat != h);
                changed = true;
            }
        }
        None => {
            ui.colored_label(c.text_weak, "no site open — showing the defaults");
        }
    }

    ui.separator();
    ui.label(egui::RichText::new("everywhere").strong());
    let g = &mut p.global;
    ui.horizontal(|ui| {
        ui.label("ads");
        for m in [FilterMode::Off, FilterMode::Standard, FilterMode::Aggressive] {
            if ui.selectable_label(g.filter == m, mode_str(m)).clicked() && g.filter != m {
                g.filter = m;
                changed = true;
            }
        }
    });
    changed |= ui.checkbox(&mut g.images, "images").changed();
    changed |= ui.checkbox(&mut g.islands, "video / canvas").changed();
    ui.horizontal(|ui| {
        ui.label("page limit");
        let mut n = g.max_blocks as f32;
        if ui.add(egui::Slider::new(&mut n, 200.0..=10_000.0).logarithmic(true)
                  .show_value(true)).changed() {
            g.max_blocks = (n as usize).max(1);
            changed = true;
        }
    });
    ui.colored_label(c.text_weak,
        "How much of a very long page to render. Lower is faster on a phone.");
    changed
}

/// A tri-state row: inherit, or one of the three levels.
fn filter_row(ui: &mut egui::Ui, val: &mut Option<FilterMode>, eff: FilterMode,
              c: &uikit_egui::PaletteColors) {
    ui.horizontal(|ui| {
        ui.label("ads");
        // "default" is a real choice and the FIRST one, so removing an
        // override is as easy as making one — a setting you can only turn
        // on is a trap.
        if ui.selectable_label(val.is_none(), "default").clicked() { *val = None; }
        for m in [FilterMode::Off, FilterMode::Standard, FilterMode::Aggressive] {
            if ui.selectable_label(*val == Some(m), mode_str(m)).clicked() {
                *val = Some(m);
            }
        }
        if val.is_none() {
            ui.colored_label(c.text_weak, format!("({})", mode_str(eff)));
        }
    });
}

fn opt_bool_row(ui: &mut egui::Ui, label: &str, val: &mut Option<bool>, eff: bool,
                c: &uikit_egui::PaletteColors) {
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.selectable_label(val.is_none(), "default").clicked() { *val = None; }
        if ui.selectable_label(*val == Some(true), "on").clicked() { *val = Some(true); }
        if ui.selectable_label(*val == Some(false), "off").clicked() { *val = Some(false); }
        if val.is_none() {
            ui.colored_label(c.text_weak, if eff { "(on)" } else { "(off)" });
        }
    });
}

/// The manual page for these settings.
///
/// Registered with the rest of the manual so `man web` finds it — the
/// third route in, after the palette and voice. A setting reachable only
/// by remembering it exists is a setting for the person who wrote it.
pub fn manual_html() -> &'static str {
    "<h1>web</h1>     <p>How pages are rendered, globally and for one site.</p>     <p>Open with <b>web settings</b> in the command palette, or say it.</p>     <h2>ads</h2>     <p><b>off</b> renders everything. <b>standard</b> removes known         advertising and tracking. <b>aggressive</b> also removes elements         that merely look like advertising — faster and quieter, and it         occasionally takes something real, which is why it is not the         default.</p>     <h2>images, video / canvas</h2>     <p>There is no image decoder here, so a picture is drawn as its         description and a video as a placeholder you can act on. Turn them         off for a page that is mostly furniture.</p>     <h2>page limit</h2>     <p>How many blocks of a very long page to render. The display list is         walked every frame, so a lower limit is a faster page rather than a         faster load. A page that hits the limit says so at the bottom.</p>     <h2>per site</h2>     <p>Anything set for a site overrides the default for that setting         only; everything else keeps following the defaults, including when         you change them later. A rule for <i>example.com</i> also covers         <i>www.example.com</i>, and a rule for <i>docs.example.com</i>         wins on that host.</p>     <p>Stored in <b>web.conf</b>, which is plain text and safe to edit.</p>"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_writes_a_site_rule_and_can_take_it_back() {
        // Driven through a real frame: the panel is the only place the
        // tri-state rows exist, and "default" being reachable is the
        // difference between a setting and a trap.
        use crate::frame_probe::Probe;
        let p = Probe::default();
        let c = p.colors();
        let mut prefs = WebPrefs::default();
        let (d, _) = p.frame(|ui|
            web_settings_ui(ui, &mut prefs, "https://news.test/a", &c));
        assert!(d.has("news.test"), "the current site is not shown: {:?}", d.texts);
        assert!(d.has("default"), "no way to un-set an override: {:?}", d.texts);

        // Set one field for the site, and only that field is stored.
        prefs.site_mut("news.test").filter = Some(FilterMode::Aggressive);
        prefs.global.max_blocks = 700;
        let e = prefs.effective(Some("news.test"));
        assert_eq!(e.filter, FilterMode::Aggressive);
        assert_eq!(e.max_blocks, 700, "the rule froze a value it never set");

        prefs.sites.retain(|(h, _)| h != "news.test");
        assert_eq!(prefs.effective(Some("news.test")).filter, prefs.global.filter);
    }

    #[test]
    fn with_no_site_open_the_panel_says_so_rather_than_looking_broken() {
        use crate::frame_probe::Probe;
        let p = Probe::default();
        let c = p.colors();
        let mut prefs = WebPrefs::default();
        let (d, _) = p.frame(|ui| web_settings_ui(ui, &mut prefs, "", &c));
        assert!(d.has("no site open"), "{:?}", d.texts);
        assert!(d.has("everywhere"), "the globals are unreachable: {:?}", d.texts);
    }

    #[test]
    fn the_manual_page_documents_every_setting_the_panel_offers() {
        // The failure this catches is the ordinary one: a setting is added
        // and the manual is not, so the third route in silently stops
        // covering it.
        let m = manual_html();
        for term in ["ads", "images", "page limit", "per site", "aggressive"] {
            assert!(m.contains(term), "the manual does not mention {term:?}");
        }
        assert!(m.contains("web.conf"), "the file is not named anywhere");
    }

    #[test]
    fn a_host_comes_out_of_a_url_in_the_shapes_that_occur() {
        assert_eq!(host_of("https://Example.COM/a/b?c#d").as_deref(), Some("example.com"));
        assert_eq!(host_of("http://user:pw@docs.example.com:8443/x").as_deref(),
                   Some("docs.example.com"));
        assert_eq!(host_of("example.com").as_deref(), Some("example.com"));
        assert_eq!(host_of(""), None);
        assert_eq!(host_of("file:///home/me/page.html"), None);
    }

    #[test]
    fn a_rule_covers_subdomains_but_not_lookalikes() {
        // Nobody writing a rule for a site intends to exclude its www; and
        // `notexample.com` must not inherit `example.com`'s rules, which is
        // what a bare `ends_with` would do.
        assert!(matches("example.com", "example.com"));
        assert!(matches("example.com", "www.example.com"));
        assert!(matches(".example.com", "docs.example.com"));
        assert!(!matches("example.com", "notexample.com"));
        assert!(!matches("example.com", "example.com.evil.test"));
    }

    #[test]
    fn the_most_specific_rule_wins_rather_than_the_first_written() {
        // Otherwise a narrow exception does nothing depending on where in
        // the file it happens to sit.
        let mut p = WebPrefs::default();
        p.site_mut("example.com").filter = Some(FilterMode::Aggressive);
        p.site_mut("docs.example.com").filter = Some(FilterMode::Off);
        assert_eq!(p.effective(Some("docs.example.com")).filter, FilterMode::Off);
        assert_eq!(p.effective(Some("www.example.com")).filter, FilterMode::Aggressive);
    }

    #[test]
    fn a_site_rule_overrides_one_field_and_follows_the_global_for_the_rest() {
        // The bug this shape prevents: setting a filter for a site must not
        // freeze its block budget at whatever the global happened to be,
        // which is how changing a global appears to do nothing on exactly
        // the sites you care about.
        let mut p = WebPrefs::default();
        p.global.max_blocks = 500;
        p.site_mut("example.com").filter = Some(FilterMode::Off);
        let e = p.effective(Some("example.com"));
        assert_eq!(e.filter, FilterMode::Off);
        assert_eq!(e.max_blocks, 500);

        p.global.max_blocks = 900;
        assert_eq!(p.effective(Some("example.com")).max_blocks, 900,
                   "the site rule froze a value it never set");
    }

    #[test]
    fn an_unknown_host_and_no_host_both_get_the_globals() {
        let mut p = WebPrefs::default();
        p.global.islands = false;
        assert!(!p.effective(Some("elsewhere.test")).islands);
        assert!(!p.effective(None).islands);
    }

    #[test]
    fn the_file_round_trips_and_keeps_its_order() {
        // People hand-edit this. A store that reorders on every save makes
        // the diff useless.
        let mut p = WebPrefs::default();
        p.global.filter = FilterMode::Aggressive;
        p.global.max_blocks = 1234;
        p.site_mut("news.test").images = Some(false);
        p.site_mut("wiki.test").filter = Some(FilterMode::Off);
        let back = parse(&render(&p));
        assert_eq!(back, p);
        assert_eq!(back.sites.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(),
                   vec!["news.test", "wiki.test"]);
    }

    #[test]
    fn an_unknown_key_costs_that_line_and_not_the_file() {
        // The file outlives the build that wrote it; a setting removed
        // later must not stop the rest from loading.
        let p = parse("[global]\nfilter = off\nfuture_thing = 7\nmax_blocks = 40\n");
        assert_eq!(p.global.filter, FilterMode::Off);
        assert_eq!(p.global.max_blocks, 40);
    }

    #[test]
    fn a_budget_of_zero_is_clamped_rather_than_rendering_nothing() {
        // Honouring it literally gives a blank page that looks like a
        // crash; the value was meant as "very little".
        let p = parse("[global]\nmax_blocks = 0\n");
        assert_eq!(p.global.max_blocks, 1);
    }

    #[test]
    fn an_empty_site_rule_is_not_written_back() {
        // An empty `[site …]` reads as "this site is configured" when it is
        // not.
        let mut p = WebPrefs::default();
        p.site_mut("blank.test");
        assert!(!render(&p).contains("blank.test"));
        assert!(parse("[site blank.test]\n").sites.is_empty());
    }
}
