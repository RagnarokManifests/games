import React from 'react';

/**
 * Steam's logo, drawn as paths.
 *
 * lucide-react dropped its brand icons, so there is no `Steam` to import —
 * which is why the button that opens a game's Steam store page was sitting
 * behind a generic globe. A globe reads as "some website"; on a screen whose
 * whole subject is Steam, the one control that actually goes to Steam should
 * say so at a glance.
 *
 * Sized in `em` by default so it scales with whatever `size` prop convention
 * the surrounding lucide icons use, and filled with `currentColor` so it
 * inherits from the same class the icon it replaced did.
 */
const SteamIcon: React.FC<{ size?: number; className?: string }> = ({
  size = 15,
  className,
}) => (
  <svg
    viewBox="0 0 24 24"
    width={size}
    height={size}
    className={className}
    fill="currentColor"
    aria-hidden="true"
    focusable="false"
  >
    {/* Outer disc with the wrench-and-bearing cut out of it. */}
    <path d="M11.98 2C6.53 2 2.06 6.2 1.64 11.54l5.36 2.22a3.03 3.03 0 0 1 1.71-.53l2.39-3.46v-.05a4.04 4.04 0 1 1 4.04 4.04h-.1l-3.41 2.43c0 .05 0 .09.01.13a3.04 3.04 0 0 1-6.03.5L1.8 15.16A10.01 10.01 0 0 0 11.98 22C17.5 22 22 17.52 22 12S17.5 2 11.98 2z" />
    {/* The bearing: hollow so the disc reads as a ring at small sizes. */}
    <path d="M7.28 17.9l-1.23-.51a2.28 2.28 0 0 0 1.18 1.11 2.3 2.3 0 0 0 3-1.24 2.28 2.28 0 0 0-1.24-2.99 2.3 2.3 0 0 0-1.72-.02l1.27.53a1.68 1.68 0 0 1-1.26 3.12z" />
    {/* The wrench head. */}
    <path d="M17.83 9.72a2.7 2.7 0 1 0-5.39 0 2.7 2.7 0 0 0 5.39 0zm-4.72 0a2.03 2.03 0 1 1 4.06 0 2.03 2.03 0 0 1-4.06 0z" />
  </svg>
);

export default SteamIcon;
