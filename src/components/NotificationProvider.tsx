import React, { useState, createContext, useContext, useCallback, useMemo } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { CheckCircle2, XCircle, Info, X, MessageCircle, ArrowRight, AlertTriangle } from 'lucide-react';

// 'warning' is for an operation that finished but not the way the user
// expects — files written that the game won't load, a partial install. It is
// deliberately distinct from 'error' (nothing happened) and from 'success'
// (it worked), because collapsing it into either one is how a toast ends up
// lying about what just happened.
type NotificationType = 'info' | 'success' | 'error' | 'warning' | 'reply';

interface Toast {
  id: string;
  key?: string;
  title?: string;
  message: string;
  type: NotificationType;
  persistent?: boolean;
  onClick?: () => void;
  count?: number;
}

interface NotifyOptions {
  // Skips the auto-dismiss timer — stays on screen until the user clicks it
  // or hits the X. For things like "the developer replied to your ticket",
  // where a 4s toast can come and go while the user is looking away and
  // never actually be seen.
  persistent?: boolean;
  onClick?: () => void;
  // Small heading above the message, e.g. "Respuesta del desarrollador" —
  // lets the message itself be the actual reply content instead of both
  // being crammed into one line.
  title?: string;
  // Groups repeated calls into ONE toast instead of stacking a new card
  // each time — e.g. a ticket's threadId, so several developer messages
  // sent close together update the same card (with a "x3" counter) rather
  // than burying the screen in a tower of identical persistent toasts.
  key?: string;
}

interface NotificationContextType {
  notify: (message: string, type?: NotificationType, options?: NotifyOptions) => void;
}

const NotificationContext = createContext<NotificationContextType | undefined>(undefined);

const TOAST_STYLES: Record<NotificationType, {
  icon: typeof CheckCircle2;
  iconBg: string;
  iconColor: string;
  bar: string;
  glow: string;
  ring: string;
}> = {
  success: { icon: CheckCircle2, iconBg: 'bg-emerald-500/15 border-emerald-500/30', iconColor: 'text-emerald-400', bar: 'bg-emerald-500', glow: 'bg-emerald-500/20', ring: 'bg-emerald-400' },
  error: { icon: XCircle, iconBg: 'bg-red-500/15 border-red-500/30', iconColor: 'text-red-400', bar: 'bg-red-500', glow: 'bg-red-500/20', ring: 'bg-red-400' },
  warning: { icon: AlertTriangle, iconBg: 'bg-amber-500/15 border-amber-500/30', iconColor: 'text-amber-400', bar: 'bg-amber-500', glow: 'bg-amber-500/20', ring: 'bg-amber-400' },
  info: { icon: Info, iconBg: 'bg-blue-500/15 border-blue-500/30', iconColor: 'text-blue-400', bar: 'bg-blue-500', glow: 'bg-blue-500/20', ring: 'bg-blue-400' },
  reply: { icon: MessageCircle, iconBg: 'bg-indigo-500/15 border-indigo-500/30', iconColor: 'text-indigo-300', bar: 'bg-indigo-500', glow: 'bg-indigo-500/25', ring: 'bg-indigo-400' },
};

const TOAST_DURATION_MS = 4000;

// Small two-note chime for the "developer replied" toast specifically —
// synthesized with the Web Audio API instead of bundling an audio file, so
// there's nothing to ship/load.
//
// A SHARED, module-level context (not a fresh one per call) matters here:
// browsers only let an AudioContext actually produce sound after a genuine
// user gesture (click/keydown) has occurred on the page — creating a brand
// new context at the exact moment a background poll detects a reply has no
// such gesture behind it, so it can stay permanently "suspended" and play
// nothing, silently (a first attempt did exactly this and produced no
// sound). Reusing one context that gets unlocked by the FIRST real click
// anywhere in the app — which will have already happened long before any
// ticket reply arrives — means it's already "running" by the time a chime
// actually needs to play.
let sharedAudioCtx: AudioContext | null = null;
const getAudioCtx = (): AudioContext | null => {
  if (sharedAudioCtx) return sharedAudioCtx;
  const Ctx = window.AudioContext || (window as any).webkitAudioContext;
  if (!Ctx) return null;
  sharedAudioCtx = new Ctx();
  return sharedAudioCtx;
};

if (typeof window !== 'undefined') {
  const unlock = () => { getAudioCtx()?.resume().catch(() => {}); };
  window.addEventListener('pointerdown', unlock, { once: true });
  window.addEventListener('keydown', unlock, { once: true });
}

