.pragma library
// Shared, stateless app-list helpers used by both the Fleet (fetch) and Push pages.

function fuzzy(hay, needle) {
    var j = 0
    for (var i = 0; i < hay.length && j < needle.length; i++) if (hay[i] === needle[j]) j++
    return j === needle.length
}

// Filter an apps array by a fuzzy query (name/description/group) and kind (all|app|lib).
function filterApps(list, q, kind) {
    var ql = (q || "").toLowerCase().trim()
    var out = []
    for (var i = 0; i < list.length; i++) {
        var a = list[i]
        if (kind === "app" && !a.is_bin) continue
        if (kind === "lib" && a.is_bin) continue
        if (ql.length) {
            var hay = (a.name + " " + (a.description || "") + " " + (a.group || "")).toLowerCase()
            if (hay.indexOf(ql) === -1 && !fuzzy(hay, ql)) continue
        }
        out.push(a)
    }
    return out
}

function targetLibc(t) { return (t.indexOf("musl") !== -1) ? "musl" : "gnu" }
