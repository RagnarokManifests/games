import { useEffect, useState } from 'react';
import { motion } from 'framer-motion';
import { invoke } from '@tauri-apps/api/tauri';
import { listen } from '@tauri-apps/api/event';
import { Library, Trophy, Settings, LogOut, ArrowUpRight } from 'lucide-react';
import logo from '../assets/logo.png';

// Right-click menu of the tray icon. It lives in its own borderless window
// (label "tray", see show_tray_popup in main.rs) and is rendered instead of
// <App /> when the page is loaded with the #tray hash. Language and accent
// colour are read from the same localStorage keys App.tsx writes.

const readLang = (): 'es' | 'en' => (localStorage.getItem('rl_lang') === 'es' ? 'es' : 'en');

const applyAccent = () => {
  const hex = localStorage.getItem('rl_accent') ?? '#dc2626';
  const m = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  const rgb = m ? `${parseInt(m[1], 16)} ${parseInt(m[2], 16)} ${parseInt(m[3], 16)}` : '220 38 38';
  const html = document.documentElement;
  html.classList.add('dark');
  html.style.setProperty('--accent-color', rgb);
  html.style.setProperty('--accent-color-hex', hex);
};

const container = { hidden: {}, show: { transition: { staggerChildren: 0.04 } } };
const item = {
  hidden: { opacity: 0, y: 6 },
  show: { opacity: 1, y: 0, transition: { duration: 0.2, ease: 'easeOut' as const } },
};

export default function TrayPopup() {
  const [lang, setLang] = useState(readLang);
  // Bumped every time the popup is shown so the entrance animation replays.
  const [shown, setShown] = useState(0);
  // "game-started" / "game-closed" are broadcast to every window, and this
  // one is created at startup and only ever hidden, so it sees them all.
  const [playing, setPlaying] = useState(false);
  const ti = (es: string, en: string) => (lang === 'es' ? es : en);

  useEffect(() => {
    // The window is created once and only hidden afterwards, so it has to
    // pick up a language/accent change made in the main window on each show.
    document.body.style.background = 'transparent';
    document.documentElement.style.background = 'transparent';
    applyAccent();
    const unlisteners = [
      listen('tray-shown', () => {
        applyAccent();
        setLang(readLang());
        setShown((n) => n + 1);
      }),
      listen('game-started', () => setPlaying(true)),
      listen('game-closed', () => setPlaying(false)),
    ];
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') invoke('tray_hide');
    };
    window.addEventListener('keydown', onKey);
    return () => {
      unlisteners.forEach((u) => u.then((f) => f()));
      window.removeEventListener('keydown', onKey);
    };
  }, []);

  const go = (tab: string) => invoke('tray_navigate', { tab });

  const quick = [
    { tab: 'Library', icon: Library, label: ti('Biblioteca', 'Library') },
    { tab: 'Achievements', icon: Trophy, label: ti('Logros', 'Achievements') },
    { tab: 'Settings', icon: Settings, label: ti('Ajustes', 'Settings') },
  ];

  return (
    <div className="h-screen w-screen p-1 select-none overflow-hidden">
      <motion.div
        key={shown}
        variants={container}
        initial="hidden"
        animate="show"
        className="relative h-full rounded-2xl bg-[#0d0e12] border border-white/[0.08] shadow-2xl flex flex-col p-3 gap-2.5 overflow-hidden"
      >
        {/* one soft ambient glow, like the rest of the app */}
        <div className="pointer-events-none absolute -top-16 -right-10 w-44 h-44 rounded-full bg-accent/15 blur-3xl" />

        <motion.div variants={item} className="relative flex items-center gap-3 px-1 pt-0.5">
          <img src={logo} alt="" draggable={false} className="w-10 h-10 rounded-xl object-contain shrink-0" />
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-black tracking-tight text-white/90 truncate">Ragnarok Launcher</div>
            <div className="text-[9px] font-black uppercase tracking-widest text-gray-500">v{__APP_VERSION__}</div>
          </div>
        </motion.div>

        {/* live status pill */}
        <motion.div
          variants={item}
          className="relative flex items-center gap-2 rounded-full bg-white/[0.05] border border-white/10 px-3 py-1.5 self-start"
        >
          <motion.span
            animate={playing ? { opacity: [1, 0.35, 1] } : { opacity: 1 }}
            transition={playing ? { duration: 1.6, repeat: Infinity, ease: 'easeInOut' } : undefined}
            className={`w-1.5 h-1.5 rounded-full ${playing ? 'bg-accent shadow-[0_0_8px_var(--accent-color-hex)]' : 'bg-emerald-400'}`}
          />
          <span className="text-[9px] font-black uppercase tracking-widest text-gray-400">
            {playing ? ti('Juego en ejecución', 'Game running') : ti('En segundo plano', 'Running in background')}
          </span>
        </motion.div>

        <motion.button
          variants={item}
          whileHover={{ scale: 1.02 }}
          whileTap={{ scale: 0.98 }}
          onClick={() => invoke('tray_open_main')}
          className="relative w-full flex items-center justify-center gap-2 rounded-xl bg-accent text-white text-[11px] font-black uppercase tracking-widest py-3 hover:brightness-110 transition-[filter]"
        >
          {ti('Abrir launcher', 'Open launcher')}
          <ArrowUpRight size={14} strokeWidth={3} />
        </motion.button>

        <motion.div variants={item} className="relative grid grid-cols-3 gap-2">
          {quick.map(({ tab, icon: Icon, label }) => (
            <button
              key={tab}
              onClick={() => go(tab)}
              className="flex flex-col items-center gap-1.5 rounded-xl bg-white/[0.04] hover:bg-white/[0.09] border border-white/[0.06] hover:border-white/15 py-2.5 text-gray-400 hover:text-white transition-colors"
            >
              <Icon size={15} />
              <span className="text-[9px] font-black uppercase tracking-widest">{label}</span>
            </button>
          ))}
        </motion.div>

        <motion.button
          variants={item}
          onClick={() => invoke('tray_quit')}
          className="relative mt-auto w-full flex items-center justify-center gap-2 rounded-xl hover:bg-red-500/10 border border-transparent hover:border-red-500/25 text-gray-500 hover:text-red-300 text-[10px] font-black uppercase tracking-widest py-2 transition-colors"
        >
          <LogOut size={12} />
          {ti('Salir por completo', 'Quit completely')}
        </motion.button>
      </motion.div>
    </div>
  );
}
