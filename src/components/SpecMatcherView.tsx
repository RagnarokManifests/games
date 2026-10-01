import React, { useState, useEffect, useMemo, useRef } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { invoke } from '@tauri-apps/api/tauri';
import { useLanguage, useTranslateInline } from '../App';
import { useNotify } from './NotificationProvider';
import {
  Cpu,
  MemoryStick,
  Monitor,
  HardDrive,
  CheckCircle,
  XCircle,
  AlertTriangle,
  RefreshCw,
  Gamepad2,
  Zap,
  Search,
  ChevronDown,
  X,
  Sparkles,
  Check,
} from 'lucide-react';

interface Game {
  id: string;
  name?: string;
  status?: string;
}

export interface SystemSpecs {
  cpu_name: string;
  /** Physical cores. `cpu_threads` is the logical-processor count. */
  cpu_cores: number;
  cpu_threads: number;
  ram_gb: number;
  gpu_name: string;
  /** Free space on `disk_drive` — the Steam library drive with the most room. */
  disk_space_gb: number;
  disk_drive: string;
}

export interface GameRequirement {
  /** Which part of the PC the row is about; the label is translated, this is not. */
  kind: 'cpu' | 'ram' | 'gpu' | 'disk';
  component: string;
  minimum: string;
  recommended: string;
  userValue: string;
  meetsMinimum: boolean;
  meetsRecommended: boolean;
  /**
   * Whether both sides were actually scored.
   *
   * Without this the "unknown" case was indistinguishable from a pass:
   * `meetsMinimum` fell back to `true` — "benefit of the doubt" — so a GPU
   * missing from the table produced a green "Mínimo OK". And plenty are
   * missing: the table has no RTX 50-series, no Intel Arc or Iris, no Vega,
   * and no Laptop/Mobile variants, which is most laptops and every current
   * card. Telling someone their PC clears the bar when nothing was compared
   * costs them an 80 GB download.
   */
  comparable: boolean;
}

interface SpecMatcherViewProps {
  games: Game[];
  steamPath: string;
}

const normalizeSearch = (s: string): string =>
  s
    .toLowerCase()
    .normalize('NFD')
    .replace(/[\u0300-\u036f]/g, '')
    .replace(/[^a-z0-9\s]/g, '')
    .replace(/\s+/g, ' ')
    .trim();

// GPU performance scores (approx. 1080p rasterization, normalized 0-100)
// Higher = faster. Used to compare user GPU vs game requirements.
const GPU_SCORES: { keywords: string[]; score: number }[] = [
  // RTX 50 series. Absent entirely until now, so every current NVIDIA card
  // scored null and fell through to "benefit of the doubt".
  { keywords: ['rtx 5090'],                          score: 145 },
  { keywords: ['rtx 5080'],                          score: 112 },
  { keywords: ['rtx 5070 ti'],                       score: 96  },
  { keywords: ['rtx 5070'],                          score: 84  },
  { keywords: ['rtx 5060 ti'],                       score: 62  },
  { keywords: ['rtx 5060'],                          score: 52  },
  // AMD RDNA4 and the RX 7000 entries that were missing.
  { keywords: ['rx 9070 xt'],                        score: 90  },
  { keywords: ['rx 9070'],                           score: 82  },
  { keywords: ['rx 9060 xt'],                        score: 58  },
  { keywords: ['rx 7700 xt'],                        score: 58  },
  { keywords: ['rx 7600 xt'],                        score: 45  },
  { keywords: ['rx 7600'],                           score: 43  },
  // Intel Arc — a whole vendor the table did not know.
  { keywords: ['arc b580'],                          score: 47  },
  { keywords: ['arc b570'],                          score: 41  },
  { keywords: ['arc a770'],                          score: 44  },
  { keywords: ['arc a750'],                          score: 40  },
  { keywords: ['arc a380'],                          score: 18  },
  // Integrated graphics. These are the ones most likely to be told they
  // "meet the minimum" when they emphatically do not.
  { keywords: ['radeon 890m'],                       score: 26  },
  { keywords: ['radeon 780m'],                       score: 22  },
  { keywords: ['radeon 760m'],                       score: 17  },
  { keywords: ['radeon 680m'],                       score: 18  },
  { keywords: ['arc 140v'],                          score: 20  },
  { keywords: ['arc 8-core', 'intel arc graphics'],  score: 19  },
  { keywords: ['iris xe'],                           score: 11  },
  { keywords: ['iris plus'],                         score: 8   },
  { keywords: ['uhd graphics 7', 'uhd graphics 6'],  score: 5   },
  { keywords: ['uhd graphics'],                      score: 4   },
  { keywords: ['hd graphics'],                       score: 3   },
  { keywords: ['vega 11'],                           score: 10  },
  { keywords: ['vega 8'],                            score: 8   },
  { keywords: ['rx vega 64'],                        score: 38  },
  { keywords: ['rx vega 56'],                        score: 34  },
  { keywords: ['rtx 4090'],                          score: 100 },
  { keywords: ['rtx 4080 super'],                    score: 92  },
  { keywords: ['rtx 4080'],                          score: 88  },
  { keywords: ['rtx 4070 ti super'],                 score: 84  },
  { keywords: ['rx 7900 xtx'],                       score: 86  },
  { keywords: ['rx 7900 xt'],                        score: 79  },
  { keywords: ['rtx 4070 ti'],                       score: 80  },
  { keywords: ['rtx 4070 super'],                    score: 77  },
  { keywords: ['rx 7800 xt'],                        score: 65  },
  { keywords: ['rtx 3080 ti'],                       score: 75  },
  { keywords: ['rtx 3080 12'],                       score: 72  },
  { keywords: ['rtx 3080'],                          score: 70  },
  { keywords: ['rtx 4070'],                          score: 68  },
  { keywords: ['rtx 3070 ti'],                       score: 65  },
  { keywords: ['rx 6800 xt'],                        score: 63  },
  { keywords: ['rtx 3070'],                          score: 61  },
  { keywords: ['rx 6800'],                           score: 59  },
  { keywords: ['rtx 2080 ti'],                       score: 65  },
  { keywords: ['rtx 2080 super', 'rtx 2080s'],       score: 57  },
  { keywords: ['rtx 2080'],                          score: 54  },
  { keywords: ['rtx 4060 ti'],                       score: 56  },
  { keywords: ['rtx 3060 ti'],                       score: 55  },
  { keywords: ['rtx 2070 super', 'rtx 2070s'],       score: 52  },
  { keywords: ['rtx 2070'],                          score: 49  },
  { keywords: ['rx 6700 xt'],                        score: 50  },
  { keywords: ['rx 5700 xt'],                        score: 47  },
  { keywords: ['rx 6700'],                           score: 46  },
  { keywords: ['rtx 4060'],                          score: 46  },
  { keywords: ['rtx 3060'],                          score: 48  },
  { keywords: ['rtx 2060 super', 'rtx 2060s'],       score: 44  },
  { keywords: ['gtx 1080 ti'],                       score: 48  },
  { keywords: ['rx 5700'],                           score: 44  },
  { keywords: ['rtx 2060'],                          score: 41  },
  { keywords: ['rx 6600 xt'],                        score: 43  },
  { keywords: ['gtx 1080'],                          score: 40  },
  { keywords: ['rx 6600'],                           score: 40  },
  { keywords: ['rx 5600 xt'],                        score: 36  },
  { keywords: ['gtx 1070 ti'],                       score: 36  },
  { keywords: ['gtx 1070'],                          score: 34  },
  { keywords: ['gtx 1660 super', 'gtx 1660s'],       score: 33  },
  { keywords: ['gtx 1660 ti'],                       score: 32  },
  { keywords: ['gtx 1660'],                          score: 29  },
  { keywords: ['rx 5500 xt'],                        score: 27  },
  { keywords: ['gtx 1650 super', 'gtx 1650s'],       score: 27  },
  { keywords: ['gtx 1060 6gb', 'gtx 1060 6'],        score: 24  },
  { keywords: ['rx 580'],                            score: 23  },
  { keywords: ['gtx 1060'],                          score: 22  },
  { keywords: ['rx 570'],                            score: 19  },
  { keywords: ['gtx 1650'],                          score: 21  },
  { keywords: ['gtx 970'],                           score: 19  },
  { keywords: ['rx 470'],                            score: 17  },
  { keywords: ['gtx 1050 ti'],                       score: 17  },
  { keywords: ['gtx 1050'],                          score: 13  },
  { keywords: ['gtx 960'],                           score: 13  },
  { keywords: ['rx 560'],                            score: 10  },
];

