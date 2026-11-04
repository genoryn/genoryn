//! Shared design assets (design/assets) embedded into the wallet binary.

macro_rules! asset {
    ($p:literal) => {
        (
            $p,
            include_bytes!(concat!("../../../design/assets/", $p)).as_slice(),
        )
    };
}

pub const FX_HEAD: &str = include_str!("../../../design/fx-head.html");

const FILES: &[(&str, &[u8])] = &[
    asset!("tokens.css"),
    asset!("fx.css"),
    asset!("fx.js"),
    asset!("charts.js"),
    asset!("theme-init.js"),
    asset!("value.js"),
    asset!("noise.webp"),
    asset!("genoryn-mark-dark.svg"),
    asset!("genoryn-mark-light.svg"),
    asset!("bg/dark-1.webp"),
    asset!("bg/dark-2.webp"),
    asset!("bg/dark-3.webp"),
    asset!("bg/dark-4.webp"),
    asset!("bg/light-1.webp"),
    asset!("bg/light-2.webp"),
    asset!("bg/light-3.webp"),
    asset!("bg/light-4.webp"),
    asset!("fonts/inter-400.woff2"),
    asset!("fonts/inter-600.woff2"),
    asset!("fonts/space-grotesk-600.woff2"),
    asset!("fonts/space-grotesk-700.woff2"),
    asset!("fonts/jetbrains-mono-400.woff2"),
];

pub fn get(path: &str) -> Option<(&'static [u8], &'static str)> {
    let (_, bytes) = FILES.iter().find(|(p, _)| *p == path)?;
    let ct = match path.rsplit('.').next()? {
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    };
    Some((bytes, ct))
}
