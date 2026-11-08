# The desktop has to be told the icons changed, or it goes on showing what it had.

ICON_THEME=.local/share/icons/hicolor

give_icon_cache_tool() {
    ln -s "$HERE/stand-ins/icon-cache" "$T/bin/${1:-gtk-update-icon-cache}"
}

# An old cache, like one some other program left, and a theme folder that hasn't changed in years.
old_icon_theme() {
    mkdir -p "$H/$ICON_THEME/scalable/apps" "$H/$ICON_THEME/symbolic/apps"
    echo "old cache" >"$H/$ICON_THEME/icon-theme.cache"
    touch -d 2001-01-01 "$H/$ICON_THEME"
    touch -d 2001-01-02 "$T/years-ago"
}

check_theme_touched() {
    [ -n "$(find "$H/$ICON_THEME" -maxdepth 0 -newer "$T/years-ago")" ] || fail "the theme folder wasn't touched"
}

check_cache_not_listed() {
    ! grep -qF icon-theme.cache "$H/.local/lib/guesthouse/manifest" || fail "the manifest lists the icon cache"
}

t_an_install_refreshes_an_icon_cache_that_is_there() {
    release 1.0
    give_everything
    give_icon_cache_tool
    old_icon_theme
    install_version 1.0
    check_exit 0
    check_log "gtk-update-icon-cache -q -t -f $H/$ICON_THEME"
    [ "$(cat "$H/$ICON_THEME/icon-theme.cache")" = "fresh cache" ] || fail "the cache wasn't rebuilt"
    check_theme_touched
    check_cache_not_listed
}

t_gtk4s_icon_cache_tool_does_too() {
    release 1.0
    give_everything
    give_icon_cache_tool gtk4-update-icon-cache
    old_icon_theme
    install_version 1.0
    check_exit 0
    check_log "gtk4-update-icon-cache -q -t -f $H/$ICON_THEME"
}

t_with_no_cache_none_is_made() {
    release 1.0
    give_everything
    give_icon_cache_tool
    install_version 1.0
    check_exit 0
    check_not_log "update-icon-cache"
    check_gone "$H/$ICON_THEME/icon-theme.cache"
}

t_with_no_icon_cache_tool_the_folder_is_still_touched() {
    release 1.0
    give_everything
    old_icon_theme
    install_version 1.0
    check_exit 0
    check_theme_touched
    [ "$(cat "$H/$ICON_THEME/icon-theme.cache")" = "old cache" ] || fail "the cache changed"
}

t_an_icon_cache_tool_that_fails_does_not_stop_the_install() {
    release 1.0
    give_everything
    printf '#!/bin/sh\nexit 1\n' >"$T/bin/gtk-update-icon-cache"
    chmod +x "$T/bin/gtk-update-icon-cache"
    old_icon_theme
    install_version 1.0
    check_exit 0
    check_there "$H/.local/lib/guesthouse/current"
}

t_an_update_refreshes_the_icons() {
    release 1.0
    release 2.0
    give_everything
    give_icon_cache_tool
    old_icon_theme
    install_version 1.0
    : >"$T/log"
    touch -d 2001-01-01 "$H/$ICON_THEME"
    install_version 2.0
    check_exit 0
    check_log "gtk-update-icon-cache -q -t -f $H/$ICON_THEME"
    check_theme_touched
    check_cache_not_listed
}

t_uninstall_refreshes_the_icons_and_leaves_the_cache() {
    release 1.0
    give_everything
    give_icon_cache_tool
    old_icon_theme
    install_version 1.0
    : >"$T/log"
    touch -d 2001-01-01 "$H/$ICON_THEME"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_log "gtk-update-icon-cache -q -t -f $H/$ICON_THEME"
    check_there "$H/$ICON_THEME/icon-theme.cache"
    check_gone "$H/$ICON_THEME/scalable/apps/io.github.xnixy.Guesthouse.svg"
    check_theme_touched
}
