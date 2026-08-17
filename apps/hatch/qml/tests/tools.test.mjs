/*
    tools.test.mjs — tests for contents/ui/code/tools.js

        node --test qml/tests/

    tools.js is the one QML-side file with no upstream to check against: the
    fork's source drop didn't include kicker's version, so it was written from
    scratch against the two call sites. That makes it the likeliest place for
    a quiet mistake, and it is also the only QML-side file that is plain
    JavaScript — no imports, no QML types — so it can be executed here instead
    of only on the phone.

    The QML engine has no module system, so the file is loaded the way QML
    loads it: read the text and evaluate it, which also proves it parses.
*/

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, "..", "contents", "ui", "code", "tools.js"), "utf8");
const Tools = new Function(`${source}; return { fillActionMenu, triggerAction };`)();

/** QML's i18n, near enough: these tests care about ids, not translations. */
const i18n = (s) => s;

/** A stand-in for kicker's favorites model. */
function favoritesModel({ favorites = [], maxFavorites = -1 } = {}) {
    return {
        favorites,
        maxFavorites,
        get count() {
            return this.favorites.length;
        },
        isFavorite(id) {
            return this.favorites.includes(id);
        },
        addFavorite(id) {
            this.favorites.push(id);
        },
        removeFavorite(id) {
            this.favorites = this.favorites.filter((f) => f !== id);
        },
    };
}

const menu = () => ({ actionList: null });

test("no favorites model and no actions leaves an empty menu", () => {
    const m = menu();
    Tools.fillActionMenu(i18n, m, [], null, null);
    assert.deepEqual(m.actionList, []);
});

test("a non-favorite app offers Add, a favorite offers Remove", () => {
    const fav = favoritesModel();
    const m = menu();

    Tools.fillActionMenu(i18n, m, [], fav, "firefox.desktop");
    assert.equal(m.actionList.length, 1);
    assert.equal(m.actionList[0].actionId, "_kicker_favorite_add");
    assert.equal(m.actionList[0].actionArgument.favoriteId, "firefox.desktop");

    fav.addFavorite("firefox.desktop");
    Tools.fillActionMenu(i18n, m, [], fav, "firefox.desktop");
    assert.equal(m.actionList[0].actionId, "_kicker_favorite_remove");
});

test("the model's own actions come first, separated from the favorite toggle", () => {
    const m = menu();
    const own = [{ text: "New Window", actionId: "newInstance" }];
    Tools.fillActionMenu(i18n, m, own, favoritesModel(), "firefox.desktop");

    assert.equal(m.actionList.length, 3, "action, separator, favorite toggle");
    assert.equal(m.actionList[0].actionId, "newInstance");
    assert.equal(m.actionList[1].type, "separator");
    assert.equal(m.actionList[2].actionId, "_kicker_favorite_add");
});

test("the caller's action list is not mutated", () => {
    // fillActionMenu is called on model.actionList, which belongs to the C++
    // model; appending to it in place would corrupt the model's own data.
    const own = [{ text: "New Window", actionId: "newInstance" }];
    Tools.fillActionMenu(i18n, menu(), own, favoritesModel(), "firefox.desktop");
    assert.equal(own.length, 1);
});

test("a full favorites model offers no Add action", () => {
    const fav = favoritesModel({ favorites: ["a.desktop", "b.desktop"], maxFavorites: 2 });
    const m = menu();
    Tools.fillActionMenu(i18n, m, [], fav, "firefox.desktop");
    assert.deepEqual(m.actionList, [], "no room, so no offer — and no separator either");
});

test("an entry with no favorite id gets only the model's actions", () => {
    const m = menu();
    Tools.fillActionMenu(i18n, m, [{ actionId: "open" }], favoritesModel(), null);
    assert.equal(m.actionList.length, 1);
    assert.equal(m.actionList[0].actionId, "open");
});

test("favorite toggles change the model and keep the launcher open", () => {
    const fav = favoritesModel();
    const arg = { favoriteModel: fav, favoriteId: "firefox.desktop" };

    assert.equal(Tools.triggerAction(null, 0, "_kicker_favorite_add", arg), false);
    assert.deepEqual(fav.favorites, ["firefox.desktop"]);

    assert.equal(Tools.triggerAction(null, 0, "_kicker_favorite_remove", arg), false);
    assert.deepEqual(fav.favorites, []);
});

test("any other action goes to the model, and only a true return closes", () => {
    const calls = [];
    const model = {
        trigger(index, actionId, actionArgument) {
            calls.push([index, actionId, actionArgument]);
            return actionId === "newInstance"; // kicker returns true to close
        },
    };

    assert.equal(Tools.triggerAction(model, 3, "newInstance", null), true);
    assert.equal(Tools.triggerAction(model, 4, "editApplication", null), false);
    assert.deepEqual(calls, [
        [3, "newInstance", null],
        [4, "editApplication", null],
    ]);
});

test("a model returning undefined does not close the launcher", () => {
    // Not every kicker model returns a value; treating undefined as "close"
    // would make the dash vanish on unrelated context-menu actions.
    const model = { trigger: () => undefined };
    assert.equal(Tools.triggerAction(model, 0, "whatever", null), false);
});
