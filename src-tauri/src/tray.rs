use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};

use crate::{config, paste};

pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    // Récupération : si un collage a échoué, la dernière dictée reste à un clic.
    let derniere_item =
        MenuItem::with_id(app, "derniere", "Copier la dernière dictée", true, None::<&str>)?;
    let journal_item = MenuItem::with_id(app, "journal", "Ouvrir le journal", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&derniere_item, &journal_item, &settings_item, &quit_item],
    )?;

    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon@2x.png"))?;

    let _tray = TrayIconBuilder::new()
        .icon(icon)
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("Light Whisper")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "derniere" => {
                if let Ok(texte) = std::fs::read_to_string(config::chemin_derniere_dictee()) {
                    let _ = paste::copier_seulement(&texte);
                }
            }
            "journal" => {
                let _ = std::process::Command::new("open")
                    .arg(config::chemin_journal())
                    .spawn();
            }
            "settings" => {
                if let Some(window) = app.get_webview_window("settings") {
                    let _ = window.show();
                    let _ = window.set_focus();
                    let _ = window.center();
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}
