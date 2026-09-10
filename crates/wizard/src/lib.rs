use tauri_plugin_updater::{Error as UpdaterError, UpdaterExt as _};

// TODO: errors?
async fn install_available_update(app: tauri::AppHandle) {
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(UpdaterError::EmptyEndpoints) => return,
        Err(error) => {
            eprintln!("failed to initialize updater: {error}");
            return;
        }
    };

    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => return,
        Err(error) => {
            eprintln!("failed to check for updates: {error}");
            return;
        }
    };

    if let Err(error) = update.download_and_install(|_, _| {}, || {}).await {
        eprintln!("failed to install update: {error}");
        return;
    }

    app.restart();
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let app = app.handle().clone();
            tauri::async_runtime::spawn(install_available_update(app));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
