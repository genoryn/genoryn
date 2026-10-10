//! Documentation screenshots: `--capture-pages DIR --password-stdin` unlocks the
//! wallet, visits every page, saves each frame as rendered by the wallet itself to
//! `DIR/wallet-<page>.png` and exits. Nothing is sent to the desktop's input, so it
//! runs beside other work.
use super::*;
use std::{io::BufWriter, path::Path};

const SETTLE: Duration = Duration::from_millis(2_600);
const MINING_WAIT: Duration = Duration::from_secs(150);

pub(super) struct Capture {
    dir: PathBuf,
    step: usize,
    since: Instant,
    awaiting_image: bool,
    started_mining: bool,
}
impl Capture {
    pub fn new(dir: &str) -> Self {
        Self {
            dir: PathBuf::from(dir),
            step: 0,
            since: Instant::now(),
            awaiting_image: false,
            started_mining: false,
        }
    }
}

/// `(page, light, file stem)` in capture order.
fn steps(lite: bool) -> Vec<(usize, bool, &'static str)> {
    if lite {
        return vec![(0, false, "wallet-lite")];
    }
    vec![
        (0, false, "wallet-overview"),
        (WALLETS_PAGE, false, "wallet-wallets"),
        (1, false, "wallet-send"),
        (2, false, "wallet-receive"),
        (3, false, "wallet-transactions"),
        (MINING_PAGE, false, "wallet-mining"),
        (5, false, "wallet-node"),
        (6, false, "wallet-network"),
        (7, false, "wallet-settings"),
        (0, true, "wallet-light"),
    ]
}

fn save_png(path: &Path, image: &egui::ColorImage) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let [width, height] = image.size;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // The window is opaque; the framebuffer's alpha channel is not, so drop it.
    let bytes: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|pixel| {
            let [red, green, blue, _] = pixel.to_array();
            [red, green, blue, 255]
        })
        .collect();
    encoder.write_header()?.write_image_data(&bytes)?;
    Ok(())
}

impl WalletApp {
    pub(super) fn capture_tick(&mut self, ctx: &egui::Context) {
        let Some(mut capture) = self.capture.take() else {
            return;
        };
        ctx.request_repaint();
        let plan = steps(self.settings.lite);
        let image = ctx.input(|input| {
            input.raw.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let (Some(image), true) = (image, capture.awaiting_image) {
            let (_, _, stem) = plan[capture.step];
            let path = capture.dir.join(format!("{stem}.png"));
            if let Err(error) = save_png(&path, &image) {
                eprintln!("could not save {}: {error:#}", path.display());
            }
            capture.step += 1;
            capture.awaiting_image = false;
            capture.since = Instant::now();
        }
        if capture.step >= plan.len() {
            if self.miner.is_some() {
                self.stop_mining();
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if !self.unlocked || self.wallet_height.is_none() {
            capture.since = Instant::now();
            self.capture = Some(capture);
            return;
        }
        let (page, light, _) = plan[capture.step];
        if self.page != page {
            self.page = page;
            capture.since = Instant::now();
        }
        if self.light != light {
            self.light = light;
            set_style(ctx, light);
            capture.since = Instant::now();
        }
        if page == MINING_PAGE && !capture.started_mining && self.miner.is_none() {
            capture.started_mining = true;
            self.start_mining();
        }
        let mining_ready = page != MINING_PAGE
            || self.mining_stats.is_some()
            || capture.since.elapsed() > MINING_WAIT;
        // Remembered wallets finish unlocking before anything is shown.
        if self.pending_password.is_some() {
            capture.since = Instant::now();
        }
        if !capture.awaiting_image && capture.since.elapsed() > SETTLE && mining_ready {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            capture.awaiting_image = true;
        }
        self.capture = Some(capture);
    }
}
