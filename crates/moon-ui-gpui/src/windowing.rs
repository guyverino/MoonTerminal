use std::sync::Arc;

use gpui::*;

use crate::design;

pub(crate) const APP_ID: &str = "MoonTerminal";

/// Значки групп (`assets/icons/<id>.png`), ВШИТЫЕ в exe build-скриптом (см. build.rs
/// `embed_group_icons`). Индекс = id значка из `GroupConfig.icon`. Подставляются из exe,
/// без путей на диск (работает в dev и в деплое).
mod group_icons {
    include!(concat!(env!("OUT_DIR"), "/group_icons.rs"));
}

/// PNG-байты значка группы по id (из embed). `None` — нет такого id.
pub(crate) fn group_icon_png(id: u32) -> Option<&'static [u8]> {
    group_icons::GROUP_ICONS.get(id as usize).copied().flatten()
}

/// AppUserModelID окна = ключ группировки в таскбаре Windows. Каждой ГРУППЕ — свой id
/// (`MoonTerminal.<группа>`), чтобы окна групп не слипались в одну кнопку.
///
/// ТОЛЬКО Windows: там значок таскбар-кнопки задаётся отдельно (`RelaunchIconResource`),
/// от app_id не зависит. На Linux/Wayland app_id — ключ сопоставления окна с `.desktop`
/// для ЗНАЧКА; вариативный id ломает иконку, поэтому держим базовый `MoonTerminal`.
/// На X11 значок идёт через `_NET_WM_ICON` (см. `app_icon`), не от app_id. macOS app_id
/// не использует. Итог: вне Windows — всегда базовый id.
pub(crate) fn group_app_id(group: &str) -> String {
    if cfg!(target_os = "windows") && !group.is_empty() {
        format!("{APP_ID}.{group}")
    } else {
        APP_ID.to_string()
    }
}

/// Значок группы (декодированный `assets/icons/<icon_id>.png` из embed) для
/// `WindowOptions.icon`. Движок применяет его на **X11** через `_NET_WM_ICON`. На Windows
/// значок таскбара ставится отдельно через `WM_SETICON` (см. main.rs `set_group_window_icon`),
/// на macOS — из бандла `.app`/`.icns`, на Wayland — из `.desktop`; там поле игнорируется.
pub(crate) fn app_icon(icon_id: u32) -> Option<Arc<image::RgbaImage>> {
    let png = group_icon_png(icon_id)?;
    image::load_from_memory(png)
        .ok()
        .map(|img| Arc::new(img.to_rgba8()))
}

fn app_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    min_size: Option<Size<Pixels>>,
    app_id: String,
    icon: Option<Arc<image::RgbaImage>>,
    transparent_titlebar: bool,
) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(window_bounds),
        display_id,
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            appears_transparent: transparent_titlebar,
            ..Default::default()
        }),
        app_id: Some(app_id),
        window_min_size: min_size,
        window_decorations: design::platform_window_decorations(),
        icon,
        ..Default::default()
    }
}

pub(crate) fn trading_window_options(
    title: impl Into<SharedString>,
    group: &str,
    icon_id: u32,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    min_size: Option<Size<Pixels>>,
) -> WindowOptions {
    app_window_options(
        title,
        window_bounds,
        display_id,
        min_size,
        group_app_id(group),
        app_icon(icon_id),
        true,
    )
}

pub(crate) fn tool_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    min_size: Option<Size<Pixels>>,
    owner: Option<AnyWindowHandle>,
) -> WindowOptions {
    owned_window_options(title, window_bounds, None, min_size, owner, true)
}

pub(crate) fn detached_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    _owner: Option<AnyWindowHandle>,
) -> WindowOptions {
    // Откреп-чарты — НЕЗАВИСИМЫЕ окна (НЕ owned). owned образует с Main группу активации ОС:
    // клик по окну чарта поднимал и Main (особенно заметно на мультимониторе — Main выскакивал
    // на другом экране). Из таскбара прячем ЯВНО (`Hidden`) — раньше скрытие шло побочкой owned.
    // `_owner` больше не используется (оставлен в сигнатуре, чтобы не трогать места вызова).
    let mut options = app_window_options(
        title,
        window_bounds,
        display_id,
        None,
        APP_ID.to_string(),
        None,
        true,
    );
    options.taskbar_visibility = WindowTaskbarVisibility::Hidden;
    options
}

pub(crate) fn debug_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    min_size: Option<Size<Pixels>>,
    owner: Option<AnyWindowHandle>,
    transparent_titlebar: bool,
) -> WindowOptions {
    owned_window_options(
        title,
        window_bounds,
        None,
        min_size,
        owner,
        transparent_titlebar,
    )
}

