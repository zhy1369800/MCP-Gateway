use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime, WindowEvent,
};

const TRAY_ID: &str = "mcp-gateway-tray";
const MENU_SHOW: &str = "mcp-gateway-tray-show";
const MENU_QUIT: &str = "mcp-gateway-tray-quit";

/// Shared state toggled when the tray `Quit` entry is activated so the
/// main window's close handler knows to let the close actually go through
/// instead of hiding to tray.
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Tracks whether we already hid the window in response to the current
/// minimize gesture, so the stream of `Resized` events that follow do not
/// trigger redundant `hide()` calls.
static HIDDEN_FOR_MINIMIZE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct TrayLabels {
    show: &'static str,
    quit: &'static str,
    tooltip: &'static str,
}

fn default_labels() -> TrayLabels {
    // Bilingual labels so users see something sensible regardless of UI
    // language before the frontend pushes its chosen translation down.
    TrayLabels {
        show: "显示主窗口 / Show",
        quit: "退出 / Quit",
        tooltip: "MCP Gateway",
    }
}

/// Installs the system tray icon, menu, and window close interceptor.
/// Safe to call once during `setup`.
pub fn install<R: Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    let handle = app.handle().clone();
    let labels = default_labels();

    let show_item = MenuItem::with_id(&handle, MENU_SHOW, labels.show, true, None::<&str>)?;
    let quit_item = MenuItem::with_id(&handle, MENU_QUIT, labels.quit, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(&handle)?;
    let menu = Menu::new(&handle)?;
    menu.append(&show_item)?;
    menu.append(&separator)?;
    menu.append(&quit_item)?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(labels.tooltip)
        .menu(&menu)
        // Right-click is the standard way to open a tray menu on every
        // platform; a left click should just bring the window back.
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(on_tray_icon_event);

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
        // On macOS the tray lives in the menu bar; a template icon adapts
        // to light/dark menu bar automatically. Everywhere else this is a
        // no-op.
        #[cfg(target_os = "macos")]
        {
            builder = builder.icon_as_template(true);
        }
    }

    builder.build(app)?;

    // Intercept close on the main window so [x] sends the window to tray
    // instead of shutting the whole gateway down.
    if let Some(window) = app.get_webview_window("main") {
        let window_for_event = window.clone();
        window.on_window_event(move |event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                if QUIT_REQUESTED.load(Ordering::SeqCst) {
                    // User picked "Quit" from the tray — let the close
                    // propagate so the app actually exits.
                    return;
                }
                api.prevent_close();
                HIDDEN_FOR_MINIMIZE.store(false, Ordering::SeqCst);
                let _ = window_for_event.hide();
            }
            // Tauri 2.10 does not surface a dedicated `Minimized` variant.
            // Minimize gestures arrive as `Resized` events, so we check
            // `is_minimized()` and route those to the tray as well.
            WindowEvent::Resized(_) => {
                if QUIT_REQUESTED.load(Ordering::SeqCst) {
                    return;
                }
                let minimized = window_for_event.is_minimized().unwrap_or(false);
                if minimized {
                    if !HIDDEN_FOR_MINIMIZE.swap(true, Ordering::SeqCst) {
                        let _ = window_for_event.hide();
                    }
                } else {
                    HIDDEN_FOR_MINIMIZE.store(false, Ordering::SeqCst);
                }
            }
            _ => {}
        });
    }

    Ok(())
}

/// Re-label the tray menu entries so they match the active UI language.
pub fn apply_labels<R: Runtime>(
    app: &AppHandle<R>,
    show_label: &str,
    quit_label: &str,
    tooltip: &str,
) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };

    let menu = Menu::new(app)?;
    let show_item = MenuItem::with_id(app, MENU_SHOW, show_label, true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT, quit_label, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    menu.append(&show_item)?;
    menu.append(&separator)?;
    menu.append(&quit_item)?;
    tray.set_menu(Some(menu))?;
    tray.set_tooltip(Some(tooltip))?;
    Ok(())
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
        MENU_SHOW => show_main_window(app),
        MENU_QUIT => quit_app(app),
        _ => {}
    }
}

fn on_tray_icon_event<R: Runtime>(tray: &tauri::tray::TrayIcon<R>, event: TrayIconEvent) {
    // A left-click should bring the window back; the menu is still
    // reachable via right-click (standard platform convention).
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        show_main_window(tray.app_handle());
    }
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        HIDDEN_FOR_MINIMIZE.store(false, Ordering::SeqCst);
    }
}

fn quit_app<R: Runtime>(app: &AppHandle<R>) {
    QUIT_REQUESTED.store(true, Ordering::SeqCst);
    app.exit(0);
}
