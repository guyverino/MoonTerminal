//! Иконки из assets/icons/{id}.png (извлечены из mb_ico.dll). Кросс-платформенно:
//! image → egui-текстура (UI) и winit::Icon (taskbar/окно). DLL в рантайме не нужна.

use std::collections::HashMap;
use std::path::PathBuf;

/// Каталог иконок: рядом с cwd или с exe.
pub fn icons_dir() -> PathBuf {
    let rel = PathBuf::from("assets/icons");
    if rel.is_dir() {
        return rel;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("assets/icons");
            if p.is_dir() {
                return p;
            }
        }
    }
    rel
}

fn load_png(id: u32) -> Option<(Vec<u8>, u32, u32)> {
    let path = icons_dir().join(format!("{id}.png"));
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some((img.into_raw(), w, h))
}

/// Иконка окна (taskbar / заголовок).
pub fn winit_icon(id: u32) -> Option<winit::window::Icon> {
    let (rgba, w, h) = load_png(id)?;
    winit::window::Icon::from_rgba(rgba, w, h).ok()
}

/// Кэш egui-текстур иконок (per egui Context — у каждого окна свой набор).
pub struct IconSet {
    pub count: u32,
    cache: HashMap<u32, Option<egui::TextureHandle>>,
}

impl IconSet {
    pub fn discover() -> Self {
        let count = std::fs::read_dir(icons_dir())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
                    .count() as u32
            })
            .unwrap_or(0);
        Self {
            count,
            cache: HashMap::new(),
        }
    }

    pub fn texture(&mut self, ctx: &egui::Context, id: u32) -> Option<egui::TextureHandle> {
        if let Some(c) = self.cache.get(&id) {
            return c.clone();
        }
        let tex = load_png(id).map(|(rgba, w, h)| {
            let img =
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            ctx.load_texture(format!("ico-{id}"), img, egui::TextureOptions::LINEAR)
        });
        self.cache.insert(id, tex.clone());
        tex
    }
}

impl Default for IconSet {
    fn default() -> Self {
        Self::discover()
    }
}