const playReplyChime = () => {
  try {
    const ctx = getAudioCtx();
    if (!ctx) return;
    const start = () => {
      const now = ctx.currentTime;
      const tone = (freq: number, offset: number, duration: number, peakGain: number) => {
        const osc = ctx.createOscillator();
        const gain = ctx.createGain();
        osc.type = 'sine';
        osc.frequency.value = freq;
        gain.gain.setValueAtTime(0, now + offset);
        gain.gain.linearRampToValueAtTime(peakGain, now + offset + 0.02);
        gain.gain.exponentialRampToValueAtTime(0.0001, now + offset + duration);
        osc.connect(gain);
        gain.connect(ctx.destination);
        osc.start(now + offset);
        osc.stop(now + offset + duration + 0.05);
      };
      tone(740, 0, 0.14, 0.18);
      tone(988, 0.09, 0.24, 0.18);
    };
    if (ctx.state === 'suspended') {
      ctx.resume().then(start).catch(() => {});
    } else {
      start();
    }
  } catch {}
};

export const NotificationProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [toasts, setToasts] = useState<Toast[]>([]);

  const dismiss = useCallback((id: string) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const notify = useCallback((message: string, type: NotificationType = 'info', options?: NotifyOptions) => {
    const id = Math.random().toString(36).substr(2, 9);
    let isNewToast = true;
    setToasts((prev) => {
      if (options?.key) {
        const idx = prev.findIndex((t) => t.key === options.key);
        if (idx !== -1) {
          isNewToast = false;
          const existing = prev[idx];
          const without = prev.filter((_, i) => i !== idx);
          // Bump it back to the top of the stack (most recent), update its
          // content, and bump the counter instead of adding a new card.
          return [...without, { ...existing, message, title: options?.title ?? existing.title, count: (existing.count ?? 1) + 1 }];
        }
      }
      return [...prev, { id, key: options?.key, message, type, title: options?.title, persistent: options?.persistent, onClick: options?.onClick, count: 1 }];
    });
    if (type === 'reply') {
      playReplyChime();
    }
    if (isNewToast && !options?.persistent) {
      setTimeout(() => {
        setToasts((prev) => prev.filter((t) => t.id !== id));
      }, TOAST_DURATION_MS);
    }
  }, []);

  // `notify` is stable, but a fresh object around it is not — and this
  // provider re-renders on every toast appearing AND disappearing. Without the
  // memo, every consumer of useNotify() re-rendered each time: a success toast
  // repainted EmulatorsView's whole grid, which can be hundreds of tiles.
  const contextValue = useMemo(() => ({ notify }), [notify]);

  return (
    <NotificationContext.Provider value={contextValue}>
      {children}
      {/* z-[110]: must sit above modals (z-[100], including the Bypass ones
          portal'd to document.body) so a toast fired while a modal is open —
          e.g. "Contraseña copiada" from the password modal's copy button —
          is never buried behind it. */}
      <div className="fixed bottom-6 right-6 z-[110] flex flex-col gap-3 w-[340px]">
        <AnimatePresence>
          {toasts.map((toast) => {
            const style = TOAST_STYLES[toast.type];
            const Icon = style.icon;

            // Ticket replies get a richer "message preview" treatment
            // (header / quoted body / action pill) instead of the compact
            // single-line toast used for plain success/error/info — it's
            // the one notification worth lingering on since it's the only
            // persistent one, so it earns more visual weight.
            if (toast.type === 'reply') {
              return (
                <motion.div
                  key={toast.id}
                  initial={{ opacity: 0, y: 16, scale: 0.96 }}
                  animate={{ opacity: 1, y: 0, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.95, transition: { duration: 0.2 } }}
                  transition={{ type: 'spring', stiffness: 380, damping: 28 }}
                  whileHover={toast.onClick ? { y: -2 } : undefined}
                  onClick={() => {
                    if (toast.onClick) {
                      toast.onClick();
                      dismiss(toast.id);
                    }
                  }}
                  className="relative overflow-hidden rounded-2xl border border-indigo-400/25 bg-[#14151e]/95 backdrop-blur-xl shadow-2xl cursor-pointer hover:border-indigo-400/50 transition-colors"
                >
                  <div className="absolute inset-x-0 top-0 h-[2px] bg-gradient-to-r from-indigo-500/0 via-indigo-400 to-indigo-500/0" />
                  <div className="absolute -right-10 -top-10 w-28 h-28 rounded-full blur-3xl opacity-50 bg-indigo-500/25 pointer-events-none" />

                  <div className="relative z-10 px-4 pt-3.5 pb-3">
                    <div className="flex items-center gap-2.5">
                      <div className="relative shrink-0">
                        {toast.persistent && (
                          <span className="absolute inset-0 rounded-full bg-indigo-400 opacity-40 animate-ping" />
                        )}
                        <div className="relative w-8 h-8 rounded-full border border-indigo-400/40 bg-indigo-500/20 flex items-center justify-center">
                          <Icon size={15} className="text-indigo-300" />
                        </div>
                      </div>
                      <p className="flex-1 text-[11px] font-bold uppercase tracking-wider text-indigo-300/90">
                        {toast.title}
                        {(toast.count ?? 1) > 1 && (
                          <span className="ml-1.5 normal-case font-semibold text-indigo-400/80">· {toast.count} mensajes</span>
                        )}
                      </p>
                      <button
                        onClick={(e) => { e.stopPropagation(); dismiss(toast.id); }}
                        className="shrink-0 w-6 h-6 rounded-lg flex items-center justify-center text-gray-500 hover:text-white hover:bg-white/10 transition-colors"
                      >
                        <X size={13} />
                      </button>
                    </div>

                    <div className="mt-2.5 rounded-xl border border-white/[0.06] bg-white/[0.04] px-3 py-2.5">
                      <p className="text-[13px] font-medium text-gray-100 leading-snug break-words">{toast.message}</p>
                    </div>

                    {toast.onClick && (
                      <div className="mt-2.5 inline-flex items-center gap-1 text-[12px] font-semibold text-indigo-300">
                        Ver conversación <ArrowRight size={12} />
                      </div>
                    )}
                  </div>
                </motion.div>
              );
            }

            return (
              <motion.div
                key={toast.id}
                initial={{ opacity: 0, x: 50, scale: 0.95 }}
                animate={{ opacity: 1, x: 0, scale: 1 }}
                exit={{ opacity: 0, scale: 0.95, transition: { duration: 0.2 } }}
                whileHover={toast.onClick ? { scale: 1.02 } : undefined}
                onClick={() => {
                  if (toast.onClick) {
                    toast.onClick();
                    dismiss(toast.id);
                  }
                }}
                className={`relative overflow-hidden bg-[#12131a]/95 backdrop-blur-xl border rounded-2xl shadow-2xl ${toast.onClick ? 'cursor-pointer border-white/15 hover:border-white/30 transition-colors' : 'border-white/10'}`}
              >
                <div className={`absolute left-0 top-0 bottom-0 w-[3px] ${style.bar}`} />
                <div className={`absolute -right-8 -top-8 w-24 h-24 rounded-full blur-2xl opacity-60 ${style.glow} pointer-events-none`} />

                <div className="relative z-10 flex items-start gap-3 px-4 py-3.5">
                  <div className="relative shrink-0">
                    {toast.persistent && (
                      <span className={`absolute inset-0 rounded-xl ${style.ring} opacity-40 animate-ping`} />
                    )}
                    <div className={`relative w-9 h-9 rounded-xl border flex items-center justify-center ${style.iconBg}`}>
                      <Icon size={17} className={style.iconColor} />
                    </div>
                  </div>
                  <div className="flex-1 min-w-0 pt-0.5">
                    {toast.title && (
                      <p className="text-[11px] font-bold uppercase tracking-wide text-gray-400 mb-0.5">{toast.title}</p>
                    )}
                    <p className="text-[13px] font-semibold text-gray-100 leading-snug break-words">{toast.message}</p>
                    {toast.onClick && (
                      <p className={`flex items-center gap-1 text-[11px] font-semibold mt-1.5 ${style.iconColor}`}>
                        Ver conversación <ArrowRight size={11} />
                      </p>
                    )}
                  </div>
                  <button
                    onClick={(e) => { e.stopPropagation(); dismiss(toast.id); }}
                    className="shrink-0 w-6 h-6 rounded-lg flex items-center justify-center text-gray-500 hover:text-white hover:bg-white/10 transition-colors"
                  >
                    <X size={13} />
                  </button>
                </div>

                {!toast.persistent && (
                  <div className="relative h-[2px] bg-white/5">
                    <motion.div
                      className={`absolute inset-y-0 left-0 ${style.bar}`}
                      initial={{ width: '100%' }}
                      animate={{ width: '0%' }}
                      transition={{ duration: TOAST_DURATION_MS / 1000, ease: 'linear' }}
                    />
                  </div>
                )}
              </motion.div>
            );
          })}
        </AnimatePresence>
      </div>
    </NotificationContext.Provider>
  );
};

export const useNotify = () => {
  const context = useContext(NotificationContext);
  if (!context) throw new Error('useNotify must be used within NotificationProvider');
  return context;
};