fn owned_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    min_size: Option<Size<Pixels>>,
    owner: Option<AnyWindowHandle>,
    transparent_titlebar: bool,
) -> WindowOptions {
    // Owned-окна (tool/detached/debug) скрыты из таскбара → значок им не нужен (None).
    let mut options = app_window_options(
        title,
        window_bounds,
        display_id,
        min_size,
        APP_ID.to_string(),
        None,
        transparent_titlebar,
    );
    options.kind = WindowKind::Floating;
    options.relationship = owner.map(WindowRelationship::owned).unwrap_or_default();
    options
}

/// DWM-стиль окна (Windows): без скругления углов, тёмная рамка/заголовок. На прочих ОС — no-op.
#[cfg(target_os = "windows")]
pub(crate) fn configure_dwm_window(window: &Window) {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE,
            DWMWCP_DONOTROUND, DwmSetWindowAttribute,
        },
    };

    window.set_background_appearance(WindowBackgroundAppearance::Opaque);

    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };

    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let corner = DWMWCP_DONOTROUND;
    let colorref_header = 0x001F1C1A_u32;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            std::mem::size_of_val(&corner) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &colorref_header as *const _ as *const _,
            std::mem::size_of_val(&colorref_header) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &colorref_header as *const _ as *const _,
            std::mem::size_of_val(&colorref_header) as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn configure_dwm_window(_: &Window) {}

/// Поставить per-group значок окну группы (из embed `assets/icons/<icon_id>.png`, вшит в exe).
/// ЖИВАЯ таскбар-кнопка/Alt-Tab берут иконку ОКНА → ставим `WM_SETICON` big+small, создавая
/// HICON прямо из PNG-байтов через `CreateIconFromResourceEx` (PNG-иконки — Vista+, dwVer
/// 0x00030000). На X11 значок уже идёт через `WindowOptions.icon` (`_NET_WM_ICON`) — это Windows.
#[cfg(target_os = "windows")]
pub(crate) fn set_group_window_icon(window: &Window, icon_id: u32) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, ICON_BIG, ICON_SMALL, LR_DEFAULTCOLOR, SendMessageW, WM_SETICON,
    };

    let Some(png) = group_icon_png(icon_id) else {
        return;
    };
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    unsafe {
        for (size, which) in [(32_i32, ICON_BIG), (16_i32, ICON_SMALL)] {
            if let Ok(hicon) =
                CreateIconFromResourceEx(png, true, 0x0003_0000, size, size, LR_DEFAULTCOLOR)
            {
                let _ = SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(which as usize)),
                    Some(LPARAM(hicon.0 as isize)),
                );
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn set_group_window_icon(_: &Window, _: u32) {}

/// Скрыть окно из таскбара/Alt-Tab (Windows) через `WS_EX_TOOLWINDOW` — БЕЗ owner-связи.
/// Нужно для откреп-чартов: они независимы (owned цеплялся за активацию Main, см.
/// `detached_window_options`), а движок MoonUI `taskbar_visibility::Hidden` пока не реализует
/// (`show_for` не подключён в бэкенде — скрытие шло только как побочка owner). Идемпотентно:
/// меняем ex-style (с hide/show — требование WinAPI для смены кнопки таскбара) лишь однажды.
#[cfg(target_os = "windows")]
pub(crate) fn hide_window_from_taskbar(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SW_HIDE, SW_SHOWNA, SetWindowLongPtrW, ShowWindow,
        WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = (ex & !(WS_EX_APPWINDOW.0 as isize)) | (WS_EX_TOOLWINDOW.0 as isize);
        if want != ex {
            // WinAPI: чтобы кнопка таскбара исчезла, окно надо спрятать, сменить ex-style и
            // показать обратно БЕЗ активации (SW_SHOWNA — иначе перехватит фокус у текущего).
            let _ = ShowWindow(hwnd, SW_HIDE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
            let _ = ShowWindow(hwnd, SW_SHOWNA);
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn hide_window_from_taskbar(_: &Window) {}

/// Восстановить окно и вернуть его на экран (Windows): разминимизировать (`SW_RESTORE`) и
/// переставить каскадом на первичный монитор (левый-верх primary = (0,0) в координатах ОС) —
/// спасение откреп-окон, уехавших за пределы экранов / на отключённый монитор / свёрнутых.
/// Сеттера позиции окна у gpui-форка нет (есть только `resize`), поэтому двигаем через WinAPI.
#[cfg(target_os = "windows")]
pub(crate) fn reset_window_onscreen(window: &Window, index: usize) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOP, SW_RESTORE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
    };

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    // Каскад с шагом 40px (и переносом по модулю), чтобы окна не легли стопкой друг на друга.
    let off = 60 + (index as i32 % 8) * 40;
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            off,
            off,
            0,
            0,
            SWP_NOSIZE | SWP_SHOWWINDOW,
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn reset_window_onscreen(_: &Window, _: usize) {}
