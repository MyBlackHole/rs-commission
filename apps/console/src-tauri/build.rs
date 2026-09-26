fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "session_login",
            "session_logout",
            "read_resource",
            "quote_commission",
            "order_detail",
            "payout_detail",
            "prepare_write",
            "execute_write",
            "discard_write",
        ]),
    ))
    .expect("Tauri build configuration is invalid");
}
