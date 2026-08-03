//! browse — browse-by-album and browse-by-artist, ported from the QML pages.
//!
//! Four surfaces at once, because they are one feature: `AlbumsPage` and
//! `ArtistsPage` are grids/lists over aggregates, and `AlbumDetailPage` /
//! `ArtistDetailPage` are the track lists you land on when you tap one. Porting
//! a list without its detail page gives you a grid of buttons that do nothing.
//!
//! ## The bits with actual behaviour
//!
//! * **Two modes, not one list.** `TOP` is play-ranked and capped (40 albums,
//!   50 artists); `ALPHA`/`A→Z` is the whole library, sorted and paged. They
//!   answer different questions — "what do I actually listen to" and "what do I
//!   own" — which is why the QML has two tabs rather than one sort chip.
//!
//! * **An album is a `(title, primary_artist)` PAIR.** The catalogue has no
//!   albums table; the aggregate is computed on demand and keyed on the pair
//!   (`media::library::AlbumSummary`). Keying on title alone silently merges
//!   every *Greatest Hits* in the library into one card.
//!
//! * **Paging with an offset guard.** The A→Z artist list fetches 100 at a
//!   time. Three things can go wrong and each has a rule: a scroll event fires
//!   every frame (in-flight guard), a page can arrive out of order (accept only
//!   when `offset == items.len()`), and the user can switch modes while a
//!   request is in flight (drop responses whose context is not the current
//!   mode). The QML learned all three; so does this.
//!
//! * **A dropped page must still clear the loading flag.** Discarding a stale
//!   page without clearing `loading` leaves the list permanently convinced a
//!   fetch is in flight, so it never asks again and the list dead-ends
//!   mid-library. This is the one that looks like a hang rather than a bug.
//!
//! * **Prefetch four rows early.** Fetching when the tail is *reached* shows
//!   the user the gap. Four rows of lead time is what the QML settled on.
//!
//! * **Most-common year, ties to the earliest.** Re-releases and compilations
//!   give one album several years; the mode wins, and an even split resolves to
//!   the older year (`refine_album_years`' `ORDER BY cnt DESC, year ASC`).
//!
//! ## An asymmetry the port reproduces rather than tidies
//!
//! `top_albums_by_plays` has `HAVING total_plays > 0`; `top_artists_by_plays`
//! has no such filter. So an unplayed album cannot appear in TOP but an
//! unplayed artist can, if the library is small enough to reach them. That
//! reads as inconsistent, and it *is* — but it is the shipped behaviour of the
//! surface being ported, and the two pages' empty states are written for it
//! (the albums page says "switch to ALPHA to browse everything", the artists
//! page does not). Changing it here would make the port disagree with the app
//! it is porting; it belongs in a backend change if it is wrong.

use std::collections::HashMap;

/// `pageSize: 100` — the A→Z fetch size.
pub const ALPHA_PAGE: usize = 100;
/// `qtobj.load_top_albums(40)`.
pub const TOP_ALBUMS: usize = 40;
/// `qtobj.load_top_artists(50)`.
pub const TOP_ARTISTS: usize = 50;
/// Rows of lead time on the prefetch trigger.
pub const PREFETCH_ROWS: f32 = 4.0;
/// Delegate height (56) + spacing (4), from the QML's `onContentYChanged`.
pub const ROW_H: f32 = 60.0;

// ── the two aggregate row types ─────────────────────────────────────────────

/// One album card. Mirrors `media::library::AlbumSummary` field for field so
/// the mapping is checkable by eye.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub title: String,
    pub primary_artist: String,
    pub year: Option<u16>,
    pub track_count: u32,
    pub total_plays: u32,
    pub last_played_at: Option<i64>,
}

/// What identifies an album: the pair, never the title alone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct AlbumKey {
    pub title: String,
    pub artist: String,
}

impl Album {
    pub fn key(&self) -> AlbumKey {
        AlbumKey { title: self.title.clone(), artist: self.primary_artist.clone() }
    }

    /// The card's title line. An untitled album draws a placeholder rather than
    /// a blank card, which reads as a broken grid.
    pub fn label(&self) -> String {
        if self.title.trim().is_empty() { "(untitled)".into() } else { self.title.clone() }
    }

    /// The artist line, never blank for the same reason.
    pub fn artist_label(&self) -> String {
        if self.primary_artist.trim().is_empty() { "—".into() } else { self.primary_artist.clone() }
    }

    /// `year • N tracks • ▶ plays`, omitting what it does not have. The QML
    /// hides the year Text when there is no year and the play count when it is
    /// zero, rather than printing "0 plays" on every unplayed album.
    pub fn meta(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(y) = self.year { parts.push(y.to_string()); }
        parts.push(plural(self.track_count as i64, "track"));
        if self.total_plays > 0 { parts.push(format!("▶ {}", self.total_plays)); }
        parts.join(" · ")
    }

    /// Which tint bucket the card falls in.
    pub fn heat(&self) -> Heat { Heat::of(self.total_plays) }
}

/// Play-count tint buckets. The grid tints heavily-played albums so they catch
/// the eye in the TOP tab, where "most played" is otherwise only conveyed by
/// position — and position is invisible once you have scrolled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heat { Cold, Played, Hot }

impl Heat {
    /// `total_plays > 10` is hot, `> 0` is played, else cold.
    pub fn of(plays: u32) -> Heat {
        if plays > 10 { Heat::Hot } else if plays > 0 { Heat::Played } else { Heat::Cold }
    }
    /// How strongly to mix the accent into the card, 0..=1.
    pub fn tint(self) -> f32 {
        match self { Heat::Hot => 0.16, Heat::Played => 0.08, Heat::Cold => 0.0 }
    }
}

/// One artist row. Mirrors `media::library::ArtistDetail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub track_count: i64,
    pub total_plays: i64,
    pub last_played_at: Option<i64>,
}

impl Artist {
    pub fn label(&self) -> String {
        if self.name.trim().is_empty() { "(unknown)".into() } else { self.name.clone() }
    }
    /// "12 tracks · 3 plays" — both pluralised, because "1 tracks" is the
    /// single most common tell that a list was generated rather than written.
    pub fn counts(&self) -> String {
        format!("{} · {}", plural(self.track_count, "track"), plural(self.total_plays, "play"))
    }
}

