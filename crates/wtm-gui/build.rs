//! Embeds the RustWTM icon and version information in the executable.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/rustwtm.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/rustwtm.ico")
        .set("ProductName", "RustWTM")
        .set("FileDescription", "RustWTM task manager")
        .set("CompanyName", "Ersin KOÇ")
        .set("LegalCopyright", "© Ersin KOÇ. MIT license.");
    res.compile().expect("embedding the Windows resources");
}
