fn main() {
    let attributes = tauri_build::Attributes::new().plugin(
        "app",
        tauri_build::InlinedPlugin::new()
            .commands(&[
                "get_status",
                "toggle_recording",
                "get_config",
                "download_model",
                "list_models",
                "change_model",
                "get_developer_mode",
                "set_developer_mode",
                "update_settings",
                "list_audio_devices",
                "set_widget_visible",
            ])
            .default_permission(tauri_build::DefaultPermissionRule::AllowAllCommands),
    );
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
