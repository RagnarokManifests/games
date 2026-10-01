import React, { useState } from 'react';
import { motion } from 'framer-motion';
import { Copy, Check, ExternalLink, Heart, AlertTriangle, X } from 'lucide-react';
import { useTranslateInline } from '../App';
import { useNotify } from './NotificationProvider';
import { DONATION_PAGE, DONATION_MESSAGE, activeDonationAddresses } from './donations';
import CoinIcon, { COIN_COLOR } from './CoinIcon';

const openExternal = (url: string) => {
  import('@tauri-apps/api/shell').then(({ open }) => {
    open(url).catch(console.error);
  });
};

/**
 * Replaces the old single Ko-fi link.
 *
 * Every option is copy-to-clipboard rather than something to read and retype:
 * a wallet address is 34 characters of mixed case, and transcribing one by
 * hand is how donations end up unrecoverable. For the same reason the address
 * is shown in full and wrapped rather than truncated — someone checking that
 * a paste worked reads the last characters, and those are exactly the ones an
 * ellipsis hides.
 */
const DonateModal: React.FC<{ onClose: () => void }> = ({ onClose }) => {
  const ti = useTranslateInline();
  const { notify } = useNotify();
  const [copiedId, setCopiedId] = useState<string | null>(null);

  // Escape closes it too. Belt and braces for a panel whose only other exits
  // are a small button and the backdrop.
  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const addresses = activeDonationAddresses();
  const hasAnything = addresses.length > 0 || DONATION_PAGE.trim().length > 0;

  const copy = async (id: string, value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopiedId(id);
      window.setTimeout(() => setCopiedId(c => (c === id ? null : c)), 1800);
    } catch {
      notify(ti('No se pudo copiar', 'Could not copy'), 'error');
    }
  };

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.2 }}
      onClick={onClose}
      className="fixed inset-0 z-[200] flex items-center justify-center bg-black/60 backdrop-blur-md p-6"
    >
      <motion.div
        initial={{ opacity: 0, y: 12, scale: 0.97 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        exit={{ opacity: 0, y: 8, scale: 0.98 }}
        transition={{ duration: 0.3, ease: 'easeOut' }}
        onClick={e => e.stopPropagation()}
        className="w-full max-w-[420px] rounded-2xl bg-[#0d0e12] border border-white/[0.08] overflow-hidden shadow-2xl shadow-black/50"
      >
        {/* ── Header ── */}
        <div className="relative px-5 pt-5 pb-4">
          {/* The one ambient glow this panel gets. */}
          <div className="pointer-events-none absolute -top-16 -left-10 w-48 h-32 rounded-full bg-accent/10 blur-3xl" />

          {/* z-10: the title row below is positioned and comes later in the
              DOM, so without it that row paints over this button and swallows
              every click on it — the modal could not be closed at all. */}
          <button
            onClick={onClose}
            className="absolute top-4 right-4 z-10 w-7 h-7 rounded-full bg-white/[0.05] border border-white/10 flex items-center justify-center text-gray-500 hover:text-white hover:bg-white/[0.1] transition-colors"
          >
            <X size={13} />
          </button>

          <div className="relative flex items-center gap-3">
            <div className="w-10 h-10 rounded-full bg-accent/10 border border-accent/25 flex items-center justify-center shrink-0">
              <Heart size={16} className="text-accent" />
            </div>
            <div className="min-w-0">
              <h3 className="text-[15px] font-black text-white/90 tracking-tight leading-none">
                {ti('Apoya el Desarrollo', 'Support Development')}
              </h3>
              <p className="text-[10px] font-black uppercase tracking-widest text-gray-600 mt-1.5">
                {ti('Desde cualquier país', 'From any country')}
              </p>
            </div>
          </div>
        </div>

        <div className="h-px bg-white/[0.06]" />

        <div className="p-5 space-y-4 max-h-[62vh] overflow-y-auto custom-scrollbar">
          {DONATION_PAGE.trim() && (
            <button
              onClick={() => openExternal(DONATION_PAGE.trim())}
              className="w-full py-3 bg-accent/10 hover:bg-accent/20 border border-accent/30 text-accent font-black text-[12px] rounded-xl transition-all uppercase tracking-widest flex items-center justify-center gap-2 hover:-translate-y-0.5"
            >
              <ExternalLink size={14} />
              {ti('Donar con tarjeta o cripto', 'Donate by card or crypto')}
            </button>
          )}

          {addresses.length > 0 && (
            <motion.div
              initial="hidden"
              animate="show"
              variants={{ show: { transition: { staggerChildren: 0.07 } } }}
              className="space-y-2.5"
            >
              {addresses.map(a => {
                const color = COIN_COLOR[a.id as keyof typeof COIN_COLOR] ?? '#9CA3AF';
                const copied = copiedId === a.id;

                return (
                  <motion.div
                    key={a.id}
                    variants={{
                      hidden: { opacity: 0, y: 8 },
                      show: { opacity: 1, y: 0, transition: { duration: 0.3, ease: 'easeOut' } },
                    }}
                    className="rounded-xl bg-white/[0.03] border border-white/[0.07] hover:border-white/[0.14] transition-colors p-3.5"
                  >
                    <div className="flex items-center gap-2.5 mb-2.5">
                      <span
                        className="w-6 h-6 rounded-full flex items-center justify-center shrink-0"
                        style={{
                          color,
                          backgroundColor: `${color}1f`,
                          border: `1px solid ${color}40`,
                        }}
                      >
                        <CoinIcon id={a.id} className="w-4 h-4" />
                      </span>
                      <span className="text-[13px] font-black text-white/90 tracking-tight">
                        {a.label}
                      </span>
                      {/* The network is what loses the money when it is wrong,
                          so it reads as a label, never as fine print. */}
                      <span className="ml-auto rounded-full bg-white/[0.05] border border-white/10 px-2 py-0.5 text-[9px] font-black uppercase tracking-widest text-gray-400">
                        {a.network}
                      </span>
                    </div>

                    <div className="flex items-stretch gap-2">
                      {/* Styled as a field because that is what it is: data to
                          be taken, not prose to be read. */}
                      <code className="flex-1 min-w-0 rounded-lg bg-black/40 border border-white/10 px-2.5 py-2 text-[11px] leading-[1.5] font-mono text-gray-300 break-all select-all">
                        {a.value}
                      </code>
                      <button
                        onClick={() => copy(a.id, a.value)}
                        title={ti('Copiar', 'Copy')}
                        className={`shrink-0 w-11 rounded-lg border flex items-center justify-center transition-colors ${
                          copied
                            ? 'bg-accent/15 border-accent/40 text-accent'
                            : 'bg-white/[0.06] hover:bg-white/[0.11] border-white/10 text-gray-400 hover:text-white'
                        }`}
                      >
                        {copied ? <Check size={15} /> : <Copy size={14} />}
                      </button>
                    </div>

                    {copied && (
                      <motion.p
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        className="mt-2 text-[10px] font-black uppercase tracking-widest text-accent"
                      >
                        {ti('Copiado', 'Copied')}
                      </motion.p>
                    )}
                  </motion.div>
                );
              })}

              <div className="flex items-start gap-2 rounded-xl bg-amber-500/[0.06] border border-amber-500/20 px-3 py-2.5">
                <AlertTriangle size={13} className="shrink-0 mt-px text-amber-500/80" />
                <p className="text-[10px] font-semibold text-amber-200/70 leading-relaxed">
                  {ti(
                    'Verifica la red antes de enviar. Un envío por la red equivocada no se puede recuperar.',
                    'Check the network before sending. A transfer on the wrong network cannot be recovered.'
                  )}
                </p>
              </div>
            </motion.div>
          )}

          {/* Nothing filled in yet. Better to say so than to render an empty
              panel that looks broken. No Discord link here — the sidebar
              already has one directly above this button. */}
          {!hasAnything && (
            <p className="text-center py-3 px-1 text-[12px] font-medium text-gray-400 leading-relaxed whitespace-pre-line">
              {ti(DONATION_MESSAGE.es, DONATION_MESSAGE.en)}
            </p>
          )}
        </div>
      </motion.div>
    </motion.div>
  );
};

export default DonateModal;
