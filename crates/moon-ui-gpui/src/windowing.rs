use gpui::*;

use crate::design;

pub(crate) const APP_ID: &str = "MoonTerminal";

fn app_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    min_size: Option<Size<Pixels>>,
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
        app_id: Some(APP_ID.to_string()),
        window_min_size: min_size,
        window_decorations: design::platform_window_decorations(),
        ..Default::default()
    }
}

pub(crate) fn trading_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    min_size: Option<Size<Pixels>>,
) -> WindowOptions {
    app_window_options(title, window_bounds, display_id, min_size, true)
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
    owner: Option<AnyWindowHandle>,
) -> WindowOptions {
    owned_window_options(title, window_bounds, display_id, None, owner, true)
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
    let mut options = app_window_options(
        title,
        window_bounds,
        display_id,
        min_size,
        transparent_titlebar,
    );
    options.kind = WindowKind::Floating;
    options.relationship = owner.map(WindowRelationship::owned).unwrap_or_default();
    options
}
