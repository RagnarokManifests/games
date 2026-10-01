import React, { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import splashBg from '../assets/ragnarok_loading_bg.png';
import appLogo from '../assets/logo.png';

interface SplashProps {
  onComplete: () => void;
}

const LOADING_PHRASES = [
  "Invocando las Antiguas Runas...",
  "Despertando a los Dioses...",
  "Sincronizando los Mundos...",
  "Abriendo el Bifröst...",
  "Preparando tu Leyenda..."
];

const RUNES = ["ᚠ", "ᚢ", "ᚦ", "ᚨ", "ᚱ", "ᚲ", "ᚷ", "ᚹ", "ᚺ", "ᚾ", "ᛁ", "ᛃ", "ᛇ", "ᛈ", "ᛉ", "ᛊ", "ᛏ", "ᛒ", "ᛖ", "ᛗ", "ᛚ", "ᛜ", "ᛞ", "ᛟ"];

const SplashNuevo: React.FC<SplashProps> = ({ onComplete }) => {
  const [phraseIndex, setPhraseIndex] = useState(0);
  const [progress, setProgress] = useState(0);

  // La barra no mide nada: es una animación de duración fija (~1,1 s). Antes
  // eran ~4,5 s con la app ya lista, así que se acortó.
  //
  // Va con requestAnimationFrame y no con setInterval, y eso importa. Un
  // setInterval sigue contando aunque el hilo principal esté bloqueado —
  // montar MainContent lo bloquea— y sus callbacks se acumulan y se disparan
  // todos juntos al liberarse. Resultado: para cuando el navegador podía
  // pintar el splash por primera vez, la barra ya iba por 60-70 y el usuario
  // nunca veía el arranque. rAF no corre mientras el hilo está tomado.
  //
  // El reloj arranca en el PRIMER frame, no al montar: un frame solo ocurre
  // después de pintar, así que el 0% es un 0% que se ve. Y el delta va
  // acotado a 50 ms para que un bloqueo largo estire la animación en vez de
  // pegar un salto.
  useEffect(() => {
    let raf = 0;
    let last: number | null = null;
    let elapsed = 0;
    const DURATION_MS = 1100;
    const MAX_FRAME_MS = 50;

    const tick = (now: number) => {
      if (last !== null) {
        elapsed += Math.min(now - last, MAX_FRAME_MS);
      }
      last = now;

      const pct = Math.min(100, Math.round((elapsed / DURATION_MS) * 100));
      setProgress(pct);

      if (pct >= 100) {
        window.setTimeout(() => onComplete(), 200);
        return;
      }
      raf = requestAnimationFrame(tick);
    };

    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [onComplete]);

  useEffect(() => {
    const phraseInterval = setInterval(() => {
      setPhraseIndex((prev) => (prev + 1) % LOADING_PHRASES.length);
    }, 2500);
    return () => clearInterval(phraseInterval);
  }, []);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-[#050507] text-white overflow-hidden select-none">

      {/* Background with very slow, epic zoom — final scale kept under 1 so the
          scene reads as slightly pulled back instead of tightly cropped.
          A radial mask fades the whole thing (image + its own overlays) out
          at the edges — without it, shrinking this box just left a hard
          rectangular cutoff where the image stopped and the flat page
          background started, which read as an ugly visible "square". */}
      <motion.div
        initial={{ scale: 1.0 }}
        animate={{ scale: 0.82 }}
        transition={{ duration: 15, ease: "easeOut" }}
        className="absolute inset-0 overflow-hidden"
        style={{
          WebkitMaskImage: 'radial-gradient(ellipse at center, black 55%, transparent 92%)',
          maskImage: 'radial-gradient(ellipse at center, black 55%, transparent 92%)',
        }}
      >
        <img
          src={splashBg}
          alt=""
          className="w-full h-full object-cover object-center opacity-50"
        />
        {/* Neutral dark grading — matches the app's near-black base instead of a hardcoded color tint */}
        <div className="absolute inset-0 bg-gradient-to-t from-[#050507] via-[#050507]/60 to-black/50" />
        {/* Cinematic Vignette */}
        <div className="absolute inset-0 bg-[radial-gradient(circle_at_center,_transparent_30%,_rgba(0,0,0,0.85)_100%)] z-10" />
        {/* Ambient glow using the app's single accent color instead of a fixed cyan */}
        <div
          className="absolute inset-0"
          style={{ background: 'radial-gradient(circle at center, rgb(var(--accent-color) / 0.15) 0%, transparent 60%)' }}
        />
      </motion.div>

      {/* Rising embers — echoes the red sparks already in the background art */}
      <div className="absolute inset-0 z-0 overflow-hidden pointer-events-none">
        {[...Array(16)].map((_, i) => (
          <motion.div
            key={`ember-${i}`}
            className="absolute rounded-full bg-accent"
            style={{
              width: Math.random() * 3 + 1 + 'px',
              height: Math.random() * 3 + 1 + 'px',
              left: Math.random() * 100 + '%',
              top: '105%',
              boxShadow: '0 0 8px 2px var(--accent-color-hex)',
            }}
            animate={{
              y: ['0vh', '-115vh'],
              x: [0, (Math.random() - 0.5) * 120],
              opacity: [0, Math.random() * 0.7 + 0.2, 0],
            }}
            transition={{
              duration: Math.random() * 6 + 6,
              repeat: Infinity,
              ease: 'linear',
              delay: Math.random() * 8,
            }}
          />
        ))}
      </div>

      {/* Floating Ancient Runes — kept for the Ragnarok theme, recolored to the accent color and thinned out */}
      <div className="absolute inset-0 z-0 overflow-hidden pointer-events-none">
        {[...Array(8)].map((_, i) => {
          const rune = RUNES[Math.floor(Math.random() * RUNES.length)];
          return (
            <motion.div
              key={`rune-${i}`}
              className="absolute text-accent/20 font-serif select-none"
              style={{
                fontSize: Math.random() * 18 + 14 + 'px',
                left: Math.random() * 80 + 10 + '%',
                top: Math.random() * 60 + 20 + '%',
              }}
              animate={{
                opacity: [0, 0.3, 0],
                y: [0, -40],
                rotate: [0, Math.random() * 60 - 30],
              }}
              transition={{
                duration: Math.random() * 7 + 7,
                repeat: Infinity,
                ease: "easeInOut",
                delay: Math.random() * 10,
              }}
            >
              {rune}
            </motion.div>
          );
        })}
      </div>

      {/* Cinematic Letterboxing (Top and Bottom black bars) */}
      <motion.div
        initial={{ height: 0 }}
        animate={{ height: "12vh" }}
        transition={{ duration: 1.5, ease: "easeInOut" }}
        className="absolute top-0 left-0 w-full bg-[#050507] z-20"
      />
      <motion.div
        initial={{ height: 0 }}
        animate={{ height: "12vh" }}
        transition={{ duration: 1.5, ease: "easeInOut" }}
        className="absolute bottom-0 left-0 w-full bg-[#050507] z-20 flex items-center justify-center"
      >
        {/* Loading text inside the bottom cinematic bar */}
        <AnimatePresence mode="wait">
          <motion.p
            key={phraseIndex}
            initial={{ opacity: 0, filter: "blur(10px)" }}
            animate={{ opacity: 1, filter: "blur(0px)" }}
            exit={{ opacity: 0, filter: "blur(10px)" }}
            transition={{ duration: 1 }}
            className="text-gray-500 font-light tracking-[0.4em] text-xs uppercase"
          >
            {LOADING_PHRASES[phraseIndex]}
          </motion.p>
        </AnimatePresence>
      </motion.div>

      {/* Center Content: Logo & Title */}
      <div className="relative z-10 flex flex-col items-center">

        {/* Logo with breathing accent glow */}
        <motion.div
          initial={{ opacity: 0, y: -20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 2, delay: 0.5 }}
          className="relative flex items-center justify-center mb-6"
        >
          <motion.div
            animate={{ scale: [1, 1.4, 1], opacity: [0.2, 0.5, 0.2] }}
            transition={{ repeat: Infinity, duration: 4, ease: "easeInOut" }}
            className="absolute w-32 h-32 bg-accent blur-[50px] rounded-full"
          />
          {/* Rune-activation shockwave rings, staggered so a new one fires every ~2s */}
          {[0, 1].map(i => (
            <motion.div
              key={`ring-${i}`}
              className="absolute w-28 h-28 rounded-full border border-accent/50"
              animate={{ scale: [1, 2.1], opacity: [0.5, 0] }}
              transition={{ repeat: Infinity, duration: 4, ease: "easeOut", delay: i * 2 }}
            />
          ))}
          <img src={appLogo} alt="Logo" className="relative w-28 h-28 object-contain drop-shadow-[0_0_20px_var(--accent-color-hex)]" />
        </motion.div>

        {/* Title with extreme tracking (letter-spacing) for epic feel — each
            letter reveals in sequence instead of the whole word at once */}
        <motion.h1
          className="flex text-5xl md:text-6xl font-light uppercase text-transparent bg-clip-text bg-gradient-to-b from-white to-accent drop-shadow-[0_0_15px_rgba(255,255,255,0.25)]"
          style={{ letterSpacing: '0.4em', marginLeft: '0.4em' }}
        >
          {"RAGNAROK".split('').map((letter, i) => (
            <motion.span
              key={i}
              initial={{ opacity: 0, y: -14 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.5, delay: 1 + i * 0.08, ease: 'easeOut' }}
            >
              {letter}
            </motion.span>
          ))}
        </motion.h1>

        {/* Launcher Subtitle */}
        <motion.span
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 2, delay: 1.5 }}
          className="text-[10px] tracking-[0.65em] text-accent/80 uppercase font-black mt-2 select-none"
        >
          Launcher
        </motion.span>

        {/* Loading progress bar */}
        <div className="mt-12 w-full max-w-lg flex flex-col gap-2 px-8">

          <div className="flex justify-end w-full">
            <motion.span
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              transition={{ duration: 1, delay: 0.3 }}
              className="text-white font-black tracking-widest text-sm drop-shadow-[0_0_10px_var(--accent-color-hex)]"
            >
              {progress}%
            </motion.span>
          </div>

          <motion.div
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 1, delay: 0.3 }}
            className="relative w-full h-[8px] bg-black/60 rounded-full border border-white/10 overflow-hidden shadow-[0_0_15px_rgba(0,0,0,0.8)]"
          >
            <motion.div
              className="absolute left-0 top-0 h-full bg-gradient-to-r from-accent/40 via-accent to-white rounded-full"
              style={{ boxShadow: '0 0 20px var(--accent-color-hex)' }}
              animate={{ width: `${progress}%` }}
              transition={{ ease: "linear", duration: 0.1 }}
            >
              <div className="absolute right-0 top-0 h-full w-4 bg-white blur-[2px] rounded-full" />
            </motion.div>
          </motion.div>

        </div>

      </div>
    </div>
  );
};

export default SplashNuevo;
