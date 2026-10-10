import { createMemo, For, Show } from 'solid-js';
import { A } from '@solidjs/router';
import { LATEST_RELEASE_URL, REPO_URL, formatBytes, type ReleaseAsset } from '@genoryn/shared';
import { Icon } from '~/components/Icon';
import { GSign } from '~/components/Logo';
import { Callout, CodeBlock, CopyButton, ExtLink, PageHero, SectionHead } from '~/components/ui';
import { detectOs } from '~/lib/dom';
import { useReleases } from '~/lib/releases';
import { createSignal, onMount } from 'solid-js';

const isLite = (a: ReleaseAsset) => /lite/i.test(a.name);
/** Releases before 1.0.0 belong to the retired pre-1.0 network and are never offered here. */
const isMainnet10 = (tag: string) => Number(/^v?(\d+)\./.exec(tag)?.[1] ?? 0) >= 1;
const FALLBACK_VERSION = '1.0.0';
const RELEASE_FILES = [
  ['GENORYN-Setup-1.0.0.exe', 'Windows installer'],
  ['GENORYN-Lite-Setup-1.0.0.exe', 'Windows installer · Lite (no miner)'],
  ['genoryn-1.0.0-mainnet-windows-amd64.zip', 'Windows portable package'],
  ['genoryn-1.0.0-mainnet-lite-windows-amd64.zip', 'Windows portable package · Lite'],
  ['genoryn-1.0.0-mainnet-linux-x86_64.zip', 'Linux package'],
  ['genoryn-1.0.0-mainnet-lite-linux-x86_64.zip', 'Linux package · Lite'],
] as const;
const releaseFileUrl = (name: string) => `${REPO_URL}/releases/download/v${FALLBACK_VERSION}/${name}`;
const OS_LABEL: Record<string, string> = { windows: 'Windows', linux: 'Linux', macos: 'macOS', other: 'Other' };
const OS_SHORT: Record<string, string> = { windows: 'WIN', linux: 'LNX', macos: 'MAC', other: 'PKG' };

function Asset(props: { a: ReleaseAsset; recommended: boolean }) {
  const installer = () => props.a.name.toLowerCase().endsWith('.exe');
  return (
    <div class={`asset ${props.recommended ? 'recommended' : ''}`}>
      <div class="os">{installer() ? 'EXE' : OS_SHORT[props.a.platform]}</div>
      <div style={{ 'min-width': 0 }}>
        <div class="row" style={{ gap: '8px' }}>
          <b class="strong mono" style={{ 'font-size': '14px' }}>{props.a.name}</b>
          <Show when={props.recommended}>
            <span class="badge solid">for your system</span>
          </Show>
        </div>
        <div class="muted" style={{ 'font-size': '13px' }}>
          {isLite(props.a) ? 'GENORYN Lite · ' : 'GENORYN · '}
          {installer() ? 'Windows installer' : props.a.name.endsWith('.zip') ? `${OS_LABEL[props.a.platform]} portable package` : OS_LABEL[props.a.platform]} · {formatBytes(props.a.size)} · {props.a.downloads.toLocaleString('en-US')} downloads
        </div>
        <Show when={props.a.sha256}>
          <div class="sha row" style={{ gap: '6px' }}>
            SHA-256 {props.a.sha256} <CopyButton text={props.a.sha256!} label="Copy checksum" />
          </div>
        </Show>
      </div>
      <a class={`btn ${props.recommended ? 'primary' : ''}`} href={props.a.url} rel="noopener">
        <Icon name="download" size={16} /> Download
      </a>
    </div>
  );
}