/// `N thing` / `N things`, via the shared helper — which carries the rule that
/// zero is plural.
pub fn plural(n: i64, word: &str) -> String { uikit_egui::fmt::plural(n, word) }

// ── modes and sorts ─────────────────────────────────────────────────────────

/// TOP (play-ranked, capped) vs ALPHA (the whole library, sorted and paged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode { Top, Alpha }

impl Mode {
    pub fn tag(self) -> &'static str {
        match self { Mode::Top => "top", Mode::Alpha => "alpha" }
    }
    pub fn from_tag(s: &str) -> Mode {
        if s == "alpha" { Mode::Alpha } else { Mode::Top }
    }
    /// The sort chips only appear in ALPHA: TOP is already play-ranked, so a
    /// sort control there would either be ignored or silently destroy the
    /// ranking the tab exists to show.
    pub fn shows_sort(self) -> bool { self == Mode::Alpha }
}

/// Album sort keys. Tags match `media::library::AlbumSort::from_tag` exactly,
/// so a chip in this port writes the tag the backend already parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumSort { Title, Artist, Year, MostPlayed }

impl AlbumSort {
    pub const ALL: [AlbumSort; 4] =
        [AlbumSort::Title, AlbumSort::Artist, AlbumSort::Year, AlbumSort::MostPlayed];

    pub fn from_tag(s: &str) -> AlbumSort {
        match s {
            "artist" => AlbumSort::Artist,
            "year" => AlbumSort::Year,
            "plays" => AlbumSort::MostPlayed,
            _ => AlbumSort::Title,       // same default as the backend's `_ =>`
        }
    }
    pub fn tag(self) -> &'static str {
        match self {
            AlbumSort::Title => "title",
            AlbumSort::Artist => "artist",
            AlbumSort::Year => "year",
            AlbumSort::MostPlayed => "plays",
        }
    }
    /// The chip's label, from the QML's model.
    pub fn label(self) -> &'static str {
        match self {
            AlbumSort::Title => "Title",
            AlbumSort::Artist => "Artist",
            AlbumSort::Year => "Year",
            AlbumSort::MostPlayed => "Most played",
        }
    }
}

/// Artist sort keys. Only two, because the paged artist query aggregates a
/// track count but not play counts — offering "most played" here would sort by
/// a column the page does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtistSort { Name, TrackCount }

impl ArtistSort {
    pub const ALL: [ArtistSort; 2] = [ArtistSort::Name, ArtistSort::TrackCount];
    pub fn from_tag(s: &str) -> ArtistSort {
        if s == "tracks" { ArtistSort::TrackCount } else { ArtistSort::Name }
    }
    pub fn tag(self) -> &'static str {
        match self { ArtistSort::Name => "name", ArtistSort::TrackCount => "tracks" }
    }
    pub fn label(self) -> &'static str {
        match self { ArtistSort::Name => "Name", ArtistSort::TrackCount => "Track count" }
    }
}

/// Order albums the way `all_albums_sorted` does, tiebreaks included. The
/// tiebreak is not decoration: without it a re-sort reshuffles equal rows every
/// time the page is re-entered, which reads as the list being unstable.
pub fn sort_albums(albums: &mut [Album], sort: AlbumSort) {
    let lower = |s: &str| s.to_lowercase();
    match sort {
        AlbumSort::Title => albums.sort_by(|a, b| {
            lower(&a.title).cmp(&lower(&b.title)).then(lower(&a.primary_artist).cmp(&lower(&b.primary_artist)))
        }),
        AlbumSort::Artist => albums.sort_by(|a, b| {
            lower(&a.primary_artist).cmp(&lower(&b.primary_artist)).then(lower(&a.title).cmp(&lower(&b.title)))
        }),
        // Newest first; albums with no year sink to the bottom rather than
        // sorting as year 0 at the top, which would fill the first screen of a
        // "newest" list with the least-tagged files in the library.
        AlbumSort::Year => albums.sort_by(|a, b| {
            b.year.unwrap_or(0).cmp(&a.year.unwrap_or(0)).then(lower(&a.title).cmp(&lower(&b.title)))
        }),
        AlbumSort::MostPlayed => albums.sort_by(|a, b| {
            b.total_plays.cmp(&a.total_plays).then(lower(&a.title).cmp(&lower(&b.title)))
        }),
    }
}

/// `top_albums_by_plays`: only albums with plays, most-played first, most
/// recently played breaking ties, capped.
pub fn top_albums(all: &[Album], limit: usize) -> Vec<Album> {
    let mut v: Vec<Album> = all.iter().filter(|a| a.total_plays > 0).cloned().collect();
    v.sort_by(|a, b| {
        b.total_plays.cmp(&a.total_plays)
            .then(b.last_played_at.unwrap_or(i64::MIN).cmp(&a.last_played_at.unwrap_or(i64::MIN)))
    });
    v.truncate(limit);
    v
}

