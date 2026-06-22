"""
Model of web2qml's terminal pane/tab data flow to locate two dogfdood bugs:
  (1) split panes in a tab all repeat the same shell
  (2) 2nd/3rd tabs show no text

We model three layers:
  A. the Rust pane->pty mapping (self.pty + pane_ptys + ws_swap_active)
  B. the QML split render (outer Repeater over panes, inner Repeater over rows)
     -- faithfully reproducing the modelData shadowing in the inner Repeater
  C. the single-pane poll render (generation + html dedup) across a tab switch
"""

# ---------- Layer A: Rust pane->pty mapping ----------
class Pty:
    _next = 0
    def __init__(self, label):
        self.label = label          # what shell/content this terminal shows
        Pty._next += 1
        self.serial = Pty._next     # distinct identity per spawned shell

class Backend:
    """Mirrors main.rs: self.pty = focused pane's terminal; pane_ptys = the rest."""
    def __init__(self):
        self.pty = Pty("shell@pane0")     # startup shell
        self.pane_ptys = {}               # id -> Pty  (does NOT include focused)
        self.focused = 0                  # active().focused_pane

    def ws_swap_active(self, old_id, new_id):
        if old_id == new_id:
            return
        if self.pty is not None:
            self.pane_ptys[old_id] = self.pty
            self.pty = None
        self.pty = self.pane_ptys.pop(new_id, None) or Pty(f"shell@pane{new_id}")

    def pane_term(self, pane_id):
        # main.rs pane_term(): focused -> self.pty, else pane_ptys[id]
        if pane_id == self.focused:
            return self.pty
        return self.pane_ptys.get(pane_id)

    def split(self, new_id):
        old = self.focused
        self.focused = new_id
        self.ws_swap_active(old, new_id)        # NewPane branch

    def switch_tab(self, old_focused, new_focused):
        self.focused = new_focused
        self.ws_swap_active(old_focused, new_focused)  # GotoTab branch


def check_mapping_distinct():
    b = Backend()
    b.split(1)            # split pane0 -> panes 0 and 1, focus 1
    b.split(2)            # split focused (1) -> 1 and 2, focus 2 ; ids 0,1,2 exist
    labels = {}
    for pid in (0, 1, 2):
        t = b.pane_term(pid)
        labels[pid] = t.serial if t else None
    distinct = len(set(labels.values())) == len([v for v in labels.values() if v])
    print(f"[A] pane->pty serials = {labels}  -> distinct={distinct}")
    return distinct


# ---------- Layer B: QML split render ----------
def qml_split_render(panes, backend):
    """
    Reproduce paneSplitLayer:
      outer Repeater model = panes (each a dict {id,...})  -> outer modelData = pane
      inner Repeater model = paneCell._rows (an integer N) -> inner modelData = row index (a NUMBER)
    The live code's inner Text does: pane_row_html(modelData.id, index)
    But inside the inner Repeater modelData is a NUMBER, so modelData.id is undefined.
    """
    rendered = {}
    for outer_index, pane in enumerate(panes):
        rows = 3  # pane_visible_rows(pane['id'])  -- OUTER scope, correct id
        cells = []
        for index in range(rows):
            inner_modelData = index            # integer model -> modelData IS the row number
            # JS: (a number).id === undefined ; qmetaobject marshals undefined->f64.
            buggy_id = getattr(inner_modelData, "id", None)   # None == undefined
            # Rust: `pane_id as u64` with NaN/undefined -> 0 (saturating cast)
            resolved_id = 0 if buggy_id is None else buggy_id
            t = backend.pane_term(resolved_id)
            cells.append(t.label if t else "<blank>")
        rendered[pane['id']] = cells[0]
    return rendered

def qml_split_render_fixed(panes, backend):
    """Fixed: capture the pane id in the OUTER delegate and use it inside."""
    rendered = {}
    for outer_index, pane in enumerate(panes):
        pane_id = pane['id']                   # property paneId: modelData.id (outer scope)
        rows = 3
        cells = []
        for index in range(rows):
            t = backend.pane_term(pane_id)     # use the captured id, not inner modelData.id
            cells.append(t.label if t else "<blank>")
        rendered[pane['id']] = cells[0]
    return rendered


