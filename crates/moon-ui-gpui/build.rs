fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons/0.png");

    #[cfg(windows)]
    if let Err(err) = embed_exe_icon() {
        println!("cargo:warning=failed to embed MoonTerminal exe icon: {err}");
    }
}

#[cfg(windows)]
fn embed_exe_icon() -> std::io::Result<()> {
    use std::fs::File;
    use std::path::Path;

    let png = File::open("../../assets/icons/0.png")?;
    let image = ico::IconImage::read_png(png)?;

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    dir.add_entry(ico::IconDirEntry::encode(&image)?);

    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("moon-terminal.ico");
    dir.write(File::create(&out)?)?;

    let mut res = winresource::WindowsResource::new();
    res.set_icon(out.to_str().expect("icon path must be valid UTF-8"));
    res.compile()
}