// CPU performance scores, same 0-100 scale and same lookup style as
// GPU_SCORES above (approximate gaming/single-thread-weighted performance,
// higher = faster). Before this existed the Specs tab's CPU row was purely
// decorative: `meetsMinimum` was hardcoded `true` and `meetsRecommended` was
// a bare `cores >= 4`, so an ancient 4-core "met the recommended spec" of
// every game on the store while the required processor was never even read.
//
// Keyed on the model number the way Steam's own requirement strings write it
// ("Intel Core i5-8400", "AMD Ryzen 5 1600X"), which is also how Windows
// reports the installed CPU — so the same table matches both sides.
const CPU_SCORES: { keywords: string[]; score: number }[] = [
  // Current high end
  { keywords: ['i9-14900', 'i9 14900'],                score: 100 },
  { keywords: ['ryzen 9 7950'],                        score: 98  },
  { keywords: ['i9-13900', 'i9 13900'],                score: 96  },
  { keywords: ['ryzen 7 7800x3d'],                     score: 94  },
  { keywords: ['ryzen 9 7900'],                        score: 92  },
  { keywords: ['i7-14700', 'i7 14700'],                score: 90  },
  { keywords: ['i7-13700', 'i7 13700'],                score: 87  },
  { keywords: ['ryzen 7 7700'],                        score: 85  },
  { keywords: ['i5-14600', 'i5 14600'],                score: 83  },
  { keywords: ['ryzen 9 5950'],                        score: 82  },
  { keywords: ['i5-13600', 'i5 13600'],                score: 80  },
  { keywords: ['ryzen 7 5800x3d'],                     score: 79  },
  { keywords: ['ryzen 9 5900'],                        score: 77  },
  { keywords: ['i9-12900', 'i9 12900'],                score: 76  },
  { keywords: ['ryzen 5 7600'],                        score: 75  },
  { keywords: ['i7-12700', 'i7 12700'],                score: 72  },
  { keywords: ['ryzen 7 5800'],                        score: 70  },
  { keywords: ['i5-12600', 'i5 12600'],                score: 68  },
  { keywords: ['i9-11900', 'i9 11900'],                score: 66  },
  { keywords: ['ryzen 5 5600'],                        score: 64  },
  { keywords: ['i5-12400', 'i5 12400'],                score: 62  },
  { keywords: ['ryzen 9 3900'],                        score: 60  },
  { keywords: ['i7-11700', 'i7 11700'],                score: 59  },
  { keywords: ['ryzen 7 3800'],                        score: 56  },
  { keywords: ['i7-10700', 'i7 10700'],                score: 55  },
  { keywords: ['ryzen 7 3700'],                        score: 54  },
  { keywords: ['i5-11400', 'i5 11400'],                score: 52  },
  { keywords: ['i5-10600', 'i5 10600'],                score: 50  },
  { keywords: ['ryzen 5 3600'],                        score: 48  },
  { keywords: ['i7-9700', 'i7 9700'],                  score: 47  },
  { keywords: ['i5-10400', 'i5 10400'],                score: 45  },
  { keywords: ['ryzen 5 2600'],                        score: 40  },
  { keywords: ['i7-8700', 'i7 8700'],                  score: 42  },
  { keywords: ['i5-9400', 'i5 9400'],                  score: 39  },
  { keywords: ['ryzen 7 2700'],                        score: 38  },
  { keywords: ['i5-8400', 'i5 8400'],                  score: 35  },
  { keywords: ['ryzen 5 1600'],                        score: 32  },
  { keywords: ['ryzen 7 1700'],                        score: 31  },
  { keywords: ['i7-7700', 'i7 7700'],                  score: 30  },
  { keywords: ['i5-7600', 'i5 7600'],                  score: 26  },
  { keywords: ['ryzen 3 1200'],                        score: 22  },
  { keywords: ['i5-6600', 'i5 6600'],                  score: 24  },
  { keywords: ['i7-6700', 'i7 6700'],                  score: 28  },
  { keywords: ['i5-4590', 'i5 4590'],                  score: 18  },
  { keywords: ['i5-4460', 'i5 4460'],                  score: 17  },
  { keywords: ['i7-4790', 'i7 4790'],                  score: 22  },
  { keywords: ['i7-3770', 'i7 3770'],                  score: 19  },
  { keywords: ['i5-3470', 'i5 3470'],                  score: 14  },
  { keywords: ['i5-2500', 'i5 2500'],                  score: 12  },
  { keywords: ['fx-8350', 'fx 8350'],                  score: 11  },
  { keywords: ['fx-6300', 'fx 6300'],                  score: 8   },
  { keywords: ['core 2 duo'],                          score: 3   },
];

// Shared by getGpuScore/getCpuScore: longest keyword wins, so a more
// specific model ("rtx 2080 super", "ryzen 7 5800x3d") is preferred over the
// shorter prefix it contains.
function scoreFromTable(name: string, table: { keywords: string[]; score: number }[]): number | null {
  if (!name) return null;
  const lower = name.toLowerCase();
  const sorted = [...table].sort((a, b) =>
    Math.max(...b.keywords.map(k => k.length)) - Math.max(...a.keywords.map(k => k.length))
  );
  for (const entry of sorted) {
    if (entry.keywords.some(k => lower.includes(k))) return entry.score;
  }
  return null;
}

function getGpuScore(name: string): number | null {
  const base = scoreFromTable(name, GPU_SCORES);
  if (base === null) return null;

  // A laptop part is not the desktop part it shares a name with.
  //
  // Windows reports these as "NVIDIA GeForce RTX 4060 Laptop GPU", which
  // matches the desktop "rtx 4060" row and scored identically — so a thin
  // laptop was credited with a desktop card's performance. The gap is roughly
  // a tier and a half once power limits are accounted for; 0.7 is the rough
  // middle of it. Approximate on purpose: better a close estimate than a
  // number that is confidently wrong in the user's favour.
  const lower = name.toLowerCase();
  const isMobile = /\b(laptop|mobile|max-q)\b/.test(lower);
  return isMobile ? Math.round(base * 0.7) : base;
}

