import { spawn, type ChildProcess } from 'node:child_process';
import type { MinerStats } from '@genoryn/shared';
import type { LiveHub } from './live';

/** A remote miner's journal, reached with a key whose only allowed command streams it. */
export interface RemoteJournal {
  target: string;
  key: string;
  knownHosts: string;
}

/**
 * Follows the miner's JSON status lines from the systemd journal, locally or of a
 * remote host. Only real events are reported; when nothing arrives the miner is
 * shown as offline/unavailable.
 */
export class MinerWatcher {
  stats: MinerStats;
  private child: ChildProcess | null = null;

  constructor(private unit: string, address: string | null, private hub: LiveHub, private remote: RemoteJournal | null = null) {
    this.stats = { online: false, threads: null, hashrate: null, hashes: null, accepted: null, rejected: null, height: null, last_event_at: null, address, raw: null };
  }

  private command(): [string, string[]] {
    if (this.remote) {
      return ['ssh', ['-i', this.remote.key, '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=yes', '-o', `UserKnownHostsFile=${this.remote.knownHosts}`, '-o', 'ServerAliveInterval=20', '-T', this.remote.target]];
    }
    return ['journalctl', ['-u', this.unit, '-f', '-n', '20', '-o', 'cat', '--no-pager']];
  }

  start() {
    const run = () => {
      const [program, args] = this.command();
      const child = spawn(program, args, { stdio: ['ignore', 'pipe', 'ignore'] });
      this.child = child;
      let buf = '';
      child.stdout?.on('data', (d: Buffer) => {
        buf += d.toString('utf8');
        let nl: number;
        while ((nl = buf.indexOf('\n')) >= 0) {
          this.line(buf.slice(0, nl));
          buf = buf.slice(nl + 1);
        }
        if (buf.length > 65536) buf = '';
      });
      child.on('error', () => undefined);
      child.on('exit', () => setTimeout(run, 10_000).unref());
    };
    run();
    setInterval(() => {
      const stale = !this.stats.last_event_at || Date.now() / 1000 - this.stats.last_event_at > 600;
      if (stale && this.stats.online) {
        this.stats = { ...this.stats, online: false };
        this.hub.publish({ type: 'miner', data: this.stats });
      }
    }, 30_000).unref();
  }

  private line(text: string) {
    if (!text.startsWith('{')) return;
    try {
      const ev = JSON.parse(text) as Record<string, unknown>;
      const num = (k: string) => (typeof ev[k] === 'number' ? (ev[k] as number) : null);
      this.stats = {
        online: true,
        threads: num('threads'),
        hashrate: num('hashrate_hs'),
        hashes: num('hashes'),
        accepted: num('accepted_blocks'),
        rejected: num('rejected_blocks'),
        height: num('height'),
        last_event_at: Math.floor(Date.now() / 1000),
        address: this.stats.address,
        raw: ev,
      };
      this.hub.publish({ type: 'miner', data: this.stats });
    } catch {
      /* not a status line */
    }
  }

  stop() {
    this.child?.kill();
  }
}
