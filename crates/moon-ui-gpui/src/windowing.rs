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

#[cfg(target_os = "windows")]
pub(crate) fn window_hwnd(window: &Window) -> Option<isize> {
    use raw_window_handle::RawWindowHandle;
    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return None;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as isize)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn window_hwnd(_window: &Window) -> Option<isize> {
    None
}

/// В какую сторону от курсора (= от кнопки, на которой курсор в момент клика) растёт попап по
/// горизонтали. `Right` — левый край у курсора (растёт вправо); `Left` — правый край у курсора
/// (растёт влево, право-выравнивание под кнопкой у правого края).
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // `Right` — задел для других попапов (параметр стороны открытия)
pub(crate) enum PopupGrowX {
    Left,
    Right,
}

/// Размещение окна-поповера размером `win_size` (логич. px) относительно кнопки-триггера, чей
/// прямоугольник `btn_local` задан в ЛОКАЛЬНЫХ логич. px окна (из canvas-пробы). Возвращает
/// `(origin, display_id)` для `open_window`. `grow_x`: `Left` — правый край попапа = правый край
/// кнопки (растёт влево); `Right` — левый край = левому краю кнопки. `drop_below` (логич. px) —
/// зазор от НИЗА кнопки.
///
/// На Windows экранную позицию кнопки берём `ClientToScreen` по реальному HWND (точно, мимо
/// багнутого `window_bounds().origin`), а монитор и его DPI — `MonitorFromPoint`+`GetDpiForMonitor`
/// по этой физ. точке (мимо перекрытия логич. `display.bounds()` разных мониторов). На прочих ОС —
/// фолбэк через `window_bounds` (там этих багов нет).
/// Физ. экранный прямоугольник `(x,y,w,h)` для корректирующего `SetWindowPos` после открытия —
/// `open_window` на мониторе со смещённым origin кладёт окно мимо (Win32 workspace-координаты +
/// DPI-баг форка), поэтому окно до-двигаем по абсолютным screen-px. `None` вне Windows.
pub(crate) type PopupPhysRect = Option<(i32, i32, i32, i32)>;

