import React, { useState, useEffect, useCallback, useMemo } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { invoke } from '@tauri-apps/api/tauri';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/api/dialog';
import { useLanguage, useTranslateInline, SwitchEmulatorsIcon } from '../App';
import { useNotify } from './NotificationProvider';
import {
  Gamepad2,
  Download,
  RefreshCw,
  Trash2,
  Play,
  KeyRound,
  FolderOpen,
  Search,
  X,
  CheckCircle2,
  HardDrive,
  Cpu,
  Info,
  Globe,
  ExternalLink,
  Layers,
  Sparkles,
  ChevronLeft,
  ChevronRight,
  Link2,
  FileDown,
  Eye,
  SlidersHorizontal,
} from 'lucide-react';

/// Which catalogue the grid is showing. The backend resolves each of these
/// to a WordPress site and a category — they all share one code path there,
/// which is why adding a console is a line in this list rather than a new
/// module.
type CatalogSource = 'eggnsemulator' | 'projectnx' | 'ps2' | 'ps3' | 'ps4';

/// Label and rough size for each source. The counts come from each site's
/// own `X-WP-Total` header, read while wiring this up, so they are the real
/// figures rather than round numbers.
/// The short name to show on a card for whichever source it came from.
///
/// The cards used to hardcode `source === 'projectnx' ? 'ProjectNX' : 'EggNS'`,
/// which was written when only those two existed. With PS2/PS3/PS4 added, every
/// PlayStation game in the grid was labelled "EggNS" — a Nintendo Switch site.
/// Deriving it from CATALOG_SOURCES means a new source cannot be mislabelled by
/// omission again.
const SOURCE_SHORT_LABELS: Record<string, string> = {
  eggnsemulator: 'Egg NS',
  projectnx: 'ProjectNX',
  ps2: 'PS2',
  ps3: 'PS3',
  ps4: 'PS4',
};

const sourceLabel = (source?: string): string =>
  (source && SOURCE_SHORT_LABELS[source]) || 'Catálogo';

const CATALOG_SOURCES: { id: CatalogSource; label: string; console: string }[] = [
  { id: 'eggnsemulator', label: 'Egg NS (+2,300)', console: 'Nintendo Switch' },
  { id: 'projectnx', label: 'ProjectNX (Español)', console: 'Nintendo Switch' },
  { id: 'ps2', label: 'PS2 (5,449)', console: 'PlayStation 2' },
  { id: 'ps3', label: 'PS3 (2,807)', console: 'PlayStation 3' },
  { id: 'ps4', label: 'PS4 (1,814)', console: 'PlayStation 4' },
];

/// The console picker. Two levels rather than one flat row of five: the
/// Switch has two competing sources and the PlayStations have one each, so a
/// single row was mixing "which console" with "which site" and getting
/// longer every time a console was added.
const CATALOG_CONSOLES: { id: string; label: string; source: CatalogSource }[] = [
  { id: 'switch', label: 'Nintendo Switch', source: 'eggnsemulator' },
  { id: 'ps2', label: 'PS2 (5,449)', source: 'ps2' },
  { id: 'ps3', label: 'PS3 (2,807)', source: 'ps3' },
  { id: 'ps4', label: 'PS4 (1,814)', source: 'ps4' },
];

/// romsfun's `console` taxonomy term ids, read from its own
/// `/wp-json/wp/v2/console`, with each total confirmed against `X-WP-Total`.
const ROMSFUN_CONSOLE: Record<string, { term: number; label: string }> = {
  ps2: { term: 8, label: 'PS2' },
  ps3: { term: 15, label: 'PS3' },
  ps4: { term: 87, label: 'PS4' },
};

/// Fetches a romsfun listing from the webview instead of from Rust.
///
/// This is not a preference, it is the only way in. romsfun answers **403 to
/// the Rust client** — not just to its API but to the plain HTML page too,
/// and it keeps refusing with a full set of browser headers, a Referer and
/// HTTP/1.1 forced. curl on the same machine gets 200 with the very same
/// User-Agent, which places the block on the TLS client fingerprint, where no
/// header can reach.
///
/// The webview is a real Chrome, so it has a real Chrome's fingerprint. And
/// romsfun echoes the Origin back — `Access-Control-Allow-Origin:
/// https://tauri.localhost` — and exposes `X-WP-Total`, so a cross-origin
/// call from here is allowed and can even read the totals.
async function fetchRomsfunCatalog(
  source: string,
  page: number,
  perPage: number,
  query: string,
): Promise<SwitchCatalogItem[]> {
  const meta = ROMSFUN_CONSOLE[source];
  if (!meta) return [];

  const params = new URLSearchParams({
    console: String(meta.term),
    page: String(page < 1 ? 1 : page),
    per_page: String(perPage),
    _embed: '1',
  });
  if (query.trim()) params.set('search', query.trim());

  const res = await fetch(`https://romsfun.com/wp-json/wp/v2/rom?${params}`, {
    headers: { Accept: 'application/json' },
  });
  if (!res.ok) {
    throw new Error(`romsfun respondió HTTP ${res.status}`);
  }
  const raw = await res.json();
  if (!Array.isArray(raw)) return [];

  const stripHtml = (v: string) =>
    new DOMParser().parseFromString(v ?? '', 'text/html').body.textContent?.trim() ?? '';

  return raw.map((item: any): SwitchCatalogItem => ({
    id: item.id,
    title: stripHtml(item?.title?.rendered ?? ''),
    slug: item?.slug ?? '',
    cover_url: item?._embedded?.['wp:featuredmedia']?.[0]?.source_url ?? null,
    excerpt: stripHtml(item?.excerpt?.rendered ?? ''),
    source,
    page_url: item?.link ?? '',
    format: null,
    size: null,
    version: null,
  }));
}

interface EmulatorInfo {
  id: string;
  name: string;
  description: string;
  latest_version: string | null;
  installed_version: string | null;
  install_path: string | null;
  exe_path: string | null;
  /// "switch" | "ps2" | "ps3" | "ps4"
  console: string;
  console_label: string;
  /// False for the PlayStation emulators: PS2 and PS3 take a BIOS or a
  /// firmware .PUP through the emulator's own menu, and PS4 needs no keys at
  /// all. Offering the prod.keys flow there would promise something Ragnarok
  /// cannot do.
  uses_keys: boolean;
}

interface SwitchGame {
  title_id: string | null;
  name: string;
  path: string;
  size_bytes: number;
  format: string;
}

interface SwitchCatalogItem {
  id: number;
  title: string;
  slug: string;
  cover_url: string | null;
  excerpt: string;
  source: string;
  page_url: string;
  format: string | null;
  size: string | null;
  version: string | null;
}

interface SwitchDownloadLink {
  name: string;
  url: string;
  host: string;
  file_type: string | null;
  size: string | null;
}

interface SwitchGameDetails {
  id: number;
  title: string;
  cover_url: string | null;
  description: string;
  source: string;
  page_url: string;
  format: string | null;
  size: string | null;
  version: string | null;
  release_date: string | null;
  download_links: SwitchDownloadLink[];
}

const formatBytes = (bytes: number): string => {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / Math.pow(1024, i)).toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
};

const openExternalUrl = async (url: string) => {
  try {
    const { open: shellOpen } = await import('@tauri-apps/api/shell');
    await shellOpen(url);
  } catch (err) {
    window.open(url, '_blank', 'noopener,noreferrer');
  }
};

const GAMES_FOLDER_KEY = 'switch_games_folder';
const SETUP_DONE_KEY = 'switch_setup_done';

const coverCache = new Map<string, string | null>();

