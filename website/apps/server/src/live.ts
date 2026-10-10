import type { FastifyReply } from 'fastify';
import type { LiveEvent } from '@genoryn/shared';

/** Server-Sent Events fan-out with a hard client cap and heartbeat. */
export class LiveHub {
  private clients = new Set<FastifyReply>();
  private last = new Map<string, LiveEvent>();
  constructor(private readonly maxClients = 2000) {
    setInterval(() => this.write(': ping\n\n'), 20_000).unref();
  }

  get size() {
    return this.clients.size;
  }

  attach(reply: FastifyReply): boolean {
    if (this.clients.size >= this.maxClients) return false;
    reply.raw.writeHead(200, {
      'content-type': 'text/event-stream; charset=utf-8',
      'cache-control': 'no-cache, no-transform',
      connection: 'keep-alive',
      'x-accel-buffering': 'no',
    });
    reply.raw.write('retry: 5000\n\n');
    for (const ev of this.last.values()) reply.raw.write(frame(ev));
    this.clients.add(reply);
    reply.raw.on('close', () => this.clients.delete(reply));
    return true;
  }

  publish(ev: LiveEvent) {
    if (ev.type !== 'block' && ev.type !== 'reorg') this.last.set(ev.type, ev);
    this.write(frame(ev));
  }

  private write(chunk: string) {
    for (const c of this.clients) {
      try {
        c.raw.write(chunk);
      } catch {
        this.clients.delete(c);
      }
    }
  }
}

const frame = (ev: LiveEvent) => `event: ${ev.type}\ndata: ${JSON.stringify(ev.data)}\n\n`;
