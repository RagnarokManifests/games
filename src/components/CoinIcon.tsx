import React from 'react';

/**
 * Coin marks, drawn as paths.
 *
 * Deliberately not text: ₮, ₿ and Ł are Unicode currency symbols, and a font
 * that lacks one draws a blank box instead. That is the same trap the flag
 * emoji fell into on Windows, and it is not worth repeating for the one
 * element whose whole job is to be recognised at a glance.
 */

export type CoinId = 'binance-pay' | 'usdt-trc20' | 'usdt-bep20' | 'btc' | 'ltc';

export const COIN_COLOR: Record<CoinId, string> = {
  'binance-pay': '#F0B90B',
  'usdt-trc20': '#26A17B',
  'usdt-bep20': '#26A17B',
  btc: '#F7931A',
  ltc: '#A6A9AA',
};

/** Tether, shared by both of its networks. */
const TETHER = (
  <g fill="currentColor">
    <path d="M4.5 4.5h15v3.4h-5.6v2.2h-3.8V7.9H4.5z" />
    <path d="M10.1 10.4h3.8v9.1h-3.8z" />
    <path d="M12 9.4c4.6 0 8.3 1 8.3 2.3S16.6 14 12 14s-8.3-1-8.3-2.3S7.4 9.4 12 9.4zm0 3.6c3.6 0 6.5-.6 6.5-1.3S15.6 10.4 12 10.4s-6.5.6-6.5 1.3S8.4 13 12 13z" />
  </g>
);

const MARKS: Record<CoinId, React.ReactNode> = {
  // Binance: the letter, not the diamond mark.
  //
  // The four-diamond logo is what Binance uses, but it renders at 16px here
  // and at that size the pieces stop reading as one shape no matter how they
  // are spaced — it came out looking like a cluster of blobs. A B is legible
  // at any size, and drawn as paths rather than text so no missing font can
  // turn it into an empty box.
  'binance-pay': (
    <g fill="currentColor">
      <path d="M6.5 3.6h6.9c3.1 0 5.1 1.5 5.1 3.9 0 1.6-.9 2.8-2.4 3.4 2 .5 3.2 1.9 3.2 3.9 0 2.7-2.2 4.2-5.7 4.2H6.5V3.6zm3.5 3v3.1h3.1c1.3 0 2.1-.6 2.1-1.6s-.8-1.5-2.1-1.5h-3.1zm0 5.9v3.5h3.6c1.4 0 2.3-.6 2.3-1.8s-.9-1.7-2.3-1.7h-3.6z" />
    </g>
  ),

  'usdt-trc20': TETHER,
  'usdt-bep20': TETHER,

  // Bitcoin: the B with its two ascenders and descenders.
  btc: (
    <g fill="currentColor">
      <path d="M9.6 3.4h1.9v2.4H9.6zM12.7 3.4h1.9v2.4h-1.9zM9.6 18.2h1.9v2.4H9.6zM12.7 18.2h1.9v2.4h-1.9z" />
      <path d="M6.4 5.8h6.9c2.7 0 4.6 1.2 4.6 3.2 0 1.3-.7 2.2-1.8 2.7 1.5.4 2.5 1.5 2.5 3.1 0 2.3-2 3.4-5 3.4H6.4V5.8zm3.2 2.6v2.8h3.2c1.1 0 1.8-.5 1.8-1.4s-.7-1.4-1.8-1.4H9.6zm0 5.2v3h3.6c1.2 0 2-.5 2-1.5s-.8-1.5-2-1.5H9.6z" />
    </g>
  ),

  // Litecoin: the slashed L.
  ltc: (
    <g fill="currentColor">
      <path d="M9.9 4.2h4.2l-2 8.1 2.4-.8-.6 2.4-2.4.8-.9 3.6h7.3l-.7 2.9H6.2l1.6-6.4-2.2.7.6-2.4 2.2-.7z" />
    </g>
  ),
};

const CoinIcon: React.FC<{ id: string; className?: string }> = ({
  id,
  className = 'w-3.5 h-3.5',
}) => {
  const mark = MARKS[id as CoinId];
  if (!mark) return null;
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true" focusable="false">
      {mark}
    </svg>
  );
};

export default CoinIcon;