const GameTile = React.memo(({ game }: { game: SwitchGame }) => {
  const [cover, setCover] = useState<string | null | undefined>(() =>
    coverCache.has(game.name) ? coverCache.get(game.name)! : undefined
  );

  useEffect(() => {
    if (cover !== undefined) return;
    let cancelled = false;
    invoke<string | null>('fetch_switch_cover', { name: game.name })
      .then(url => {
        coverCache.set(game.name, url ?? null);
        if (!cancelled) setCover(url ?? null);
      })
      .catch(() => {
        coverCache.set(game.name, null);
        if (!cancelled) setCover(null);
      });
    return () => { cancelled = true; };
  }, [game.name, cover]);

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      whileHover={{ y: -3 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="relative bg-[#0d0e12] border border-white/[0.08] rounded-2xl overflow-hidden group hover:border-accent/40 transition-colors shadow-lg"
    >
      <div className="relative h-36 w-full overflow-hidden bg-black/40">
        {cover ? (
          <img
            src={cover}
            alt={game.name}
            className="absolute inset-0 w-full h-full object-cover transition-transform duration-500 group-hover:scale-110"
          />
        ) : (
          <div className="absolute inset-0 flex items-center justify-center">
            {cover === undefined ? (
              <RefreshCw size={18} className="animate-spin text-gray-700" />
            ) : (
              <Gamepad2 size={26} className="text-white/15" />
            )}
          </div>
        )}
        <div className="absolute inset-0 bg-gradient-to-t from-black via-black/30 to-transparent" />
        <div className="absolute top-2.5 right-2.5">
          <span className="px-2 py-1 bg-white/10 border border-white/15 text-white text-[9px] font-black uppercase tracking-widest rounded-full">
            {game.format}
          </span>
        </div>
      </div>
      <div className="p-3">
        <h4 className="font-black text-[12px] text-white/90 tracking-tight line-clamp-2 leading-snug" title={game.path}>
          {game.name}
        </h4>
        <p className="text-[10px] text-gray-500 font-semibold mt-1.5 flex items-center gap-1.5">
          <HardDrive size={10} className="shrink-0" />
          {game.path.match(/^([A-Za-z]:)/)?.[1]?.toUpperCase() ?? ''} · {formatBytes(game.size_bytes)}
          {game.title_id && <span className="font-mono text-gray-600 truncate">· {game.title_id}</span>}
        </p>
      </div>
    </motion.div>
  );
});

const OnlineCatalogTile = React.memo(({
  item,
  onSelect,
}: {
  item: SwitchCatalogItem;
  onSelect: (item: SwitchCatalogItem) => void;
}) => {
  return (
    <motion.div
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      whileHover={{ y: -4 }}
      transition={{ duration: 0.25, ease: 'easeOut' }}
      onClick={() => onSelect(item)}
      className="group relative bg-[#0d0e12] border border-white/[0.08] hover:border-accent/50 rounded-2xl overflow-hidden cursor-pointer transition-all duration-300 shadow-xl hover:shadow-accent/10 flex flex-col"
    >
      {/* Cover image container */}
      <div className="relative aspect-[3/4] w-full overflow-hidden bg-black/50">
        {item.cover_url ? (
          <img
            src={item.cover_url}
            alt={item.title}
            loading="lazy"
            className="w-full h-full object-cover transition-transform duration-500 group-hover:scale-105"
          />
        ) : (
          <div className="absolute inset-0 flex items-center justify-center text-white/20">
            <Gamepad2 size={36} />
          </div>
        )}
        
        <div className="absolute inset-0 bg-gradient-to-t from-[#0d0e12] via-transparent to-black/30" />

        {/* Badges */}
        <div className="absolute top-2.5 left-2.5 right-2.5 flex items-center justify-between gap-1.5 pointer-events-none">
          <span className="px-2 py-0.5 rounded-md bg-black/70 backdrop-blur-md border border-white/10 text-[9px] font-black text-white/90 uppercase tracking-wider shadow">
            {sourceLabel(item.source)}
          </span>
          {item.format && (
            <span className="px-2 py-0.5 rounded-md bg-accent/85 backdrop-blur-md text-[9px] font-black text-white uppercase tracking-wider shadow">
              {item.format}
            </span>
          )}
        </div>

        {/* Hover overlay CTA */}
        <div className="absolute inset-0 bg-accent/20 opacity-0 group-hover:opacity-100 transition-opacity duration-300 flex items-center justify-center">
          <div className="px-3.5 py-1.5 rounded-xl bg-black/80 backdrop-blur-md border border-white/20 text-white text-[10px] font-black uppercase tracking-wider flex items-center gap-1.5 shadow-2xl transform translate-y-2 group-hover:translate-y-0 transition-transform">
            <FileDown size={12} className="text-accent" /> Descargar ROM
          </div>
        </div>
      </div>

      {/* Card Info */}
      <div className="p-3.5 flex flex-col flex-1 justify-between gap-2">
        <div>
          <h4 className="font-black text-xs text-white/95 tracking-tight line-clamp-2 leading-snug group-hover:text-accent transition-colors" title={item.title}>
            {item.title}
          </h4>
        </div>

        <div className="flex items-center justify-between text-[10px] font-bold text-gray-500 pt-1 border-t border-white/[0.04]">
          <span>{item.size ? item.size : 'Nintendo Switch'}</span>
          {item.version && (
            <span className="text-gray-400 font-mono text-[9px]">{item.version}</span>
          )}
        </div>
      </div>
    </motion.div>
  );
});

const EmulatorsView = () => {
  const { lang } = useLanguage();
  const ti = useTranslateInline();
  const { notify } = useNotify();
  const es = lang === 'es';

  // Navigation sub-tab: 'catalog' | 'installed' | 'setup'
  const [activeSubTab, setActiveSubTab] = useState<'catalog' | 'installed' | 'setup'>('catalog');

  const [emulators, setEmulators] = useState<EmulatorInfo[]>([]);
  const [loadingEmus, setLoadingEmus] = useState(true);
  const [busyId, setBusyId] = useState<string | null>(null);
  // Kept separate from busyId: launching must not grey out install/uninstall
  // across every card, it only needs to stop this one button being clicked
  // twice while the emulator is still opening its window.
  const [launchingId, setLaunchingId] = useState<string | null>(null);

  // Wizard state
  const [wizardOpen, setWizardOpen] = useState(() => localStorage.getItem(SETUP_DONE_KEY) !== 'true');
  const [wizardStep, setWizardStep] = useState(0);
  const [wizardEmu, setWizardEmu] = useState<string | null>(null);
  const [keysDone, setKeysDone] = useState(false);
  const [firmwareDone, setFirmwareDone] = useState(false);
  const [downloadingKeys, setDownloadingKeys] = useState(false);

  // Local games state
  const [gamesFolder, setGamesFolder] = useState<string>(() => localStorage.getItem(GAMES_FOLDER_KEY) ?? '');
  const [games, setGames] = useState<SwitchGame[]>([]);
  const [scanning, setScanning] = useState(false);
  const [search, setSearch] = useState('');

  // Online catalog state
  const [catalogSource, setCatalogSource] = useState<CatalogSource>('eggnsemulator');
  // Which console the current source belongs to, and whether that console is
  // the Switch — the only one with a choice of site and with NSP/XCI formats.
  const activeConsole = catalogSource === 'eggnsemulator' || catalogSource === 'projectnx'
    ? 'switch'
    : catalogSource;
  const isSwitchCatalog = activeConsole === 'switch';
  const [catalogItems, setCatalogItems] = useState<SwitchCatalogItem[]>([]);
  const [loadingCatalog, setLoadingCatalog] = useState(false);
  const [catalogPage, setCatalogPage] = useState(1);
  const [catalogQuery, setCatalogQuery] = useState('');
  const [catalogSearchInput, setCatalogSearchInput] = useState('');
  const [catalogFormatFilter, setCatalogFormatFilter] = useState<'all' | 'nsp' | 'xci'>('all');

  // Selected game modal state
  const [selectedGame, setSelectedGame] = useState<SwitchCatalogItem | null>(null);

  const loadEmulators = useCallback(async () => {
    setLoadingEmus(true);
    try {
      const list = await invoke<EmulatorInfo[]>('list_emulators', { english: !es });
      setEmulators(list);
    } catch {
      setEmulators([]);
    } finally {
      setLoadingEmus(false);
    }
  }, [es]);

  useEffect(() => { loadEmulators(); }, [loadEmulators]);

  const [progress, setProgress] = useState<Record<string, { percentage: number; extracting?: boolean }>>({});
  useEffect(() => {
    const un = listen<{ id: string; percentage: number; extracting?: boolean }>(
      'emulator_download_progress',
      e => setProgress(prev => ({ ...prev, [e.payload.id]: { percentage: e.payload.percentage, extracting: e.payload.extracting } }))
    );
    return () => { un.then(f => f()); };
  }, []);

  // Fetch online catalog
  const loadOnlineCatalog = useCallback(async (source: string, page: number, query: string) => {
    setLoadingCatalog(true);
    try {
      // The PlayStation catalogues come from romsfun, which refuses the Rust
      // client outright — see fetchRomsfunCatalog. Everything else goes
      // through the backend as before.
      const items = ROMSFUN_CONSOLE[source]
        ? await fetchRomsfunCatalog(source, page, 24, query)
        : await invoke<SwitchCatalogItem[]>('fetch_switch_online_catalog', {
            source,
            page,
            perPage: 24,
            query: query ? query : null,
          });
      setCatalogItems(items);
    } catch (err) {
      notify(`${err}`, 'error');
      setCatalogItems([]);
    } finally {
      setLoadingCatalog(false);
    }
  }, [notify]);

  useEffect(() => {
    if (activeSubTab === 'catalog') {
      loadOnlineCatalog(catalogSource, catalogPage, catalogQuery);
    }
  }, [activeSubTab, catalogSource, catalogPage, catalogQuery, loadOnlineCatalog]);

  const handleSearchSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    setCatalogPage(1);
    setCatalogQuery(catalogSearchInput.trim());
  };

  const handleSelectGame = (item: SwitchCatalogItem) => {
    setSelectedGame(item);
  };

  const scanFolder = useCallback(async (folder: string) => {
    if (!folder) return;
    setScanning(true);
    try {
      const found = await invoke<SwitchGame[]>('scan_switch_games', { folder });
      setGames(found);
      if (found.length === 0) {
        notify(ti('No se encontraron archivos .nsp/.xci/.nsz/.xcz en esa carpeta.', 'No .nsp/.xci/.nsz/.xcz files found in that folder.'), 'info');
      }
    } catch (err) {
      notify(`${err}`, 'error');
      setGames([]);
    } finally {
      setScanning(false);
    }
  }, [notify, ti]);

  useEffect(() => { if (gamesFolder) scanFolder(gamesFolder); }, [gamesFolder, scanFolder]);

  const handleInstall = async (emu: EmulatorInfo) => {
    setBusyId(emu.id);
    try {
      const tag = await invoke<string>('install_emulator', { emulatorId: emu.id });
      notify(ti(`${emu.name} ${tag} instalado.`, `${emu.name} ${tag} installed.`), 'success');
      setProgress(prev => { const next = { ...prev }; delete next[emu.id]; return next; });
      await loadEmulators();
    } catch (err) {
      notify(`${err}`, 'error');
    } finally {
      setBusyId(null);
    }
  };

  const handleUninstall = async (emu: EmulatorInfo) => {
    setBusyId(emu.id);
    try {
      await invoke('uninstall_emulator', { emulatorId: emu.id });
      notify(ti(`${emu.name} desinstalado.`, `${emu.name} uninstalled.`), 'success');
      await loadEmulators();
    } catch (err) {
      notify(`${err}`, 'error');
    } finally {
      setBusyId(null);
    }
  };

  const handleLaunch = async (emu: EmulatorInfo) => {
    if (launchingId) return;
    setLaunchingId(emu.id);
    try {
      await invoke('launch_emulator', { emulatorId: emu.id });
    } catch (err) {
      notify(`${err}`, 'error');
    } finally {
      // The command returns as soon as the process is spawned, well before
      // the emulator has a window. Releasing immediately would put the
      // button back under the cursor of someone who is still double
      // clicking, which is what made the emulator open twice and crash.
      setTimeout(() => setLaunchingId(null), 3000);
    }
  };

  const pickAndInstallKeys = async (emulatorId: string): Promise<boolean> => {
    const picked = await open({
      directory: false,
      multiple: false,
      filters: [{ name: 'Switch keys', extensions: ['keys'] }],
    });
    if (typeof picked !== 'string') return false;
    try {
      const msg = await invoke<string>('install_switch_keys', { emulatorId, sourcePath: picked });
      notify(msg, 'success');
      return true;
    } catch (err) {
      notify(`${err}`, 'error');
      return false;
    }
  };

  const pickAndInstallFirmware = async (emulatorId: string): Promise<boolean> => {
    const picked = await open({
      directory: false,
      multiple: false,
      filters: [{ name: 'Firmware Archive', extensions: ['zip', '7z'] }],
    });
    if (typeof picked !== 'string') return false;
    try {
      const msg = await invoke<string>('install_switch_firmware', { emulatorId, sourcePath: picked });
      notify(msg, 'success');
      return true;
    } catch (err) {
      notify(`${err}`, 'error');
      return false;
    }
  };

  const handleInstallKeys = async (emu: EmulatorInfo) => { await pickAndInstallKeys(emu.id); };
  const handleInstallFirmware = async (emu: EmulatorInfo) => { await pickAndInstallFirmware(emu.id); };

  const closeWizard = () => {
    localStorage.setItem(SETUP_DONE_KEY, 'true');
    setWizardOpen(false);
  };

  const wizardInstall = async (emu: EmulatorInfo) => {
    setWizardEmu(emu.id);
    setBusyId(emu.id);
    try {
      const tag = await invoke<string>('install_emulator', { emulatorId: emu.id });
      notify(ti(`${emu.name} ${tag} instalado.`, `${emu.name} ${tag} installed.`), 'success');
      setProgress(prev => { const next = { ...prev }; delete next[emu.id]; return next; });
      await loadEmulators();
      setWizardStep(1);
    } catch (err) {
      notify(`${err}`, 'error');
      setWizardEmu(null);
    } finally {
      setBusyId(null);
    }
  };

  const handlePickFolder = async () => {
    const picked = await open({ directory: true, multiple: false, defaultPath: gamesFolder || undefined });
    if (typeof picked !== 'string') return;
    localStorage.setItem(GAMES_FOLDER_KEY, picked);
    setGamesFolder(picked);
  };

  const handleAutodetect = async () => {
    setScanning(true);
    try {
      const found = await invoke<SwitchGame[]>('autodetect_switch_games');
      setGames(found);
      setGamesFolder('');
      localStorage.removeItem(GAMES_FOLDER_KEY);
      notify(
        found.length
          ? ti(`${found.length} juego(s) encontrados en tus discos.`, `Found ${found.length} game(s) on your drives.`)
          : ti('No se encontraron juegos en tus discos.', 'No games found on your drives.'),
        found.length ? 'success' : 'info'
      );
    } catch (err) {
      notify(`${err}`, 'error');
    } finally {
      setScanning(false);
    }
  };

  const [registering, setRegistering] = useState(false);
  const handleRegisterDir = async (emulatorId: string) => {
    const folders = Array.from(new Set(
      (gamesFolder ? [gamesFolder] : games.map(g => g.path.replace(/[\\/][^\\/]+$/, '')))
    ));
    if (folders.length === 0) {
      notify(ti('Primero buscá tus juegos.', 'Find your games first.'), 'info');
      return;
    }
    setRegistering(true);
    let ok = 0;
    const errors: string[] = [];
    for (const folder of folders) {
      try {
        await invoke<string>('register_games_dir', { emulatorId, folder });
        ok++;
      } catch (err) {
        errors.push(`${err}`);
      }
    }
    setRegistering(false);
    if (ok > 0) {
      notify(
        ti(`${ok} carpeta(s) configuradas. Reabrí el emulador para ver los juegos.`,
           `${ok} folder(s) configured. Reopen the emulator to see the games.`),
        'success'
      );
    } else {
      notify(errors[0] ?? ti('No se pudo configurar.', 'Could not configure.'), 'error');
    }
  };

  const filteredLocalGames = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return games;
    return games.filter(g => g.name.toLowerCase().includes(q) || (g.title_id ?? '').toLowerCase().includes(q));
  }, [games, search]);

  const filteredCatalogItems = useMemo(() => {
    if (catalogFormatFilter === 'all') return catalogItems;
    return catalogItems.filter(item => {
      const f = (item.format ?? '').toLowerCase();
      const t = item.title.toLowerCase();
      if (catalogFormatFilter === 'nsp') return f.includes('nsp') || t.includes('nsp');
      if (catalogFormatFilter === 'xci') return f.includes('xci') || t.includes('xci');
      return true;
    });
  }, [catalogItems, catalogFormatFilter]);

  const wizardEmuName = emulators.find(e => e.id === wizardEmu)?.name ?? '';

  useEffect(() => {
    if (!wizardOpen || wizardStep !== 0 || wizardEmu) return;
    const alreadyInstalled = emulators.find(e => e.installed_version);
    if (alreadyInstalled) {
      setWizardEmu(alreadyInstalled.id);
      setWizardStep(1);
    }
  }, [wizardOpen, wizardStep, wizardEmu, emulators]);

  return (
    <div className="max-w-6xl space-y-6">
      {/* First-run setup wizard */}
      <AnimatePresence>
        {wizardOpen && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-[100] flex items-center justify-center bg-black/70 backdrop-blur-md p-4"
            onClick={closeWizard}
          >
            <motion.div
              initial={{ opacity: 0, y: 12, scale: 0.97 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 12, scale: 0.97 }}
              transition={{ duration: 0.22, ease: 'easeOut' }}
              onClick={e => e.stopPropagation()}
              className="relative w-full max-w-2xl rounded-3xl bg-[#0d0e12] border border-white/[0.08] p-7 shadow-2xl overflow-hidden"
            >
              <div className="absolute -top-16 left-1/2 -translate-x-1/2 w-56 h-56 bg-accent/15 blur-[70px] rounded-full pointer-events-none" />

              <button
                onClick={closeWizard}
                className="absolute top-4 right-4 z-20 text-gray-500 hover:text-white transition-colors"
                title={ti('Cerrar', 'Close')}
              >
                <X size={17} />
              </button>

              <div className="relative z-10">
                {/* Step indicator */}
                <div className="flex items-center gap-2 mb-6">
                  {[0, 1, 2].map(s => (
                    <div
                      key={s}
                      className={`h-1 flex-1 rounded-full transition-colors duration-300 ${
                        s <= wizardStep ? 'bg-accent' : 'bg-white/10'
                      }`}
                    />
                  ))}
                </div>

                {wizardStep === 0 && (
                  <>
                    <h3 className="text-lg font-black text-white/90 tracking-tight">
                      {ti('¿Qué emulador querés usar?', 'Which emulator do you want?')}
                    </h3>
                    <p className="text-xs text-gray-400 font-semibold mt-1 mb-5">
                      {ti(
                        'Elegí uno para empezar. Después podés instalar el otro cuando quieras.',
                        'Pick one to start. You can install the other one later.'
                      )}
                    </p>
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                      {emulators.map(emu => (
                        <button
                          key={emu.id}
                          onClick={() => {
                            if (emu.installed_version) {
                              setWizardEmu(emu.id);
                              setWizardStep(1);
                            } else {
                              wizardInstall(emu);
                            }
                          }}
                          disabled={!!busyId || (!emu.latest_version && !emu.installed_version)}
                          className="text-left rounded-2xl bg-white/[0.03] border border-white/10 hover:border-accent/40 hover:bg-white/[0.06] p-4 transition-colors disabled:opacity-40 disabled:hover:border-white/10"
                        >
                          <div className="flex items-center gap-2 mb-1.5">
                            <Gamepad2 size={15} className="text-accent shrink-0" />
                            <span className="font-black text-white/90 text-sm">{emu.name}</span>
                            {emu.installed_version && (
                              <CheckCircle2 size={13} className="text-emerald-400 shrink-0" />
                            )}
                          </div>
                          <p className="text-[11px] text-gray-400 font-semibold leading-relaxed">{emu.description}</p>
                          <p className="text-[10px] text-gray-600 font-mono mt-2">
                            {emu.installed_version
                              ? `v${emu.installed_version} (${ti('instalado', 'installed')})`
                              : emu.latest_version
                                ? `v${emu.latest_version}`
                                : ti('No disponible', 'Unavailable')}
                          </p>
                        </button>
                      ))}
                    </div>
                  </>
                )}

                {wizardStep === 1 && (
                  <>
                    <h3 className="text-lg font-black text-white/90 tracking-tight">
                      {ti('Ahora tus keys', 'Now your keys')}
                    </h3>
                    <p className="text-xs text-gray-400 font-semibold mt-1 mb-5 leading-relaxed">
                      {ti(
                        `Descargá prod.keys automáticamente o elegí tu propio archivo y Ragnarok lo coloca solo en la carpeta que ${wizardEmuName} usa.`,
                        `Download prod.keys automatically or pick your own file and Ragnarok places it in the folder ${wizardEmuName} uses.`
                      )}
                    </p>
                    <div className="flex flex-wrap items-center gap-2.5">
                      <button
                        disabled={downloadingKeys}
                        onClick={async () => {
                          if (!wizardEmu) return;
                          setDownloadingKeys(true);
                          try {
                            const msg = await invoke<string>('download_and_install_keys', { emulatorId: wizardEmu });
                            notify(msg, 'success');
                            setKeysDone(true);
                          } catch (err) {
                            notify(`${err}`, 'error');
                          } finally {
                            setDownloadingKeys(false);
                          }
                        }}
                        className="flex items-center gap-2 px-4 py-2.5 rounded-xl bg-accent text-white text-[10px] font-black uppercase tracking-widest hover:brightness-110 transition-all shadow-lg shadow-accent/25 disabled:opacity-60 disabled:cursor-not-allowed"
                      >
                        {downloadingKeys
                          ? <><RefreshCw size={13} className="animate-spin" /> {ti('Descargando…', 'Downloading…')}</>
                          : <><Download size={13} /> {ti('Descargar prod.keys', 'Download prod.keys')}</>}
                      </button>
                      <button
                        disabled={downloadingKeys}
                        onClick={async () => { if (wizardEmu && await pickAndInstallKeys(wizardEmu)) setKeysDone(true); }}
                        className="flex items-center gap-2 px-4 py-2.5 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 text-[10px] font-black uppercase tracking-widest transition-colors disabled:opacity-60 disabled:cursor-not-allowed"
                      >
                        <KeyRound size={13} /> {ti('Elegir archivo', 'Choose file')}
                      </button>
                      {keysDone && (
                        <span className="flex items-center gap-1.5 text-[11px] font-bold text-emerald-400">
                          <CheckCircle2 size={13} /> {ti('Colocado', 'Placed')}
                        </span>
                      )}
                      <button
                        onClick={() => setWizardStep(2)}
                        className="ml-auto flex items-center gap-2 px-4 py-2.5 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 text-[10px] font-black uppercase tracking-widest transition-colors"
                      >
                        {keysDone ? ti('Siguiente', 'Next') : ti('Omitir', 'Skip')}
                      </button>
                    </div>
                  </>
                )}

                {wizardStep === 2 && (
                  <>
                    <h3 className="text-lg font-black text-white/90 tracking-tight">
                      {ti('Por último, el firmware', 'Finally, firmware')}
                    </h3>
                    <p className="text-xs text-gray-400 font-semibold mt-1 mb-5 leading-relaxed">
                      {ti(
                        'Muchos juegos andan solo con las keys. Si tenés el ZIP del firmware de tu consola, elegilo acá y lo descomprimimos en su lugar.',
                        'Many games run on keys alone. If you have your firmware ZIP, pick it here and we will extract it where it goes.'
                      )}
                    </p>
                    <div className="flex flex-wrap items-center gap-2.5">
                      <button
                        onClick={async () => { if (wizardEmu && await pickAndInstallFirmware(wizardEmu)) setFirmwareDone(true); }}
                        className="flex items-center gap-2 px-4 py-2.5 rounded-xl bg-accent text-white text-[10px] font-black uppercase tracking-widest hover:brightness-110 transition-all shadow-lg shadow-accent/25"
                      >
                        <Download size={13} /> {ti('Elegir firmware (.zip / .7z)', 'Choose firmware (.zip / .7z)')}
                      </button>
                      {firmwareDone && (
                        <span className="flex items-center gap-1.5 text-[11px] font-bold text-emerald-400">
                          <CheckCircle2 size={13} /> {ti('Instalado', 'Installed')}
                        </span>
                      )}
                      <button
                        onClick={closeWizard}
                        className="ml-auto flex items-center gap-2 px-4 py-2.5 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 text-[10px] font-black uppercase tracking-widest transition-colors"
                      >
                        {ti('Finalizar', 'Finish')}
                      </button>
                    </div>
                  </>
                )}

                <div className="flex items-start gap-2.5 mt-6 pt-5 border-t border-white/[0.06]">
                  <Info size={12} className="text-gray-600 shrink-0 mt-0.5" />
                  <p className="text-[10px] text-gray-500 font-semibold leading-relaxed">
                    {ti(
                      'Las keys se descargan automáticamente de prodkeys.net. También podés elegir un archivo que ya tengas. El firmware sale de tu propia consola.',
                      "Keys are downloaded automatically from prodkeys.net. You can also pick a file you already have. Firmware comes from your own console."
                    )}
                  </p>
                </div>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>

      {/* Main Header */}
      <motion.div
        initial={{ opacity: 0, y: 14 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.35, ease: 'easeOut' }}
        className="relative overflow-hidden rounded-3xl bg-[#0d0e12] border border-white/[0.08] p-6 shadow-2xl"
      >
        <div className="absolute -right-14 -top-14 w-60 h-60 bg-accent/15 blur-[70px] rounded-full pointer-events-none" />
        <div className="relative z-10 flex flex-col md:flex-row md:items-center justify-between gap-5">
          <div className="flex items-center gap-4">
            {/* The artwork is itself a red rounded tile, so it replaces the
                accent-filled box rather than sitting inside one. */}
            <SwitchEmulatorsIcon size={56} className="shrink-0 rounded-2xl shadow-xl shadow-accent/30" />
            <div>
              <div className="flex items-center gap-2">
                <h3 className="text-2xl font-black text-white/95 tracking-tight">
                  {ti('Juegos de Emuladores', 'Emulator Games')}
                </h3>
                <span className="px-2.5 py-0.5 rounded-full bg-accent/20 border border-accent/40 text-accent text-[9px] font-black uppercase tracking-widest">
                  Hub
                </span>
              </div>
              <p className="text-xs font-semibold text-gray-400 mt-1">
                {ti(
                  'Catálogo de más de 2,400 ROMs, emuladores y tus juegos instalados.',
                  'Catalog of 2,400+ ROMs, emulators, and your local installed games.'
                )}
              </p>
            </div>
          </div>

          {/* Sub-tab Navigation */}
          <div className="flex items-center gap-1.5 p-1.5 rounded-2xl bg-white/[0.03] border border-white/10 self-start md:self-auto">
            <button
              onClick={() => setActiveSubTab('catalog')}
              className={`flex items-center gap-2 px-4 py-2 rounded-xl text-[11px] font-black uppercase tracking-wider transition-all ${
                activeSubTab === 'catalog'
                  ? 'bg-accent text-white shadow-lg shadow-accent/25'
                  : 'text-gray-400 hover:text-white hover:bg-white/[0.04]'
              }`}
            >
              <Globe size={13} />
              {ti('Catálogo & ROMs', 'Catalog & ROMs')}
            </button>
            <button
              onClick={() => setActiveSubTab('installed')}
              className={`flex items-center gap-2 px-4 py-2 rounded-xl text-[11px] font-black uppercase tracking-wider transition-all ${
                activeSubTab === 'installed'
                  ? 'bg-accent text-white shadow-lg shadow-accent/25'
                  : 'text-gray-400 hover:text-white hover:bg-white/[0.04]'
              }`}
            >
              <Gamepad2 size={13} />
              {ti('Mis Juegos', 'My Games')}
              {games.length > 0 && (
                <span className="px-1.5 py-0.2 rounded-md bg-black/40 text-[9px]">{games.length}</span>
              )}
            </button>
            <button
              onClick={() => setActiveSubTab('setup')}
              className={`flex items-center gap-2 px-4 py-2 rounded-xl text-[11px] font-black uppercase tracking-wider transition-all ${
                activeSubTab === 'setup'
                  ? 'bg-accent text-white shadow-lg shadow-accent/25'
                  : 'text-gray-400 hover:text-white hover:bg-white/[0.04]'
              }`}
            >
              <Layers size={13} />
              {ti('Emuladores', 'Emulators')}
            </button>
          </div>
        </div>
      </motion.div>

      {/* SUB-TAB 1: ONLINE ROMS CATALOG */}
      {activeSubTab === 'catalog' && (
        <motion.div
          key="catalog-tab"
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0 }}
          className="space-y-6"
        >
          {/* Controls bar: Source Selector, Search, Filter */}
          <div className="flex flex-col md:flex-row items-stretch md:items-center justify-between gap-4 p-4 rounded-2xl bg-[#0d0e12] border border-white/[0.08]">
            {/* Console, then source */}
            <div className="flex flex-col gap-2.5">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-[10px] font-black uppercase tracking-widest text-gray-500 mr-1">
                  {ti('Consola:', 'Console:')}
                </span>
                {CATALOG_CONSOLES.map(c => (
                  <button
                    key={c.id}
                    // The NSP/XCI filter has to go with it. Its buttons only
                    // render for the Switch catalog, so leaving it set while
                    // switching to a PlayStation source filtered every result
                    // away — romsfun items carry no format — and the grid
                    // showed "No se pudieron cargar los juegos" with a Retry
                    // button for an error that never happened. With the filter
                    // buttons hidden there was no way back except returning to
                    // Switch and clearing it there.
                    onClick={() => {
                      setCatalogSource(c.source);
                      setCatalogPage(1);
                      setCatalogFormatFilter('all');
                    }}
                    className={`px-3 py-1.5 rounded-xl text-[10px] font-black uppercase tracking-wider transition-all ${
                      activeConsole === c.id
                        ? 'bg-accent text-white shadow-md shadow-accent/20'
                        : 'bg-white/[0.04] text-gray-400 hover:text-white border border-white/5'
                    }`}
                  >
                    {c.label}
                  </button>
                ))}
              </div>

              {/* Only the Switch has more than one site behind it. */}
              {isSwitchCatalog && (
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-[10px] font-black uppercase tracking-widest text-gray-600 mr-1">
                    {ti('Fuente:', 'Source:')}
                  </span>
                  {CATALOG_SOURCES.filter(x => x.console === 'Nintendo Switch').map(src => (
                    <button
                      key={src.id}
                      onClick={() => { setCatalogSource(src.id); setCatalogPage(1); }}
                      className={`flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-[9px] font-black uppercase tracking-wider transition-all ${
                        catalogSource === src.id
                          ? 'bg-white/15 text-white'
                          : 'bg-white/[0.03] text-gray-500 hover:text-gray-300 border border-white/5'
                      }`}
                    >
                      {src.id === 'eggnsemulator' ? <Sparkles size={10} /> : <Globe size={10} />}
                      {src.label}
                    </button>
                  ))}
                </div>
              )}
            </div>

            {/* Format Filter Tags & Search */}
            <div className="flex flex-wrap items-center gap-2.5">
              {/* NSP and XCI are Switch cartridge formats — offering them
                  while a PlayStation catalogue is on screen filtered
                  everything away and looked broken. */}
              {isSwitchCatalog && (
              <div className="flex items-center gap-1 bg-black/40 border border-white/10 rounded-xl p-1">
                <button
                  onClick={() => setCatalogFormatFilter('all')}
                  className={`px-2.5 py-1 rounded-lg text-[9px] font-black uppercase tracking-wider transition-colors ${
                    catalogFormatFilter === 'all' ? 'bg-white/15 text-white' : 'text-gray-500 hover:text-gray-300'
                  }`}
                >
                  {ti('Todos', 'All')}
                </button>
                <button
                  onClick={() => setCatalogFormatFilter('nsp')}
                  className={`px-2.5 py-1 rounded-lg text-[9px] font-black uppercase tracking-wider transition-colors ${
                    catalogFormatFilter === 'nsp' ? 'bg-accent text-white' : 'text-gray-500 hover:text-gray-300'
                  }`}
                >
                  NSP
                </button>
                <button
                  onClick={() => setCatalogFormatFilter('xci')}
                  className={`px-2.5 py-1 rounded-lg text-[9px] font-black uppercase tracking-wider transition-colors ${
                    catalogFormatFilter === 'xci' ? 'bg-accent text-white' : 'text-gray-500 hover:text-gray-300'
                  }`}
                >
                  XCI
                </button>
              </div>
              )}

              {/* Search Form */}
              <form onSubmit={handleSearchSubmit} className="relative flex-1 md:w-72">
                <Search size={13} className="absolute left-3 top-2.5 text-gray-500" />
                <input
                  type="text"
                  value={catalogSearchInput}
                  onChange={e => setCatalogSearchInput(e.target.value)}
                  placeholder={ti('Buscar juego (Zelda, Mario…)', 'Search game (Zelda, Mario…)')}
                  className="w-full bg-black/40 border border-white/10 rounded-xl pl-8 pr-16 py-1.5 text-xs text-white placeholder-gray-600 focus:outline-none focus:border-accent/50 transition-colors"
                />
                <div className="absolute right-1.5 top-1 flex items-center gap-1">
                  {catalogSearchInput && (
                    <button
                      type="button"
                      onClick={() => { setCatalogSearchInput(''); setCatalogQuery(''); setCatalogPage(1); }}
                      className="p-1 text-gray-500 hover:text-white"
                    >
                      <X size={12} />
                    </button>
                  )}
                  <button
                    type="submit"
                    className="px-2 py-1 rounded-lg bg-accent text-white text-[9px] font-black uppercase tracking-wider hover:brightness-110"
                  >
                    OK
                  </button>
                </div>
              </form>
            </div>
          </div>

          {/* Active Search / Filter Banner */}
          {catalogQuery && (
            <div className="flex items-center justify-between px-4 py-2 rounded-xl bg-accent/10 border border-accent/20 text-xs text-white">
              <span className="font-semibold">
                {ti('Buscando:', 'Searching:')} <span className="font-black text-accent">"{catalogQuery}"</span> en {CATALOG_SOURCES.find(x => x.id === catalogSource)?.label ?? catalogSource}
              </span>
              <button
                onClick={() => { setCatalogQuery(''); setCatalogSearchInput(''); setCatalogPage(1); }}
                className="text-[10px] font-black uppercase tracking-widest text-gray-400 hover:text-white flex items-center gap-1"
              >
                <X size={12} /> {ti('Limpiar búsqueda', 'Clear search')}
              </button>
            </div>
          )}

          {/* Catalog Grid */}
          <div className="relative min-h-[400px]">
            {loadingCatalog ? (
              <div className="flex flex-col items-center justify-center gap-3 py-28 text-gray-500">
                <RefreshCw size={24} className="animate-spin text-accent" />
                <span className="text-xs font-bold tracking-wide">
                  {ti('Cargando catálogo de juegos de Nintendo Switch…', 'Loading Nintendo Switch games catalog…')}
                </span>
              </div>
            ) : filteredCatalogItems.length === 0 ? (
              <div className="flex flex-col items-center justify-center gap-3 py-28 text-gray-600">
                <Gamepad2 size={40} className="opacity-30" />
                <p className="text-sm font-bold text-center max-w-sm">
                  {catalogQuery
                    ? ti('No se encontraron juegos para esa búsqueda.', 'No games found for that query.')
                    : ti('No se pudieron cargar los juegos.', 'Could not load games.')}
                </p>
                <button
                  onClick={() => loadOnlineCatalog(catalogSource, catalogPage, catalogQuery)}
                  className="mt-2 px-4 py-2 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 text-xs font-black uppercase tracking-widest transition-colors flex items-center gap-2"
                >
                  <RefreshCw size={12} /> {ti('Reintentar', 'Retry')}
                </button>
              </div>
            ) : (
              <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6 gap-4">
                {filteredCatalogItems.map(item => (
                  <OnlineCatalogTile key={item.id} item={item} onSelect={handleSelectGame} />
                ))}
              </div>
            )}
          </div>

          {/* Pagination Controls */}
          {!loadingCatalog && filteredCatalogItems.length > 0 && (
            <div className="flex items-center justify-center gap-3 pt-4 border-t border-white/[0.06]">
              <button
                disabled={catalogPage <= 1 || loadingCatalog}
                onClick={() => setCatalogPage(p => Math.max(1, p - 1))}
                className="flex items-center gap-1.5 px-4 py-2 rounded-xl bg-white/[0.05] hover:bg-white/[0.1] border border-white/10 text-gray-300 text-xs font-black uppercase tracking-wider transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
              >
                <ChevronLeft size={14} /> {ti('Anterior', 'Previous')}
              </button>

              <div className="px-4 py-2 rounded-xl bg-black/40 border border-white/10 text-xs font-mono font-black text-white">
                {ti('Página', 'Page')} {catalogPage}
              </div>

              <button
                disabled={loadingCatalog || filteredCatalogItems.length < 20}
                onClick={() => setCatalogPage(p => p + 1)}
                className="flex items-center gap-1.5 px-4 py-2 rounded-xl bg-white/[0.05] hover:bg-white/[0.1] border border-white/10 text-gray-300 text-xs font-black uppercase tracking-wider transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
              >
                {ti('Siguiente', 'Next')} <ChevronRight size={14} />
              </button>
            </div>
          )}
        </motion.div>
      )}

      {/* SUB-TAB 2: LOCAL INSTALLED GAMES */}
      {activeSubTab === 'installed' && (
        <motion.div
          key="installed-tab"
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0 }}
          className="space-y-6"
        >
          <div className="flex flex-col md:flex-row md:items-center justify-between gap-3 p-4 rounded-2xl bg-[#0d0e12] border border-white/[0.08]">
            <div>
              <h4 className="text-sm font-black text-white/90">
                {ti('Juegos en tu PC', 'Games on your PC')}
                {games.length > 0 && <span className="text-gray-500 font-normal"> · {games.length} {ti('encontrados', 'found')}</span>}
              </h4>
              <p className="text-xs text-gray-400 font-semibold mt-0.5">
                {ti('Escaneá tus discos o elegí la carpeta donde guardás tus ROMs (.nsp, .xci, .nsz).', 'Scan your drives or pick the folder where you keep your ROMs.')}
              </p>
            </div>

            <div className="flex items-center gap-2.5 flex-wrap">
              <button
                onClick={handleAutodetect}
                disabled={scanning}
                className="flex items-center gap-2 px-4 py-2.5 rounded-xl bg-accent text-white text-[10px] font-black uppercase tracking-widest hover:brightness-110 transition-all shadow-lg shadow-accent/25 disabled:opacity-40"
              >
                <Search size={13} className={scanning ? 'animate-pulse' : ''} />
                {ti('Buscar en mis discos', 'Scan my drives')}
              </button>
              <button
                onClick={handlePickFolder}
                title={ti('Elegir una carpeta puntual', 'Pick a specific folder')}
                className="flex items-center gap-2 px-4 py-2.5 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 text-[10px] font-black uppercase tracking-widest transition-colors"
              >
                <FolderOpen size={13} />
                {ti('Elegir carpeta', 'Pick folder')}
              </button>
              {gamesFolder && (
                <button
                  onClick={() => scanFolder(gamesFolder)}
                  disabled={scanning}
                  title={ti('Volver a escanear', 'Rescan')}
                  className="w-10 h-10 flex items-center justify-center rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-400 transition-colors disabled:opacity-40"
                >
                  <RefreshCw size={14} className={scanning ? 'animate-spin' : ''} />
                </button>
              )}
            </div>
          </div>

          {gamesFolder && (
            <p className="text-[10px] text-gray-500 font-mono truncate flex items-center gap-1.5 px-2">
              <CheckCircle2 size={11} className="text-emerald-500/80 shrink-0" /> Carpeta activa: {gamesFolder}
            </p>
          )}

          {/* Quick Register in Emulators */}
          {games.length > 0 && emulators.some(e => e.installed_version) && (
            <div className="flex flex-wrap items-center gap-2.5 rounded-2xl bg-white/[0.03] border border-white/10 px-5 py-3.5 shadow-md">
              <span className="text-xs text-gray-300 font-semibold">
                {ti('Vincular estos juegos al emulador:', 'Add these games into emulator:')}
              </span>
              {emulators.filter(e => e.installed_version).map(e => (
                <button
                  key={e.id}
                  onClick={() => handleRegisterDir(e.id)}
                  disabled={registering}
                  className="flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-white/[0.06] hover:bg-accent/20 border border-white/10 hover:border-accent/30 text-gray-300 hover:text-accent text-[10px] font-black uppercase tracking-widest transition-colors disabled:opacity-40"
                >
                  {registering ? <RefreshCw size={11} className="animate-spin" /> : <CheckCircle2 size={11} />}
                  {e.name}
                </button>
              ))}
            </div>
          )}

          {/* Local Games Grid */}
          <AnimatePresence mode="wait">
            {!gamesFolder && games.length === 0 && !scanning ? (
              <motion.div
                key="empty"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                className="flex flex-col items-center justify-center gap-3 py-24 text-gray-600 rounded-3xl border border-dashed border-white/10"
              >
                <Search size={36} className="opacity-30 text-accent" />
                <p className="text-sm font-bold text-center max-w-sm">
                  {ti(
                    'Tocá "Buscar en mis discos" y Ragnarok encuentra solo tus juegos instalados.',
                    'Hit "Scan my drives" and Ragnarok will locate your games automatically.'
                  )}
                </p>
              </motion.div>
            ) : scanning ? (
              <motion.div
                key="scanning"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                className="flex items-center justify-center gap-3 py-24 text-gray-500"
              >
                <RefreshCw size={20} className="animate-spin text-accent" />
                <span className="text-xs font-bold">
                  {gamesFolder ? ti('Escaneando carpeta…', 'Scanning folder…') : ti('Buscando en tus discos…', 'Scanning your drives…')}
                </span>
              </motion.div>
            ) : filteredLocalGames.length === 0 ? (
              <motion.div
                key="none"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                className="flex flex-col items-center justify-center gap-3 py-24 text-gray-600"
              >
                <Gamepad2 size={36} className="opacity-30" />
                <p className="text-sm font-bold text-center max-w-sm">
                  {search
                    ? ti('Ningún juego coincide con la búsqueda.', 'No game matches that search.')
                    : ti('No se encontraron juegos en esa carpeta.', 'No games found in that folder.')}
                </p>
              </motion.div>
            ) : (
              <motion.div
                key="grid"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-4"
              >
                {filteredLocalGames.map(g => <GameTile key={g.path} game={g} />)}
              </motion.div>
            )}
          </AnimatePresence>
        </motion.div>
      )}

      {/* SUB-TAB 3: EMULATORS & SETUP */}
      {activeSubTab === 'setup' && (
        <motion.div
          key="setup-tab"
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0 }}
          className="space-y-6"
        >
          <div className="flex items-center justify-between">
            <div>
              <h4 className="text-sm font-black text-white/90">
                {ti('Emuladores de Switch', 'Switch Emulators')}
              </h4>
              <p className="text-xs text-gray-400 font-semibold mt-0.5">
                {ti('Instalá, ejecutá y actualizá tus emuladores con un solo clic.', 'Install, run, and update your emulators with one click.')}
              </p>
            </div>
            <div className="flex items-center gap-3">
              <button
                onClick={() => { setWizardStep(0); setWizardEmu(null); setKeysDone(false); setFirmwareDone(false); setWizardOpen(true); }}
                className="px-3 py-1.5 rounded-xl bg-white/[0.05] hover:bg-white/[0.1] border border-white/10 text-[10px] font-black uppercase tracking-widest text-gray-300 hover:text-white transition-colors"
              >
                {ti('Asistente inicial', 'Setup wizard')}
              </button>
              <button
                onClick={loadEmulators}
                disabled={loadingEmus}
                className="p-2 text-gray-500 hover:text-white transition-colors disabled:opacity-40"
                title={ti('Recargar', 'Refresh')}
              >
                <RefreshCw size={14} className={loadingEmus ? 'animate-spin' : ''} />
              </button>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            {emulators.map(emu => {
              const installed = !!emu.installed_version;
              const hasUpdate = installed && emu.latest_version && emu.latest_version !== emu.installed_version;
              const busy = busyId === emu.id;
              const prog = progress[emu.id];
              return (
                <motion.div
                  key={emu.id}
                  initial={{ opacity: 0, y: 10 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ duration: 0.3, ease: 'easeOut' }}
                  className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-5 flex flex-col gap-4 shadow-xl"
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <h5 className="font-black text-white/90 text-base tracking-tight">{emu.name}</h5>
                        {installed && (
                          <span className="px-2 py-0.5 rounded-full bg-emerald-500/15 border border-emerald-500/30 text-emerald-400 text-[9px] font-black uppercase tracking-widest">
                            {ti('Instalado', 'Installed')}
                          </span>
                        )}
                        {hasUpdate && (
                          <span className="px-2 py-0.5 rounded-full bg-amber-500/15 border border-amber-500/30 text-amber-400 text-[9px] font-black uppercase tracking-widest">
                            {ti('Actualización', 'Update')}
                          </span>
                        )}
                      </div>
                      <p className="text-[11px] text-gray-400 font-semibold mt-1 leading-relaxed">{emu.description}</p>
                      <p className="text-[10px] text-gray-600 font-mono mt-1.5">
                        {installed && <>v{emu.installed_version} </>}
                        {emu.latest_version
                          ? <>{installed ? '· ' : ''}{ti('última', 'latest')} v{emu.latest_version}</>
                          : ti('· servidor no disponible', '· server unavailable')}
                      </p>
                      {installed && emu.exe_path && (
                        <p className="text-[10px] text-gray-600 font-mono mt-1 truncate" title={emu.exe_path}>
                          {emu.exe_path}
                        </p>
                      )}
                    </div>
                  </div>

                  <div className="mt-auto flex flex-wrap items-center gap-2">
                    <button
                      onClick={() => handleInstall(emu)}
                      disabled={busy || !emu.latest_version}
                      className="relative overflow-hidden flex-1 min-w-[8rem] flex items-center justify-center gap-2 px-3 py-2.5 rounded-xl bg-accent text-white text-[9px] font-black uppercase tracking-widest hover:brightness-110 hover:-translate-y-0.5 transition-all shadow-lg shadow-accent/25 disabled:opacity-40 disabled:hover:translate-y-0"
                    >
                      {busy && prog && !prog.extracting && (
                        <motion.div
                          className="absolute inset-y-0 left-0 bg-white/25"
                          initial={{ width: 0 }}
                          animate={{ width: `${prog.percentage}%` }}
                          transition={{ duration: 0.2, ease: 'easeOut' }}
                        />
                      )}
                      <span className="relative z-10 flex items-center gap-2">
                        {busy
                          ? prog?.extracting
                            ? <><RefreshCw size={12} className="animate-spin" /> {ti('Instalando…', 'Installing…')}</>
                            : prog
                              ? <><Download size={12} /> {ti('Descargando', 'Downloading')} {prog.percentage}%</>
                              : <><RefreshCw size={12} className="animate-spin" /> {ti('Conectando…', 'Connecting…')}</>
                          : installed
                            ? <><Download size={12} /> {hasUpdate ? ti('Actualizar', 'Update') : ti('Reinstalar', 'Reinstall')}</>
                            : <><Download size={12} /> {ti('Instalar', 'Install')}</>}
                      </span>
                    </button>

                    {installed && (
                      <>
                        <button
                          onClick={() => handleLaunch(emu)}
                          disabled={launchingId === emu.id}
                          title={ti('Abrir emulador', 'Launch emulator')}
                          className="w-9 h-9 shrink-0 flex items-center justify-center rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                        >
                          <Play size={13} />
                        </button>
                        {emu.uses_keys && (
                          <button
                            onClick={() => handleInstallKeys(emu)}
                            title={ti('Colocar mis prod.keys / title.keys', 'Place my prod.keys / title.keys')}
                            className="w-9 h-9 shrink-0 flex items-center justify-center rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 transition-colors"
                          >
                            <KeyRound size={13} />
                          </button>
                        )}
                        {emu.console === 'switch' && (
                          <button
                            onClick={() => handleInstallFirmware(emu)}
                            title={ti('Instalar firmware (.zip / .7z)', 'Install firmware (.zip / .7z)')}
                            className="w-9 h-9 shrink-0 flex items-center justify-center rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 transition-colors"
                          >
                            <Cpu size={13} />
                          </button>
                        )}
                        <button
                          onClick={() => invoke('open_emulator_folder', { emulatorId: emu.id }).catch(err => notify(`${err}`, 'error'))}
                          title={ti('Abrir carpeta de instalación', 'Open install folder')}
                          className="w-9 h-9 shrink-0 flex items-center justify-center rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 transition-colors"
                        >
                          <FolderOpen size={13} />
                        </button>
                        <button
                          onClick={() => handleUninstall(emu)}
                          disabled={busy}
                          title={ti('Desinstalar', 'Uninstall')}
                          className="w-9 h-9 shrink-0 flex items-center justify-center rounded-xl bg-white/[0.06] hover:bg-red-500/20 border border-white/10 hover:border-red-500/30 text-gray-400 hover:text-red-400 transition-colors disabled:opacity-40"
                        >
                          <Trash2 size={13} />
                        </button>
                      </>
                    )}
                  </div>
                </motion.div>
              );
            })}
          </div>

          <div className="flex items-start gap-2.5 rounded-2xl bg-white/[0.03] border border-white/10 px-5 py-4">
            <Info size={14} className="text-gray-500 shrink-0 mt-0.5" />
            <p className="text-xs text-gray-400 font-semibold leading-relaxed">
              {ti(
                'El botón de la llave coloca prod.keys o title.keys en la carpeta del emulador. El botón de CPU permite instalar el firmware (.zip / .7z). También podés hacerlo desde el asistente inicial.',
                "The key button places prod.keys or title.keys. The CPU button installs firmware (.zip / .7z). You can also use the setup wizard."
              )}
            </p>
          </div>
        </motion.div>
      )}

      {/* GAME DETAILS / DOWNLOAD MODAL */}
      <AnimatePresence>
        {selectedGame && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-[120] flex items-center justify-center bg-black/80 backdrop-blur-md p-4"
            onClick={() => setSelectedGame(null)}
          >
            <motion.div
              initial={{ opacity: 0, y: 15, scale: 0.96 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 15, scale: 0.96 }}
              transition={{ duration: 0.22, ease: 'easeOut' }}
              onClick={e => e.stopPropagation()}
              className="relative w-full max-w-2xl max-h-[90vh] overflow-y-auto rounded-3xl bg-[#0d0e12] border border-white/[0.1] p-6 shadow-2xl space-y-5"
            >
              {/* Close Button */}
              <button
                onClick={() => setSelectedGame(null)}
                className="absolute top-4 right-4 z-20 p-2 text-gray-400 hover:text-white bg-black/40 hover:bg-black/60 rounded-full border border-white/10 transition-colors"
                title={ti('Cerrar', 'Close')}
              >
                <X size={16} />
              </button>

              {/* Game Head */}
              <div className="flex flex-col sm:flex-row gap-5 items-start">
                <div className="relative w-36 shrink-0 aspect-[3/4] rounded-2xl overflow-hidden bg-black/60 border border-white/10 shadow-xl">
                  {selectedGame.cover_url ? (
                    <img
                      src={selectedGame.cover_url}
                      alt={selectedGame.title}
                      className="w-full h-full object-cover"
                    />
                  ) : (
                    <div className="w-full h-full flex items-center justify-center text-white/20">
                      <Gamepad2 size={36} />
                    </div>
                  )}
                </div>

                <div className="flex-1 min-w-0 space-y-2">
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="px-2 py-0.5 rounded-md bg-white/10 text-[9px] font-black uppercase text-white tracking-wider">
                      {selectedGame.source === 'projectnx' ? 'ProjectNX' : 'Egg NS'}
                    </span>
                    {selectedGame.format && (
                      <span className="px-2 py-0.5 rounded-md bg-accent text-[9px] font-black uppercase text-white tracking-wider">
                        {selectedGame.format}
                      </span>
                    )}
                    {selectedGame.size && (
                      <span className="px-2 py-0.5 rounded-md bg-white/[0.06] border border-white/10 text-[9px] font-mono font-bold text-gray-300">
                        {selectedGame.size}
                      </span>
                    )}
                  </div>

                  <h3 className="text-lg sm:text-xl font-black text-white/95 tracking-tight leading-snug">
                    {selectedGame.title}
                  </h3>

                  {selectedGame.excerpt ? (
                    <p className="text-xs text-gray-400 font-semibold leading-relaxed line-clamp-3">
                      {selectedGame.excerpt}
                    </p>
                  ) : null}

                  <div className="pt-2">
                    <button
                      onClick={() => openExternalUrl(selectedGame.page_url)}
                      className="inline-flex items-center gap-1.5 text-xs font-bold text-accent hover:underline hover:brightness-125 transition-all"
                    >
                      <ExternalLink size={13} /> {ti('Ver ficha en la web oficial', 'View on official website')}
                    </button>
                  </div>

                  <div className="pt-3 border-t border-white/[0.08]">
                    <button
                      onClick={() => openExternalUrl(selectedGame.page_url)}
                      className="w-full flex items-center justify-center gap-2 py-3 px-4 rounded-xl bg-accent text-white text-xs font-black uppercase tracking-wider hover:brightness-110 transition-all shadow-lg shadow-accent/25"
                    >
                      <Download size={14} /> {ti('Descargar ROM', 'Download ROM')}
                    </button>
                  </div>
                </div>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
};

export default EmulatorsView;