function getCpuScore(name: string): number | null {
  return scoreFromTable(name, CPU_SCORES);
}

const extractTextFromHTML = (html: string): string => {
  if (!html) return '';
  // Steam separates requirements with <li> and <br>, and textContent alone
  // runs them into a single line. Everything below reads line by line, so on
  // a game like Resident Evil Requiem the "processor" lookup came back with
  // the entire requirements text.
  const withBreaks = html.replace(/<br\s*\/?>/gi, '\n').replace(/<\/(li|p|ul|div)>/gi, '\n');
  const doc = new DOMParser().parseFromString(withBreaks, 'text/html');
  return doc.body.textContent || '';
};

const extractValue = (text: string, keywords: string[]): string => {
  const lines = text.split('\n').map(l => l.trim()).filter(Boolean);
  // A labelled line first — "Processor: Intel Core i7-8700". Steam's first
  // line is often "Requires a 64-bit processor and operating system", which
  // mentions the word in passing and used to win.
  for (const line of lines) {
    const colon = line.indexOf(':');
    if (colon <= 0) continue;
    const label = line.slice(0, colon).toLowerCase();
    const value = line.slice(colon + 1).trim();
    if (value && keywords.some(k => label.includes(k))) return value;
  }
  for (let i = 0; i < lines.length; i++) {
    const lower = lines[i].toLowerCase();
    if (keywords.some(k => lower.includes(k))) {
      if (lines[i + 1]) return lines[i + 1].trim();
      const afterColon = lines[i].split(':').slice(1).join(':').trim();
      if (afterColon) return afterColon;
    }
  }
  return '';
};

type Translate = (es: string, en: string) => string;

/**
 * A game's Steam requirements against this PC, one row per component.
 *
 * Moved out of the Specs tab so the game page can show the same comparison
 * next to the Install button, where the question actually comes up.
 */
export function compareRequirements(
  specs: SystemSpecs,
  minimumHtml: string | null | undefined,
  recommendedHtml: string | null | undefined,
  ti: Translate,
): GameRequirement[] {
  if (!minimumHtml) return [];
  const minText = extractTextFromHTML(minimumHtml);
  const recText = recommendedHtml ? extractTextFromHTML(recommendedHtml) : '';

  const reqs: GameRequirement[] = [];

  const minCpu = extractValue(minText, ['processor', 'procesador', 'cpu']);
  const recCpu = extractValue(recText, ['processor', 'procesador', 'cpu']);
  if (minCpu) {
    // Mirrors the GPU comparison below: score both sides off the same
    // table, 10% margin, and fall back to "benefit of the doubt for the
    // minimum / conservative for the recommended" when either side isn't
    // in the table. Previously this was hardcoded to always pass the
    // minimum and to treat any 4-core CPU as meeting the recommended spec.
    const userCpuScore = getCpuScore(specs.cpu_name);
    const minCpuScore = getCpuScore(minCpu);
    const recCpuScore = getCpuScore(recCpu || minCpu);

    const cpuComparable = userCpuScore !== null && minCpuScore !== null;
    const meetsMinCpu = cpuComparable
      ? userCpuScore! >= minCpuScore! * 0.90
      : true;
    const meetsRecCpu = (userCpuScore !== null && recCpuScore !== null)
      ? userCpuScore >= recCpuScore * 0.90
      : specs.cpu_cores >= 4;

    reqs.push({
      kind: 'cpu',
      comparable: cpuComparable,
      component: ti('Procesador', 'Processor'),
      minimum: minCpu,
      recommended: recCpu || minCpu,
      userValue: `${specs.cpu_name} (${specs.cpu_cores} ${ti('núcleos', 'cores')} / ${specs.cpu_threads} ${ti('hilos', 'threads')})`,
      meetsMinimum: meetsMinCpu,
      meetsRecommended: meetsRecCpu,
    });
  }

  const minRamText = extractValue(minText, ['memory', 'memoria', 'ram']);
  const recRamText = extractValue(recText, ['memory', 'memoria', 'ram']);
  if (minRamText) {
    const minRamMatch = minRamText.match(/(\d+)\s*GB/i);
    const recRamMatch = recRamText.match(/(\d+)\s*GB/i);
    const minRam = minRamMatch ? parseInt(minRamMatch[1]) : 0;
    const recRam = recRamMatch ? parseInt(recRamMatch[1]) : minRam;
    reqs.push({
      kind: 'ram',
      comparable: true, // números reales de ambos lados
      component: ti('Memoria RAM', 'RAM Memory'),
      minimum: minRam > 0 ? `${minRam} GB` : minRamText,
      recommended: recRam > 0 ? `${recRam} GB` : recRamText || minRamText,
      userValue: `${specs.ram_gb.toFixed(1)} GB`,
      meetsMinimum: minRam > 0 ? (specs.ram_gb + 2.0) >= minRam : true,
      meetsRecommended: recRam > 0 ? (specs.ram_gb + 2.0) >= recRam : true,
    });
  }

  const minGpu = extractValue(minText, ['graphics', 'gráficos', 'video', 'gpu', 'tarjeta']);
  const recGpu = extractValue(recText, ['graphics', 'gráficos', 'video', 'gpu', 'tarjeta']);
  if (minGpu) {
    const userGpuScore = getGpuScore(specs.gpu_name);
    const minGpuScore = getGpuScore(minGpu);
    const recGpuScore = getGpuScore(recGpu || minGpu);

    // If both scores are known, compare them; allow 10% margin for minimum.
    // When either side is missing from the table the row is marked
    // `comparable: false` and the UI says so, instead of quietly passing.
    const gpuComparable = userGpuScore !== null && minGpuScore !== null;
    const meetsMin = gpuComparable
      ? userGpuScore! >= minGpuScore! * 0.90
      : true;

    // Recommended needs at least 90% of required score
    const meetsRec = (userGpuScore !== null && recGpuScore !== null)
      ? userGpuScore >= recGpuScore * 0.90
      : false; // unknown → be conservative

    reqs.push({
      kind: 'gpu',
      comparable: gpuComparable,
      component: ti('Tarjeta Gráfica', 'Graphics Card'),
      minimum: minGpu,
      recommended: recGpu || minGpu,
      userValue: specs.gpu_name,
      meetsMinimum: meetsMin,
      meetsRecommended: meetsRec,
    });
  }

  const minStorageText = extractValue(minText, ['storage', 'almacenamiento', 'disco', 'espacio']);
  const recStorageText = extractValue(recText, ['storage', 'almacenamiento', 'disco', 'espacio']);
  if (minStorageText) {
    const minStorageMatch = minStorageText.match(/(\d+(?:\.\d+)?)\s*GB/i);
    const recStorageMatch = recStorageText.match(/(\d+(?:\.\d+)?)\s*GB/i);
    const minStorage = minStorageMatch ? parseFloat(minStorageMatch[1]) : 0;
    const recStorage = recStorageMatch ? parseFloat(recStorageMatch[1]) : minStorage;
    reqs.push({
      kind: 'disk',
      comparable: true, // números reales de ambos lados
      component: ti('Espacio en Disco', 'Disk Space'),
      minimum: minStorage > 0 ? `${minStorage} GB` : minStorageText,
      recommended: recStorage > 0 ? `${recStorage} GB` : recStorageText || minStorageText,
      // Names the drive: this is the Steam library drive with the most room
      // free, which is not necessarily C: (see get_system_specs).
      userValue: `${specs.disk_space_gb.toFixed(1)} GB ${ti('libres en', 'free on')} ${specs.disk_drive}`,
      meetsMinimum: minStorage > 0 ? specs.disk_space_gb >= minStorage : true,
      meetsRecommended: recStorage > 0 ? specs.disk_space_gb >= recStorage : true,
    });
  }

  return reqs;
}