/// Order artists as `artists_with_counts_page_sorted` does.
pub fn sort_artists(artists: &mut [Artist], sort: ArtistSort) {
    match sort {
        ArtistSort::Name => artists.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        // Most tracks first, name for a stable tiebreak ("n DESC, a.norm").
        ArtistSort::TrackCount => artists.sort_by(|a, b| {
            b.track_count.cmp(&a.track_count).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
    }
}

/// `top_artists_by_plays`: plays, then track count, then name. Note the absent
/// `total_plays > 0` filter — see the module doc.
pub fn top_artists(all: &[Artist], limit: usize) -> Vec<Artist> {
    let mut v = all.to_vec();
    v.sort_by(|a, b| {
        b.total_plays.cmp(&a.total_plays)
            .then(b.track_count.cmp(&a.track_count))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    v.truncate(limit);
    v
}

/// The leading badge on an artist row: rank in TOP, initial letter in A→Z.
///
/// Two modes need two badges. A rank number in an alphabetical list is a
/// meaningless ordinal, and an initial letter in a ranked list hides the
/// ranking, which is the only thing that tab conveys.
pub fn badge(mode: Mode, index: usize, name: &str) -> String {
    match mode {
        Mode::Top => (index + 1).to_string(),
        Mode::Alpha => name.trim().chars().next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into()),
    }
}

/// What an empty list says, which depends on the mode — the two empties have
/// different causes and different fixes.
pub fn albums_empty_text(mode: Mode) -> &'static str {
    match mode {
        Mode::Top => "No albums with play history yet. Switch to ALPHA to browse everything in your library.",
        Mode::Alpha => "No albums in your library. Run a scan to populate the catalog.",
    }
}

pub fn artists_empty_text(_mode: Mode) -> &'static str { "No artists yet — scan your library first" }

// ── the paged A→Z list ──────────────────────────────────────────────────────

/// A fetch the view wants the host to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub offset: usize,
    pub limit: usize,
    pub sort: ArtistSort,
}

/// A page coming back. `context` is the mode it was requested for, so a
/// response for a tab the user has left can be recognised and dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub context: String,
    pub offset: usize,
    pub total: i64,
    pub items: Vec<Artist>,
}

/// Why a page was not adopted. Returned rather than a bare bool because the
/// three cases want different handling and a bool loses that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject { WrongContext, WrongOffset }

/// The A→Z accumulator.
#[derive(Debug, Clone, Default)]
pub struct Pager {
    pub items: Vec<Artist>,
    pub total: i64,
    /// True while a fetch is in flight. Gates the scroll trigger so a scroll
    /// that fires every frame does not issue the same request repeatedly.
    pub loading: bool,
    /// The mode this pager is accumulating for.
    pub context: String,
}

impl Pager {
    /// Start again from offset 0 — on entering A→Z, or on a sort change, which
    /// invalidates every fetched row (keeping them would interleave two
    /// orderings, which looks like corruption rather than a sort).
    pub fn refresh(&mut self, sort: ArtistSort) -> Request {
        self.items.clear();
        self.total = 0;
        self.loading = true;
        self.context = Mode::Alpha.tag().to_string();
        Request { offset: 0, limit: ALPHA_PAGE, sort }
    }

    /// Adopt a page, or say why not.
    ///
    /// The offset check is the ordering guard: a page is accepted only when it
    /// starts exactly where the accumulator ends. A late page from a previous
    /// request would otherwise append a duplicate or leave a hole.
    ///
    /// Either rejection clears `loading`. Dropping a page while leaving the
    /// flag set convinces the list a fetch is still in flight forever, so it
    /// never asks again — a dead-end mid-library that reads as a hang.
    pub fn accept(&mut self, page: &Page) -> Result<usize, Reject> {
        if page.context != self.context {
            self.loading = false;
            return Err(Reject::WrongContext);
        }
        if page.offset != self.items.len() {
            self.loading = false;
            return Err(Reject::WrongOffset);
        }
        let n = page.items.len();
        self.items.extend(page.items.iter().cloned());
        self.total = page.total;
        self.loading = false;
        Ok(n)
    }

    pub fn complete(&self) -> bool { self.total > 0 && self.items.len() as i64 >= self.total }

    /// The next fetch, if one is warranted. `None` while loading, when
    /// complete, or before the first page has established a total.
    pub fn maybe_fetch_more(&mut self, sort: ArtistSort) -> Option<Request> {
        if self.loading || self.complete() { return None; }
        self.loading = true;
        Some(Request { offset: self.items.len(), limit: ALPHA_PAGE, sort })
    }
}

/// Is the viewport close enough to the tail to prefetch?
///
/// Four rows of lead time: triggering when the tail is *reached* means the user
/// watches the gap while the fetch runs, which is the thing prefetching exists
/// to avoid.
pub fn should_prefetch(viewport_bottom: f32, content_height: f32, row_h: f32) -> bool {
    content_height - viewport_bottom < row_h * PREFETCH_ROWS
}

// ── detail pages ────────────────────────────────────────────────────────────

/// Which detail page is open, if any.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Detail {
    #[default]
    None,
    Album(AlbumKey),
    Artist { id: i64, name: String },
}

impl Detail {
    /// The context tag a response for this page carries — `artist:<id>` is the
    /// backend's own tagging (`library_context`).
    pub fn context(&self) -> String {
        match self {
            Detail::None => String::new(),
            Detail::Album(k) => format!("album:{}\u{1}{}", k.title, k.artist),
            Detail::Artist { id, .. } => format!("artist:{id}"),
        }
    }

    /// Adopt a track list only if it was loaded for THIS page.
    ///
    /// Tap artist A, tap back, tap artist B: A's response can still be in
    /// flight, and without this check it populates B's page with A's tracks —
    /// which looks like a catalogue error rather than a race.
    pub fn adopt(&self, context: &str) -> bool {
        !matches!(self, Detail::None) && context == self.context()
    }

    pub fn is_open(&self) -> bool { !matches!(self, Detail::None) }
}

/// A track as the detail pages list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailTrack {
    pub id: i64,
    pub title: String,
    pub artist: String,
}

impl DetailTrack {
    pub fn label(&self) -> String {
        if self.title.trim().is_empty() { format!("track {}", self.id) } else { self.title.clone() }
    }
}

/// What a tap on a browse surface asks the host to do.
///
/// Returned rather than performed, for the reason `catalog_source` states: the
/// view does not own the queue. bo-mux does, and a second writer in the UI
/// thread would fight it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    OpenAlbum(AlbumKey),
    OpenArtist { id: i64, name: String },
    Back,
    Play(i64),
    Queue(i64),
    /// Every track on the page, in the order shown. Order matters: "play all"
    /// on an album means the album, and re-deriving the order host-side would
    /// lose whatever sort the user is looking at.
    PlayAll(Vec<i64>),
    QueueAll(Vec<i64>),
    /// The user changed a tab or a sort chip; the host reloads.
    Reload,
}

// ── local aggregation ───────────────────────────────────────────────────────