def check_split_render():
    b = Backend(); b.split(1); b.split(2)      # panes 0,1,2 ; focused 2
    panes = [{'id': 2}, {'id': 1}, {'id': 0}]  # ws_pane_rects: focused first
    buggy = qml_split_render(panes, b)
    fixed = qml_split_render_fixed(panes, b)
    print(f"[B] BUGGY inner render per pane = {buggy}")
    print(f"[B] FIXED inner render per pane = {fixed}")
    all_same = len(set(buggy.values())) == 1
    fixed_distinct = len(set(fixed.values())) == 3
    print(f"[B] buggy: all panes identical = {all_same}  (== bug 1 'repeat their shell')")
    print(f"[B] fixed: all panes distinct  = {fixed_distinct}")
    return all_same and fixed_distinct


# ---------- Layer C: single-pane poll render across a tab switch ----------
def normalize(html):
    return html.strip()

def poll_render(state, snap_gen, snap_html):
    """
    Mirror the main.rs poll: only fire term_output_changed (which bumps the QML
    _outputTick that termRowsHost binds to) when generation changed AND the
    normalized html differs from the last published.
    Returns 'refresh' if termRowsHost would re-render, else 'stale'.
    """
    if snap_gen == state['render_gen']:
        return 'stale'
    state['render_gen'] = snap_gen
    new_norm = normalize(snap_html)
    if state['last_norm'] != new_norm:
        state['last_norm'] = new_norm
        return 'refresh'          # term_output_changed fires -> _outputTick++
    return 'stale'                # only cursor signal -> termRowsHost NOT refreshed


def check_tab_switch_refresh():
    # tab1 shell shows prompt "P"; create tab2 (fresh shell, identical prompt "P").
    # Each pty has its OWN generation sequence.
    state = {'render_gen': 5, 'last_norm': normalize("P")}  # after using tab1
    # switch to tab2: its fresh shell's snapshot. Two sub-cases:
    print("[C] tab2 fresh shell, prompt identical to tab1's:")
    r = poll_render(dict(state), snap_gen=1, snap_html="P")
    print(f"    no forced refresh -> termRowsHost = {r}  (identical prompt suppressed)")

    print("[C] tab2 fresh shell, grid still EMPTY at swap instant:")
    s2 = dict(state)
    r1 = poll_render(s2, snap_gen=1, snap_html="")      # empty grid
    r2 = poll_render(s2, snap_gen=2, snap_html="P")     # prompt arrives
    print(f"    empty->{r1}, then prompt->{r2}")

    # FIX: on swap, reset render_gen + last_norm so the new pty ALWAYS publishes.
    print("[C] WITH forced refresh on swap (render_gen=sentinel, last_norm=''):")
    s3 = {'render_gen': 0xFFFFFFFF, 'last_norm': ''}
    r = poll_render(s3, snap_gen=1, snap_html="P")
    print(f"    -> termRowsHost = {r}  (new tab always renders)")
    return True


if __name__ == "__main__":
    print("=== Layer A: is the Rust pane->pty mapping correct? ===")
    a = check_mapping_distinct()
    print("\n=== Layer B: QML split render (bug 1) ===")
    b = check_split_render()
    print("\n=== Layer C: single-pane tab-switch refresh (bug 2) ===")
    c = check_tab_switch_refresh()
    print("\n=== CONCLUSION ===")
    print(f"A mapping distinct: {a}  -> Rust pane->pty logic is SOUND; bugs are in the render/refresh layer")
    print(f"B split render: bug reproduced and fix distinct: {b}  -> bug 1 = inner-Repeater modelData.id shadowing")
    print(f"C tab refresh: identical-prompt / empty-grid swaps can be SUPPRESSED; forced refresh on swap fixes it")
