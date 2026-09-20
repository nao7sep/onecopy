//! The native menu, in the interface language. It has the same items as
//! Tauri's default menu, which OneCopy used before it was localized, except
//! that quitting routes through the main window so the app's own exit path
//! runs (`SAFE_QUIT_MENU_ID`, handled in lib.rs). The old menu patched Tauri's
//! default by finding a submenu by its English title, which no longer holds
//! once the titles are translated.

/// The quit item OneCopy handles itself, closing the main window so the
/// ordinary exit path quiesces mutations before the process ends.
pub const SAFE_QUIT_MENU_ID: &str = "onecopy.safe-quit";

#[cfg(any(target_os = "macos", target_os = "windows"))]
use tauri::menu::{
    AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID,
};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use tauri::{AppHandle, Wry};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use crate::i18n;

/// Every catalogue key the native menu reads; the tests check each one exists
/// in every language.
pub const KEYS: [&str; 22] = [
    "nativeMenu.about",
    "nativeMenu.services",
    "nativeMenu.hide",
    "nativeMenu.hideOthers",
    "nativeMenu.quit",
    "nativeMenu.exit",
    "nativeMenu.file",
    "nativeMenu.closeWindow",
    "nativeMenu.edit",
    "nativeMenu.undo",
    "nativeMenu.redo",
    "nativeMenu.cut",
    "nativeMenu.copy",
    "nativeMenu.paste",
    "nativeMenu.selectAll",
    "nativeMenu.view",
    "nativeMenu.fullscreen",
    "nativeMenu.window",
    "nativeMenu.minimize",
    "nativeMenu.zoom",
    "nativeMenu.maximize",
    "nativeMenu.help",
];

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn build(app: &AppHandle, language: &str) -> tauri::Result<Menu<Wry>> {
    let name = app.package_info().name.clone();
    let text = i18n::catalogue(language);
    let t = |key: &str| text.text(key, &name);
    let config = app.config();
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(app.package_info().version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|publisher| vec![publisher]),
        ..Default::default()
    };
    let quit_text = if cfg!(target_os = "macos") { t("nativeMenu.quit") } else { t("nativeMenu.exit") };
    let quit = MenuItem::with_id(app, SAFE_QUIT_MENU_ID, quit_text, true, Some("CmdOrCtrl+Q"))?;
    let about_item = PredefinedMenuItem::about(app, Some(&t("nativeMenu.about")), Some(about))?;

    let file = Submenu::with_items(
        app,
        t("nativeMenu.file"),
        true,
        &[
            &PredefinedMenuItem::close_window(app, Some(&t("nativeMenu.closeWindow")))?,
            #[cfg(not(target_os = "macos"))]
            &quit,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        t("nativeMenu.edit"),
        true,
        &[
            &PredefinedMenuItem::undo(app, Some(&t("nativeMenu.undo")))?,
            &PredefinedMenuItem::redo(app, Some(&t("nativeMenu.redo")))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some(&t("nativeMenu.cut")))?,
            &PredefinedMenuItem::copy(app, Some(&t("nativeMenu.copy")))?,
            &PredefinedMenuItem::paste(app, Some(&t("nativeMenu.paste")))?,
            &PredefinedMenuItem::select_all(app, Some(&t("nativeMenu.selectAll")))?,
        ],
    )?;
    let maximize_key = if cfg!(target_os = "macos") { "nativeMenu.zoom" } else { "nativeMenu.maximize" };
    let window = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        t("nativeMenu.window"),
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some(&t("nativeMenu.minimize")))?,
            &PredefinedMenuItem::maximize(app, Some(&t(maximize_key)))?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some(&t("nativeMenu.closeWindow")))?,
        ],
    )?;
    let help = Submenu::with_id_and_items(
        app,
        HELP_SUBMENU_ID,
        t("nativeMenu.help"),
        true,
        &[
            #[cfg(not(target_os = "macos"))]
            &about_item,
        ],
    )?;

    #[cfg(target_os = "macos")]
    {
        let app_menu = Submenu::with_items(
            app,
            name.as_str(),
            true,
            &[
                &about_item,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::services(app, Some(&t("nativeMenu.services")))?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, Some(&t("nativeMenu.hide")))?,
                &PredefinedMenuItem::hide_others(app, Some(&t("nativeMenu.hideOthers")))?,
                &PredefinedMenuItem::separator(app)?,
                &quit,
            ],
        )?;
        let view = Submenu::with_items(
            app,
            t("nativeMenu.view"),
            true,
            &[&PredefinedMenuItem::fullscreen(app, Some(&t("nativeMenu.fullscreen")))?],
        )?;
        Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window, &help])
    }
    #[cfg(not(target_os = "macos"))]
    {
        Menu::with_items(app, &[&file, &edit, &window, &help])
    }
}
