/**
 * Where donations go.
 *
 * Ko-fi closed the previous account, and every Western alternative
 * (Buy Me a Coffee, Patreon, Liberapay) runs on the same two payment
 * processors and enforces the same terms — so the options here are ones
 * nobody can close: a crypto donation page, plus raw addresses as the
 * fallback for when even that page is gone.
 *
 * ─────────────────────────────────────────────────────────────────────────
 * TO SET UP: paste your own values below. Anything left as an empty string
 * is not rendered at all, so a half-filled config never shows a card with a
 * placeholder address that somebody could copy and send real money to.
 * ─────────────────────────────────────────────────────────────────────────
 *
 * Check every address twice against the wallet it came from. A single wrong
 * character sends a donation somewhere unrecoverable, and unlike a card
 * payment there is nobody to reverse it.
 */

export interface DonationAddress {
  id: string;
  /** What the donor is sending. */
  label: string;
  /** The network. Shown prominently: sending on the wrong chain loses the money. */
  network: string;
  /** The address or ID. Empty means "not set up" — the entry is hidden. */
  value: string;
}

/**
 * A hosted donation page (NOWPayments or similar): the donor picks a coin and
 * pays, without needing to know which address matches which network. This is
 * the closest replacement for what Ko-fi did, so it goes first when set.
 */
export const DONATION_PAGE = '';

/**
 * What the modal says while no method is set up yet.
 *
 * Lives here rather than in the component so the wording can change without
 * touching any code — this is the one part of the modal that is likely to be
 * rewritten more than once before the real methods land.
 */
export const DONATION_MESSAGE = {
  es: 'Estamos configurando nuevos métodos de donación.',
  en: 'New donation methods are being set up.',
};

export const DONATION_ADDRESSES: DonationAddress[] = [
  {
    id: 'binance-pay',
    label: 'Binance Pay',
    network: 'Pay ID',
    value: '1141141344',
  },
  {
    id: 'usdt-trc20',
    label: 'USDT',
    network: 'TRC-20 · Tron',
    value: 'TBJFbVHdR9fQhhNZtMSriujYxHoZBrCdan',
  },
  {
    id: 'usdt-bep20',
    label: 'USDT',
    network: 'BEP-20 · BNB Chain',
    value: '',
  },
  {
    id: 'btc',
    label: 'Bitcoin',
    network: 'BTC',
    value: '',
  },
  {
    id: 'ltc',
    label: 'Litecoin',
    network: 'LTC',
    value: '',
  },
];

/**
 * Whether a value has the right shape for its network.
 *
 * This is a typo guard, not a security check. A mistyped address still looks
 * like a perfectly good address to the eye, and the mistake only surfaces
 * when a donation vanishes — by which point it is unrecoverable and nobody
 * can tell whose fault it was. Checking the shape catches the realistic
 * error (a character dropped or changed while pasting) before it ships.
 *
 * Anything that fails is hidden rather than shown broken: an option missing
 * from the modal is a small problem, an option that eats donations is not.
 */
export const looksValid = (a: DonationAddress): boolean => {
  const v = a.value.trim();
  if (!v) return false;

  switch (a.id) {
    // Tron: base58, always 34 characters, always starts with T.
    case 'usdt-trc20':
      return /^T[1-9A-HJ-NP-Za-km-z]{33}$/.test(v);

    // EVM chains: 0x plus 40 hex characters.
    case 'usdt-bep20':
      return /^0x[0-9a-fA-F]{40}$/.test(v);

    // Legacy, P2SH and bech32 all in use.
    case 'btc':
      return /^(bc1[0-9a-z]{25,62}|[13][1-9A-HJ-NP-Za-km-z]{25,34})$/.test(v);

    case 'ltc':
      return /^(ltc1[0-9a-z]{25,62}|[LM3][1-9A-HJ-NP-Za-km-z]{25,34})$/.test(v);

    // A Pay ID is digits only.
    case 'binance-pay':
      return /^[0-9]{6,20}$/.test(v);

    default:
      return true;
  }
};

/** Only the entries that are filled in and shaped correctly. */
export const activeDonationAddresses = (): DonationAddress[] =>
  DONATION_ADDRESSES.filter(a => {
    if (!a.value.trim()) return false;
    if (looksValid(a)) return true;
    // Loud in the console, silent in the UI: whoever edited the config needs
    // to know, the user does not.
    console.warn(
      `[donations] "${a.id}" no tiene el formato de ${a.network} y no se mostrara.`
    );
    return false;
  });
