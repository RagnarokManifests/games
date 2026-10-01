import React from 'react';
import ReactDOM from 'react-dom/client';
import { ErrorBoundary } from './components/ErrorBoundary';
import './index.css';

// The tray icon's popup and the achievement popup are extra windows that load
// this same bundle with a #tray / #ach-toast hash. Each gets its own tiny view
// so they never boot the whole app (and its watchers, listeners and invoke()
// calls) a second time.
const hash = window.location.hash;

const view =
  hash === '#tray'
    ? import('./components/TrayPopup').then((m) => <m.default />)
    : hash === '#ach-toast'
      ? import('./components/AchievementToast').then((m) => <m.default />)
      : import('./App').then((m) => <m.default />);

view.then((el) => {
  ReactDOM.createRoot(document.getElementById('root')!).render(
    <React.StrictMode>
      <ErrorBoundary>{el}</ErrorBoundary>
    </React.StrictMode>
  );
});
