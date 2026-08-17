/*
    tools.js — reimplemented for hatch.

    The fork's source drop didn't include kicker's code/tools.js, so this is a
    from-scratch cover of exactly the two calls the dash makes: building a
    delegate's context menu (favorite toggle + the model's own actions) and
    dispatching the chosen action. Semantics follow upstream kicker: the
    special _kicker_favorite_* ids are handled here, everything else goes to
    model.trigger(), and the return value means "close the launcher".
*/

function fillActionMenu(i18n, actionMenu, actionList, favoriteModel, favoriteId) {
    var actions = (actionList || []).slice();

    if (favoriteModel !== null && favoriteId !== null && favoriteId !== undefined && favoriteId !== "") {
        var action = {};
        if (favoriteModel.isFavorite(favoriteId)) {
            action.text = i18n("Remove from Favorites");
            action.icon = "bookmark-remove";
            action.actionId = "_kicker_favorite_remove";
        } else if (favoriteModel.maxFavorites === -1 || favoriteModel.count < favoriteModel.maxFavorites) {
            action.text = i18n("Add to Favorites");
            action.icon = "bookmark-new";
            action.actionId = "_kicker_favorite_add";
        }
        if (action.actionId) {
            action.actionArgument = { favoriteModel: favoriteModel, favoriteId: favoriteId };
            if (actions.length) {
                actions.push({ type: "separator" });
            }
            actions.push(action);
        }
    }

    actionMenu.actionList = actions;
}

function triggerAction(model, index, actionId, actionArgument) {
    if (actionId === "_kicker_favorite_remove") {
        actionArgument.favoriteModel.removeFavorite(actionArgument.favoriteId);
        return false; // favorite toggles keep the launcher open
    }
    if (actionId === "_kicker_favorite_add") {
        actionArgument.favoriteModel.addFavorite(actionArgument.favoriteId);
        return false;
    }
    var closeRequested = model.trigger(index, actionId, actionArgument);
    return closeRequested === true;
}