/// One track, with the fields the aggregate queries actually select. Separate
/// from `catalog_source::CatalogTrack` because that mirrors `catalog::Track`,
/// which carries bpm and key and no play counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseTrack {
    pub id: i64,
    pub title: String,
    pub album: Option<String>,
    pub primary_artist: String,
    pub artist_id: i64,
    pub year: Option<u16>,
    pub play_count: u32,
    pub last_played_at: Option<i64>,
}

/// Group tracks into albums, as `all_albums_alpha_base` does.
///
/// Tracks with no album are excluded, not grouped under "". The SQL says
/// `WHERE t.album IS NOT NULL AND t.album != ''`, and a card titled "(untitled)"
/// holding every loose file in the library is not an album.
pub fn aggregate_albums(tracks: &[BrowseTrack]) -> Vec<Album> {
    let mut by_key: HashMap<AlbumKey, Vec<&BrowseTrack>> = HashMap::new();
    for t in tracks {
        let Some(album) = t.album.as_deref() else { continue };
        if album.trim().is_empty() { continue; }
        by_key.entry(AlbumKey { title: album.to_string(), artist: t.primary_artist.clone() })
            .or_default().push(t);
    }
    let mut out: Vec<Album> = by_key.into_iter().map(|(k, ts)| Album {
        title: k.title,
        primary_artist: k.artist,
        year: modal_year(&ts),
        track_count: ts.len() as u32,
        total_plays: ts.iter().map(|t| t.play_count).sum(),
        last_played_at: ts.iter().filter_map(|t| t.last_played_at).max(),
    }).collect();
    sort_albums(&mut out, AlbumSort::Title);
    out
}

/// The most common year across an album's tracks; an even split resolves to the
/// earlier year. Re-releases and compilations routinely disagree with
/// themselves, and picking any one track's year makes the displayed year depend
/// on scan order.
fn modal_year(tracks: &[&BrowseTrack]) -> Option<u16> {
    let mut counts: HashMap<u16, usize> = HashMap::new();
    for t in tracks { if let Some(y) = t.year { *counts.entry(y).or_default() += 1; } }
    counts.into_iter()
        .max_by(|(ya, ca), (yb, cb)| ca.cmp(cb).then(yb.cmp(ya)))   // count desc, year asc
        .map(|(y, _)| y)
}

/// Group tracks by primary artist, as `all_artists_alpha_base` does.
pub fn aggregate_artists(tracks: &[BrowseTrack]) -> Vec<Artist> {
    let mut by_id: HashMap<i64, Vec<&BrowseTrack>> = HashMap::new();
    for t in tracks { by_id.entry(t.artist_id).or_default().push(t); }
    let mut out: Vec<Artist> = by_id.into_iter().map(|(id, ts)| Artist {
        id,
        name: ts[0].primary_artist.clone(),
        track_count: ts.len() as i64,
        total_plays: ts.iter().map(|t| t.play_count as i64).sum(),
        last_played_at: ts.iter().filter_map(|t| t.last_played_at).max(),
    }).collect();
    sort_artists(&mut out, ArtistSort::Name);
    out
}

/// Tracks on an album, in the order the detail page lists them.
pub fn album_tracks(tracks: &[BrowseTrack], key: &AlbumKey) -> Vec<DetailTrack> {
    tracks.iter()
        .filter(|t| t.album.as_deref() == Some(key.title.as_str()) && t.primary_artist == key.artist)
        .map(|t| DetailTrack { id: t.id, title: t.title.clone(), artist: t.primary_artist.clone() })
        .collect()
}

/// Tracks by an artist.
pub fn artist_tracks(tracks: &[BrowseTrack], artist_id: i64) -> Vec<DetailTrack> {
    tracks.iter().filter(|t| t.artist_id == artist_id)
        .map(|t| DetailTrack { id: t.id, title: t.title.clone(), artist: t.primary_artist.clone() })
        .collect()
}

// ── view state ──────────────────────────────────────────────────────────────

/// Which browse list is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab { #[default] Albums, Artists }

/// Everything the four surfaces need across frames. Held by the Controller, so
/// leaving the Library page and coming back keeps the mode, the sort and the
/// scroll — a browse that forgets where it was makes a large library unusable.
#[derive(Debug, Clone, Default)]
pub struct Browse {
    pub tab: Tab,
    pub album_mode: Mode,
    pub artist_mode: Mode,
    pub album_sort: AlbumSort,
    pub artist_sort: ArtistSort,
    pub albums: Vec<Album>,
    /// TOP artists: unpaginated, capped at 50.
    pub artists_top: Vec<Artist>,
    /// A→Z artists: the paged accumulator.
    pub pager: Pager,
    pub detail: Detail,
    pub detail_tracks: Vec<DetailTrack>,
    /// True once a source has filled the lists, so an unloaded browse can say
    /// so instead of claiming the library is empty.
    pub loaded: bool,
}

impl Default for Mode { fn default() -> Self { Mode::Top } }
impl Default for AlbumSort { fn default() -> Self { AlbumSort::Title } }
impl Default for ArtistSort { fn default() -> Self { ArtistSort::Name } }

impl Browse {
    /// Fill from a complete track list. Correct only when the whole catalogue
    /// is in memory; on device the host supplies the aggregates from SQL, which
    /// is why `Action` exists rather than this being the only path.
    pub fn load_local(&mut self, tracks: &[BrowseTrack]) {
        self.albums = aggregate_albums(tracks);
        let artists = aggregate_artists(tracks);
        self.artists_top = top_artists(&artists, TOP_ARTISTS);
        self.pager.items = artists;
        self.pager.total = self.pager.items.len() as i64;
        self.pager.context = Mode::Alpha.tag().to_string();
        self.pager.loading = false;
        self.loaded = true;
    }

    /// The albums the grid draws, for the current mode and sort.
    pub fn visible_albums(&self) -> Vec<Album> {
        match self.album_mode {
            Mode::Top => top_albums(&self.albums, TOP_ALBUMS),
            Mode::Alpha => {
                let mut v = self.albums.clone();
                sort_albums(&mut v, self.album_sort);
                v
            }
        }
    }

