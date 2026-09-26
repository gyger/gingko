fn main() {
    // Declaring the app commands makes them subject to capabilities, so only
    // windows granted `allow-<command>` can invoke them (by default every
    // window, including modals loading third-party scripts, could).
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_home_state",
            "get_doc_state",
            "new_document",
            "open_document",
            "open_document_dialog",
            "import_document",
            "remove_recent_document",
            "save_file",
            "save_as",
            "commit_data",
            "local_store_set",
            "set_edit_mode",
            "export_file",
            "export_docx",
            "ask_save_changes",
            "import_json_dialog",
            "save_file_dialog",
            "export_file_dialog",
            "close_document",
            "open_modal",
            "open_external",
        ]),
    ))
    .expect("failed to run tauri-build");
}