#[cfg(target_os = "windows")]
pub(crate) fn popup_placement(
    window: &Window,
    _cx: &App,
    win_size: Size<Pixels>,
    btn_local: Bounds<Pixels>,
    grow_x: PopupGrowX,
    drop_below: f32,
) -> (Point<Pixels>, Option<DisplayId>, PopupPhysRect) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};

    let fallback = || (point(px(80.0), px(80.0)), None, None);
    let Some(hwnd_raw) = window_hwnd(window) else {
        return fallback();
    };
    let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut _);
    // DPI окна берём НАПРЯМУЮ из ОС (`GetDpiForWindow`), НЕ из `window.scale_factor()` — последний
    // на не-первичном мониторе в форке врёт (тот же баг), из-за чего client→physical офсет кнопки
    // считался неверно и попап уезжал влево. 0 (ошибка) → фолбэк на gpui-scale.
    let sf = {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi == 0 {
            window.scale_factor().max(0.1)
        } else {
            dpi as f32 / 96.0
        }
    };
    // Якорная точка кнопки (низ; по X — нужный край) в ЛОКАЛЬНЫХ физ. px → ClientToScreen → экран.
    let anchor_x_local = match grow_x {
        PopupGrowX::Left => btn_local.origin.x + btn_local.size.width, // правый край кнопки
        PopupGrowX::Right => btn_local.origin.x,                       // левый край кнопки
    };
    let anchor_y_local = btn_local.origin.y + btn_local.size.height; // низ кнопки
    let mut pt = POINT {
        x: (f32::from(anchor_x_local) * sf).round() as i32,
        y: (f32::from(anchor_y_local) * sf).round() as i32,
    };
    if unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt) }
        .as_bool()
        .eq(&false)
    {
        return fallback();
    }
    let monitor: HMONITOR = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_err() {
        return fallback();
    }
    let scale = (dpi_x as f32 / 96.0).max(0.1);
    let ww = f32::from(win_size.width) * scale; // физ. размер окна на этом мониторе
    let wh = f32::from(win_size.height) * scale;
    // По X — сторона роста относительно края кнопки; по Y — ниже низа кнопки на drop_below.
    let mut x = match grow_x {
        PopupGrowX::Left => pt.x as f32 - ww, // правый край попапа = правый край кнопки
        PopupGrowX::Right => pt.x as f32,     // левый край попапа = левый край кнопки
    };
    let mut y = pt.y as f32 + drop_below * scale;
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut mi) }.as_bool() {
        let r = mi.rcWork; // рабочая область монитора (физ. px) — кламп, чтобы не уехать за край
        x = x.clamp(r.left as f32, (r.right as f32 - ww).max(r.left as f32));
        y = y.clamp(r.top as f32, (r.bottom as f32 - wh).max(r.top as f32));
    }
    // origin в логич. px монитора (open умножит обратно на его scale); display_id = HMONITOR.
    let origin = point(px(x / scale), px(y / scale));
    let display_id = DisplayId::new(monitor.0 as usize as u64);
    let phys = Some((
        x.round() as i32,
        y.round() as i32,
        ww.round() as i32,
        wh.round() as i32,
    ));
    (origin, Some(display_id), phys)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn popup_placement(
    window: &Window,
    cx: &App,
    win_size: Size<Pixels>,
    btn_local: Bounds<Pixels>,
    grow_x: PopupGrowX,
    drop_below: f32,
) -> (Point<Pixels>, Option<DisplayId>, PopupPhysRect) {
    let win = window.window_bounds().get_bounds();
    let right_edge = match grow_x {
        PopupGrowX::Left => btn_local.origin.x + btn_local.size.width,
        PopupGrowX::Right => btn_local.origin.x + win_size.width,
    };
    let x = win.origin.x + right_edge - win_size.width;
    let y = win.origin.y + btn_local.origin.y + btn_local.size.height + px(drop_below);
    let origin = point(x, y);
    let display_id = cx
        .displays()
        .into_iter()
        .find(|d| d.bounds().contains(&origin))
        .map(|d| d.id());
    (origin, display_id, None)
}

/// Принудительно поставить окно в АБСОЛЮТНЫЕ физ. screen-координаты (минуя workspace-координаты
/// `SetWindowPlacement`, которые на смещённых мониторах кладут мимо). Windows-only; иначе no-op.
#[cfg(target_os = "windows")]
pub(crate) fn move_window_to_physical(window: &Window, rect: (i32, i32, i32, i32)) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
    };
    let Some(hwnd_raw) = window_hwnd(window) else {
        return;
    };
    let (x, y, w, h) = rect;
    let _ = unsafe {
        SetWindowPos(
            HWND(hwnd_raw as *mut _),
            None,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
    };
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn move_window_to_physical(_window: &Window, _rect: (i32, i32, i32, i32)) {}

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

fn rgb_to_rgba(rgb_hex: u32) -> Rgba {
    rgba((rgb_hex << 8) | 0xFF)
}

pub(crate) fn configure_shell_clear_color(window: &Window, cx: &App) {
    window.set_clear_color(Some(rgb_to_rgba(moon_ui::MoonPalette::active(cx).shell)));
}

pub(crate) fn configure_chart_clear_color(window: &Window, cx: &App) {
    window.set_clear_color(Some(rgb_to_rgba(moon_ui::MoonPalette::active(cx).chart_bg)));
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

/// Безрамочное окно-поповер (например, настройки раскладки чарта). Кроссплатформенно через
/// `WindowKind::PopUp`: каждый бэкенд gpui сам делает его без рамки и без кнопки в таскбаре и
/// поверх остальных окон. Без titlebar, неперемещаемое и неизменяемого размера; закрытие — по
/// потере фокуса (см. `chart_tabs::layout_popup_window`). Фон окна задаёт `window_clear_color`
/// (выставляет вызывающий под активную тему). НЕ зовём Win32-специфику — только gpui-опции.
pub(crate) fn popup_window_options(
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(window_bounds),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        app_id: Some(APP_ID.to_string()),
        // Полупрозрачное окно: фон даёт сам контент (rgba с alpha), а незакрытое — прозрачно.
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    }
}

pub(crate) fn tool_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    min_size: Option<Size<Pixels>>,
    owner: Option<AnyWindowHandle>,
) -> WindowOptions {
    owned_window_options(title, window_bounds, None, min_size, owner, true)
}

