import React, { memo, useState, useEffect } from 'react';
import { convertFileSrc } from '@tauri-apps/api/tauri';

interface SeasonDef {
  gradient: string;
}

const SEASONS: Record<string, SeasonDef> = {
  christmas: { gradient: 'linear-gradient(160deg, #0a0f2a 0%, #05081a 55%, #0f1016 100%)' },
  halloween: { gradient: 'linear-gradient(160deg, #1a0800 0%, #0f0500 55%, #0f1016 100%)' },
  spring:    { gradient: 'linear-gradient(160deg, #041a0d 0%, #071a0d 55%, #0f1016 100%)' },
  summer:    { gradient: 'linear-gradient(160deg, #1a1200 0%, #0f0a00 55%, #0f1016 100%)' },
  autumn:    { gradient: 'linear-gradient(160deg, #1a0800 0%, #120500 55%, #0f1016 100%)' },
  winter:    { gradient: 'linear-gradient(160deg, #060a12 0%, #0a0f1a 55%, #0f1016 100%)' },
};

function detectSeason(): string {
  const now = new Date();
  const m = now.getMonth() + 1;
  const d = now.getDate();
  if ((m === 12 && d >= 20) || (m === 1 && d <= 6)) return 'christmas';
  if (m === 10 && d >= 25) return 'halloween';
  if (m >= 3 && m <= 5) return 'spring';
  if (m >= 6 && m <= 8) return 'summer';
  if (m >= 9 && m <= 11) return 'autumn';
  return 'winter';
}

const DynamicBackground = memo(({ customPath, preset }: { customPath: string; preset?: string }) => {
  const trimmed = customPath.trim();
  const [mediaFailed, setMediaFailed] = useState(false);

  // Without this, picking a broken background once (onError fires) would
  // permanently show the fallback gradient even after the user later selects
  // a different, valid background — `mediaFailed` never had a reason to
  // clear since it isn't tied to `customPath` at all.
  useEffect(() => {
    setMediaFailed(false);
  }, [customPath]);

  if (trimmed && !mediaFailed) {
    const ext = trimmed.split('.').pop()?.toLowerCase() ?? '';
    const src = convertFileSrc(trimmed);
    const isVideo = ext === 'mp4' || ext === 'webm' || ext === 'mov';

    return (
      <div className="fixed inset-0 z-0 overflow-hidden pointer-events-none">
        {isVideo ? (
          <video
            key={src}
            src={src}
            autoPlay
            loop
            muted
            playsInline
            disablePictureInPicture
            onError={() => setMediaFailed(true)}
            className="absolute inset-0 w-full h-full object-cover -z-10 [transform:translateZ(0)]"
          />
        ) : (
          <img
            key={src}
            src={src}
            alt=""
            onError={() => setMediaFailed(true)}
            className="absolute inset-0 w-full h-full object-cover"
          />
        )}
        <div className="absolute inset-0 bg-black/50" />
      </div>
    );
  }

  // Preset background styles
  const PRESETS: Record<string, string> = {
    cyberpunk: 'radial-gradient(circle at 20% 30%, rgba(0, 180, 216, 0.15) 0%, transparent 50%), radial-gradient(circle at 80% 70%, rgba(255, 0, 127, 0.15) 0%, transparent 50%), linear-gradient(160deg, #0b071e 0%, #030207 100%)',
    elden: 'radial-gradient(circle at 50% 30%, rgba(212, 163, 89, 0.12) 0%, transparent 60%), linear-gradient(160deg, #1c1d21 0%, #0d0e11 100%)',
    nebula: 'radial-gradient(circle at 30% 40%, rgba(124, 58, 237, 0.18) 0%, transparent 50%), radial-gradient(circle at 70% 60%, rgba(6, 182, 212, 0.15) 0%, transparent 50%), linear-gradient(160deg, #020204 0%, #000000 100%)',
  };

  const backgroundGradient = preset && PRESETS[preset] ? PRESETS[preset] : SEASONS[detectSeason()].gradient;

  return (
    <div
      className="fixed inset-0 z-0 pointer-events-none transition-all duration-1000"
      style={{ background: backgroundGradient }}
    />
  );
});

export default DynamicBackground;
