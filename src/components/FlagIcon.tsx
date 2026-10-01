import React from 'react';

/**
 * Real flags, drawn.
 *
 * The language picker used emoji flags (🇺🇸, 🇪🇸, …), which are regional
 * indicator pairs rather than pictures. Windows ships no font that renders
 * them, so it falls back to drawing the two letters — which is why the
 * selector read "US ES BR FR RU" instead of showing anything. Inline SVG has
 * no such dependency, and the artifact CSP blocks external images anyway.
 *
 * Drawn at 3:2 on a 24×16 viewBox and scaled by the caller.
 */

export type FlagCode = 'en' | 'es' | 'pt' | 'fr' | 'ru';

const FLAGS: Record<FlagCode, React.ReactNode> = {
  // United States. Thirteen stripes turn to mush at this size, so it is drawn
  // with seven — the reading is the canton plus red-and-white banding.
  en: (
    <>
      <rect width="24" height="16" fill="#F7F7F7" />
      {[0, 2, 4, 6, 8, 10, 12].map(i => (
        <rect key={i} y={(i * 16) / 13} width="24" height={16 / 13} fill="#B22234" />
      ))}
      <rect width="10" height="8.62" fill="#3C3B6E" />
      {[
        [1.6, 1.6], [4.2, 1.6], [6.8, 1.6],
        [2.9, 3.3], [5.5, 3.3], [8.1, 3.3],
        [1.6, 5.0], [4.2, 5.0], [6.8, 5.0],
        [2.9, 6.8], [5.5, 6.8], [8.1, 6.8],
      ].map(([cx, cy], i) => (
        <circle key={i} cx={cx} cy={cy} r="0.62" fill="#F7F7F7" />
      ))}
    </>
  ),

  // Spain, in the 1:2:1 band proportion.
  es: (
    <>
      <rect width="24" height="16" fill="#AA151B" />
      <rect y="4" width="24" height="8" fill="#F1BF00" />
    </>
  ),

  // Brazil.
  pt: (
    <>
      <rect width="24" height="16" fill="#009B3A" />
      <path d="M12 2 L22 8 L12 14 L2 8 Z" fill="#FEDF00" />
      <circle cx="12" cy="8" r="3.3" fill="#002776" />
      <path d="M8.9 6.6a3.3 3.3 0 0 1 6.3 1.1" stroke="#F7F7F7" strokeWidth="0.75" fill="none" />
    </>
  ),

  // France.
  fr: (
    <>
      <rect width="8" height="16" fill="#0055A4" />
      <rect x="8" width="8" height="16" fill="#F7F7F7" />
      <rect x="16" width="8" height="16" fill="#EF4135" />
    </>
  ),

  // Russia.
  ru: (
    <>
      <rect width="24" height="16" fill="#F7F7F7" />
      <rect y="5.34" width="24" height="5.33" fill="#0039A6" />
      <rect y="10.67" width="24" height="5.33" fill="#D52B1E" />
    </>
  ),
};

const FlagIcon: React.FC<{ code: FlagCode; className?: string }> = ({
  code,
  className = 'w-6 h-4',
}) => (
  <svg
    viewBox="0 0 24 16"
    className={`${className} rounded-[2px] shrink-0`}
    // Decorative: the language name sits right next to it.
    aria-hidden="true"
    focusable="false"
  >
    {FLAGS[code]}
    {/* A hairline keeps the white-edged flags from bleeding into the card. */}
    <rect
      width="24"
      height="16"
      fill="none"
      stroke="rgba(255,255,255,0.18)"
      strokeWidth="1"
    />
  </svg>
);

export default FlagIcon;