    /// The artists the list draws. TOP is the capped ranked list; A→Z is the
    /// accumulator, re-sorted by the chip.
    pub fn visible_artists(&self) -> Vec<Artist> {
        match self.artist_mode {
            Mode::Top => self.artists_top.clone(),
            Mode::Alpha => {
                let mut v = self.pager.items.clone();
                sort_artists(&mut v, self.artist_sort);
                v
            }
        }
    }

    pub fn open_album(&mut self, key: AlbumKey, tracks: &[BrowseTrack]) {
        self.detail_tracks = album_tracks(tracks, &key);
        self.detail = Detail::Album(key);
    }

    pub fn open_artist(&mut self, id: i64, name: String, tracks: &[BrowseTrack]) {
        self.detail_tracks = artist_tracks(tracks, id);
        self.detail = Detail::Artist { id, name };
    }

    /// Back clears the tracks as well as the page. Leaving them means the next
    /// detail page flashes the previous one's list for a frame before its own
    /// arrives, which reads as the wrong album opening.
    pub fn back(&mut self) {
        self.detail = Detail::None;
        self.detail_tracks.clear();
    }

    /// The detail header line: title over artist, or artist over a count.
    pub fn detail_title(&self) -> (String, String) {
        match &self.detail {
            Detail::None => (String::new(), String::new()),
            Detail::Album(k) => (
                if k.title.is_empty() { "(no album)".into() } else { k.title.clone() },
                if k.artist.is_empty() { "—".into() } else { k.artist.clone() },
            ),
            Detail::Artist { name, .. } => (
                name.clone(),
                plural(self.detail_tracks.len() as i64, "track"),
            ),
        }
    }
}

// ── drawing ─────────────────────────────────────────────────────────────────

/// Draw whichever browse surface is current, returning what the user asked for.
///
/// One entry point for four pages, because the detail pages are a mode of the
/// list pages rather than siblings of them — which is also how the QML's
/// SwipeView parents them.
pub fn browse_ui(ui: &mut egui::Ui, b: &mut Browse,
                 c: &uikit_egui::PaletteColors) -> Option<Action> {
    if b.detail.is_open() { return detail_ui(ui, b, c); }
    let mut action = None;

    ui.horizontal(|ui| {
        if ui.selectable_label(b.tab == Tab::Albums, "ALBUMS").clicked() { b.tab = Tab::Albums; }
        if ui.selectable_label(b.tab == Tab::Artists, "ARTISTS").clicked() { b.tab = Tab::Artists; }
    });

    match b.tab {
        Tab::Albums => {
            let n = b.albums.len();
            ui.horizontal(|ui| {
                ui.colored_label(c.text_weak, format!("· {n} loaded"));
                for m in [Mode::Top, Mode::Alpha] {
                    let label = if m == Mode::Top { "TOP" } else { "ALPHA" };
                    if ui.selectable_label(b.album_mode == m, label).clicked() && b.album_mode != m {
                        b.album_mode = m;
                        action = Some(Action::Reload);
                    }
                }
            });
            if b.album_mode.shows_sort() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(c.text_weak, "SORT");
                    for s in AlbumSort::ALL {
                        if ui.selectable_label(b.album_sort == s, s.label()).clicked()
                            && b.album_sort != s {
                            b.album_sort = s;
                            action = Some(Action::Reload);
                        }
                    }
                });
            }
            let rows = b.visible_albums();
            if rows.is_empty() {
                ui.colored_label(c.text_weak,
                    if !b.loaded { "catalogue not opened" } else { albums_empty_text(b.album_mode) });
                return action;
            }
            egui::ScrollArea::vertical().id_salt("browse_albums").show(ui, |ui| {
                for a in rows {
                    let heat = a.heat();
                    ui.horizontal(|ui| {
                        // The tint is the ranking made visible once position has
                        // scrolled off the top of the screen.
                        if heat != Heat::Cold {
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(4.0, 28.0), egui::Sense::hover());
                            let alpha = (heat.tint() * 4.0 * 255.0).min(255.0) as u8;
                            ui.painter().rect_filled(rect, 1.0,
                                egui::Color32::from_rgba_unmultiplied(
                                    c.accent.r(), c.accent.g(), c.accent.b(), alpha));
                        }
                        if ui.selectable_label(false, a.label()).clicked() {
                            action = Some(Action::OpenAlbum(a.key()));
                        }
                        ui.colored_label(c.text_weak, a.artist_label());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.colored_label(c.text_weak, a.meta());
                        });
                    });
                }
            });
        }
        Tab::Artists => {
            ui.horizontal(|ui| {
                for m in [Mode::Top, Mode::Alpha] {
                    let label = if m == Mode::Top { "Top" } else { "A→Z" };
                    if ui.selectable_label(b.artist_mode == m, label).clicked() && b.artist_mode != m {
                        b.artist_mode = m;
                        action = Some(Action::Reload);
                    }
                }
                if b.pager.loading && b.artist_mode == Mode::Alpha {
                    ui.colored_label(c.text_weak, "Loading more artists…");
                }
            });
            if b.artist_mode.shows_sort() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(c.text_weak, "SORT");
                    for s in ArtistSort::ALL {
                        if ui.selectable_label(b.artist_sort == s, s.label()).clicked()
                            && b.artist_sort != s {
                            b.artist_sort = s;
                            action = Some(Action::Reload);
                        }
                    }
                });
            }
            let rows = b.visible_artists();
            if rows.is_empty() && !b.pager.loading {
                ui.colored_label(c.text_weak,
                    if !b.loaded { "catalogue not opened" } else { artists_empty_text(b.artist_mode) });
                return action;
            }
            let mode = b.artist_mode;
            egui::ScrollArea::vertical().id_salt("browse_artists").show(ui, |ui| {
                for (i, a) in rows.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.colored_label(c.accent, badge(mode, i, &a.name));
                        if ui.selectable_label(false, a.label()).clicked() {
                            action = Some(Action::OpenArtist { id: a.id, name: a.name.clone() });
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.colored_label(c.text_weak, a.counts());
                        });
                    });
                }
            });
        }
    }
    action
}