/// Открепленная non-chart панель (`Orders`, `Assets`, `Log`, `Report`).
///
/// Это owned/tool окно, когда есть владелец: оно не получает отдельную taskbar-кнопку и
/// живёт вместе с окном группы. При restore owner может отсутствовать; тогда окно
/// становится independent, что лучше, чем потерять восстановленную панель.
pub(crate) fn detached_panel_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    owner: Option<AnyWindowHandle>,
) -> WindowOptions {
    owned_window_options(title, window_bounds, display_id, None, owner, true)
}

/// Открепленное chart-окно — **independent** (НЕ owned, НЕ tool-window).
///
/// Только обычное independent-окно видит PowerToys FancyZones и снапит по зонам: tool-окна
/// (`WS_EX_TOOLWINDOW`) и owned-окна FancyZones игнорирует (нет присутствия в таскбаре).
/// Поэтому кнопку из таскбара убираем НЕ стилем окна, а `ITaskbarList::DeleteTab` после показа
/// (см. `hide_window_from_taskbar`) — стиль не меняется → FancyZones продолжает работать.
/// `taskbar Hidden` → без `WS_EX_APPWINDOW`, чтобы DeleteTab держался (APPWINDOW делает кнопку
/// «липкой»). Independent (в отличие от owned) не поднимает окно группы при клике — это плюс.
pub(crate) fn detached_chart_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
) -> WindowOptions {
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

/// Геометрия окна в логич. px `(x, y, w, h)` — `None`, если окно НЕ в обычном (Windowed)
/// состоянии (свёрнуто/во весь экран). Единая точка приведения f32→i32/u32 для персиста
/// откреп-окон: одинаковая выборка жила в `detached.rs` и `chart_tabs::windows`.
pub(crate) fn window_geom(window: &Window) -> Option<(i32, i32, u32, u32)> {
    let WindowBounds::Windowed(b) = window.window_bounds() else {
        return None;
    };
    Some((
        f32::from(b.origin.x) as i32,
        f32::from(b.origin.y) as i32,
        f32::from(b.size.width) as u32,
        f32::from(b.size.height) as u32,
    ))
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

/// Убрать кнопку окна из таскбара через `ITaskbarList::DeleteTab` — БЕЗ смены стиля окна.
/// Для откреп-чартов: они остаются обычными independent-окнами → PowerToys FancyZones их видит
/// и снапит по зонам, но кнопки в таскбаре нет. (`WS_EX_TOOLWINDOW` дал бы «нет кнопки», но
/// FancyZones игнорирует tool-окна; owned — тоже игнорирует. Поэтому именно DeleteTab.)
/// Вызывать после показа окна (кнопка уже создана); идемпотентно.
#[cfg(target_os = "windows")]
pub(crate) fn hide_window_from_taskbar(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    unsafe {
        let taskbar: ITaskbarList = match CoCreateInstance(&TaskbarList, None, CLSCTX_ALL) {
            Ok(t) => t,
            Err(_) => return,
        };
        if taskbar.HrInit().is_err() {
            return;
        }
        let _ = taskbar.DeleteTab(hwnd);
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
