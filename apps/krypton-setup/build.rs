// Embeds the release payload (a zip of the binaries) and the application icon.
// The payload path comes from KRYPTON_SETUP_PAYLOAD; without it an empty payload is embedded
// (development builds).
fn main() {
    println!("cargo:rerun-if-env-changed=KRYPTON_SETUP_PAYLOAD");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("payload.zip");
    match std::env::var("KRYPTON_SETUP_PAYLOAD") {
        Ok(p) => {
            println!("cargo:rerun-if-changed={p}");
            std::fs::copy(&p, &out).expect("copy payload");
        }
        Err(_) => std::fs::write(&out, []).unwrap(),
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/genoryn.ico")
            .set("FileDescription", "Genoryn Krypton Setup")
            .set("ProductName", "Genoryn Krypton")
            .set("CompanyName", "Genoryn Krypton contributors")
            .set("LegalCopyright", "MIT OR Apache-2.0");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded: {e}");
        }
    }
}
