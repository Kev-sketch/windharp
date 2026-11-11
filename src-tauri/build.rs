fn main() {
  // App-ACL manifest: autogenerate allow-<command> permissions for the pear
  // bridge commands so the capability file can grant them to the remote
  // music.youtube.com origin. Without this, has_app_acl=false and Tauri's IPC
  // gate (webview/mod.rs: "remote content can never reach custom commands
  // unless an explicit remote capability has been configured") rejects every
  // peard_* invoke from the YouTube Music page — which silently kills the
  // whole renderer plugin boot chain.
  tauri_build::try_build(
    tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
      "peard_store_set",
      "peard_store_set_partial",
      "peard_set_menu_option",
      "peard_emit",
      "peard_subscribe",
      "peard_ipc_invoke",
      "get_menu",
      "get_menu_by_id",
      "peard_menu_event",
      "peard_video_src_changed",
      "peard_play_or_paused",
      "peard_time_changed",
      "peard_restart",
    ])),
  )
  .expect("failed to run tauri-build");
}
