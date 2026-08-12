// Отключаем консольное окно в релизных сборках Windows,
// чтобы приложение вело себя как нативное desktop-приложение.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Принудительный software render: WebKitGTK без доступного EGL падает с
    // "Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...".
    // Отключаем GPU-композитинг, DMABUF-рендерер и форсируем Mesa llvmpipe.
    // Установка безусловна, чтобы AppImage не падал независимо от окружения.
    std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    std::env::set_var("WEBKIT_HARDWARE_ACCELERATION_POLICY", "NEVER");
    std::env::set_var("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", "1");
    std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    md_viewer_lib::run()
}