/**
 * The overall answer for a set of rows.
 *
 * A verdict needs the parts that decide it. RAM and disk are real numeric
 * comparisons, but CPU and GPU come from a lookup table, and when the user's
 * card is not in it the row passed on "benefit of the doubt" — so an unknown
 * GPU produced a confident "MÍNIMO". Saying nothing is the honest answer; the
 * alternative sends someone to download 80 GB. Judged by `kind`, not by the
 * translated label, so it holds in every interface language.
 */
export function requirementsVerdict(reqs: GameRequirement[]): 'excellent' | 'good' | 'poor' | 'unknown' {
  if (reqs.length === 0) return 'unknown';
  const decisive = reqs.filter(r => r.kind === 'gpu' || r.kind === 'cpu');
  if (decisive.length > 0 && decisive.some(r => !r.comparable)) return 'unknown';

  const allMin = reqs.every(r => r.meetsMinimum);
  const allRec = reqs.every(r => r.meetsRecommended);
  if (allRec) return 'excellent';
  if (allMin) return 'good';
  return 'poor';
}


const SpecMatcherView = ({ games, steamPath }: SpecMatcherViewProps) => {
  const { lang, t } = useLanguage();
  const ti = useTranslateInline();
  const { notify } = useNotify();
  const [specs, setSpecs] = useState<SystemSpecs | null>(null);
  const [loading, setLoading] = useState(true);
  const [selectedGame, setSelectedGame] = useState<Game | null>(null);
  const [gameMedia, setGameMedia] = useState<any>(null);
  const [gameLoading, setGameLoading] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  // Re-runs if steamPath resolves after mount — the free-space figure now
  // depends on which drives Steam's libraries live on, so specs read before
  // the path was known would fall back to C:.
  useEffect(() => {
    loadSpecs();
  }, [steamPath]);

  useEffect(() => {
    if (!dropdownOpen) return;
    const handler = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setDropdownOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [dropdownOpen]);

  const loadSpecs = async () => {
    setLoading(true);
    try {
      const s = await invoke<SystemSpecs>('get_system_specs', { steamPath });
      setSpecs(s);
    } catch (e) {
      console.error('Failed to load system specs:', e);
    } finally {
      setLoading(false);
    }
  };

  const loadGameRequirements = async (game: Game) => {
    setGameLoading(true);
    setSelectedGame(game);
    setGameMedia(null);
    setDropdownOpen(false);
    setSearchQuery('');
    try {
      const media = await invoke<any>('fetch_steam_data', { appId: game.id });
      setGameMedia(media);
    } catch (e) {
      // A console.error is invisible to the person using the app. With
      // gameMedia left null the view fell back to "Esperando Selección de
      // Juego" — right after they selected a game, whose name was on screen —
      // so the only reading available was "my click did nothing", and they
      // clicked again.
      console.error('Failed to fetch game data:', e);
      notify(
        `${ti('No se pudieron cargar los requisitos de', 'Could not load requirements for')} ${game.name}: ${e}`,
        'error'
      );
    } finally {
      setGameLoading(false);
    }
  };

  const filteredGames = useMemo(() => {
    const q = normalizeSearch(searchQuery);
    if (!q) return games.slice(0, 50);
    return games.filter(g => {
      const rawName = g.name?.trim() ?? '';
      if (g.id.includes(q)) return true;
      if (!rawName || rawName.startsWith('Game ID:')) {
        return rawName.toLowerCase().includes(q) || g.id.includes(q);
      }
      return normalizeSearch(rawName).includes(q);
    }).slice(0, 50);
  }, [games, searchQuery]);

  const reqs = useMemo(
    () => (gameMedia?.minimum_reqs && specs ? compareRequirements(specs, gameMedia.minimum_reqs, gameMedia.recommended_reqs, ti) : []),
    [gameMedia, specs]
  );

  const overallStatus = useMemo(() => requirementsVerdict(reqs), [reqs]);

  // GPU ratio: user score / recommended-requirement score
  const gpuRatio = useMemo(() => {
    if (!specs || !gameMedia) return null;
    const userScore = getGpuScore(specs.gpu_name);
    if (!userScore) return null;
    const minTxt = extractTextFromHTML(gameMedia.minimum_reqs || '');
    const recTxt = gameMedia.recommended_reqs ? extractTextFromHTML(gameMedia.recommended_reqs) : '';
    const gpuKeywords = ['graphics', 'gráficos', 'video', 'gpu', 'tarjeta'];
    const refGpu = extractValue(recTxt, gpuKeywords) || extractValue(minTxt, gpuKeywords);
    const refScore = getGpuScore(refGpu);
    if (!refScore) return null;
    return userScore / refScore;
  }, [specs, gameMedia]);

  const performancePrediction = useMemo(() => {
    if (!overallStatus || overallStatus === 'unknown') return null;

    const ratio = gpuRatio;

    // Ratio-based prediction (most accurate when both GPUs are in our DB)
    if (ratio !== null) {
      if (ratio >= 1.5)
        return { fps: '60+ FPS', resolution: '4K',          settings: 'Ultra',         color: 'text-emerald-400', bg: 'bg-emerald-500/10', border: 'border-emerald-500/20' };
      if (ratio >= 1.1)
        return { fps: '60+ FPS', resolution: '1440p',        settings: 'High / Ultra',   color: 'text-emerald-400', bg: 'bg-emerald-500/10', border: 'border-emerald-500/20' };
      if (ratio >= 0.90)
        return { fps: '60 FPS',  resolution: '1080p',        settings: 'High',           color: 'text-emerald-400', bg: 'bg-emerald-500/10', border: 'border-emerald-500/20' };
      if (ratio >= 0.70)
        return { fps: '45-60 FPS', resolution: '1080p',      settings: 'Medium / High',  color: 'text-amber-400',   bg: 'bg-amber-500/10',   border: 'border-amber-500/20'   };
      if (ratio >= 0.50)
        return { fps: '30-45 FPS', resolution: '1080p',      settings: 'Medio',          color: 'text-amber-400',   bg: 'bg-amber-500/10',   border: 'border-amber-500/20'   };
      if (ratio >= 0.35)
        return { fps: '20-30 FPS', resolution: '720p / 1080p', settings: 'Bajo',         color: 'text-red-400',     bg: 'bg-red-500/10',     border: 'border-red-500/20'     };
      return   { fps: '< 20 FPS', resolution: '720p',         settings: 'Muy Bajo',      color: 'text-red-400',     bg: 'bg-red-500/10',     border: 'border-red-500/20'     };
    }

    // Fallback when GPU not in DB
    switch (overallStatus) {
      case 'excellent':
        return { fps: '60+ FPS',   resolution: '1440p', settings: 'High / Ultra',  color: 'text-emerald-400', bg: 'bg-emerald-500/10', border: 'border-emerald-500/20' };
      case 'good':
        return { fps: '30-60 FPS', resolution: '1080p', settings: 'Medium / High', color: 'text-amber-400',   bg: 'bg-amber-500/10',   border: 'border-amber-500/20'   };
      case 'poor':
        return { fps: '< 30 FPS',  resolution: '720p',  settings: 'Bajo',          color: 'text-red-400',     bg: 'bg-red-500/10',     border: 'border-red-500/20'     };
      default:
        return null;
    }
  }, [overallStatus, gpuRatio]);

  const displayName = (g: Game) => {
    const raw = g.name?.trim() ?? '';
    return raw && !raw.startsWith('Game ID:') ? raw : `App ${g.id}`;
  };

  return (
    <div className="max-w-5xl space-y-8 animate-in fade-in slide-in-from-bottom-6 duration-1000 px-4 pb-12">
      
      {/* Premium Header */}
      <div className="relative overflow-hidden rounded-[2rem] border border-white/10 shadow-2xl p-1 bg-gradient-to-r from-cyan-500/10 via-blue-600/5 to-purple-500/10">
        <div className="absolute inset-0 bg-[#0e0f18]/95 z-0" />
        <div className="absolute -right-20 -top-20 w-72 h-72 bg-cyan-500/10 blur-[80px] rounded-full pointer-events-none" />
        <div className="absolute -left-20 -bottom-20 w-72 h-72 bg-purple-600/10 blur-[80px] rounded-full pointer-events-none" />
        <div
          className="absolute inset-0 opacity-[0.03] pointer-events-none"
          style={{ backgroundImage: 'linear-gradient(rgba(255,255,255,0.6) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.6) 1px, transparent 1px)', backgroundSize: '28px 28px' }}
        />

        <div className="relative z-10 p-8 flex flex-col md:flex-row items-center justify-between gap-6">
          <div className="flex items-center gap-6">
            <div className="relative w-16 h-16 shrink-0 flex items-center justify-center">
              <motion.div
                className="absolute -inset-1.5 rounded-full border border-cyan-400/40"
                style={{ borderTopColor: 'transparent', borderLeftColor: 'transparent' }}
                animate={{ rotate: 360 }}
                transition={{ repeat: Infinity, duration: 5, ease: 'linear' }}
              />
              <div className="w-16 h-16 rounded-2xl bg-gradient-to-br from-cyan-400 via-blue-500 to-indigo-600 flex items-center justify-center shadow-lg shadow-cyan-500/35 transition-transform duration-500 hover:scale-105 relative z-10">
                <Cpu size={30} className="text-white" />
              </div>
            </div>
            <div className="space-y-1">
              <h3 className="font-black text-2xl text-transparent bg-clip-text bg-gradient-to-r from-white via-white to-gray-400 tracking-tight flex items-center gap-2">
                Spec Matcher <Sparkles size={16} className="text-cyan-400 animate-pulse" />
              </h3>
              <p className="text-xs text-gray-400 font-medium leading-relaxed">
                {ti('Diagnóstico avanzado de hardware vs requisitos oficiales de la tienda Steam.', 'Advanced hardware diagnostics vs official Steam store requirements.')}
              </p>
            </div>
          </div>
          <button
            onClick={loadSpecs}
            disabled={loading}
            className="flex items-center gap-2 px-5 py-3 rounded-xl bg-white/5 hover:bg-white/10 border border-white/8 hover:border-white/15 text-xs text-gray-300 font-black uppercase tracking-wider transition-all disabled:opacity-50 hover:-translate-y-0.5 active:translate-y-0 shadow-lg"
          >
            <RefreshCw size={13} className={loading ? 'animate-spin' : ''} />
            {loading ? ti('Analizando...', 'Analyzing...') : ti('Recargar Specs', 'Reload Specs')}
          </button>
        </div>
      </div>

      {/* System Specs Overview Grid */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-5">
        
        {/* CPU */}
        <motion.div 
          whileHover={{ y: -5, borderColor: 'rgba(34,211,238,0.3)' }}
          className="relative overflow-hidden bg-[#0e0f18]/80 backdrop-blur-2xl border border-white/5 rounded-3xl p-6 shadow-2xl transition-all duration-500 group"
        >
          <div className="absolute -right-8 -top-8 w-24 h-24 bg-cyan-500/5 blur-[35px] rounded-full group-hover:scale-125 transition-transform duration-500" />
          <div className="relative z-10 flex flex-col justify-between h-full">
            <div className="flex justify-between items-start">
              <div className="w-11 h-11 rounded-2xl bg-cyan-500/10 border border-cyan-500/20 flex items-center justify-center mb-4 text-cyan-400 group-hover:scale-110 transition-transform">
                <Cpu size={20} />
              </div>
              <span className="text-[8px] font-black uppercase tracking-[0.25em] text-cyan-400/70 bg-cyan-500/10 px-2 py-0.5 rounded-md">PROCESSOR</span>
            </div>
            <div>
              <p className="text-[10px] font-black text-gray-500 uppercase tracking-widest mb-1.5">{ti('Procesador', 'Processor')}</p>
              {loading ? (
                <div className="h-5 w-3/4 bg-white/5 rounded animate-pulse" />
              ) : (
                <p className="text-sm font-black text-white truncate drop-shadow" title={specs?.cpu_name}>
                  {specs?.cpu_name || ti('Desconocido', 'Unknown')}
                </p>
              )}
              {!loading && specs && (
                <p className="text-[10px] text-cyan-400 font-bold uppercase tracking-wider mt-1.5 flex items-center gap-1.5">
                  <span className="w-1.5 h-1.5 rounded-full bg-cyan-400 animate-pulse" />
                  {specs.cpu_cores} {ti('núcleos', 'cores')}
                </p>
              )}
            </div>
          </div>
        </motion.div>

        {/* RAM */}
        <motion.div 
          whileHover={{ y: -5, borderColor: 'rgba(168,85,247,0.3)' }}
          className="relative overflow-hidden bg-[#0e0f18]/80 backdrop-blur-2xl border border-white/5 rounded-3xl p-6 shadow-2xl transition-all duration-500 group"
        >
          <div className="absolute -right-8 -top-8 w-24 h-24 bg-purple-500/5 blur-[35px] rounded-full group-hover:scale-125 transition-transform duration-500" />
          <div className="relative z-10 flex flex-col justify-between h-full">
            <div className="flex justify-between items-start">
              <div className="w-11 h-11 rounded-2xl bg-purple-500/10 border border-purple-500/20 flex items-center justify-center mb-4 text-purple-400 group-hover:scale-110 transition-transform">
                <MemoryStick size={20} />
              </div>
              <span className="text-[8px] font-black uppercase tracking-[0.25em] text-purple-400/70 bg-purple-500/10 px-2 py-0.5 rounded-md">MEMORY</span>
            </div>
            <div>
              <p className="text-[10px] font-black text-gray-500 uppercase tracking-widest mb-1.5">{ti('Memoria RAM', 'RAM Memory')}</p>
              {loading ? (
                <div className="h-5 w-1/2 bg-white/5 rounded animate-pulse" />
              ) : (
                <p className="text-sm font-black text-white truncate drop-shadow">
                  {specs?.ram_gb ? `${specs.ram_gb.toFixed(1)} GB` : ti('Desconocido', 'Unknown')}
                </p>
              )}
              {!loading && specs && (
                <p className="text-[10px] text-purple-400 font-bold uppercase tracking-wider mt-1.5 flex items-center gap-1.5">
                  <span className="w-1.5 h-1.5 rounded-full bg-purple-400" />
                  {ti('Estado Óptimo', 'Optimal State')}
                </p>
              )}
            </div>
          </div>
        </motion.div>

        {/* GPU */}
        <motion.div 
          whileHover={{ y: -5, borderColor: 'rgba(16,185,129,0.3)' }}
          className="relative overflow-hidden bg-[#0e0f18]/80 backdrop-blur-2xl border border-white/5 rounded-3xl p-6 shadow-2xl transition-all duration-500 group"
        >
          <div className="absolute -right-8 -top-8 w-24 h-24 bg-emerald-500/5 blur-[35px] rounded-full group-hover:scale-125 transition-transform duration-500" />
          <div className="relative z-10 flex flex-col justify-between h-full">
            <div className="flex justify-between items-start">
              <div className="w-11 h-11 rounded-2xl bg-emerald-500/10 border border-emerald-500/20 flex items-center justify-center mb-4 text-emerald-400 group-hover:scale-110 transition-transform">
                <Monitor size={20} />
              </div>
              <span className="text-[8px] font-black uppercase tracking-[0.25em] text-emerald-400/70 bg-emerald-500/10 px-2 py-0.5 rounded-md">GRAPHICS</span>
            </div>
            <div>
              <p className="text-[10px] font-black text-gray-500 uppercase tracking-widest mb-1.5">{ti('GPU / Gráficos', 'GPU / Graphics')}</p>
              {loading ? (
                <div className="h-5 w-3/4 bg-white/5 rounded animate-pulse" />
              ) : (
                <p className="text-sm font-black text-white truncate drop-shadow" title={specs?.gpu_name}>
                  {specs?.gpu_name || ti('Desconocido', 'Unknown')}
                </p>
              )}
              {!loading && specs && (
                <p className="text-[10px] text-emerald-400 font-bold uppercase tracking-wider mt-1.5 flex items-center gap-1.5">
                  <span className="w-1.5 h-1.5 rounded-full bg-emerald-400" />
                  {ti('Soporte DirectX 12', 'DirectX 12 Support')}
                </p>
              )}
            </div>
          </div>
        </motion.div>

        {/* Space */}
        <motion.div 
          whileHover={{ y: -5, borderColor: 'rgba(249,115,22,0.3)' }}
          className="relative overflow-hidden bg-[#0e0f18]/80 backdrop-blur-2xl border border-white/5 rounded-3xl p-6 shadow-2xl transition-all duration-500 group"
        >
          <div className="absolute -right-8 -top-8 w-24 h-24 bg-orange-500/5 blur-[35px] rounded-full group-hover:scale-125 transition-transform duration-500" />
          <div className="relative z-10 flex flex-col justify-between h-full">
            <div className="flex justify-between items-start">
              <div className="w-11 h-11 rounded-2xl bg-orange-500/10 border border-orange-500/20 flex items-center justify-center mb-4 text-orange-400 group-hover:scale-110 transition-transform">
                <HardDrive size={20} />
              </div>
              <span className="text-[8px] font-black uppercase tracking-[0.25em] text-orange-400/70 bg-orange-500/10 px-2 py-0.5 rounded-md">STORAGE</span>
            </div>
            <div>
              <p className="text-[10px] font-black text-gray-500 uppercase tracking-widest mb-1.5">{ti('Disco Disponible', 'Available Disk')}</p>
              {loading ? (
                <div className="h-5 w-1/2 bg-white/5 rounded animate-pulse" />
              ) : (
                <p className="text-sm font-black text-white truncate drop-shadow">
                  {specs?.disk_space_gb ? `${specs.disk_space_gb.toFixed(1)} GB` : ti('Desconocido', 'Unknown')}
                </p>
              )}
              {!loading && specs && (
                <p className="text-[10px] text-orange-400 font-bold uppercase tracking-wider mt-1.5 flex items-center gap-1.5">
                  <span className="w-1.5 h-1.5 rounded-full bg-orange-400" />
                  {ti('Ruta: Steam Library', 'Path: Steam Library')}
                </p>
              )}
            </div>
          </div>
        </motion.div>
      </div>

      {/* Game Selection Block (Fusing Stitch Layout) */}
      <div className="relative z-20 bg-[#0e0f18]/60 backdrop-blur-2xl border border-white/8 rounded-[2rem] p-8 space-y-6 shadow-2xl">
        <div className="absolute -right-16 -top-16 w-52 h-52 bg-accent/5 blur-[65px] rounded-full pointer-events-none" />

        <div className="flex flex-col md:flex-row md:items-center justify-between gap-4">
          <div className="flex items-center gap-4">
            <div className="w-12 h-12 rounded-2xl bg-accent/15 border border-accent/20 flex items-center justify-center shadow-lg shadow-accent/10">
              <Gamepad2 size={22} className="text-accent" />
            </div>
            <div>
              <h4 className="font-black text-white text-base uppercase tracking-wider">{ti('Comparar con un Videojuego', 'Compare with a Video Game')}</h4>
              <p className="text-xs text-gray-400 mt-0.5">{ti('Analiza en tiempo real si tu hardware es apto para ejecutar un juego.', 'Analyze in real-time if your hardware is fit to run a game.')}</p>
            </div>
          </div>
        </div>

        {/* Clean Interactive Search Input — ref is scoped to just this input+dropdown
            pair so clicking anywhere else in the card (header, padding, chip) counts
            as "outside" and actually closes the dropdown. */}
        <div className="relative" ref={dropdownRef}>
          <div className="absolute inset-y-0 left-4 flex items-center pointer-events-none text-gray-500">
            <Search size={16} />
          </div>
          <input
            type="text"
            value={searchQuery}
            onChange={e => { setSearchQuery(e.target.value); setDropdownOpen(true); }}
            onFocus={() => setDropdownOpen(true)}
            placeholder={ti(`Escribe el nombre de un juego... (${games.length} en el catálogo)`, `Type a game name... (${games.length} in catalog)`)}
            className="w-full bg-[#161724] border border-white/8 focus:border-accent/40 rounded-2xl py-4 pl-12 pr-10 text-xs outline-none transition-all placeholder:text-gray-600 text-gray-300 font-bold shadow-inner"
          />
          {(searchQuery || selectedGame) && (
            <button
              onClick={() => { setSearchQuery(''); setSelectedGame(null); setGameMedia(null); }}
              className="absolute inset-y-0 right-4 flex items-center text-gray-500 hover:text-white transition-colors"
            >
              <X size={16} />
            </button>
          )}

          {/* Dropdown Suggestions */}
          <AnimatePresence>
            {dropdownOpen && filteredGames.length > 0 && (
              <motion.div
                initial={{ opacity: 0, y: -8, scale: 0.98 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, y: -8, scale: 0.98 }}
                transition={{ duration: 0.2 }}
                className="absolute left-0 right-0 top-[calc(100%+8px)] z-50 rounded-2xl overflow-hidden border border-white/10"
                style={{
                  background: 'rgba(11,11,18,0.98)',
                  backdropFilter: 'blur(30px)',
                  boxShadow: '0 24px 70px rgba(0,0,0,0.85)',
                  maxHeight: '260px',
                  overflowY: 'auto',
                }}
              >
                {filteredGames.slice(0, 15).map(game => (
                  <button
                    key={game.id}
                    onClick={() => loadGameRequirements(game)}
                    className="w-full flex items-center justify-between px-5 py-3.5 hover:bg-white/[0.04] transition-all text-left border-b border-white/5 last:border-b-0"
                  >
                    <div className="flex items-center gap-4 min-w-0">
                      <div className="w-9 h-9 rounded-xl bg-white/5 flex items-center justify-center shrink-0 border border-white/5">
                        <Gamepad2 size={15} className="text-gray-400" />
                      </div>
                      <div className="min-w-0">
                        <p className="text-sm font-bold text-white truncate">{displayName(game)}</p>
                        <p className="text-[9px] text-gray-500 font-mono">AppID: {game.id}</p>
                      </div>
                    </div>
                    {game.status === 'Installed' && (
                      <span className="px-2.5 py-0.5 rounded-full bg-emerald-500/10 border border-emerald-500/25 text-[8px] font-black text-emerald-400 uppercase tracking-widest">
                        Instalado
                      </span>
                    )}
                  </button>
                ))}
              </motion.div>
            )}
          </AnimatePresence>
        </div>

        {/* Selected Game Card Chip */}
        {selectedGame && (
          <motion.div 
            initial={{ opacity: 0, scale: 0.95 }}
            animate={{ opacity: 1, scale: 1 }}
            className="flex items-center gap-3 px-5 py-3 rounded-2xl bg-accent/5 border border-accent/20 w-fit"
          >
            <Zap size={14} className="text-accent animate-pulse" />
            <span className="text-xs font-bold text-white uppercase tracking-wider">{displayName(selectedGame)}</span>
            <span className="text-[10px] text-gray-500 font-mono">/ AppID {selectedGame.id}</span>
          </motion.div>
        )}
      </div>

      {/* Game Requirements & Dynamic Comparison View */}
      <AnimatePresence mode="wait">
        {gameMedia && (
          <motion.div
            initial={{ opacity: 0, y: 30 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -20 }}
            transition={{ type: 'spring', stiffness: 260, damping: 24 }}
            className="space-y-6"
          >
            {/* Game Info and Overall Meter */}
            <div className="relative overflow-hidden rounded-[2.5rem] border border-white/10 shadow-2xl p-8 bg-[#0b0c13]/90">
              {/* Giant blurred background image of the game */}
              {selectedGame && (
                <div 
                  className="absolute inset-0 bg-cover bg-center opacity-15 blur-2xl z-0 pointer-events-none scale-105"
                  style={{ backgroundImage: `url(https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/${selectedGame.id}/header.jpg)` }}
                />
              )}
              {/* Dark overlay */}
              <div className="absolute inset-0 bg-gradient-to-t from-[#0e0f18] via-transparent to-[#0e0f18]/30 z-0 pointer-events-none" />

              <div className="relative z-10 flex flex-col md:flex-row items-center justify-between gap-8 w-full">
                <div className="flex items-center gap-6">
                  {/* Game header image thumbnail */}
                  {selectedGame && (
                    <img 
                      src={`https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/${selectedGame.id}/header.jpg`}
                      alt="Game Cover"
                      className="w-48 h-24 object-cover rounded-2xl border border-white/10 shadow-2xl hidden sm:block shrink-0"
                      onError={(e) => { e.currentTarget.style.display = 'none'; }}
                    />
                  )}
                  <div className="space-y-2">
                    <h4 className="font-black text-3xl text-white tracking-tight drop-shadow-lg">{gameMedia.name || displayName(selectedGame!)}</h4>
                    <div className="flex items-center gap-3">
                      {gameMedia.metacritic_score && (
                        <span className="px-2.5 py-1 bg-green-500/10 border border-green-500/20 text-[10px] font-black text-green-400 rounded-lg">
                          Metacritic: {gameMedia.metacritic_score}
                        </span>
                      )}
                      <span className="text-[10px] text-gray-500 font-bold uppercase tracking-wider">AppID: {selectedGame?.id}</span>
                    </div>
                  </div>
                </div>
                
                <div className="flex items-center gap-6">
                  {/* PREDICTION CARD */}
                  {performancePrediction && (
                    <motion.div 
                      initial={{ opacity: 0, x: 20 }}
                      animate={{ opacity: 1, x: 0 }}
                      className={`hidden lg:flex flex-col p-4 rounded-2xl border ${performancePrediction.border} ${performancePrediction.bg} gap-2 min-w-[200px]`}
                    >
                      <div className="flex items-center gap-2 mb-1">
                        <Zap size={14} className={performancePrediction.color} />
                        <span className="text-[9px] font-black text-gray-400 uppercase tracking-widest">{ti('Predicción de Rendimiento', 'Performance Estimate')}</span>
                      </div>
                      <div className="grid grid-cols-2 gap-x-4 gap-y-2">
                        <div className="flex flex-col">
                          <span className="text-[8px] text-gray-500 font-bold uppercase">FPS</span>
                          <span className={`text-sm font-black ${performancePrediction.color}`}>{performancePrediction.fps}</span>
                        </div>
                        <div className="flex flex-col">
                          <span className="text-[8px] text-gray-500 font-bold uppercase">{ti('Resolución', 'Resolution')}</span>
                          <span className={`text-sm font-black ${performancePrediction.color}`}>{performancePrediction.resolution}</span>
                        </div>
                        <div className="flex flex-col col-span-2">
                          <span className="text-[8px] text-gray-500 font-bold uppercase">{ti('Ajustes Sugeridos', 'Suggested Settings')}</span>
                          <span className={`text-xs font-black ${performancePrediction.color}`}>{performancePrediction.settings}</span>
                        </div>
                      </div>
                    </motion.div>
                  )}

                  {/* Compatibility gauge / Dial */}
                  <div className="flex flex-col items-center justify-center shrink-0">
                    <div className="relative w-24 h-24 flex items-center justify-center">
                      {/* Ring background */}
                      <svg className="w-full h-full -rotate-90" viewBox="0 0 100 100">
                        <circle cx="50" cy="50" r="40" fill="none" stroke="rgba(255,255,255,0.03)" strokeWidth="8" />
                        <motion.circle
                          cx="50"
                          cy="50"
                          r="40"
                          fill="none"
                          stroke={overallStatus === 'excellent' ? '#10b981' : overallStatus === 'good' ? '#f59e0b' : overallStatus === 'unknown' ? '#6b7280' : '#ef4444'}
                          strokeWidth="8"
                          strokeLinecap="round"
                          strokeDasharray={`${2 * Math.PI * 40}`}
                          initial={{ strokeDashoffset: 2 * Math.PI * 40 }}
                          animate={{ 
                            strokeDashoffset: 2 * Math.PI * 40 * (1 - (overallStatus === 'excellent' ? 1 : overallStatus === 'good' ? 0.7 : overallStatus === 'unknown' ? 0 : 0.3)) 
                          }}
                          transition={{ duration: 1.2, ease: 'easeOut' }}
                          style={{ filter: `drop-shadow(0 0 8px ${overallStatus === 'excellent' ? 'rgba(16,185,129,0.4)' : overallStatus === 'good' ? 'rgba(245,158,11,0.4)' : overallStatus === 'unknown' ? 'rgba(107,114,128,0.3)' : 'rgba(239,68,68,0.4)'})` }}
                        />
                      </svg>
                      <div className="absolute inset-0 flex flex-col items-center justify-center text-center">
                        {/* 'unknown' means the requirements could not be read at
                            all, which is not the same as failing them. It used to
                            fall through to the red branch, so a game whose Steam
                            page has no parseable requirements showed a red ring at
                            30% reading "MEJORABLE" — telling the user their PC was
                            not good enough when nothing had been compared. */}
                        <span className="text-[9px] font-black text-gray-400 uppercase tracking-widest">
                          {ti('DIAGNÓSTICO', 'DIAGNOSIS')}
                        </span>
                        <span className={`text-xs font-black uppercase tracking-wider mt-0.5 ${overallStatus === 'excellent' ? 'text-emerald-400' : overallStatus === 'good' ? 'text-amber-400' : overallStatus === 'unknown' ? 'text-gray-500' : 'text-red-400'}`}>
                          {overallStatus === 'excellent'
                            ? ti('ÓPTIMO', 'OPTIMAL')
                            : overallStatus === 'good'
                              ? ti('MÍNIMO', 'MINIMUM')
                              : overallStatus === 'unknown'
                                ? ti('SIN DATOS', 'NO DATA')
                                : ti('MEJORABLE', 'BELOW SPEC')}
                        </span>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </div>

            {/* Comparison Cards (Grid layout) */}
            {reqs.length > 0 && (
              <div className="grid grid-cols-1 md:grid-cols-2 gap-5">
                  {reqs.map((req, i) => {
                    const meetsRec = req.meetsRecommended;
                    const meetsMin = req.meetsMinimum;
                    
                    return (
                      <motion.div
                        key={i}
                        initial={{ opacity: 0, scale: 0.98 }}
                        animate={{ opacity: 1, scale: 1 }}
                        transition={{ delay: i * 0.1 }}
                        className="bg-[#0e0f18]/80 backdrop-blur-xl border border-white/5 rounded-3xl p-6 flex flex-col justify-between shadow-2xl relative overflow-hidden"
                      >
                        {/* Left color bar matching status */}
                        <div className={`absolute left-0 top-0 bottom-0 w-[4px] ${meetsRec ? 'bg-emerald-500' : meetsMin ? 'bg-amber-500' : 'bg-red-500'}`} />

                        <div className="space-y-4">
                          {/* Title and Badge */}
                          <div className="flex items-center justify-between">
                            <h5 className="text-sm font-black text-white uppercase tracking-wider">{req.component}</h5>
                            <div className="flex items-center gap-1.5">
                              {/* "Could not compare" is its own answer. It used
                                  to fall through to the green "Mínimo OK",
                                  because an unscored component defaulted to
                                  passing — so a card missing from the table
                                  looked like a card that cleared the bar. */}
                              {!req.comparable ? (
                                <span className="px-2 py-0.5 rounded bg-white/[0.05] border border-white/10 text-[9px] font-black text-gray-400 uppercase tracking-wider">
                                  {ti('Sin datos', 'No data')}
                                </span>
                              ) : meetsRec ? (
                                <span className="px-2 py-0.5 rounded bg-emerald-500/10 border border-emerald-500/20 text-[9px] font-black text-emerald-400 uppercase tracking-wider">{ti('Recomendado OK', 'Meets Recommended')}</span>
                              ) : meetsMin ? (
                                <span className="px-2 py-0.5 rounded bg-amber-500/10 border border-amber-500/20 text-[9px] font-black text-amber-400 uppercase tracking-wider">{ti('Mínimo OK', 'Meets Minimum')}</span>
                              ) : (
                                <span className="px-2 py-0.5 rounded bg-red-500/10 border border-red-500/20 text-[9px] font-black text-red-400 uppercase tracking-wider">{ti('Mejorable', 'Below Spec')}</span>
                              )}
                            </div>
                          </div>

                          {/* Spec Comparison row */}
                          <div className="grid grid-cols-2 gap-3.5">
                            {/* Requisitos Oficiales */}
                            <div className="bg-white/[0.02] border border-white/5 rounded-2xl p-4 min-h-[90px]">
                              <p className="text-[8px] font-black text-gray-500 uppercase tracking-widest mb-1.5">{ti('Requisito Oficial', 'Official Requirement')}</p>
                              <div className="space-y-1">
                                <p className="text-[10px] text-gray-400 font-bold uppercase tracking-wider">Mín: <span className="text-gray-300 font-medium normal-case">{req.minimum}</span></p>
                                <p className="text-[10px] text-gray-400 font-bold uppercase tracking-wider">Rec: <span className="text-gray-300 font-medium normal-case">{req.recommended}</span></p>
                              </div>
                            </div>

                            {/* Tu Computadora */}
                            <div className={`border rounded-2xl p-4 min-h-[90px] ${
                              meetsRec ? 'bg-emerald-500/5 border-emerald-500/15' : meetsMin ? 'bg-amber-500/5 border-amber-500/15' : 'bg-red-500/5 border-red-500/15'
                            }`}>
                              <div className="flex items-center justify-between">
                                <p className="text-[8px] font-black text-gray-500 uppercase tracking-widest">{ti('Tu Computadora', 'Your PC')}</p>
                                {meetsRec ? (
                                  <CheckCircle size={12} className="text-emerald-400" />
                                ) : meetsMin ? (
                                  <AlertTriangle size={12} className="text-amber-400" />
                                ) : (
                                  <XCircle size={12} className="text-red-400" />
                                )}
                              </div>
<p className="text-[11px] font-black text-white leading-relaxed mt-2 line-clamp-2">{req.userValue}</p>
                            </div>
                          </div>
                        </div>
                      </motion.div>
                    );
                    })}
                  </div>
                )}
              </motion.div>
            )}
          </AnimatePresence>

      {/* Empty State */}
      {!gameMedia && !gameLoading && (
        <div className="text-center py-16 bg-[#0e0f18]/30 border border-dashed border-white/5 rounded-[2.5rem]">
          <div className="w-16 h-16 rounded-2xl bg-white/5 border border-white/5 flex items-center justify-center mx-auto mb-4">
            <Gamepad2 size={24} className="text-gray-600" />
          </div>
          <p className="text-sm text-gray-500 font-black uppercase tracking-wider">{ti('Esperando Selección de Juego', 'Waiting for Game Selection')}</p>
          <p className="text-xs text-gray-600 mt-1">{ti('Busca un título en la barra superior para iniciar el escaneo de hardware.', 'Search for a title in the top bar to start hardware scanning.')}</p>
        </div>
      )}

      {/* Loading State */}
      {gameLoading && (
        <div className="text-center py-20 bg-[#0e0f18]/30 border border-dashed border-white/5 rounded-[2.5rem] flex flex-col items-center justify-center gap-4">
          <div className="relative">
            <div className="absolute inset-0 rounded-full bg-accent/20 blur-xl animate-pulse" />
            <RefreshCw size={36} className="text-accent animate-spin relative z-10" />
          </div>
          <div>
            <p className="text-sm text-gray-400 font-black uppercase tracking-widest animate-pulse">{ti('Analizando Requisitos', 'Analyzing Requirements')}</p>
            <p className="text-[10px] text-gray-600 font-bold uppercase tracking-wider mt-1">{ti('Comparando hardware actual con la base de datos de Steam...', 'Comparing current hardware with Steam database...')}</p>
          </div>
        </div>
      )}
    </div>
  );
};

export default SpecMatcherView;