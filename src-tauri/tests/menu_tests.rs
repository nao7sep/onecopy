// Native menu items cannot call into the webview, so the Settings and About
// items reach Main through these events, which Main's global commands listen
// for; every other item opens nothing in Main.
use onecopy_lib::menu::{main_surface_event, OPEN_ABOUT_MENU_ID, OPEN_SETTINGS_MENU_ID, SAFE_QUIT_MENU_ID};

#[test]
fn the_settings_and_about_items_open_mains_own_surfaces() {
    assert_eq!(main_surface_event(OPEN_SETTINGS_MENU_ID), Some("menu://open-settings"));
    assert_eq!(main_surface_event(OPEN_ABOUT_MENU_ID), Some("menu://open-about"));
}

#[test]
fn other_menu_items_open_nothing_in_main() {
    assert_eq!(main_surface_event(SAFE_QUIT_MENU_ID), None);
    assert_eq!(main_surface_event("about"), None);
}
