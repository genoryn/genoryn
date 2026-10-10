fn main() {
    println!("cargo:rerun-if-changed=assets/genoryn.ico");
    // Explorer, the taskbar pin and shortcuts read the icon from the executable.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/genoryn.ico")
            .set("ProductName", "GENORYN")
            .set("FileDescription", "GENORYN Wallet")
            .set("LegalCopyright", "MIT OR Apache-2.0");
        resource
            .compile()
            .expect("compile the Windows icon and version resource");
    }
}