export default function Download() {
  const [data] = useReleases();
  const [os, setOs] = createSignal<string>('other');
  onMount(() => setOs(detectOs()));
  const latest = createMemo(() => {
    const current = (data()?.releases ?? []).filter((r) => isMainnet10(r.tag));
    return current.find((r) => !r.prerelease) ?? current[0];
  });
  const sorted = createMemo(() => {
    const assets = latest()?.assets ?? [];
    const rank = (a: ReleaseAsset) => (a.platform === os() ? 10 : 0) + (a.name.toLowerCase().endsWith('.exe') ? 2 : 0) + (isLite(a) ? 0 : 1);
    return [...assets].sort((a, b) => rank(b) - rank(a));
  });
  const primary = () => sorted().find((a) => a.platform === os() && !isLite(a)) ?? sorted().find((a) => !isLite(a)) ?? sorted()[0];
  const lite = () => sorted().find((a) => a.platform === os() && isLite(a));
  const ver = () => latest()?.tag?.replace(/^v/, '') ?? FALLBACK_VERSION;

  return (
    <>
      <PageHero eyebrow="download.genoryn.org" title={<>Download GENORYN</>} lead="GENORYN 1.0.0 for Mainnet 1.0: one wallet with Quick mode and Full mode, the full node, CLI tools and a CPU miner. Windows installer or portable packages — open source, with SHA-256 checksums on every file." origin="Always the latest 1.x GitHub release, fetched automatically" />
      <section class="section tight">
        <div class="container">
          <div class="dl-hero-card">
            <div>
              <div class="row">
                <GSign size={44} />
                <div>
                  <div class="mono muted" style={{ 'font-size': '12px', 'letter-spacing': '0.1em' }}>LATEST RELEASE</div>
                  <div class="strong" style={{ 'font-family': 'var(--font-display)', 'font-size': '30px', 'font-weight': 700, 'letter-spacing': '-0.03em' }}>
                    <Show when={latest()} fallback={`GENORYN ${FALLBACK_VERSION}`}>{latest()!.name}</Show>
                  </div>
                </div>
              </div>
              <p class="muted mt-2">
                <Show when={latest()} fallback={data.loading ? 'Loading the latest release from GitHub…' : <>Tag <code>v{FALLBACK_VERSION}</code> · Mainnet 1.0 release</>}>
                  Published {new Date(latest()!.published_at).toUTCString().slice(5, 16)} · tag <code>{latest()!.tag}</code> · Mainnet 1.0 release
                </Show>
              </p>
              <div class="row mt-3">
                <Show
                  when={primary()}
                  fallback={
                    <ExtLink href={LATEST_RELEASE_URL} class="btn primary lg">
                      <Icon name="download" size={18} /> Latest release on GitHub
                    </ExtLink>
                  }
                >
                  <a class="btn primary lg" href={primary()!.url} rel="noopener">
                    <Icon name="download" size={18} /> Download for {OS_LABEL[primary()!.platform]}
                  </a>
                </Show>
                <Show when={lite()}>
                  <a class="btn lg" href={lite()!.url} rel="noopener">
                    <Icon name="download" size={18} /> Lite (no miner)
                  </a>
                </Show>
                <Show when={latest()}>
                  <ExtLink href={latest()!.url} class="btn ghost lg">
                    Release notes <Icon name="arrowUpRight" size={16} />
                  </ExtLink>
                </Show>
              </div>
              <Show when={os() === 'macos'}>
                <p class="hint mt-2">macOS packages are not published yet. Build from source below, or use a Linux or Windows machine.</p>
              </Show>
              <Show when={data()?.error}>
                <p class="error-text mt-2">GitHub is unreachable right now ({data()!.error}). Use the link above.</p>
              </Show>
            </div>
            <div>
              <h3 style={{ 'font-size': '17px' }}>What's inside</h3>
              <ul class="muted" style={{ 'padding-left': '18px', 'font-size': '14.5px', 'line-height': 1.9, margin: 0 }}>
                <li><b class="strong">GENORYN</b> or <b class="strong">GENORYN Lite</b> (no miner) — Windows installers, no admin rights</li>
                <li><b class="strong">Built-in Tor</b> — new installs start in Tor-only mode</li>
                <li><b class="strong">genoryn-wallet</b> — native desktop wallet, Quick mode or Full mode</li>
                <li><b class="strong">genorynd</b> — full node daemon</li>
                <li><b class="strong">genoryn-miner</b> — RandomX CPU miner</li>
                <li><b class="strong">genoryn-wallet-cli</b> · <b class="strong">genoryn-cli</b></li>
                <li><b class="strong">genoryn-emission</b> — audit the supply schedule</li>
                <li>Licenses, SBOM, manifest with source commit</li>
              </ul>
              <Show when={data()?.repo}>
                <div class="row mt-3" style={{ gap: '8px' }}>
                  <span class="badge">★ {data()!.repo!.stars ?? 0} stars</span>
                  <span class="badge">{data()!.repo!.forks ?? 0} forks</span>
                  <span class="badge">{data()!.repo!.license ?? 'MIT / Apache-2.0'}</span>
                </div>
              </Show>
            </div>
          </div>

          <div class="mt-4">
            <div class="panel-title">
              <h2>All packages</h2>
              <ExtLink href={`${REPO_URL}/releases`} class="btn sm ghost">
                All releases <Icon name="arrowUpRight" size={14} />
              </ExtLink>
            </div>
            <Show
              when={sorted().length}
              fallback={
                <Show when={!data.loading} fallback={<div class="empty">Loading packages from GitHub… or open the <ExtLink href={LATEST_RELEASE_URL} class="strong">latest release</ExtLink> directly.</div>}>
                  <div class="table-wrap">
                    <table class="data">
                      <thead><tr><th>Release {FALLBACK_VERSION} file</th><th>Package</th><th>Checksum</th></tr></thead>
                      <tbody>
                        <For each={RELEASE_FILES}>
                          {([name, label]) => (
                            <tr>
                              <td><ExtLink href={releaseFileUrl(name)} class="link mono">{name}</ExtLink></td>
                              <td class="muted">{label}</td>
                              <td><ExtLink href={releaseFileUrl(`${name}.sha256`)} class="link mono">.sha256</ExtLink></td>
                            </tr>
                          )}
                        </For>
                      </tbody>
                    </table>
                  </div>
                </Show>
              }
            >
              <For each={sorted()}>{(a) => <Asset a={a} recommended={a === primary() && a.platform === os()} />}</For>
            </Show>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Two editions" title={<>GENORYN or GENORYN Lite.</>} lead="Both are the same wallet with Quick mode and Full mode. Lite simply has no miner program and no Mining page. They share your wallets and can be installed side by side; each updates only to itself." />
          <div class="table-wrap">
            <table class="data">
              <thead><tr><th /><th>GENORYN</th><th>GENORYN Lite</th></tr></thead>
              <tbody>
                <tr><td class="strong">Wallet: send, receive, @usernames, payment proofs</td><td>✓</td><td>✓</td></tr>
                <tr><td class="strong">Full mode: own full node that validates everything</td><td>✓</td><td>✓</td></tr>
                <tr><td class="strong">Quick mode: no local node, every block validated locally over Tor</td><td>✓</td><td>✓</td></tr>
                <tr><td class="strong">Built-in Tor</td><td>✓</td><td>✓</td></tr>
                <tr><td class="strong">CPU mining</td><td>✓</td><td class="muted">—</td></tr>
              </tbody>
            </table>
          </div>
          <Callout class="mt-3">
            <b>Tor from the first start.</b> New installations start in Tor-only mode with the built-in Tor client; the first start downloads about 10 MB of Tor directory data. <A href="/privacy" class="strong">How Tor works in GENORYN →</A>
          </Callout>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Verify before you run" title={<>Check the checksum.</>} lead="Every package has a published SHA-256 checksum. If the value you compute does not match the one shown above and on GitHub, do not run the file." />
          <div class="grid cols-2">
            <CodeBlock
              title="Windows · PowerShell"
              code={`# In the folder where you downloaded the installer
$ Get-FileHash .\\GENORYN-Setup-${ver()}.exe -Algorithm SHA256
# Compare the Hash column with the published SHA-256
$ Get-Content .\\GENORYN-Setup-${ver()}.exe.sha256`}
            />
            <CodeBlock
              title="Linux · terminal"
              code={`$ sha256sum -c genoryn-${ver()}-mainnet-linux-x86_64.zip.sha256
# Expected output: ... OK
$ unzip genoryn-${ver()}-mainnet-linux-x86_64.zip && cd genoryn-${ver()}-mainnet-linux-x86_64
$ sha256sum -c SHA256SUMS --quiet && ./genoryn-wallet`}
            />
          </div>
          <div class="steps mt-4">
            <div class="step reveal">
              <h3>Download and verify</h3>
              <p>Get the package for your system and confirm its SHA-256 checksum.</p>
            </div>
            <div class="step reveal">
              <h3>Open the wallet</h3>
              <p>Run the installer on Windows (or genoryn-wallet on Linux) and choose Quick mode (no local node) or Full mode (your own node). Both join GENORYN Mainnet 1.0.</p>
            </div>
            <div class="step reveal">
              <h3>Create a wallet</h3>
              <p>Choose a password of at least ten characters and write the 24-word recovery phrase down offline. Nothing is reachable until the wallet is unlocked.</p>
            </div>
            <div class="step reveal">
              <h3>Mine when you want</h3>
              <p>In Full mode, open the Mining page and press start. Rewards show as Just mined, Maturing, then Mature after 60 blocks; mining keeps running when the wallet locks.</p>
            </div>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <div class="grid cols-2">
            <div>
              <SectionHead eyebrow="Requirements" title={<>Platforms</>} />
              <div class="table-wrap">
                <table class="data">
                  <tbody>
                    <tr><td class="strong">Windows</td><td class="muted">Windows 10/11 x64, Direct3D 12; the installer offers the Microsoft Visual C++ runtime if missing</td></tr>
                    <tr><td class="strong">Linux</td><td class="muted">Ubuntu 24.04 x86-64 (glibc 2.39) with X11/Wayland and OpenGL for the GUI; node and miner run headless</td></tr>
                    <tr><td class="strong">Memory</td><td class="muted">~256 MiB for node verification, plus ~256 MiB per mining thread (up to 16)</td></tr>
                    <tr><td class="strong">Disk</td><td class="muted">About 2 GB for the extracted application, plus chain storage that grows over time</td></tr>
                    <tr><td class="strong">Network</td><td class="muted">TCP 29710 (Mainnet 1.0 P2P). Outbound-only nodes behind NAT also work</td></tr>
                  </tbody>
                </table>
              </div>
            </div>
            <div>
              <SectionHead eyebrow="Build from source" title={<>Trust nothing.</>} />
              <CodeBlock
                code={`# Prerequisites: Git, Rust 1.92.0, a C/C++ compiler and CMake
$ git clone https://github.com/genoryn/genoryn.git
$ cd genoryn
$ cargo build --release --locked --workspace
$ ./target/release/genoryn-wallet`}
              />
              <p class="muted mt-2" style={{ 'font-size': '14px' }}>
                Linux also needs X11/Wayland development libraries; Windows needs Visual Studio Build Tools with the C++ desktop workload.
              </p>
            </div>
          </div>
          <Callout warn class="mt-4">
            <b>Not yet independently audited.</b> Mine or hold only what you can afford to lose. Packages are unsigned builds: SmartScreen may warn before the first run, and if Windows 11 Smart App Control is enforcing, the installer stops, explains why and offers to open its settings. Always check the SHA-256 first. Mainnet 1.0 is a new network: coins of the retired pre-1.0 network (releases 0.4–0.6) and of the devnet do not carry over, and reusing an old recovery phrase does not bring a balance with it. <A href="/security" class="strong">Security details →</A>
          </Callout>
        </div>
      </section>
    </>
  );
}
