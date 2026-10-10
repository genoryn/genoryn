import { renderToString } from 'solid-js/web';
import App from './App';

export { ROUTES } from '@genoryn/shared';

export function render(url: string): string {
  return renderToString(() => <App url={url} />);
}
