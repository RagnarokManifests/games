import { useEffect, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { listen } from '@tauri-apps/api/event';
import { Trophy } from 'lucide-react';

// The popup shown by the "ach_toast" window (see build_achievement_toast in
// main.rs). Rust owns the timing — it shows the window, plays the sound and
// hides it again — so this view only animates inside that window. How long a
// toast stays comes from the backend (`duration_ms`), which keeps the window up
// for the same time.

type Payload = {
  name: string;
  description: string;
  icon: string;
  percent: number | null;
  // Set by the backend; a burst of unlocks gets shorter toasts and a counter.
  duration_ms?: number;
  index?: number;
  total?: number;
};

// How much of the backend's slot is left for the exit animation.
const EXIT_MS = 600;
const DEFAULT_MS = 5000;
const isEs = () => localStorage.getItem('rl_lang') === 'es';

const applyAccent = () => {
  const hex = localStorage.getItem('rl_accent') ?? '#dc2626';
  const m = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  const rgb = m ? `${parseInt(m[1], 16)} ${parseInt(m[2], 16)} ${parseInt(m[3], 16)}` : '220 38 38';
  const html = document.documentElement;
  html.classList.add('dark');
  html.style.setProperty('--accent-color', rgb);
  html.style.setProperty('--accent-color-hex', hex);
};

// Rarity comes from the share of players who have it. Gold is reserved for the
// rare ones — it means something here, unlike the accent, which is everyone's.
type Tier = { es: string; en: string; tone: string; gold: boolean };
const tierOf = (percent: number | null): Tier => {
  const accent = 'var(--accent-color-hex)';
  if (percent == null) return { es: 'Logro', en: 'Achievement', tone: accent, gold: false };
  if (percent < 5) return { es: 'Legendario', en: 'Legendary', tone: '#f5b942', gold: true };
  if (percent < 10) return { es: 'Épico', en: 'Epic', tone: '#f5b942', gold: true };
  if (percent < 20) return { es: 'Raro', en: 'Rare', tone: accent, gold: false };
  return { es: 'Común', en: 'Common', tone: accent, gold: false };
};

const SPARKS = Array.from({ length: 10 }, (_, i) => (i / 10) * Math.PI * 2);

const DEMO: Payload = {
  name: 'Ragnarok',
  description: 'Prueba del aviso de logros',
  icon: '',
  percent: 4.2,
};

export default function AchievementToast() {
  const [toast, setToast] = useState<(Payload & { id: number }) | null>(null);
  const [iconFailed, setIconFailed] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  const counter = useRef(0);

  const show = (p: Payload) => {
    applyAccent();
    setIconFailed(false);
    setToast({ ...p, id: ++counter.current });
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setToast(null), Math.max(1200, (p.duration_ms ?? DEFAULT_MS) - EXIT_MS));
  };

  useEffect(() => {
    document.body.style.background = 'transparent';
    document.documentElement.style.background = 'transparent';
    applyAccent();

    // Outside the app (plain browser, `?demo`) there is no Tauri to deliver
    // the event, so this lets the design be looked at with `npm run dev`.
    if (new URLSearchParams(window.location.search).has('demo')) {
      const t = window.setTimeout(() => show(DEMO), 400);
      return () => window.clearTimeout(t);
    }

    let unlisten: Promise<() => void> | undefined;
    try {
      unlisten = listen<Payload>('achievement-toast', (e) => show(e.payload));
    } catch {
      /* not running inside Tauri */
    }
    return () => {
      unlisten?.then((f) => f());
      window.clearTimeout(timer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const es = isEs();
  const tier = tierOf(toast?.percent ?? null);

  return (
    <div className="h-screen w-screen p-1.5 overflow-hidden select-none">
      <AnimatePresence>
        {toast && (
          <motion.div
            key={toast.id}
            initial={{ x: -48, opacity: 0 }}
            animate={{ x: 0, opacity: 1 }}
            exit={{ x: -32, opacity: 0, transition: { duration: 0.35, ease: 'easeIn' } }}
            transition={{ type: 'spring', stiffness: 260, damping: 26 }}
            style={{ ['--tone' as string]: tier.tone }}
            className="relative h-full"
          >
            {/* The card unrolls from the icon, like the console popup. */}
            <motion.div
              initial={{ clipPath: 'inset(0 calc(100% - 66px) 0 0 round 33px)' }}
              animate={{ clipPath: 'inset(0 0% 0 0 round 30px)' }}
              transition={{ delay: 0.35, duration: 0.55, ease: [0.22, 1, 0.36, 1] }}
              className="relative h-full overflow-hidden bg-[#0d0e12] border border-white/[0.1] shadow-2xl"
              style={{ borderRadius: 30 }}
            >
              {/* one soft glow in the rarity tone */}
              <div
                className="pointer-events-none absolute -left-10 top-1/2 -translate-y-1/2 w-44 h-44 rounded-full blur-3xl opacity-30"
                style={{ background: 'var(--tone)' }}
              />

              {/* light sweep, once, after it has unrolled */}
              <motion.div
                initial={{ x: '-120%' }}
                animate={{ x: '420%' }}
                transition={{ delay: 0.95, duration: 0.9, ease: 'easeInOut' }}
                className="pointer-events-none absolute inset-y-0 left-0 w-14 -skew-x-12 bg-gradient-to-r from-transparent via-white/[0.12] to-transparent"
              />

              <div className="relative h-full flex items-center gap-3 pl-1.5 pr-5">
                <div className="w-[66px] shrink-0" />

                <div className="min-w-0 flex-1">
                  <motion.div
                    initial={{ opacity: 0, x: -10 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{ delay: 0.6, duration: 0.3 }}
                    className="text-[9px] font-black uppercase tracking-[0.22em]"
                    style={{ color: 'var(--tone)' }}
                  >
                    {es ? 'Logro desbloqueado' : 'Achievement unlocked'}
                  </motion.div>
                  <motion.div
                    initial={{ opacity: 0, x: -10 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{ delay: 0.68, duration: 0.3 }}
                    className="text-[17px] font-black tracking-tight text-white truncate leading-tight mt-0.5"
                  >
                    {toast.name}
                  </motion.div>
                  {toast.description && (
                    <motion.div
                      initial={{ opacity: 0, x: -10 }}
                      animate={{ opacity: 1, x: 0 }}
                      transition={{ delay: 0.76, duration: 0.3 }}
                      className="text-[12px] font-semibold text-gray-300 truncate mt-0.5"
                    >
                      {toast.description}
                    </motion.div>
                  )}
                  <motion.div
                    initial={{ opacity: 0, y: 6 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ delay: 0.86, duration: 0.3 }}
                    className="mt-1.5 flex items-center gap-1.5"
                  >
                    <span
                      className="rounded-full border px-2 py-[3px] text-[9px] font-black uppercase tracking-wider"
                      style={{
                        color: 'var(--tone)',
                        borderColor: 'color-mix(in srgb, var(--tone) 40%, transparent)',
                        background: 'color-mix(in srgb, var(--tone) 12%, transparent)',
                      }}
                    >
                      {es ? tier.es : tier.en}
                    </span>
                    {(toast.total ?? 1) > 1 && (
                      <span className="rounded-full bg-white/[0.06] border border-white/10 px-2 py-[3px] text-[9px] font-black uppercase tracking-wider text-white/80 tabular-nums">
                        {toast.index} / {toast.total}
                      </span>
                    )}
                    {toast.percent != null && (
                      <span className="rounded-full bg-white/[0.06] border border-white/10 px-2 py-[3px] text-[9px] font-black uppercase tracking-wider text-gray-300">
                        {toast.percent.toFixed(1)}% {es ? 'de jugadores' : 'of players'}
                      </span>
                    )}
                  </motion.div>
                </div>
              </div>

              {/* time left */}
              <motion.div
                initial={{ scaleX: 1 }}
                animate={{ scaleX: 0 }}
                transition={{ delay: 0.9, duration: Math.max(0.4, ((toast.duration_ms ?? DEFAULT_MS) - EXIT_MS - 900) / 1000), ease: 'linear' }}
                className="absolute bottom-0 left-0 right-0 h-[3px] origin-left"
                style={{ background: 'var(--tone)', opacity: 0.85 }}
              />
            </motion.div>

            {/* icon: outside the clipped card so its ring and sparks can spill out */}
            <div className="absolute left-0 top-1/2 -translate-y-1/2 w-[66px] h-[66px] flex items-center justify-center">
              {[0, 1].map((i) => (
                <motion.span
                  key={i}
                  initial={{ scale: 1, opacity: 0.55 }}
                  animate={{ scale: 2, opacity: 0 }}
                  transition={{ delay: 0.15 + i * 0.35, duration: 0.9, ease: 'easeOut' }}
                  className="absolute inset-1 rounded-full border-2"
                  style={{ borderColor: 'var(--tone)' }}
                />
              ))}

              {tier.gold &&
                SPARKS.map((a, i) => (
                  <motion.span
                    key={i}
                    initial={{ x: 0, y: 0, opacity: 1, scale: 1 }}
                    animate={{ x: Math.cos(a) * 46, y: Math.sin(a) * 46, opacity: 0, scale: 0.2 }}
                    transition={{ delay: 0.3, duration: 0.8, ease: 'easeOut' }}
                    className="absolute w-1.5 h-1.5 rounded-full"
                    style={{ background: 'var(--tone)', boxShadow: '0 0 8px var(--tone)' }}
                  />
                ))}

              <motion.div
                initial={{ scale: 0, rotate: -25 }}
                animate={{ scale: 1, rotate: 0 }}
                transition={{ type: 'spring', stiffness: 320, damping: 16, delay: 0.05 }}
                className="relative w-[54px] h-[54px] rounded-full bg-[#0d0e12] border-2 flex items-center justify-center overflow-hidden"
                style={{
                  borderColor: 'var(--tone)',
                  boxShadow: '0 0 22px color-mix(in srgb, var(--tone) 55%, transparent)',
                }}
              >
                {toast.icon && !iconFailed ? (
                  <img
                    src={toast.icon}
                    alt=""
                    draggable={false}
                    onError={() => setIconFailed(true)}
                    className="w-full h-full object-cover"
                  />
                ) : (
                  <Trophy size={22} style={{ color: 'var(--tone)' }} />
                )}
              </motion.div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
