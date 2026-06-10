//! Раздельные кнопки/иконки в taskbar Windows через AppUserModelID на окно.
//! На не-Windows — no-op (и зависимость `windows` туда не тянется).

#[cfg(windows)]
pub fn set_app_id(window: &winit::window::Window, id: &str) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::core::{GUID, PROPVARIANT};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::PropertiesSystem::{
        IPropertyStore, SHGetPropertyStoreForWindow, PROPERTYKEY,
    };

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(w) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(w.hwnd.get() as *mut core::ffi::c_void);

    // PKEY_AppUserModel_ID = {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3} / pid 5.
    const PKEY_APPUSERMODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3),
        pid: 5,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Ok(store) = SHGetPropertyStoreForWindow::<_, IPropertyStore>(hwnd) {
            let pv = PROPVARIANT::from(id);
            let _ = store.SetValue(&PKEY_APPUSERMODEL_ID, &pv);
            let _ = store.Commit();
        }
    }
}

#[cfg(not(windows))]
pub fn set_app_id(_window: &winit::window::Window, _id: &str) {}

/// HWND окна как isize (для owner-window дочерних чарт-окон). None — не Windows
/// или хендл недоступен.
#[cfg(windows)]
pub fn hwnd_of(window: &winit::window::Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}
#[cfg(not(windows))]
pub fn hwnd_of(_window: &winit::window::Window) -> Option<isize> {
    None
}
