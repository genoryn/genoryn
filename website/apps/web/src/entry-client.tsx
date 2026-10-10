import '@fontsource/unbounded/500.css';
import '@fontsource/unbounded/700.css';
import '@fontsource/unbounded/800.css';
import '@fontsource-variable/inter';
import '@fontsource/jetbrains-mono/400.css';
import '@fontsource/jetbrains-mono/600.css';
import '@fontsource/instrument-serif/400.css';
import '@fontsource/instrument-serif/400-italic.css';
import './styles/tokens.css';
import './styles/base.css';
import './styles/components.css';
import './styles/pages.css';
import { render } from 'solid-js/web';
import App from './App';

const root = document.getElementById('app')!;
// The prerendered markup exists for crawlers and first paint; the app re-renders it in the same task.
root.textContent = '';
render(() => <App />, root);
