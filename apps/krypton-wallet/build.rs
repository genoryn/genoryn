fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/genoryn.ico")
            .set("FileDescription", "Genoryn Krypton Wallet")
            .set("ProductName", "Genoryn Krypton")
            .set("CompanyName", "Genoryn Krypton contributors")
            .set("LegalCopyright", "MIT OR Apache-2.0");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded: {e}");
        }
    }
}
