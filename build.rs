//! Сборочный скрипт: вшивает иконку самого exe из ОДНОГО источника —
//! `assets/icons/0.png` (тот же файл, что и иконки окон). Заменил 0.png и
//! пересобрал → иконка обновилась везде: окна, таскбар и файл exe в Explorer.
//! Только Windows; на macOS/Linux — пустой no-op (ресурс-иконок там нет).

fn main() {
    // Замена брендовой иконки → пересборка (и build.rs, и include_bytes! в icons.rs).
    println!("cargo:rerun-if-changed=assets/icons/0.png");

    // Встраивание не должно ронять сборку: если нет rc/SDK — предупреждаем и идём дальше.
    #[cfg(windows)]
    if let Err(e) = embed_exe_icon() {
        println!("cargo:warning=иконку exe вшить не удалось: {e}");
    }
}

/// PNG → .ico в OUT_DIR → ресурс exe (icon group). Источник — assets/icons/0.png.
#[cfg(windows)]
fn embed_exe_icon() -> std::io::Result<()> {
    use std::fs::File;
    use std::path::Path;

    let png = File::open("assets/icons/0.png")?;
    let image = ico::IconImage::read_png(png)?;

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    dir.add_entry(ico::IconDirEntry::encode(&image)?);

    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("app.ico");
    dir.write(File::create(&out)?)?;

    let mut res = winresource::WindowsResource::new();
    res.set_icon(out.to_str().expect("путь app.ico"));
    res.compile()
}