fn detail_ui(ui: &mut egui::Ui, b: &mut Browse,
             c: &uikit_egui::PaletteColors) -> Option<Action> {
    let mut action = None;
    let (title, sub) = b.detail_title();
    ui.horizontal(|ui| {
        if ui.button("‹").clicked() { action = Some(Action::Back); }
        ui.vertical(|ui| {
            ui.strong(title);
            ui.colored_label(c.text_weak, sub);
        });
    });
    let ids: Vec<i64> = b.detail_tracks.iter().map(|t| t.id).collect();
    ui.horizontal(|ui| {
        // Disabled rather than hidden when the album is empty: a Play All that
        // vanishes reads as a missing feature, one that is greyed reads as an
        // empty album, which is the true statement.
        let can = !ids.is_empty();
        if ui.add_enabled(can, egui::Button::new("Play all")).clicked() {
            action = Some(Action::PlayAll(ids.clone()));
        }
        if ui.add_enabled(can, egui::Button::new("Queue all")).clicked() {
            action = Some(Action::QueueAll(ids.clone()));
        }
    });
    ui.separator();
    if b.detail_tracks.is_empty() {
        ui.colored_label(c.text_weak, "no tracks");
        return action;
    }
    egui::ScrollArea::vertical().id_salt("browse_detail").show(ui, |ui| {
        for t in &b.detail_tracks {
            ui.horizontal(|ui| {
                if ui.selectable_label(false, t.label()).clicked() {
                    action = Some(Action::Play(t.id));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("+ queue").clicked() { action = Some(Action::Queue(t.id)); }
                });
            });
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alb(title: &str, artist: &str, year: Option<u16>, plays: u32, last: Option<i64>) -> Album {
        Album { title: title.into(), primary_artist: artist.into(), year,
                track_count: 3, total_plays: plays, last_played_at: last }
    }
    fn art(id: i64, name: &str, tracks: i64, plays: i64) -> Artist {
        Artist { id, name: name.into(), track_count: tracks, total_plays: plays, last_played_at: None }
    }
    fn trk(id: i64, title: &str, album: Option<&str>, artist: &str, aid: i64,
           year: Option<u16>, plays: u32) -> BrowseTrack {
        BrowseTrack { id, title: title.into(), album: album.map(str::to_string),
                      primary_artist: artist.into(), artist_id: aid, year,
                      play_count: plays, last_played_at: None }
    }

    #[test]
    fn an_album_is_identified_by_title_and_artist_not_title_alone() {
        // Keying on title merges every "Greatest Hits" in the library into one
        // card, and opening it shows a mixture of artists.
        let tracks = vec![
            trk(1, "a", Some("Greatest Hits"), "Ada", 1, None, 0),
            trk(2, "b", Some("Greatest Hits"), "Bo", 2, None, 0),
        ];
        let albums = aggregate_albums(&tracks);
        assert_eq!(albums.len(), 2, "same title, different artists, two albums");
        let mine = album_tracks(&tracks, &AlbumKey { title: "Greatest Hits".into(), artist: "Bo".into() });
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].id, 2, "and opening one shows only its own tracks");
    }

    #[test]
    fn tracks_with_no_album_are_excluded_rather_than_grouped_under_blank() {
        let tracks = vec![
            trk(1, "loose", None, "Ada", 1, None, 0),
            trk(2, "also loose", Some("  "), "Ada", 1, None, 0),
            trk(3, "real", Some("Blue"), "Ada", 1, None, 0),
        ];
        let albums = aggregate_albums(&tracks);
        assert_eq!(albums.len(), 1, "a card holding every loose file is not an album");
        assert_eq!(albums[0].title, "Blue");
    }

    #[test]
    fn the_album_year_is_the_most_common_one_and_a_tie_goes_to_the_earlier() {
        // Re-releases disagree with themselves; picking a sample makes the
        // displayed year depend on scan order.
        let tracks = vec![
            trk(1, "a", Some("X"), "Ada", 1, Some(1999), 0),
            trk(2, "b", Some("X"), "Ada", 1, Some(2007), 0),
            trk(3, "c", Some("X"), "Ada", 1, Some(2007), 0),
        ];
        assert_eq!(aggregate_albums(&tracks)[0].year, Some(2007), "the mode wins");
        let tied = vec![
            trk(1, "a", Some("X"), "Ada", 1, Some(2007), 0),
            trk(2, "b", Some("X"), "Ada", 1, Some(1999), 0),
        ];
        assert_eq!(aggregate_albums(&tied)[0].year, Some(1999), "an even split takes the earlier");
    }

    #[test]
    fn top_albums_excludes_the_unplayed_and_ranks_by_plays_then_recency() {
        let all = vec![
            alb("Quiet", "Ada", None, 0, None),
            alb("Loud", "Ada", None, 9, Some(100)),
            alb("Older", "Bo", None, 9, Some(10)),
            alb("Loudest", "Bo", None, 30, Some(1)),
        ];
        let top = top_albums(&all, 40);
        assert_eq!(top.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
                   vec!["Loudest", "Loud", "Older"]);
        assert!(!top.iter().any(|a| a.title == "Quiet"),
                "'top played' over something never played is not a ranking");
    }

    #[test]
    fn top_albums_respects_the_cap() {
        let all: Vec<Album> = (0..60).map(|i| alb(&format!("A{i}"), "Ada", None, i + 1, None)).collect();
        assert_eq!(top_albums(&all, TOP_ALBUMS).len(), 40);
    }

    #[test]
    fn a_year_sort_sinks_the_untagged_instead_of_putting_them_first() {
        // Treating a missing year as 0 fills the first screen of a "newest"
        // list with the least-tagged files in the library.
        let mut v = vec![
            alb("No year", "Ada", None, 0, None),
            alb("Old", "Ada", Some(1990), 0, None),
            alb("New", "Ada", Some(2020), 0, None),
        ];
        sort_albums(&mut v, AlbumSort::Year);
        assert_eq!(v.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
                   vec!["New", "Old", "No year"]);
    }

    #[test]
    fn equal_rows_keep_a_stable_order_across_re_sorts() {
        // Without the tiebreak the list reshuffles every time the page is
        // re-entered, which reads as an unstable list rather than a sort.
        let mut v = vec![
            alb("Zebra", "Ada", None, 5, None),
            alb("apple", "Bo", None, 5, None),
        ];
        sort_albums(&mut v, AlbumSort::MostPlayed);
        let first = v.iter().map(|a| a.title.clone()).collect::<Vec<_>>();
        sort_albums(&mut v, AlbumSort::MostPlayed);
        assert_eq!(first, v.iter().map(|a| a.title.clone()).collect::<Vec<_>>());
        assert_eq!(first[0], "apple", "case-insensitive, so 'Zebra' does not sort before 'apple'");
    }

    #[test]
    fn sort_tags_match_the_backends_exactly_including_the_unknown_default() {
        for s in AlbumSort::ALL { assert_eq!(AlbumSort::from_tag(s.tag()), s); }
        for s in ArtistSort::ALL { assert_eq!(ArtistSort::from_tag(s.tag()), s); }
        // A chip writing a tag the backend parses to something else silently
        // sorts by the wrong column.
        assert_eq!(AlbumSort::from_tag("nonsense"), AlbumSort::Title);
        assert_eq!(ArtistSort::from_tag("nonsense"), ArtistSort::Name);
    }

    #[test]
    fn artists_rank_by_plays_then_track_count_then_name() {
        let all = vec![art(1, "Cy", 2, 5), art(2, "Ada", 9, 5), art(3, "Bo", 1, 99)];
        let top = top_artists(&all, TOP_ARTISTS);
        assert_eq!(top.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Bo", "Ada", "Cy"]);
    }

    #[test]
    fn an_unplayed_artist_can_top_rank_where_an_unplayed_album_cannot() {
        // Not a tidy rule — it is the backend's, and the two pages' empty
        // states are written for it. Recorded so a later "fix" is deliberate.
        let arts = vec![art(1, "Ada", 3, 0)];
        assert_eq!(top_artists(&arts, TOP_ARTISTS).len(), 1);
        let albums = vec![alb("X", "Ada", None, 0, None)];
        assert!(top_albums(&albums, TOP_ALBUMS).is_empty());
    }

    #[test]
    fn track_count_sort_puts_the_biggest_first_and_breaks_ties_by_name() {
        let mut v = vec![art(1, "Zed", 4, 0), art(2, "Ada", 4, 0), art(3, "Bo", 9, 0)];
        sort_artists(&mut v, ArtistSort::TrackCount);
        assert_eq!(v.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Bo", "Ada", "Zed"]);
    }

    #[test]
    fn the_badge_is_a_rank_in_top_and_a_letter_in_alpha() {
        assert_eq!(badge(Mode::Top, 0, "ada"), "1", "one-based: there is no artist zero");
        assert_eq!(badge(Mode::Alpha, 0, "ada"), "A");
        assert_eq!(badge(Mode::Alpha, 3, "  bo"), "B", "leading space is not an initial");
        assert_eq!(badge(Mode::Alpha, 0, ""), "?", "an unnamed artist still gets a badge");
    }

    #[test]
    fn the_sort_chips_only_appear_where_they_mean_something() {
        // TOP is already play-ranked; a sort there would destroy the ranking
        // the tab exists to show.
        assert!(!Mode::Top.shows_sort());
        assert!(Mode::Alpha.shows_sort());
    }

    #[test]
    fn the_empty_state_says_which_empty_it_is() {
        assert_ne!(albums_empty_text(Mode::Top), albums_empty_text(Mode::Alpha));
        assert!(albums_empty_text(Mode::Top).contains("ALPHA"),
                "the top-tab empty names the way out; the alpha one cannot");
        assert!(albums_empty_text(Mode::Alpha).contains("scan"));
    }

    #[test]
    fn a_page_is_adopted_only_when_it_starts_where_the_list_ends() {
        let mut p = Pager::default();
        let req = p.refresh(ArtistSort::Name);
        assert_eq!(req, Request { offset: 0, limit: ALPHA_PAGE, sort: ArtistSort::Name });
        assert!(p.loading);
        let page = Page { context: "alpha".into(), offset: 0, total: 250,
                          items: vec![art(1, "Ada", 1, 0)] };
        assert_eq!(p.accept(&page), Ok(1));
        assert!(!p.loading);
        // A late page from a previous request would append a duplicate.
        let stale = Page { context: "alpha".into(), offset: 0, total: 250,
                           items: vec![art(1, "Ada", 1, 0)] };
        assert_eq!(p.accept(&stale), Err(Reject::WrongOffset));
        assert_eq!(p.items.len(), 1, "the duplicate is not appended");
    }

    #[test]
    fn a_rejected_page_still_clears_the_loading_flag() {
        // THE bug this guard exists for: dropping a page while leaving
        // `loading` set convinces the list a fetch is in flight forever, so it
        // never asks again and dead-ends mid-library. It reads as a hang.
        let mut p = Pager::default();
        p.refresh(ArtistSort::Name);
        let wrong = Page { context: "top".into(), offset: 0, total: 9, items: vec![] };
        assert_eq!(p.accept(&wrong), Err(Reject::WrongContext));
        assert!(!p.loading, "otherwise maybe_fetch_more never fires again");
        assert!(p.maybe_fetch_more(ArtistSort::Name).is_some(), "and the list recovers");
    }

    #[test]
    fn a_response_for_a_tab_the_user_left_is_dropped() {
        let mut p = Pager::default();
        p.refresh(ArtistSort::Name);
        let from_top = Page { context: "top".into(), offset: 0, total: 50,
                              items: vec![art(1, "Ada", 1, 0)] };
        assert_eq!(p.accept(&from_top), Err(Reject::WrongContext));
        assert!(p.items.is_empty(), "a TOP response must not populate the A→Z list");
    }

    #[test]
    fn a_scroll_that_fires_every_frame_issues_one_request() {
        let mut p = Pager::default();
        p.refresh(ArtistSort::Name);
        p.accept(&Page { context: "alpha".into(), offset: 0, total: 250,
                         items: (0..100).map(|i| art(i, "A", 1, 0)).collect() }).unwrap();
        assert!(p.maybe_fetch_more(ArtistSort::Name).is_some());
        assert!(p.maybe_fetch_more(ArtistSort::Name).is_none(), "in flight");
        assert!(p.maybe_fetch_more(ArtistSort::Name).is_none());
    }

    #[test]
    fn a_complete_list_stops_asking() {
        let mut p = Pager::default();
        p.refresh(ArtistSort::Name);
        p.accept(&Page { context: "alpha".into(), offset: 0, total: 2,
                         items: vec![art(1, "A", 1, 0), art(2, "B", 1, 0)] }).unwrap();
        assert!(p.complete());
        assert!(p.maybe_fetch_more(ArtistSort::Name).is_none());
    }

    #[test]
    fn a_refresh_discards_the_accumulator() {
        // A sort change invalidates every fetched row; keeping them interleaves
        // two orderings, which looks like corruption rather than a sort.
        let mut p = Pager::default();
        p.refresh(ArtistSort::Name);
        p.accept(&Page { context: "alpha".into(), offset: 0, total: 250,
                         items: (0..100).map(|i| art(i, "A", 1, 0)).collect() }).unwrap();
        let req = p.refresh(ArtistSort::TrackCount);
        assert_eq!(req.offset, 0);
        assert!(p.items.is_empty());
        assert_eq!(p.total, 0);
    }

    #[test]
    fn the_prefetch_fires_four_rows_early_not_at_the_tail() {
        // At the tail the user watches the gap, which is what prefetching is
        // for.
        assert!(should_prefetch(1000.0, 1100.0, ROW_H), "within four rows");
        assert!(!should_prefetch(1000.0, 1400.0, ROW_H), "still far from the end");
        assert!(should_prefetch(1000.0, 1000.0, ROW_H), "at the tail, certainly");
    }

    #[test]
    fn a_detail_page_adopts_only_its_own_tracks() {
        // Tap artist A, back, tap artist B: A's response can still be in
        // flight, and populating B with A's tracks looks like a catalogue
        // error rather than a race.
        let d = Detail::Artist { id: 7, name: "Ada".into() };
        assert!(d.adopt("artist:7"));
        assert!(!d.adopt("artist:3"));
        assert!(!d.adopt(""));
        assert!(!Detail::None.adopt(""), "a closed page adopts nothing");
    }

    #[test]
    fn an_album_detail_context_distinguishes_two_albums_with_one_title() {
        let a = Detail::Album(AlbumKey { title: "Hits".into(), artist: "Ada".into() });
        let b = Detail::Album(AlbumKey { title: "Hits".into(), artist: "Bo".into() });
        assert_ne!(a.context(), b.context());
        assert!(!a.adopt(&b.context()));
    }

    #[test]
    fn going_back_clears_the_previous_pages_tracks() {
        // Otherwise the next detail page flashes the previous one's list for a
        // frame, which reads as the wrong album opening.
        let tracks = vec![trk(1, "a", Some("X"), "Ada", 1, None, 0)];
        let mut b = Browse::default();
        b.open_album(AlbumKey { title: "X".into(), artist: "Ada".into() }, &tracks);
        assert_eq!(b.detail_tracks.len(), 1);
        b.back();
        assert!(!b.detail.is_open());
        assert!(b.detail_tracks.is_empty());
    }

    #[test]
    fn an_unloaded_browse_does_not_claim_the_library_is_empty() {
        let b = Browse::default();
        assert!(!b.loaded, "the distinction the empty-state text depends on");
        assert!(b.visible_albums().is_empty());
    }

    #[test]
    fn loading_locally_fills_both_tabs_and_their_modes() {
        let tracks = vec![
            trk(1, "a", Some("Blue"), "Ada", 1, Some(1999), 4),
            trk(2, "b", Some("Blue"), "Ada", 1, Some(1999), 0),
            trk(3, "c", Some("Red"), "Bo", 2, None, 0),
        ];
        let mut b = Browse::default();
        b.load_local(&tracks);
        assert!(b.loaded);
        assert_eq!(b.albums.len(), 2);
        // TOP shows only the played album; ALPHA shows both.
        assert_eq!(b.visible_albums().len(), 1);
        b.album_mode = Mode::Alpha;
        assert_eq!(b.visible_albums().len(), 2);
        b.tab = Tab::Artists;
        assert_eq!(b.visible_artists().len(), 2);
    }

    #[test]
    fn a_row_never_draws_blank() {
        let a = alb("  ", "", None, 0, None);
        assert_eq!(a.label(), "(untitled)");
        assert_eq!(a.artist_label(), "—");
        assert_eq!(art(1, " ", 0, 0).label(), "(unknown)");
        assert_eq!(DetailTrack { id: 9, title: "".into(), artist: "".into() }.label(), "track 9");
    }

    #[test]
    fn the_meta_line_omits_what_it_does_not_have() {
        // "0 plays" on every unplayed album is noise, and a bare year separator
        // reads as missing data rather than absent data.
        let unplayed = Album { year: None, total_plays: 0, ..alb("X", "Ada", None, 0, None) };
        assert_eq!(unplayed.meta(), "3 tracks");
        let full = Album { year: Some(1999), total_plays: 12, ..alb("X", "Ada", None, 0, None) };
        assert_eq!(full.meta(), "1999 · 3 tracks · ▶ 12");
    }

    #[test]
    fn counts_are_pluralised_including_the_zero_case() {
        assert_eq!(plural(1, "track"), "1 track");
        assert_eq!(plural(2, "track"), "2 tracks");
        assert_eq!(plural(0, "play"), "0 plays");
        assert_eq!(art(1, "Ada", 1, 1).counts(), "1 track · 1 play");
    }

    #[test]
    fn the_tint_buckets_split_where_the_qml_splits_them() {
        assert_eq!(Heat::of(0), Heat::Cold);
        assert_eq!(Heat::of(1), Heat::Played);
        assert_eq!(Heat::of(10), Heat::Played, "the threshold is strictly greater than ten");
        assert_eq!(Heat::of(11), Heat::Hot);
        assert_eq!(Heat::Cold.tint(), 0.0);
    }
}
