import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const client = join(root, 'dist/client');
const { render, ROUTES } = await import(pathToFileURL(join(root, 'dist/server/entry-server.js')).href);
const template = readFileSync(join(client, 'index.html'), 'utf8');

if (!template.includes('<!--app-html-->') || !template.includes('<!--app-head-->')) {
  throw new Error('index.html template placeholders missing');
}

const write = (path, html) => {
  const file = path === '/' ? join(client, 'index.html') : join(client, path.slice(1), 'index.html');
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, html);
};

write('/_shell', template.replace('<!--app-html-->', ''));

let count = 0;
for (const r of ROUTES) {
  const html = render(r.path);
  write(r.path, template.replace('<!--app-html-->', html));
  count++;
}
rmSync(join(root, 'dist/server'), { recursive: true, force: true });
console.log(`prerendered ${count} routes + shell`);
