import React, { useState, useEffect, useRef, useMemo, useCallback, memo } from 'react';
import { createPortal } from 'react-dom';
import { motion, AnimatePresence } from 'framer-motion';
import { invoke, convertFileSrc } from '@tauri-apps/api/tauri';
import { listen } from '@tauri-apps/api/event';
import { open, save } from '@tauri-apps/api/dialog';
import { downloadDir } from '@tauri-apps/api/path';
import {
  Home,
  Gamepad2,
  Library,
  Globe,
  Wrench,
  Settings,
  Search,
  RefreshCw,
  Download,
  AlertTriangle,
  X,
  ArrowLeft,
  Play,
  Maximize2,
  CheckCircle,
  Zap,
  Star,
  Trash2,
  Package,
  HardDrive,
  Bell,
  LifeBuoy,
  Send,
  ChevronDown,
  ChevronRight,
  ShieldOff,
  DownloadCloud,
  UploadCloud,
  CloudOff,
  Cloud,
  XCircle,
  MessageSquare,
  Bug,
  Zap as ZapIcon,
  HelpCircle,
  AlertOctagon,
  CheckCircle2,
  Cpu,
  Heart,
  Trophy,
  UserPlus,
  Copy,
  Users,
  ArrowUpRight,
  ImagePlus,
  FileText,
  ChevronLeft,
  PanelLeft,
  Waves,
  Lock,
  Pin,
  PinOff,
  FileCode,
  Puzzle,
  ExternalLink,
  Circle,
  Languages,
  ThumbsUp,
  Check,
  MoreHorizontal,
} from 'lucide-react';
import SplashNuevo from './components/SplashNuevo';
import SteamIcon from './components/SteamIcon';
import { NotificationProvider, useNotify } from './components/NotificationProvider';
import DynamicBackground from './components/DynamicBackground';
import SpecMatcherView, { compareRequirements, requirementsVerdict, type SystemSpecs } from './components/SpecMatcherView';
import EmulatorsView from './components/EmulatorsView';
import DonateModal from './components/DonateModal';
import { activeDonationAddresses } from './components/donations';

/// "Binance Pay · USDT" — the methods actually set up in donations.ts, so the
/// sidebar never names a service that is not in use (it said ko-fi.com long
/// after that account was gone).
const DONATION_METHODS = Array.from(new Set(activeDonationAddresses().map(a => a.label))).join(' · ');
import FlagIcon from './components/FlagIcon';
import logo from './assets/logo.png';
import switchEmulatorsIconImg from './assets/switch-emulators-icon.png';
import onlineFixExample from './assets/onlinefix-launch-options-example.png';

// --- Types ---

interface Game {
  id: string;
  name: string;
  image?: string;
  developers?: string[];
  genres?: string[];
  metacritic?: { score: number };
  status?: string;
  size_bytes?: number;
  is_new?: boolean;
  /** When the catalog last updated this game's ticket. */
  updated_at?: string;
  nsfw?: boolean;
  // True for a real Steam install the catalog has never heard of — e.g. a
  // game the user legitimately bought and installed outside Ragnarok.
  // GameCard hides "Desinstalar" for these: delete_game removes the real
  // appmanifest_<id>.acf Steam itself created, which is fine for a game
  // Ragnarok installed but would corrupt Steam's tracking of a game it
  // never touched. DLC Manager/Cloud Saves/Unlock DLC stay available since
  // those don't touch the real Steam install state.
  foreign?: boolean;
  // "Denuvo" cuando el juego lo trae, cadena vacía cuando ya se preguntó y no
  // lo trae. `undefined` es "todavía no se preguntó", que es lo que dispara
  // la consulta por página. La única fuente es la ficha de Steam del juego:
  // el catálogo sólo sabe decir sí o no, y eso no distingue Denuvo de
  // cualquier otro anti-tamper.
  drm?: string;
}

interface SteamMedia {
  /** Full size, 1920x1080. Only the lightbox needs these. */
  screenshots: string[];
  /** The same shots at 600x338, index for index. What the grid draws. */
  screenshots_thumbs?: string[];
  trailer_mp4: string | null;
  trailer_webm: string | null;
  trailer_thumbnail: string | null;
  name: string | null;
  metacritic_score: number | null;
  minimum_reqs: string | null;
  recommended_reqs: string | null;
  short_description: string | null;
  /** Steam's trailer as it serves it now: an HLS stream (see SteamTrailer). */
  trailer_hls?: string | null;
  developers?: string[];
  publishers?: string[];
  release_date?: string | null;
  languages?: { name: string; audio: boolean }[];
  reviews?: { description: string; positive: number; total: number } | null;
}

interface DrmCheckResult {
  status: 'clean' | 'denuvo' | 'third_party' | 'drm_detected' | 'error';
  message: string;
}

interface BackupInfo {
  id: string;
  app_id: string;
  name: string;
  created_at: string;
  size_bytes: number;
}

interface UpdateInfo {
  has_update: boolean;
  current_version: string;
  latest_version: string;
  download_url: string | null;
}

// --- i18n ---

// Russian noun pluralization has 3 forms depending on the count (unlike
// Spanish/English/Portuguese's simple 1-vs-everything-else split) — 1/21/31...
// use `one`, 2-4/22-24... use `few`, everything else (0, 5-20, 25-30...)
// uses `many`. The n%100 check is what excludes the 11-14 range from the
// %10-based one/few match (11-14 always take `many` despite ending in 1-4).
function ruPlural(n: number, one: string, few: string, many: string): string {
  const mod10 = n % 10;
  const mod100 = n % 100;
  if (mod10 === 1 && mod100 !== 11) return one;
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return few;
  return many;
}

const TRANSLATIONS = {
  en: {
    sidebar: { home: 'Home', games: 'Games', adults: 'Adults', library: 'Library', online: 'Online Games', fix: 'Fix', bypass: 'Bypass', emulators: 'Emulator Games', specs: 'Specs', achievements: 'Achievements', system: 'System', settings: 'Settings', support: 'Support' },
    home: {
      steamActive: 'Steam Active', steamClosed: 'Steam Closed',
      totalGames: 'Total Games', installed: 'Installed',
      tools: 'Steam Tools', restartSteam: 'Restart Steam', installPlugin: 'Install Plugin', repairPlugin: 'Repair Plugin',
      fixerTitle: "Steam won't open?",
      fixerDesc: "If Steam doesn't start correctly, run the automatic repair.",
      fixerBtn: 'Run Repair', fixerRunning: 'Running...',
    },
    detail: {
      back: 'Back to Games', loading: 'Loading...', noTrailer: 'No trailer available',
      screenshots: 'Screenshots', noScreenshots: 'No screenshots available', download: 'Download',
    },
    catalog: { search: 'Search by name or AppID...', results: (n: number, q: string) => `${n} result${n !== 1 ? 's' : ''} for "${q}"`, allCategories: 'All' },
    library: { search: 'Search library...' },
    saves: {
      title: 'Cloud Saves',
      connect: 'Connect Google Drive',
      connected: 'Google Drive Connected',
      connecting: 'Connecting...',
      backup: 'Backup Saves',
      backing: 'Backing up...',
      restore: 'Restore',
      restoring: 'Restoring...',
      noSaves: 'No saves found for this game in Steam userdata.',
      noBackups: 'No backups yet. Click "Backup Saves" to create one.',
      backupOk: 'Backup completed!',
      restoreOk: 'Saves restored successfully!',
      sizeLabel: 'Size',
      credentials: 'Add your Google OAuth2 credentials to cloud_saves.rs before using this feature.',
    },
    online: { goTo: 'Go to Website' },
    settings: {
      steam: 'Steam',
      updates: 'Updates', lastChecked: 'Last checked', never: 'Never',
      checkUpdates: 'Check for Updates', checking: 'Checking...', upToDate: "You're up to date!", upToDateSub: (v: string) => `v${v} is the latest version.`,
      language: 'Language', languageDesc: 'Select the interface language.',
    },
    notify: {
      fixer: 'Running repair...', fixerOk: 'Repair launched — accept UAC if prompted', fixerErr: 'Error running repair: ',
      restarting: 'Restarting Steam...', restartErr: 'Failed: ', catalogErr: 'Failed to load catalog',
      preparing: 'Preparing installation for AppID: ', injected: 'Manifest injected successfully!', installErr: 'Installation failed: ',
    },
    update: {
      detected: 'NEW VERSION DETECTED',
      auto: 'The launcher will update automatically in',
      seconds: 's',
      cancel: 'Cancel',
      updateNow: 'Update Now',
      downloading: 'Downloading update...',
      noUpdate: "You're up to date!",
      noUpdateSub: (v: string) => `v${v} is the latest version.`,
      checkBtn: 'Check for Updates',
      checking: 'Checking...',
    },
    fix: {
      title: 'Fix Dashboard',
      subtitle: 'Deep manifest repair for games stuck on "Downloading" or showing "Buy".',
      appIdLabel: 'Steam AppID',
      appIdPlaceholder: 'e.g. 730',
      startBtn: 'Start Repair',
      repairing: 'Repairing...',
      steamPath: 'Steam Path',
      step1: 'Reading Lua tracker...',
      step2: 'Querying SteamCMD...',
      step3: 'Downloading manifests...',
      step4: 'Injecting into depotcache...',
      done: 'Repair complete',
      error: 'Repair failed',
      clearLog: 'Clear',
    },
    appearance: {
      title: 'Appearance',
      accent: 'Accent Color',
      accentDesc: 'Customize the launcher accent color.',
      background: 'Dynamic Background',
      backgroundDesc: 'Path to a custom image or video (mp4/webm). Leave empty to use automatic seasonal backgrounds.',
      backgroundPlaceholder: 'C:\\path\\to\\background.mp4',
      backgroundClear: 'Clear',
    },
    notifications: {
      title: 'Notifications',
      empty: 'No notifications',
      update: (v: string) => `New update available: v${v}`,
      newGames: (n: number) => `${n} new game${n !== 1 ? 's' : ''} added to the catalog`,
      markRead: 'Mark all as read',
    },
    support: {
      title: 'Support',
      subtitle: 'Report bugs or suggest improvements for Ragnarok Launcher.',
      typeLabel: 'Problem type',
      descLabel: 'Description',
      descPlaceholder: 'Describe the issue or suggestion in as much detail as possible...',
      sendBtn: 'Send Report',
      sending: 'Sending...',
      sent: 'Sent!',
      noWebhook: 'Discord webhook not configured. Contact the developer.',
      categories: {
        cache: 'Cache error (program loads nothing)',
        download: 'Games fail to download',
        install: 'Game installation error',
        steam: 'Steam-related issue',
        crash: 'Program crashes unexpectedly',
        feature: 'Missing feature / Suggestion',
        other: 'Other error',
      },
    },
  },
  es: {
    sidebar: { home: 'Inicio', games: 'Juegos', adults: 'Zona +18', library: 'Librería', online: 'Online', fix: 'Reparar', bypass: 'Bypass', emulators: 'Juegos de Emuladores', specs: 'Specs', achievements: 'Logros', system: 'Sistema', settings: 'Ajustes', support: 'Soporte' },
    home: {
      steamActive: 'Steam Activo', steamClosed: 'Steam Cerrado',
      totalGames: 'Juegos Totales', installed: 'Instalados',
      tools: 'Herramientas Steam', restartSteam: 'Reiniciar Steam', installPlugin: 'Instalar Plugin', repairPlugin: 'Reparar Plugin',
      fixerTitle: '¿Steam no abre?',
      fixerDesc: 'Si Steam no inicia correctamente, ejecuta el reparador automático.',
      fixerBtn: 'Ejecutar Reparador', fixerRunning: 'Ejecutando...',
    },
    detail: {
      back: 'Volver a Games', loading: 'Cargando...', noTrailer: 'Sin trailer disponible',
      screenshots: 'Capturas de pantalla', noScreenshots: 'No hay capturas disponibles', download: 'Descargar',
    },
    catalog: { search: 'Buscar por nombre o AppID...', results: (n: number, q: string) => `${n} resultado${n !== 1 ? 's' : ''} para "${q}"`, allCategories: 'Todos' },
    library: { search: 'Buscar en librería...' },
    saves: {
      title: 'Saves en la Nube',
      connect: 'Conectar Google Drive',
      connected: 'Google Drive Conectado',
      connecting: 'Conectando...',
      backup: 'Hacer Backup',
      backing: 'Guardando...',
      restore: 'Restaurar',
      restoring: 'Restaurando...',
      noSaves: 'No se encontraron saves para este juego en Steam userdata.',
      noBackups: 'Sin backups todavía. Haz clic en "Hacer Backup" para crear uno.',
      backupOk: '¡Backup completado!',
      restoreOk: '¡Saves restaurados correctamente!',
      sizeLabel: 'Tamaño',
      credentials: 'Agrega tus credenciales OAuth2 de Google en cloud_saves.rs antes de usar esta función.',
    },
    online: { goTo: 'Ir al sitio' },
    settings: {
      steam: 'Steam',
      updates: 'Actualizaciones', lastChecked: 'Última verificación', never: 'Nunca',
      checkUpdates: 'Buscar Actualizaciones', checking: 'Verificando...', upToDate: '¡Estás actualizado!', upToDateSub: (v: string) => `v${v} es la versión más reciente.`,
      language: 'Idioma', languageDesc: 'Selecciona el idioma de la interfaz.',
    },
    notify: {
      fixer: 'Ejecutando reparador...', fixerOk: 'Reparador ejecutado — acepta el UAC si aparece', fixerErr: 'Error al ejecutar reparador: ',
      restarting: 'Reiniciando Steam...', restartErr: 'Error: ', catalogErr: 'Error al cargar el catálogo',
      preparing: 'Preparando instalación para AppID: ', injected: '¡Manifest inyectado correctamente!', installErr: 'Error de instalación: ',
    },
    update: {
      detected: 'NUEVA VERSIÓN DETECTADA',
      auto: 'El launcher se actualizará automáticamente en',
      seconds: 's',
      cancel: 'Cancelar',
      updateNow: 'Actualizar Ahora',
      downloading: 'Descargando actualización...',
      noUpdate: '¡Estás actualizado!',
      noUpdateSub: (v: string) => `v${v} es la versión más reciente.`,
      checkBtn: 'Buscar Actualizaciones',
      checking: 'Verificando...',
    },
    fix: {
      title: 'Fix Dashboard',
      subtitle: 'Reparación profunda de manifiestos para juegos atascados en "Descargando" o que piden "Comprar".',
      appIdLabel: 'Steam AppID',
      appIdPlaceholder: 'ej. 730',
      startBtn: 'Iniciar Reparación',
      repairing: 'Reparando...',
      steamPath: 'Ruta de Steam',
      step1: 'Leyendo rastreador Lua...',
      step2: 'Consultando SteamCMD...',
      step3: 'Descargando manifiestos...',
      step4: 'Inyectando en depotcache...',
      done: 'Reparación completa',
      error: 'Reparación fallida',
      clearLog: 'Limpiar',
    },
    appearance: {
      title: 'Apariencia',
      accent: 'Color de Acento',
      accentDesc: 'Personaliza el color de acento del launcher.',
      background: 'Fondo Dinámico',
      backgroundDesc: 'Ruta a imagen o video personalizado (mp4/webm). Vacío para fondos automáticos por temporada.',
      backgroundPlaceholder: 'C:\\ruta\\al\\fondo.mp4',
      backgroundClear: 'Limpiar',
    },
    notifications: {
      title: 'Notificaciones',
      empty: 'Sin notificaciones',
      update: (v: string) => `Nueva versión disponible: v${v}`,
      newGames: (n: number) => `${n} nuevo${n !== 1 ? 's' : ''} juego${n !== 1 ? 's' : ''} en el catálogo`,
      markRead: 'Marcar todo como leído',
    },
    support: {
      title: 'Soporte',
      subtitle: 'Reporta errores o sugiere mejoras para Ragnarok Launcher.',
      typeLabel: 'Tipo de problema',
      descLabel: 'Descripción',
      descPlaceholder: 'Describe el problema o sugerencia con el mayor detalle posible...',
      sendBtn: 'Enviar Reporte',
      sending: 'Enviando...',
      sent: '¡Enviado!',
      noWebhook: 'Webhook de Discord no configurado. Contacta al desarrollador.',
      categories: {
        cache: 'Error de caché (el programa no carga nada)',
        download: 'Los juegos no se descargan',
        install: 'Error al instalar un juego',
        steam: 'Problema con Steam',
        crash: 'El programa se cierra inesperadamente',
        feature: 'Falta algo / Sugerencia',
        other: 'Otro error',
      },
    },
  },
  pt: {
    sidebar: { home: 'Início', games: 'Jogos', adults: 'Adultos', library: 'Biblioteca', online: 'Jogos Online', fix: 'Reparar', bypass: 'Bypass', emulators: 'Jogos de Emuladores', specs: 'Specs', achievements: 'Conquistas', system: 'Sistema', settings: 'Configurações', support: 'Suporte' },
    home: {
      steamActive: 'Steam Ativo', steamClosed: 'Steam Fechado',
      totalGames: 'Jogos Totais', installed: 'Instalados',
      tools: 'Ferramentas Steam', restartSteam: 'Reiniciar Steam', installPlugin: 'Instalar Plugin', repairPlugin: 'Reparar Plugin',
      fixerTitle: 'Steam não abre?',
      fixerDesc: 'Se o Steam não iniciar corretamente, execute o reparo automático.',
      fixerBtn: 'Executar Reparo', fixerRunning: 'Executando...',
    },
    detail: {
      back: 'Voltar para Jogos', loading: 'Carregando...', noTrailer: 'Nenhum trailer disponível',
      screenshots: 'Capturas de tela', noScreenshots: 'Nenhuma captura disponível', download: 'Baixar',
    },
    catalog: { search: 'Buscar por nome ou AppID...', results: (n: number, q: string) => `${n} resultado${n !== 1 ? 's' : ''} para "${q}"`, allCategories: 'Todos' },
    library: { search: 'Buscar na biblioteca...' },
    saves: {
      title: 'Saves na Nuvem',
      connect: 'Conectar Google Drive',
      connected: 'Google Drive Conectado',
      connecting: 'Conectando...',
      backup: 'Fazer Backup',
      backing: 'Salvando...',
      restore: 'Restaurar',
      restoring: 'Restaurando...',
      noSaves: 'Nenhum save encontrado para este jogo no userdata do Steam.',
      noBackups: 'Nenhum backup ainda. Clique em "Fazer Backup" para criar um.',
      backupOk: 'Backup concluído!',
      restoreOk: 'Saves restaurados com sucesso!',
      sizeLabel: 'Tamanho',
      credentials: 'Adicione suas credenciais OAuth2 do Google em cloud_saves.rs antes de usar este recurso.',
    },
    online: { goTo: 'Ir ao Site' },
    settings: {
      steam: 'Steam',
      updates: 'Atualizações', lastChecked: 'Última verificação', never: 'Nunca',
      checkUpdates: 'Buscar Atualizações', checking: 'Verificando...', upToDate: 'Você está atualizado!', upToDateSub: (v: string) => `v${v} é a versão mais recente.`,
      language: 'Idioma', languageDesc: 'Selecione o idioma da interface.',
    },
    notify: {
      fixer: 'Executando reparo...', fixerOk: 'Reparo iniciado — aceite o UAC se aparecer', fixerErr: 'Erro ao executar reparo: ',
      restarting: 'Reiniciando Steam...', restartErr: 'Falha: ', catalogErr: 'Falha ao carregar o catálogo',
      preparing: 'Preparando instalação para AppID: ', injected: 'Manifest injetado com sucesso!', installErr: 'Falha na instalação: ',
    },
    update: {
      detected: 'NOVA VERSÃO DETECTADA',
      auto: 'O launcher será atualizado automaticamente em',
      seconds: 's',
      cancel: 'Cancelar',
      updateNow: 'Atualizar Agora',
      downloading: 'Baixando atualização...',
      noUpdate: 'Você está atualizado!',
      noUpdateSub: (v: string) => `v${v} é a versão mais recente.`,
      checkBtn: 'Buscar Atualizações',
      checking: 'Verificando...',
    },
    fix: {
      title: 'Painel de Reparo',
      subtitle: 'Reparo profundo de manifestos para jogos travados em "Baixando" ou mostrando "Comprar".',
      appIdLabel: 'Steam AppID',
      appIdPlaceholder: 'ex. 730',
      startBtn: 'Iniciar Reparo',
      repairing: 'Reparando...',
      steamPath: 'Caminho do Steam',
      step1: 'Lendo rastreador Lua...',
      step2: 'Consultando SteamCMD...',
      step3: 'Baixando manifestos...',
      step4: 'Injetando no depotcache...',
      done: 'Reparo concluído',
      error: 'Falha no reparo',
      clearLog: 'Limpar',
    },
    appearance: {
      title: 'Aparência',
      accent: 'Cor de Destaque',
      accentDesc: 'Personalize a cor de destaque do launcher.',
      background: 'Fundo Dinâmico',
      backgroundDesc: 'Caminho para uma imagem ou vídeo personalizado (mp4/webm). Deixe vazio para usar fundos sazonais automáticos.',
      backgroundPlaceholder: 'C:\\caminho\\para\\fundo.mp4',
      backgroundClear: 'Limpar',
    },
    notifications: {
      title: 'Notificações',
      empty: 'Sem notificações',
      update: (v: string) => `Nova atualização disponível: v${v}`,
      newGames: (n: number) => `${n} novo${n !== 1 ? 's' : ''} jogo${n !== 1 ? 's' : ''} adicionado${n !== 1 ? 's' : ''} ao catálogo`,
      markRead: 'Marcar tudo como lido',
    },
    support: {
      title: 'Suporte',
      subtitle: 'Relate bugs ou sugira melhorias para o Ragnarok Launcher.',
      typeLabel: 'Tipo de problema',
      descLabel: 'Descrição',
      descPlaceholder: 'Descreva o problema ou sugestão com o máximo de detalhes possível...',
      sendBtn: 'Enviar Relatório',
      sending: 'Enviando...',
      sent: 'Enviado!',
      noWebhook: 'Webhook do Discord não configurado. Entre em contato com o desenvolvedor.',
      categories: {
        cache: 'Erro de cache (o programa não carrega nada)',
        download: 'Os jogos não baixam',
        install: 'Erro ao instalar um jogo',
        steam: 'Problema relacionado ao Steam',
        crash: 'O programa fecha inesperadamente',
        feature: 'Recurso ausente / Sugestão',
        other: 'Outro erro',
      },
    },
  },
  fr: {
    sidebar: { home: 'Accueil', games: 'Jeux', adults: 'Adultes', library: 'Bibliothèque', online: 'Jeux en ligne', fix: 'Réparer', bypass: 'Bypass', emulators: 'Émulateurs', specs: 'Specs', achievements: 'Succès', system: 'Système', settings: 'Paramètres', support: 'Support' },
    home: {
      steamActive: 'Steam Actif', steamClosed: 'Steam Fermé',
      totalGames: 'Jeux Totaux', installed: 'Installés',
      tools: 'Outils Steam', restartSteam: 'Redémarrer Steam', installPlugin: 'Installer le Plugin', repairPlugin: 'Réparer le Plugin',
      fixerTitle: "Steam ne s'ouvre pas ?",
      fixerDesc: 'Si Steam ne démarre pas correctement, lancez la réparation automatique.',
      fixerBtn: 'Lancer la Réparation', fixerRunning: 'En cours...',
    },
    detail: {
      back: 'Retour aux Jeux', loading: 'Chargement...', noTrailer: 'Aucune bande-annonce disponible',
      screenshots: "Captures d'écran", noScreenshots: 'Aucune capture disponible', download: 'Télécharger',
    },
    catalog: { search: 'Rechercher par nom ou AppID...', results: (n: number, q: string) => `${n} résultat${n !== 1 ? 's' : ''} pour "${q}"`, allCategories: 'Tous' },
    library: { search: 'Rechercher dans la bibliothèque...' },
    saves: {
      title: 'Sauvegardes Cloud',
      connect: 'Connecter Google Drive',
      connected: 'Google Drive Connecté',
      connecting: 'Connexion...',
      backup: 'Sauvegarder',
      backing: 'Sauvegarde en cours...',
      restore: 'Restaurer',
      restoring: 'Restauration...',
      noSaves: 'Aucune sauvegarde trouvée pour ce jeu dans les données Steam.',
      noBackups: 'Aucune sauvegarde pour le moment. Cliquez sur "Sauvegarder" pour en créer une.',
      backupOk: 'Sauvegarde terminée !',
      restoreOk: 'Sauvegardes restaurées avec succès !',
      sizeLabel: 'Taille',
      credentials: "Ajoutez vos identifiants OAuth2 Google dans cloud_saves.rs avant d'utiliser cette fonction.",
    },
    online: { goTo: 'Aller au Site' },
    settings: {
      steam: 'Steam',
      updates: 'Mises à jour', lastChecked: 'Dernière vérification', never: 'Jamais',
      checkUpdates: 'Vérifier les Mises à jour', checking: 'Vérification...', upToDate: 'Vous êtes à jour !', upToDateSub: (v: string) => `v${v} est la dernière version.`,
      language: 'Langue', languageDesc: "Sélectionnez la langue de l'interface.",
    },
    notify: {
      fixer: 'Réparation en cours...', fixerOk: "Réparation lancée — acceptez l'UAC si demandé", fixerErr: 'Erreur lors de la réparation : ',
      restarting: 'Redémarrage de Steam...', restartErr: 'Échec : ', catalogErr: 'Échec du chargement du catalogue',
      preparing: "Préparation de l'installation pour AppID : ", injected: 'Manifest injecté avec succès !', installErr: "Échec de l'installation : ",
    },
    update: {
      detected: 'NOUVELLE VERSION DÉTECTÉE',
      auto: 'Le launcher se mettra à jour automatiquement dans',
      seconds: 's',
      cancel: 'Annuler',
      updateNow: 'Mettre à jour maintenant',
      downloading: 'Téléchargement de la mise à jour...',
      noUpdate: 'Vous êtes à jour !',
      noUpdateSub: (v: string) => `v${v} est la dernière version.`,
      checkBtn: 'Vérifier les Mises à jour',
      checking: 'Vérification...',
    },
    fix: {
      title: 'Tableau de Réparation',
      subtitle: 'Réparation approfondie des manifests pour les jeux bloqués sur "Téléchargement" ou affichant "Acheter".',
      appIdLabel: 'Steam AppID',
      appIdPlaceholder: 'ex. 730',
      startBtn: 'Démarrer la Réparation',
      repairing: 'Réparation...',
      steamPath: 'Chemin de Steam',
      step1: 'Lecture du tracker Lua...',
      step2: 'Interrogation de SteamCMD...',
      step3: 'Téléchargement des manifests...',
      step4: 'Injection dans depotcache...',
      done: 'Réparation terminée',
      error: 'Échec de la réparation',
      clearLog: 'Effacer',
    },
    appearance: {
      title: 'Apparence',
      accent: "Couleur d'Accent",
      accentDesc: "Personnalisez la couleur d'accent du launcher.",
      background: 'Arrière-plan Dynamique',
      backgroundDesc: 'Chemin vers une image ou vidéo personnalisée (mp4/webm). Laissez vide pour les arrière-plans saisonniers automatiques.',
      backgroundPlaceholder: 'C:\\chemin\\vers\\fond.mp4',
      backgroundClear: 'Effacer',
    },
    notifications: {
      title: 'Notifications',
      empty: 'Aucune notification',
      update: (v: string) => `Nouvelle mise à jour disponible : v${v}`,
      newGames: (n: number) => `${n} nouveau${n > 1 ? 'x' : ''} jeu${n > 1 ? 'x' : ''} ajouté${n > 1 ? 's' : ''} au catalogue`,
      markRead: 'Tout marquer comme lu',
    },
    support: {
      title: 'Support',
      subtitle: 'Signalez des bugs ou suggérez des améliorations pour Ragnarok Launcher.',
      typeLabel: 'Type de problème',
      descLabel: 'Description',
      descPlaceholder: 'Décrivez le problème ou la suggestion avec le plus de détails possible...',
      sendBtn: 'Envoyer le Rapport',
      sending: 'Envoi...',
      sent: 'Envoyé !',
      noWebhook: 'Webhook Discord non configuré. Contactez le développeur.',
      categories: {
        cache: 'Erreur de cache (le programme ne charge rien)',
        download: 'Les jeux ne se téléchargent pas',
        install: "Erreur lors de l'installation d'un jeu",
        steam: 'Problème lié à Steam',
        crash: 'Le programme se ferme de manière inattendue',
        feature: 'Fonctionnalité manquante / Suggestion',
        other: 'Autre erreur',
      },
    },
  },
  ru: {
    sidebar: { home: 'Главная', games: 'Игры', adults: 'Для взрослых', library: 'Библиотека', online: 'Онлайн-игры', fix: 'Исправить', bypass: 'Байпас', emulators: 'Эмуляторы', specs: 'Характеристики', achievements: 'Достижения', system: 'Система', settings: 'Настройки', support: 'Поддержка' },
    home: {
      steamActive: 'Steam активен', steamClosed: 'Steam закрыт',
      totalGames: 'Всего игр', installed: 'Установлено',
      tools: 'Инструменты Steam', restartSteam: 'Перезапустить Steam', installPlugin: 'Установить плагин', repairPlugin: 'Восстановить плагин',
      fixerTitle: 'Steam не открывается?',
      fixerDesc: 'Если Steam не запускается корректно, запустите автоматическое восстановление.',
      fixerBtn: 'Запустить восстановление', fixerRunning: 'Выполняется...',
    },
    detail: {
      back: 'Назад к играм', loading: 'Загрузка...', noTrailer: 'Трейлер недоступен',
      screenshots: 'Скриншоты', noScreenshots: 'Скриншоты недоступны', download: 'Скачать',
    },
    catalog: { search: 'Поиск по названию или AppID...', results: (n: number, q: string) => `${n} ${ruPlural(n, 'результат', 'результата', 'результатов')} по запросу "${q}"`, allCategories: 'Все' },
    library: { search: 'Поиск в библиотеке...' },
    saves: {
      title: 'Облачные сохранения',
      connect: 'Подключить Google Drive',
      connected: 'Google Drive подключён',
      connecting: 'Подключение...',
      backup: 'Создать резервную копию',
      backing: 'Сохранение...',
      restore: 'Восстановить',
      restoring: 'Восстановление...',
      noSaves: 'Сохранения для этой игры не найдены в userdata Steam.',
      noBackups: 'Резервных копий пока нет. Нажмите «Создать резервную копию», чтобы создать её.',
      backupOk: 'Резервное копирование завершено!',
      restoreOk: 'Сохранения успешно восстановлены!',
      sizeLabel: 'Размер',
      credentials: 'Добавьте свои учётные данные Google OAuth2 в cloud_saves.rs перед использованием этой функции.',
    },
    online: { goTo: 'Перейти на сайт' },
    settings: {
      steam: 'Steam',
      updates: 'Обновления', lastChecked: 'Последняя проверка', never: 'Никогда',
      checkUpdates: 'Проверить обновления', checking: 'Проверка...', upToDate: 'У вас последняя версия!', upToDateSub: (v: string) => `v${v} — последняя версия.`,
      language: 'Язык', languageDesc: 'Выберите язык интерфейса.',
    },
    notify: {
      fixer: 'Выполняется восстановление...', fixerOk: 'Восстановление запущено — подтвердите UAC, если появится запрос', fixerErr: 'Ошибка при восстановлении: ',
      restarting: 'Перезапуск Steam...', restartErr: 'Ошибка: ', catalogErr: 'Не удалось загрузить каталог',
      preparing: 'Подготовка установки для AppID: ', injected: 'Манифест успешно внедрён!', installErr: 'Ошибка установки: ',
    },
    update: {
      detected: 'ОБНАРУЖЕНА НОВАЯ ВЕРСИЯ',
      auto: 'Лаунчер автоматически обновится через',
      seconds: 'с',
      cancel: 'Отмена',
      updateNow: 'Обновить сейчас',
      downloading: 'Загрузка обновления...',
      noUpdate: 'У вас последняя версия!',
      noUpdateSub: (v: string) => `v${v} — последняя версия.`,
      checkBtn: 'Проверить обновления',
      checking: 'Проверка...',
    },
    fix: {
      title: 'Панель восстановления',
      subtitle: 'Глубокое восстановление манифестов для игр, застрявших на «Загрузка» или показывающих «Купить».',
      appIdLabel: 'Steam AppID',
      appIdPlaceholder: 'напр. 730',
      startBtn: 'Начать восстановление',
      repairing: 'Восстановление...',
      steamPath: 'Путь к Steam',
      step1: 'Чтение Lua-трекера...',
      step2: 'Запрос к SteamCMD...',
      step3: 'Загрузка манифестов...',
      step4: 'Внедрение в depotcache...',
      done: 'Восстановление завершено',
      error: 'Ошибка восстановления',
      clearLog: 'Очистить',
    },
    appearance: {
      title: 'Внешний вид',
      accent: 'Акцентный цвет',
      accentDesc: 'Настройте акцентный цвет лаунчера.',
      background: 'Динамический фон',
      backgroundDesc: 'Путь к изображению или видео (mp4/webm). Оставьте пустым для автоматических сезонных фонов.',
      backgroundPlaceholder: 'C:\\путь\\к\\фону.mp4',
      backgroundClear: 'Очистить',
    },
    notifications: {
      title: 'Уведомления',
      empty: 'Нет уведомлений',
      update: (v: string) => `Доступно новое обновление: v${v}`,
      newGames: (n: number) => `${n} ${ruPlural(n, 'новая игра', 'новые игры', 'новых игр')} добавлено в каталог`,
      markRead: 'Отметить всё как прочитанное',
    },
    support: {
      title: 'Поддержка',
      subtitle: 'Сообщите об ошибках или предложите улучшения для Ragnarok Launcher.',
      typeLabel: 'Тип проблемы',
      descLabel: 'Описание',
      descPlaceholder: 'Опишите проблему или предложение как можно подробнее...',
      sendBtn: 'Отправить отчёт',
      sending: 'Отправка...',
      sent: 'Отправлено!',
      noWebhook: 'Webhook Discord не настроен. Свяжитесь с разработчиком.',
      categories: {
        cache: 'Ошибка кэша (программа ничего не загружает)',
        download: 'Игры не скачиваются',
        install: 'Ошибка установки игры',
        steam: 'Проблема, связанная со Steam',
        crash: 'Программа неожиданно закрывается',
        feature: 'Отсутствует функция / Предложение',
        other: 'Другая ошибка',
      },
    },
  },
} as const;

// ⚠️  Rellena esta URL con tu webhook de Discord antes de compilar.
// Crea uno en: Discord Server → Configuración → Integraciones → Webhooks
const DISCORD_SUPPORT_WEBHOOK = 'https://discord.com/api/webhooks/1497130183772344481/psgqq-vqyY4eHdCyhRQcvl4MkgzahRGpZZdpCk2GdWrC8xB8l8_W7lNifRraaIVPymZ1';

type TranslationStructure = typeof TRANSLATIONS['en'];
type Lang = keyof typeof TRANSLATIONS;

// Drives both the Settings language grid and the fallback logic in ti()
// below — adding a language means adding it here (and to TRANSLATIONS),
// nowhere else.
// No emoji here: Windows has no font for regional-indicator flags and draws
// the two letters instead, so every flag in the picker rendered as "US", "ES",
// "BR"... See FlagIcon, which draws them.
const LANGUAGE_OPTIONS: { code: Lang; label: string }[] = [
  { code: 'en', label: 'EN' },
  { code: 'es', label: 'ES' },
  { code: 'pt', label: 'PT' },
  { code: 'fr', label: 'FR' },
  { code: 'ru', label: 'RU' },
];

const LanguageContext = React.createContext<{
  lang: Lang;
  setLang: (l: Lang) => void;
  t: TranslationStructure
}>({
  lang: 'en', setLang: () => { }, t: TRANSLATIONS.en,
});
export const useLanguage = () => React.useContext(LanguageContext);

// Inline translator for one-off strings that don't warrant a TRANSLATIONS
// key: ti('texto en español', 'english text') returns whichever matches the
// current language. The 125+ existing call sites only ever pass these first
// two — extra languages are opt-in via a 3rd, optional object, so none of
// those calls needed to change to support pt/fr/ru. Any language not in
// `extra` (or not yet given a translation there) falls back to English,
// same as an untranslated string would in any i18n setup — never blank,
// never Spanish-by-accident.
// Memoized on `lang`, not rebuilt per render. `ti` is a dependency of a lot
// of effects and useCallbacks across this file; handing each of them a brand
// new function identity on every single render made those effects re-run
// constantly — the Ragnarok Legends loader was re-firing on every keystroke
// anywhere in the app for exactly this reason.
export const useTranslateInline = () => {
  const { lang } = useLanguage();
  return useCallback(
    (es: string, en: string, extra?: Partial<Record<Exclude<Lang, 'en' | 'es'>, string>>) => {
      if (lang === 'es') return es;
      if (lang === 'en') return en;
      return extra?.[lang] ?? en;
    },
    [lang]
  );
};

const LanguageProvider = ({ children }: { children: React.ReactNode }) => {
  const [lang, setLangState] = useState<Lang>(() => (localStorage.getItem('rl_lang') as Lang) ?? 'en');
  const setLang = useCallback((l: Lang) => { setLangState(l); localStorage.setItem('rl_lang', l); }, []);
  // A fresh object literal here changes the context value on every provider
  // render, which re-renders every consumer in the app — and every component
  // in this file is a consumer.
  const value = useMemo(
    () => ({ lang, setLang, t: TRANSLATIONS[lang] as unknown as TranslationStructure }),
    [lang, setLang]
  );
  return (
    <LanguageContext.Provider value={value}>
      {children}
    </LanguageContext.Provider>
  );
};

// --- Theme ---

interface ThemeCtx {
  theme: 'dark' | 'light';
  setTheme: (t: 'dark' | 'light') => void;
  accent: string;
  setAccent: (c: string) => void;
  bgPath: string;
  setBgPath: (p: string) => void;
}

const ThemeContext = React.createContext<ThemeCtx>({
  theme: 'dark', setTheme: () => { },
  accent: '#dc2626', setAccent: () => { },
  bgPath: '', setBgPath: () => { },
});
const useTheme = () => React.useContext(ThemeContext);

const ACCENT_PRESETS = [
  '#dc2626', // red
  '#ea580c', // orange
  '#ca8a04', // yellow
  '#16a34a', // green
  '#0891b2', // cyan
  '#2563eb', // blue
  '#7c3aed', // violet
  '#db2777', // pink
];

const ThemeProvider = ({ children }: { children: React.ReactNode }) => {
  const [theme, setThemeRaw] = useState<'dark' | 'light'>(
    () => (localStorage.getItem('rl_theme') as 'dark' | 'light') ?? 'dark'
  );
  const [accent, setAccentRaw] = useState(
    () => localStorage.getItem('rl_accent') ?? '#dc2626'
  );
  const [bgPath, setBgPathRaw] = useState(
    () => localStorage.getItem('rl_bg_path') ?? ''
  );

  const setTheme = (t: 'dark' | 'light') => { setThemeRaw(t); localStorage.setItem('rl_theme', t); };
  const setAccent = (c: string) => { setAccentRaw(c); localStorage.setItem('rl_accent', c); };
  const setBgPath = (p: string) => { setBgPathRaw(p); localStorage.setItem('rl_bg_path', p); };

  useEffect(() => {
    const html = document.documentElement;
    if (theme === 'light') {
      html.classList.add('light');
      html.classList.remove('dark');
    } else {
      html.classList.add('dark');
      html.classList.remove('light');
    }
    const hexToRgb = (hex: string) => {
      const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
      return result ? `${parseInt(result[1], 16)} ${parseInt(result[2], 16)} ${parseInt(result[3], 16)}` : '220 38 38';
    };
    html.style.setProperty('--accent-color', hexToRgb(accent));
    html.style.setProperty('--accent-color-hex', accent);
  }, [theme, accent]);

  return (
    <ThemeContext.Provider value={{ theme, setTheme, accent, setAccent, bgPath, setBgPath }}>
      {children}
    </ThemeContext.Provider>
  );
};

// --- Steam CDN helpers ---

const STEAM_HEADERS = (id: string): string[] => [
  `https://cdn.akamai.steamstatic.com/steam/apps/${id}/header.jpg`,
  `https://cdn.cloudflare.steamstatic.com/steam/apps/${id}/header.jpg`,
  `https://steamcdn-a.akamaihd.net/steam/apps/${id}/header.jpg`,
  `https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/${id}/header.jpg`,
  `https://cdn.akamai.steamstatic.com/steam/apps/${id}/capsule_616x353.jpg`,
];

// Fetches via Rust backend (bypasses CORS)
async function fetchSteamMedia(appId: string, lang: string): Promise<SteamMedia> {
  try {
    return await invoke<SteamMedia>('fetch_steam_data', { appId, lang });
  } catch {
    return { screenshots: [], trailer_mp4: null, trailer_webm: null, trailer_thumbnail: null, name: null, metacritic_score: null, minimum_reqs: null, recommended_reqs: null, short_description: null };
  }
}

// Lightweight batch name fetch — much faster than fetchSteamMedia
// fetch_game_names actually returns { [id]: [name, hasDenuvo] } tuples, not
// bare name strings — a caller elsewhere (Denuvo badge) already destructures
// it that way. This wrapper's return type used to lie about that (declared
// Record<string, string>), so its one caller stored the whole [name, bool]
// tuple as `game.name`, which is truthy but has no .trim() — that's what
// was crashing the whole window with "game.name?.trim is not a function"
// wherever a game resolved through this path got rendered.
async function fetchGameNames(appIds: string[]): Promise<Record<string, [string, string]>> {
  try {
    return await invoke<Record<string, [string, string]>>('fetch_game_names', { appIds });
  } catch {
    return {};
  }
}

// Reads the persisted name cache, coercing out any [name, hasDenuvo] tuples
// the bug above already wrote to localStorage on past runs (fixing the
// resolver alone doesn't retroactively clean up what it already saved —
// every future load would keep reinjecting those corrupted array values
// into `game.name` and crashing the same way). Re-saves the cache once it
// finds anything to fix, so this is a one-time cleanup per corrupted entry.
const getCleanNameCache = (): Record<string, string> => {
  let raw: unknown;
  try { raw = JSON.parse(localStorage.getItem('rl_names') ?? '{}'); } catch { raw = {}; }
  const obj = (raw && typeof raw === 'object') ? raw as Record<string, unknown> : {};
  const clean: Record<string, string> = {};
  let hadCorruption = false;
  for (const [id, value] of Object.entries(obj)) {
    if (typeof value === 'string') {
      clean[id] = value;
    } else if (Array.isArray(value) && typeof value[0] === 'string') {
      clean[id] = value[0];
      hadCorruption = true;
    }
    // Anything else (unexpected shape) is dropped rather than risking
    // another non-string `name` downstream.
  }
  if (hadCorruption) localStorage.setItem('rl_names', JSON.stringify(clean));
  return clean;
};

/**
 * Pulls a Steam AppID out of whatever the user pasted.
 *
 * People share a game as a link, never as a number. Every one of these forms
 * turns up in support threads — the store page, a community page, SteamDB, and
 * the `steam://` protocol — and until now each of them meant the user had to
 * find the digits in the URL and retype them.
 *
 * Returns null when there is no id in the text, so a caller can fall back to
 * treating the input as an ordinary search term.
 */
export const parseSteamAppId = (input: string): string | null => {
  const text = input.trim();
  // Already just an id. Bounded so a pasted build number or a timestamp does
  // not get mistaken for one.
  if (/^\d{1,8}$/.test(text)) return text;

  const fromUrl = text.match(
    /(?:store\.steampowered\.com|steamcommunity\.com|steamdb\.info)\/(?:app|apps)\/(\d+)/i
  );
  if (fromUrl) return fromUrl[1];

  const fromProtocol = text.match(/steam:\/\/(?:store|run|rungameid|install)\/(\d+)/i);
  if (fromProtocol) return fromProtocol[1];

  return null;
};

/**
 * Strips everything executable out of publisher-written HTML.
 *
 * Steam's `pc_requirements` is markup the game's publisher supplies, not
 * something Valve authors, and it was going straight into
 * `dangerouslySetInnerHTML`. `innerHTML` will not run a `<script>`, but it very
 * much fires `<img src=x onerror=...>` — and the context here is a Tauri
 * webview with the full API surface exposed, which turns that into code
 * execution on the machine rather than a defaced panel.
 *
 * Parsed with DOMParser into an inert document (nothing loads or executes
 * there), then rebuilt keeping only the tags this content actually uses for
 * layout, with every attribute dropped. The same DOMParser approach is already
 * used in SpecMatcherView to read these strings — this only preserves the
 * formatting instead of flattening it to text.
 */
const sanitizeRequirementsHtml = (html: string): string => {
  const ALLOWED = new Set(['BR', 'STRONG', 'B', 'EM', 'I', 'UL', 'OL', 'LI', 'P', 'SPAN', 'DIV']);
  try {
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const clean = (node: Node): string => {
      if (node.nodeType === Node.TEXT_NODE) {
        return (node.textContent ?? '').replace(/[<>&]/g, c =>
          c === '<' ? '&lt;' : c === '>' ? '&gt;' : '&amp;'
        );
      }
      if (node.nodeType !== Node.ELEMENT_NODE) return '';
      const el = node as Element;
      const inner = Array.from(el.childNodes).map(clean).join('');
      // Not on the list: keep the text, drop the element and every attribute
      // it carried — which is where onerror/onload would have lived.
      if (!ALLOWED.has(el.tagName)) return inner;
      return el.tagName === 'BR' ? '<br>' : `<${el.tagName.toLowerCase()}>${inner}</${el.tagName.toLowerCase()}>`;
    };
    return Array.from(doc.body.childNodes).map(clean).join('');
  } catch {
    return '';
  }
};

// Fallback cuando todavia no se resolvio la ruta de Steam.
const DEFAULT_STEAM_PATH = 'C:\\Program Files (x86)\\Steam';

// --- Image cache component ---

// Loads images from local disk cache via the Rust backend.
// First visit: downloads from Steam CDN and caches to disk.
// All subsequent visits: instant load from disk, even offline.
const imageMemoryCache = new Map<string, string>();

// Failures are remembered, with a TTL. Without this, a cover that can't be
// fetched is retried in full — the backend walks a chain of seven candidate
// URLs per attempt — every single time its card scrolls back into view. The
// TTL is what keeps that from being permanent: someone who opened the app
// with no connection gets their covers once the connection comes back,
// instead of blank tiles until they restart.
const imageFailedAt = new Map<string, number>();
const IMAGE_FAILURE_TTL_MS = 5 * 60 * 1000;
const recentlyFailed = (appId: string) => {
  const at = imageFailedAt.get(appId);
  if (at === undefined) return false;
  if (Date.now() - at < IMAGE_FAILURE_TTL_MS) return true;
  imageFailedAt.delete(appId);
  return false;
};

// Cover requests run through a queue with a fixed width rather than all at
// once. A catalog page mounts ~30 cards simultaneously and each used to fire
// its own invoke immediately, so thirty downloads competed for the same
// connection — every one of them slower than if they had been run six at a
// time, on a link the user is probably also using for something else.
const IMAGE_CONCURRENCY = 6;
const imageQueue: string[] = [];
const imageWaiters = new Map<string, Array<(url: string | null) => void>>();
let imageActive = 0;

const pumpImageQueue = () => {
  while (imageActive < IMAGE_CONCURRENCY && imageQueue.length > 0) {
    const appId = imageQueue.shift()!;
    imageActive++;
    invoke<string>('get_cached_image_path', { appId })
      .then(path => {
        // Native asset:// URL — the WebView streams the file directly instead
        // of us shipping a base64-encoded copy of it through Tauri's IPC/JSON
        // bridge, which is what used to make scrolling the catalog feel laggy.
        const assetUrl = convertFileSrc(path);
        imageMemoryCache.set(appId, assetUrl);
        return assetUrl;
      })
      .catch(() => {
        imageFailedAt.set(appId, Date.now());
        return null;
      })
      .then(url => {
        imageActive--;
        const waiters = imageWaiters.get(appId) ?? [];
        imageWaiters.delete(appId);
        // Every waiter is settled, including on failure. The previous
        // implementation had each duplicate caller poll a Map every 200ms
        // and only ever clear that interval on success — a cover that failed
        // left one timer per card running for the life of the session.
        for (const w of waiters) w(url);
        pumpImageQueue();
      });
  }
};

/// Resolves to the asset URL, or null if the cover can't be fetched.
/// Callers asking for the same appId share one request.
const loadCoverArt = (appId: string): Promise<string | null> => {
  const hit = imageMemoryCache.get(appId);
  if (hit) return Promise.resolve(hit);
  if (recentlyFailed(appId)) return Promise.resolve(null);
  return new Promise(resolve => {
    const existing = imageWaiters.get(appId);
    if (existing) { existing.push(resolve); return; }
    imageWaiters.set(appId, [resolve]);
    imageQueue.push(appId);
    pumpImageQueue();
  });
};

const CachedImage = memo(({ appId, alt, className }: {
  appId: string, alt: string, className?: string
}) => {
  // One piece of state carrying the appId it describes, so a recycled card
  // (same component, different game) can't briefly show the previous game's
  // cover — which the old `[appId]`-only effect allowed, since it returned
  // early whenever `src` was already set.
  const [state, setState] = useState<{ id: string; src: string | null; failed: boolean }>(() => ({
    id: appId,
    src: imageMemoryCache.get(appId) ?? null,
    failed: recentlyFailed(appId),
  }));
  const { src, failed } = state;

  useEffect(() => {
    if (!appId) return;
    const hit = imageMemoryCache.get(appId);
    if (hit) {
      setState(prev => (prev.id === appId && prev.src === hit ? prev : { id: appId, src: hit, failed: false }));
      return;
    }
    if (recentlyFailed(appId)) {
      setState(prev => (prev.id === appId && prev.failed ? prev : { id: appId, src: null, failed: true }));
      return;
    }
    // Returning `prev` unchanged on the common first-mount path keeps this
    // from costing an extra render.
    setState(prev =>
      prev.id === appId && prev.src === null && !prev.failed ? prev : { id: appId, src: null, failed: false }
    );

    let cancelled = false;
    loadCoverArt(appId).then(url => {
      if (cancelled) return;
      setState({ id: appId, src: url, failed: url === null });
    });
    return () => { cancelled = true; };
  }, [appId]);

  if (failed) {
    const initials = alt.split(' ').slice(0, 2).map(w => w[0] ?? '').join('').toUpperCase();
    return (
      // The placeholder's own background lives on an inner, absolutely-positioned
      // layer instead of being merged into `className` — some callers pass their
      // own bg-* utility (e.g. bg-black/40) in className, and whichever bg-*
      // class Tailwind happens to emit later in the stylesheet used to win,
      // sometimes hiding this placeholder behind a solid color with no visible
      // fallback (looked like "the image just doesn't show sometimes").
      <div className={`${className} relative overflow-hidden`}>
        <div className="absolute inset-0 bg-gradient-to-br from-[#1a1b26] to-[#0a0a0c] flex items-center justify-center">
          <span className="text-3xl font-black text-white/20 select-none tracking-widest">{initials || '?'}</span>
        </div>
      </div>
    );
  }

  if (!src) {
    return (
      <div className={`${className} relative overflow-hidden`}>
        <div className="absolute inset-0 bg-white/5 animate-pulse flex items-center justify-center">
          <div className="w-8 h-8 rounded-full bg-white/10" />
        </div>
      </div>
    );
  }

  return <img src={src} alt={alt} className={className} loading="lazy" decoding="async" />;
});

// --- Shared components ---

const SidebarItem = memo(({ icon: Icon, label, active, onClick, section, collapsed }: { icon?: any, label: string, active: boolean, onClick: () => void, section?: boolean, collapsed?: boolean }) => {
  if (section) {
    if (collapsed) {
      return <div className="mt-3 mb-1.5 px-4 flex items-center justify-center"><span className="h-px w-5 bg-white/[0.08]" /></div>;
    }
    return (
      <div className="mt-3 mb-1.5 px-8 text-[9px] tracking-[0.25em] font-black text-gray-600 uppercase flex items-center gap-3">
        <span className="h-px w-4 bg-white/[0.08]" />
        {label}
        <span className="h-px flex-1 bg-gradient-to-r from-white/[0.08] to-transparent" />
      </div>
    );
  }

  return (
    <div className={collapsed ? 'px-2.5 py-0.5' : 'px-4 py-0.5'}>
      <motion.button
        onClick={onClick}
        whileHover={{ x: active || collapsed ? 0 : 2 }}
        whileTap={{ scale: 0.98 }}
        transition={{ duration: 0.15, ease: 'easeOut' }}
        title={collapsed ? label : undefined}
        className={`w-full flex items-center rounded-xl relative group overflow-hidden ${collapsed ? 'justify-center px-0 py-2' : 'gap-4 px-4 py-2'} ${
          active
            ? 'text-white'
            : 'text-gray-500 hover:text-white hover:bg-white/[0.03]'
        }`}
      >
        {active && (
          <motion.div
            layoutId="activeTabBg"
            transition={{ type: 'spring', stiffness: 420, damping: 34 }}
            className="absolute inset-0 bg-accent/[0.10] border border-accent/25 rounded-xl"
          />
        )}
        <div className={`relative z-10 flex items-center ${collapsed ? '' : 'gap-4'}`}>
          <div className={`relative flex items-center justify-center shrink-0 transition-colors duration-300 ${active ? 'text-accent' : 'group-hover:text-white'}`}>
            {active && <motion.div layoutId="activeIconGlow" className="absolute inset-0 blur-md bg-accent opacity-40 rounded-full" />}
            {Icon && <Icon size={17} className="relative z-10" />}
          </div>
          <AnimatePresence initial={false}>
            {!collapsed && (
              <motion.span
                initial={{ opacity: 0, width: 0 }}
                animate={{ opacity: 1, width: 'auto' }}
                exit={{ opacity: 0, width: 0 }}
                transition={{ duration: 0.2, ease: 'easeOut' }}
                className="text-[12px] tracking-widest font-black uppercase whitespace-nowrap overflow-hidden"
              >
                {label}
              </motion.span>
            )}
          </AnimatePresence>
        </div>
      </motion.button>
    </div>
  );
});

const StatCard = memo(({ label, value, colorClass, icon: Icon }: { label: string, value: string | number, colorClass: string, icon?: any }) => {
  const bgGlow = colorClass.includes('accent') ? 'bg-red-500' : colorClass.includes('green') ? 'bg-green-500' : 'bg-blue-500';
  return (
    <div className="relative overflow-hidden bg-white/[0.02] backdrop-blur-3xl border border-white/5 rounded-3xl p-6 flex flex-col gap-3 flex-1 hover:bg-white/[0.04] hover:border-white/10 transition-all duration-700 group shadow-2xl">
      <div className={`absolute -right-8 -top-8 w-32 h-32 rounded-full blur-[40px] opacity-20 transition-all duration-700 group-hover:opacity-40 group-hover:scale-150 ${bgGlow}`} />
      <div className="flex items-center justify-between z-10">
        <span className="text-[10px] uppercase tracking-[0.3em] text-gray-500 font-black">{label}</span>
        {Icon && <Icon size={18} className={`${colorClass} opacity-50 group-hover:opacity-100 transition-opacity duration-500`} />}
      </div>
      <span className={`text-5xl font-black z-10 drop-shadow-2xl tracking-tighter ${colorClass} group-hover:scale-105 origin-left transition-transform duration-500`}>
        {value}
      </span>
    </div>
  );
});

const ToolButton = memo(({ icon: Icon, label, color, onClick, disabled }: { icon: any, label: string, color: string, onClick?: () => void, disabled?: boolean }) => {
  const solidBg = color.replace('text-', 'bg-').replace('-400', '-500');
  return (
    <button onClick={onClick} disabled={disabled} className="relative overflow-hidden flex items-center gap-3 bg-[#0d0e12] hover:bg-[#141519] border border-white/[0.08] hover:border-white/[0.14] pl-2.5 pr-6 py-3 rounded-full transition-all duration-300 group disabled:opacity-40 disabled:cursor-not-allowed shadow-lg">
      <div className={`w-10 h-10 rounded-full ${solidBg} flex items-center justify-center shrink-0 shadow-lg group-hover:scale-110 transition-transform duration-300`}>
        <Icon size={18} className="text-white" />
      </div>
      <span className="text-[14px] font-black text-gray-200 tracking-wide group-hover:text-white transition-colors whitespace-nowrap">{label}</span>
    </button>
  );
});

// Reusable fallback image
const SteamImage = ({ id, fixedSrc, alt, className }: { id: string, fixedSrc?: string, alt: string, className?: string }) => {
  const [idx, setIdx] = useState(0);
  const [failed, setFailed] = useState(false);
  const sources = STEAM_HEADERS(id);
  const src = fixedSrc ?? sources[idx];

  return failed ? (
    <div className={`flex items-center justify-center bg-white/[0.02] ${className}`}>
      <Gamepad2 className="w-16 h-16 text-white/10" />
    </div>
  ) : (
    <img
      src={src}
      alt={alt}
      className={className}
      onError={() => idx < sources.length - 1 ? setIdx(i => i + 1) : setFailed(true)}
    />
  );
};

// Metacritic badge
/// The Metascore the way Metacritic draws it: a square tile in its own colour
/// bands (green from 75, yellow from 50, red below) with the verdict beside
/// it. The earlier pill squeezed the number into a tiny capsule next to an
/// all-caps "METACRITIC", and read as just another tag. `compact` is the tile
/// alone, for the corner of a game card.
const MetacriticBadge = ({ score, compact }: { score: number; compact?: boolean }) => {
  const ti = useTranslateInline();
  const tile = score >= 75
    ? 'bg-[#66cc33] text-black'
    : score >= 50 ? 'bg-[#ffcc33] text-black' : 'bg-[#ff4a4a] text-white';
  // Metacritic's own wording for each band.
  const verdict = score >= 90
    ? ti('Aclamación universal', 'Universal acclaim')
    : score >= 75
      ? ti('Críticas favorables', 'Generally favorable')
      : score >= 50
        ? ti('Críticas mixtas', 'Mixed or average')
        : score >= 20
          ? ti('Críticas desfavorables', 'Generally unfavorable')
          : ti('Rechazo abrumador', 'Overwhelming dislike');

  if (compact) {
    return (
      <span
        title={`Metascore ${score} · ${verdict}`}
        className={`w-8 h-8 rounded-lg flex items-center justify-center text-[13px] font-black tabular-nums shadow-lg shadow-black/50 ${tile}`}
      >
        {score}
      </span>
    );
  }
  return (
    <div title={`Metascore ${score}/100`} className="flex items-center gap-2.5">
      <span className={`w-9 h-9 rounded-lg flex items-center justify-center text-[15px] font-black tabular-nums ${tile}`}>
        {score}
      </span>
      <span className="flex flex-col leading-tight">
        <span className="text-[12px] font-black text-white/90">Metascore</span>
        <span className="text-[10px] font-semibold text-gray-500">{verdict}</span>
      </span>
    </div>
  );
};

// --- Steam Install Picker ---
// Shown once at startup only when detect_steam_installations() finds more
// than one real Steam client on the machine (a folder with its own
// Steam.exe — not just a library folder). Without this, Ragnarok silently
// trusted whichever install the Windows registry remembered as "most
// recently active", which can be the wrong one if the user actually runs a
// different Steam install — the plugin would then get written into a Steam
// that's currently inactive and do nothing, with no error to explain why.
const SteamInstallPickerModal = ({
  choices,
  defaultPath,
  onSelect,
  onSkip,
}: {
  choices: string[];
  defaultPath: string;
  onSelect: (path: string) => void;
  onSkip: () => void;
}) => {
  const { lang } = useLanguage();
  const es = lang === 'es';

  // Portal'd to document.body — this can be rendered from inside the
  // tab-switch wrapper (whose inline Framer Motion transform traps
  // `position: fixed` descendants to the content pane instead of the real
  // window), same fix as the other modals in this file.
  return createPortal(
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      className="fixed inset-0 z-[100] flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,0.75)', backdropFilter: 'blur(8px)' }}
    >
      <motion.div
        initial={{ scale: 0.94, opacity: 0 }}
        animate={{ scale: 1, opacity: 1 }}
        exit={{ scale: 0.94, opacity: 0 }}
        className="relative w-full max-w-md bg-[#13141c] border border-white/10 rounded-3xl shadow-2xl overflow-hidden"
      >
        <div className="flex items-center gap-3 px-6 py-5 border-b border-white/5">
          <div className="w-9 h-9 rounded-xl bg-accent/15 border border-accent/25 flex items-center justify-center shrink-0">
            <HardDrive size={16} className="text-accent" />
          </div>
          <div className="min-w-0">
            <h3 className="font-black text-sm text-white uppercase tracking-wider">
              {es ? 'Se encontró más de un Steam' : 'More than one Steam found'}
            </h3>
          </div>
        </div>

        <div className="p-6 space-y-4">
          <p className="text-[13px] text-gray-300 leading-relaxed">
            {es
              ? 'Se detectaron varias instalaciones de Steam en esta PC. Elegí con cuál trabaja Ragnarok — si elegís la que no usás, el plugin se instala ahí y no vas a ver ningún cambio.'
              : "Several Steam installs were found on this PC. Choose which one Ragnarok should work with — picking the one you don't use means the plugin installs there and you won't see any change."}
          </p>

          <div className="space-y-2">
            {choices.map(path => (
              <motion.button
                key={path}
                whileHover={{ y: -1 }}
                whileTap={{ scale: 0.98 }}
                onClick={() => onSelect(path)}
                className="w-full flex items-center gap-3 px-4 py-3 rounded-xl bg-white/[0.03] hover:bg-accent/[0.08] border border-white/[0.08] hover:border-accent/30 transition-colors text-left group"
              >
                <HardDrive size={14} className="text-gray-500 group-hover:text-accent shrink-0 transition-colors" />
                <span className="text-[12px] font-semibold text-gray-200 group-hover:text-white break-all">{path}</span>
              </motion.button>
            ))}
          </div>

          <button
            onClick={onSkip}
            className="w-full py-2.5 text-[11px] font-bold text-gray-500 hover:text-gray-300 transition-colors"
          >
            {es ? `Usar detección automática (${defaultPath})` : `Use automatic detection (${defaultPath})`}
          </button>
        </div>
      </motion.div>
    </motion.div>,
    document.body
  );
};

// --- Update Countdown Overlay ---

const UpdateCountdownOverlay = ({
  info,
  onCancel,
}: {
  info: UpdateInfo;
  onCancel: () => void;
}) => {
  const { t } = useLanguage();
  const [count, setCount] = useState(10);
  const [downloading, setDownloading] = useState(false);
  const [dlProgress, setDlProgress] = useState(0);
  const [dlError, setDlError] = useState<string | null>(null);

  useEffect(() => {
    if (downloading) return;
    if (count <= 0) { triggerUpdate(); return; }
    const id = setTimeout(() => setCount(c => c - 1), 1000);
    return () => clearTimeout(id);
  }, [count, downloading]);

  // Listen for download progress events from Rust
  useEffect(() => {
    if (!downloading) return;
    let unlisten: (() => void) | undefined;
    listen<number>('update_download_progress', e => setDlProgress(e.payload))
      .then(fn => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, [downloading]);

  const triggerUpdate = async () => {
    if (!info.download_url) return;
    setDownloading(true);
    setDlError(null);
    setDlProgress(0);
    try {
      await invoke('download_and_apply_update', { downloadUrl: info.download_url });
      // App will exit from Rust side — if we get here something went wrong
    } catch (e: any) {
      // If progress reached 95%+, the app is closing/restarting normally — not an error
      setDlProgress(prev => {
        if (prev >= 95) return prev; // app exiting, ignore
        const msg = String(e);
        setDlError(msg || 'Error desconocido al descargar');
        setDownloading(false);
        return prev;
      });
    }
  };

  // Portal'd to document.body — same fixed-positioning fix as the other
  // modals in this file (see SteamInstallPickerModal's comment above).
  return createPortal(
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      className="fixed inset-0 z-[200] flex items-center justify-center"
      style={{ background: 'radial-gradient(ellipse at center, rgba(220,38,38,0.08) 0%, rgba(0,0,0,0.88) 70%)', backdropFilter: 'blur(16px)' }}
    >
      <motion.div
        initial={{ scale: 0.88, opacity: 0, y: 20 }}
        animate={{ scale: 1, opacity: 1, y: 0 }}
        exit={{ scale: 0.88, opacity: 0, y: 20 }}
        transition={{ type: 'spring', stiffness: 280, damping: 22 }}
        className="relative w-full max-w-md mx-5 overflow-hidden"
      >
        {/* Glass card */}
        <div className="relative bg-[#0e0f18]/95 border border-white/8 rounded-3xl overflow-hidden shadow-2xl"
          style={{ boxShadow: '0 0 0 1px rgba(220,38,38,0.15), 0 32px 64px rgba(0,0,0,0.7), 0 0 80px rgba(220,38,38,0.06)' }}>

          {/* Top gradient bar */}
          <div className="h-[3px] w-full bg-gradient-to-r from-transparent via-red-500 to-transparent" />

          {/* Background glow orbs */}
          <div className="absolute -top-16 -right-16 w-48 h-48 bg-red-600/8 blur-[60px] rounded-full pointer-events-none" />
          <div className="absolute -bottom-16 -left-16 w-48 h-48 bg-red-900/10 blur-[60px] rounded-full pointer-events-none" />

          <div className="relative z-10 p-7 space-y-6">

            {/* Version badge row */}
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <span className="w-2 h-2 rounded-full bg-red-500 animate-pulse" />
                <span className="text-[9px] font-black uppercase tracking-[0.25em] text-red-400">{t.update.detected}</span>
              </div>
              <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-white/5 border border-white/8">
                <span className="text-[9px] font-bold text-gray-500">v{info.current_version}</span>
                <span className="text-[9px] text-gray-600">→</span>
                <span className="text-[9px] font-black text-white">v{info.latest_version}</span>
              </div>
            </div>

            {/* Main content: countdown or download */}
            {!downloading ? (
              <>
                {/* Countdown display */}
                <div className="flex items-center gap-6 py-1">
                  {/* Ring */}
                  <div className="relative w-20 h-20 shrink-0">
                    <svg className="w-full h-full -rotate-90" viewBox="0 0 100 100">
                      <circle cx="50" cy="50" r="42" fill="none" stroke="rgba(255,255,255,0.04)" strokeWidth="7" />
                      <circle
                        cx="50" cy="50" r="42" fill="none"
                        stroke="#dc2626" strokeWidth="7" strokeLinecap="round"
                        strokeDasharray={`${2 * Math.PI * 42}`}
                        strokeDashoffset={`${2 * Math.PI * 42 * (1 - count / 10)}`}
                        style={{ transition: 'stroke-dashoffset 1s linear', filter: 'drop-shadow(0 0 6px rgba(220,38,38,0.6))' }}
                      />
                    </svg>
                    <div className="absolute inset-0 flex items-center justify-center flex-col">
                      <span className="text-2xl font-black text-white tabular-nums leading-none">{count}</span>
                      <span className="text-[8px] text-gray-600 font-bold uppercase tracking-wider">seg</span>
                    </div>
                  </div>
                  {/* Text */}
                  <div className="space-y-1">
                    <h2 className="text-xl font-black text-white tracking-tight leading-tight">
                      Nueva versión<br />disponible
                    </h2>
                    <p className="text-[11px] text-gray-500 leading-relaxed">
                      {t.update.auto} <span className="text-red-400 font-black">{count}s</span>
                    </p>
                  </div>
                </div>

                {/* Action buttons */}
                <div className="flex gap-2.5">
                  <button
                    onClick={onCancel}
                    className="flex-1 py-3 bg-white/5 hover:bg-white/8 border border-white/8 hover:border-white/15 rounded-2xl font-black text-[11px] uppercase tracking-widest text-gray-400 hover:text-white transition-all duration-200"
                  >
                    {t.update.cancel}
                  </button>
                  <button
                    onClick={triggerUpdate}
                    className="flex-1 py-3 rounded-2xl font-black text-[11px] uppercase tracking-widest text-white transition-all duration-200 flex items-center justify-center gap-2"
                    style={{ background: 'linear-gradient(135deg, #dc2626 0%, #991b1b 100%)', boxShadow: '0 4px 20px rgba(220,38,38,0.35)' }}
                  >
                    <Download size={13} />
                    {t.update.updateNow}
                  </button>
                </div>
              </>
            ) : (
              /* Download progress */
              <div className="space-y-4 py-2">
                {dlError ? (
                  <div className="space-y-3 text-center">
                    <div className="w-12 h-12 rounded-2xl bg-red-500/10 border border-red-500/20 flex items-center justify-center mx-auto">
                      <AlertTriangle size={20} className="text-red-400" />
                    </div>
                    <div>
                      <p className="text-sm font-black text-white">Error al descargar</p>
                      <p className="text-[11px] text-gray-500 mt-1 break-all leading-relaxed">{dlError}</p>
                    </div>
                    <button
                      onClick={() => triggerUpdate()}
                      className="px-5 py-2.5 bg-accent hover:brightness-125 rounded-xl text-[11px] font-black uppercase tracking-widest text-white transition-all"
                    >
                      Reintentar
                    </button>
                  </div>
                ) : (
                  <div className="space-y-3">
                    <div className="flex items-center gap-3">
                      <div className="w-10 h-10 rounded-xl bg-red-500/10 border border-red-500/20 flex items-center justify-center shrink-0">
                        <RefreshCw size={16} className="text-red-400 animate-spin" />
                      </div>
                      <div className="flex-1 min-w-0">
                        <p className="text-sm font-black text-white">
                          {dlProgress > 0 ? `Descargando... ${dlProgress}%` : 'Iniciando descarga...'}
                        </p>
                        <p className="text-[10px] text-gray-600">Ragnarok Launcher v{info.latest_version}</p>
                      </div>
                    </div>
                    <div className="space-y-1.5">
                      <div className="h-1.5 w-full bg-white/5 rounded-full overflow-hidden">
                        <motion.div
                          className="h-full rounded-full"
                          style={{ background: 'linear-gradient(90deg, #dc2626, #f87171)' }}
                          animate={{ width: `${dlProgress}%` }}
                          transition={{ duration: 0.3 }}
                        />
                      </div>
                      <p className="text-[9px] text-gray-600 text-right tabular-nums">{dlProgress}% completado</p>
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      </motion.div>
    </motion.div>,
    document.body
  );
};

// --- Home View ---

/// What `check_plugin_update` reports about the plugin published in the games
/// repo. Dates are ISO strings straight from GitHub.
interface PluginUpdateInfo {
  available: boolean;
  published_at: string;
  seen_at: string;
}

/// "hace 2 días" reads as news; "7/15/2026, 12:19:43 AM" reads as a log line.
/// The exact timestamp is still shown underneath for anyone checking against
/// the repo.
const publishedAgo = (iso: string, es: boolean): string => {
  const then = new Date(iso).getTime();
  if (!isFinite(then)) return iso;
  const mins = Math.max(0, Math.round((Date.now() - then) / 60000));
  if (mins < 1) return es ? 'recién ahora' : 'just now';
  if (mins < 60) return es ? `hace ${mins} min` : `${mins} min ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return es ? `hace ${hours} h` : `${hours} h ago`;
  const days = Math.round(hours / 24);
  if (days < 30) return es ? `hace ${days} día${days === 1 ? '' : 's'}` : `${days} day${days === 1 ? '' : 's'} ago`;
  const months = Math.round(days / 30);
  return es ? `hace ${months} mes${months === 1 ? '' : 'es'}` : `${months} month${months === 1 ? '' : 's'} ago`;
};

const HomeView = ({ games, steamPath, onNavigate, loading, setGames }: { games: Game[]; steamPath: string; onNavigate: (tab: string) => void; loading?: boolean; setGames: React.Dispatch<React.SetStateAction<Game[]>> }) => {
  const { notify } = useNotify();
  const { t, lang } = useLanguage();
  const ti = useTranslateInline();
  const [isSteamRunning, setIsSteamRunning] = useState(false);
  const [fixerRunning, setFixerRunning] = useState(false);
  const [pluginRunning, setPluginRunning] = useState(false);
  const [pluginLogs, setPluginLogs] = useState<string[]>([]);
  const [showPluginModal, setShowPluginModal] = useState(false);
  const [showManualLuaModal, setShowManualLuaModal] = useState(false);
  const [manualLuaBusy, setManualLuaBusy] = useState(false);
  const [manualLuaResult, setManualLuaResult] = useState<{ message: string; games: { id: string; name: string; image?: string }[] } | null>(null);
  const [manualLuaDragOver, setManualLuaDragOver] = useState(false);
  const [cleaningZeroByte, setCleaningZeroByte] = useState(false);
  const [showZeroByteConfirm, setShowZeroByteConfirm] = useState(false);

  // The page-wide Steam check only runs every 8 s, which is too slow for this
  // dialog: someone closes Steam specifically to press Continuar, and a button
  // that stays disabled for several seconds afterwards reads as broken.
  useEffect(() => {
    if (!showZeroByteConfirm) return;
    const check = async () => {
      try { setIsSteamRunning(await invoke('is_steam_running') as boolean); } catch { }
    };
    check();
    const interval = setInterval(check, 1500);
    return () => clearInterval(interval);
  }, [showZeroByteConfirm]);

  useEffect(() => {
    const check = async () => {
      try { setIsSteamRunning(await invoke('is_steam_running') as boolean); } catch { }
    };
    check();
    const interval = setInterval(check, 8000);
    return () => clearInterval(interval);
  }, []);

  const handleRestartSteam = async () => {
    notify(t.notify.restarting, 'info');
    try { await invoke('restart_steam'); } catch (err) { notify(`${t.notify.restartErr}${err}`, 'error'); }
  };

  // Whether the plugin published in the games repo differs from the one
  // installed here. Checked once on mount: it is a single metadata request,
  // and the answer only changes when Sheaker uploads a new zip.
  const [pluginUpdate, setPluginUpdate] = useState<PluginUpdateInfo | null>(null);

  // Held back for 2.1.3. The check and the modal below both work, but the
  // plugin published in the repo is not yet the one that should be handed
  // out — it lacks the `opensteamtool/` signature files and contains a copy
  // of the user's SteamTools.lua. Announcing an update to it would be worse
  // than announcing nothing.
  //
  // Re-enabling is this effect: everything it feeds is still here.
  useEffect(() => {
    setPluginUpdate(null);
  }, []);

  /// Marks this publish date as seen so the notice does not come back until
  /// the file changes again.
  const dismissPluginUpdate = async () => {
    const info = pluginUpdate;
    setPluginUpdate(null);
    if (info) {
      await invoke('dismiss_plugin_update', { publishedAt: info.published_at }).catch(() => {});
    }
  };

  /// Downloads the published plugin and then installs it through the normal
  /// path, which is what closes Steam and handles locked DLLs.
  const handleUpdatePlugin = async () => {
    setPluginUpdate(null);
    setPluginRunning(true);
    setPluginLogs([]);
    setShowPluginModal(true);
    const unlisten = await listen<string>('plugin_log', e => {
      setPluginLogs(prev => [...prev, e.payload]);
    });
    try {
      await invoke('download_plugin_update');
      await invoke('install_plugin');
      notify(ti('Plugin actualizado correctamente', 'Plugin updated successfully'), 'success');
      // install_plugin already recorded the date, so nothing is pending.
      setPluginUpdate(null);
    } catch (err) {
      setPluginLogs(prev => [...prev, `✗ Error: ${err}`]);
      notify(`${ti('Error al actualizar el plugin', 'Failed to update the plugin')}: ${err}`, 'error');
    } finally {
      unlisten();
      setPluginRunning(false);
    }
  };

  const handleInstallPlugin = async () => {
    setPluginRunning(true);
    setPluginLogs([]);
    setShowPluginModal(true);
    const unlisten = await listen<string>('plugin_log', e => {
      setPluginLogs(prev => [...prev, e.payload]);
    });
    try {
      await invoke('install_plugin');
      notify('Plugin instalado correctamente', 'success');
    } catch (err) {
      setPluginLogs(prev => [...prev, `✗ Error: ${err}`]);
      notify(`Error al instalar plugin: ${err}`, 'error');
    } finally {
      unlisten();
      setPluginRunning(false);
    }
  };

  const handleRepairPlugin = async () => {
    setPluginRunning(true);
    setPluginLogs([]);
    setShowPluginModal(true);
    const unlisten = await listen<string>('plugin_log', e => {
      setPluginLogs(prev => [...prev, e.payload]);
    });
    try {
      const result = await invoke<string>('repair_steam_plugin', { steamPath });
      // repair_steam_plugin can succeed at reinstalling the files but still
      // detect that Steam auto-updated to a build OpenSteamTool doesn't
      // have IPC compatibility data for yet — that's a real, different
      // situation from "all good now", so show whatever it actually
      // reported instead of always claiming success.
      const isIpcGap = result.includes('IPC');
      notify(result, isIpcGap ? 'error' : 'success');
    } catch (err) {
      setPluginLogs(prev => [...prev, `✗ Error: ${err}`]);
      notify(`Error al reparar plugin: ${err}`, 'error');
    } finally {
      unlisten();
      setPluginRunning(false);
    }
  };


  // Removes the ACFs whose game folder is gone — the entries Steam lists at
  // 0 B in Almacenamiento and keeps offering Play for. The backend also strips
  // them from libraryfolders.vdf, which is the half that made the manual
  // workaround necessary in the first place.
  const handleCleanZeroByte = async () => {
    setCleaningZeroByte(true);
    try {
      const removed = await invoke<{ app_id: string; name: string; library: string }[]>(
        'clean_orphan_manifests',
        { steamPath }
      );
      if (removed.length === 0) {
        notify(ti('No hay juegos de 0 B que limpiar.', 'No 0 B entries to clean.'), 'info');
      } else {
        // Naming them matters: this deletes things from the user's Steam, and
        // "se limpiaron 3" gives them no way to tell whether that was right.
        const names = removed.map(r => r.name).join(', ');
        notify(
          ti(
            `Se limpiaron ${removed.length} juego(s) de 0 B: ${names}. Abrí Steam de nuevo.`,
            `Cleaned ${removed.length} 0 B entr${removed.length === 1 ? 'y' : 'ies'}: ${names}. Reopen Steam.`
          ),
          'success'
        );
      }
    } catch (err) {
      notify(`${err}`, 'error');
    } finally {
      setCleaningZeroByte(false);
    }
  };

  const handleRunFixer = async () => {
    setFixerRunning(true);
    notify(t.notify.fixer, 'info');
    try {
      await invoke('run_fixer');
      notify(t.notify.fixerOk, 'success');
    } catch (err) {
      notify(`${t.notify.fixerErr}${err}`, 'error');
    } finally {
      setTimeout(() => setFixerRunning(false), 3000);
    }
  };

  // Shared by both entry points — the "Seleccionar archivo" button (native
  // file dialog) and dropping a .zip/.rar directly onto the modal below.
  const processManualLuaArchive = async (archivePath: string) => {
    setManualLuaBusy(true);
    setManualLuaResult(null);
    try {
      const result = await invoke<{ message: string; games: { id: string; name: string; image?: string }[] }>(
        'install_manual_lua',
        { steamPath, archivePath }
      );
      setManualLuaResult(result);
      notify(result.message, 'success');
      // The backend registers these games (fake ACF marking them Installed,
      // sidecar entry) and even returns their id/name/image right here —
      // but nothing was ever done with that on the frontend, so they never
      // actually joined the app's `games` state. Library/Games/etc. all
      // filter over that state, so a manually-added game would install
      // correctly on disk yet never appear anywhere in the app itself.
      if (result.games.length > 0) {
        setGames(prev => {
          const byId = new Map(prev.map(g => [g.id, g]));
          for (const g of result.games) {
            const existing = byId.get(g.id);
            byId.set(g.id, existing
              ? { ...existing, name: g.name || existing.name, image: g.image ?? existing.image, status: 'Installed' }
              : { id: g.id, name: g.name, image: g.image, status: 'Installed' });
          }
          return Array.from(byId.values());
        });
      }
    } catch (err) {
      notify(ti(`Error: ${err}`, `Error: ${err}`), 'error');
    } finally {
      setManualLuaBusy(false);
    }
  };

  const handleAddManualLua = async () => {
    const archivePath = await open({
      directory: false,
      multiple: false,
      filters: [{ name: 'Archive', extensions: ['zip', 'rar'] }],
    });
    if (!archivePath || Array.isArray(archivePath)) return;
    await processManualLuaArchive(archivePath);
  };

  // Tauri's native OS file-drop — fires on the whole window (no coordinates
  // in this version's payload), so it's only wired up while the modal is
  // actually open rather than trying to scope it to the dropzone div.
  useEffect(() => {
    if (!showManualLuaModal) return;
    let cancelled = false;
    let unlistenDrop: (() => void) | null = null;
    let unlistenHover: (() => void) | null = null;
    let unlistenCancel: (() => void) | null = null;

    listen<string[]>('tauri://file-drop', (event) => {
      setManualLuaDragOver(false);
      if (manualLuaBusy || manualLuaResult) return;
      const path = event.payload.find(p => /\.(zip|rar)$/i.test(p));
      if (path) {
        processManualLuaArchive(path);
      } else {
        notify(ti('Solo se aceptan archivos .zip o .rar', 'Only .zip or .rar files are accepted'), 'error');
      }
    }).then(fn => { if (cancelled) fn(); else unlistenDrop = fn; });

    listen('tauri://file-drop-hover', () => setManualLuaDragOver(true))
      .then(fn => { if (cancelled) fn(); else unlistenHover = fn; });

    listen('tauri://file-drop-cancelled', () => setManualLuaDragOver(false))
      .then(fn => { if (cancelled) fn(); else unlistenCancel = fn; });

    return () => {
      cancelled = true;
      unlistenDrop?.();
      unlistenHover?.();
      unlistenCancel?.();
    };
  }, [showManualLuaModal, manualLuaBusy, manualLuaResult]);

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.35, ease: 'easeOut' }}
      className="space-y-4 max-w-5xl mx-auto px-4 pb-10"
    >

      {/* Hero status card */}
      <motion.div
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.35, ease: 'easeOut' }}
        className="relative overflow-hidden rounded-2xl bg-[#0d0e12] border border-white/[0.08] px-7 py-6 flex flex-col md:flex-row md:items-center gap-5 md:gap-8"
      >
        <div
          className="absolute inset-0 opacity-[0.04] pointer-events-none"
          style={{ backgroundImage: 'linear-gradient(rgba(255,255,255,0.6) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.6) 1px, transparent 1px)', backgroundSize: '28px 28px' }}
        />
        <div className={`absolute -top-20 -left-20 w-64 h-64 rounded-full blur-[80px] opacity-[0.15] pointer-events-none ${isSteamRunning ? 'bg-green-500' : 'bg-red-500'}`} />
        <div className={`absolute top-0 left-0 right-0 h-[2px] ${isSteamRunning ? 'bg-gradient-to-r from-green-500 via-green-500/40 to-transparent' : 'bg-gradient-to-r from-red-500 via-red-500/40 to-transparent'}`} />

        <div className="relative z-10 flex items-center gap-4 flex-1">
          <div className="relative shrink-0">
            <motion.div
              className={`absolute inset-0 rounded-full blur-lg ${isSteamRunning ? 'bg-green-500' : 'bg-red-500'}`}
              animate={{ opacity: [0.55, 0.2, 0.55] }}
              transition={{ duration: 2.2, repeat: Infinity, ease: 'easeInOut' }}
            />
            <div className={`w-14 h-14 rounded-full border-2 flex items-center justify-center bg-black/50 relative z-10 ${isSteamRunning ? 'border-green-500 text-green-500' : 'border-red-500 text-red-500'}`}>
              {isSteamRunning ? <CheckCircle size={24} /> : <XCircle size={24} />}
            </div>
          </div>
          <div className="text-left">
            <h2 className="text-xl font-black uppercase tracking-wide text-white/90">
              {isSteamRunning ? t.home.steamActive : t.home.steamClosed}
            </h2>
            <p className="text-gray-500 font-medium text-xs mt-0.5">
              {isSteamRunning ? ti('Todos los servicios operando con normalidad.', 'All services operating normally.') : ti('Steam no se encuentra en ejecución.', 'Steam is not running.')}
            </p>
          </div>
        </div>

        <div className="relative z-10 h-px w-full md:h-14 md:w-px bg-white/[0.08] shrink-0" />

        <div className="relative z-10 flex gap-10 justify-around md:justify-end shrink-0">
          <motion.button
            whileHover={{ y: -2 }}
            whileTap={{ scale: 0.96 }}
            onClick={() => onNavigate('Games')}
            title={ti('Ir a Games', 'Go to Games')}
            className="text-right group/stat outline-none"
          >
            <p className="text-[9px] font-black uppercase tracking-widest text-gray-500 group-hover/stat:text-gray-300 transition-colors">{t.home.totalGames}</p>
            {loading ? (
              <div className="h-8 w-14 mt-1.5 rounded-md bg-white/[0.06] animate-pulse ml-auto" />
            ) : (
              <p className="text-3xl font-black text-red-500 leading-none tabular-nums mt-1.5 group-hover/stat:brightness-125 transition-all">{games.length}</p>
            )}
          </motion.button>
          <motion.button
            whileHover={{ y: -2 }}
            whileTap={{ scale: 0.96 }}
            onClick={() => onNavigate('Library')}
            title={ti('Ir a la Librería', 'Go to Library')}
            className="text-right group/stat outline-none"
          >
            <p className="text-[9px] font-black uppercase tracking-widest text-gray-500 group-hover/stat:text-gray-300 transition-colors">{t.home.installed}</p>
            {loading ? (
              <div className="h-8 w-10 mt-1.5 rounded-md bg-white/[0.06] animate-pulse ml-auto" />
            ) : (
              <p className="text-3xl font-black text-green-400 leading-none tabular-nums mt-1.5 group-hover/stat:brightness-125 transition-all">{games.filter(g => g.status === 'Installed').length}</p>
            )}
          </motion.button>
        </div>
      </motion.div>

      {/* Quick fixes: repair, then manual Lua stacked below it */}
      <div className="grid grid-cols-1 gap-4">
        <motion.div
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.35, ease: 'easeOut', delay: 0.05 }}
          className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] px-5 py-4 flex items-center gap-4"
        >
          <div className="w-9 h-9 rounded-lg bg-amber-400/10 border border-amber-400/20 flex items-center justify-center shrink-0">
            <AlertTriangle size={16} className="text-amber-400" />
          </div>
          <div className="flex-1 min-w-0">
            <h4 className="font-black text-sm text-white/90 tracking-tight truncate">
              {t.home.fixerTitle}
            </h4>
            <p className="text-[11px] text-gray-500 font-medium truncate">
              {t.home.fixerDesc}
            </p>
          </div>
            <button
              onClick={handleRunFixer}
              disabled={fixerRunning}
              className="shrink-0 px-5 py-2 rounded-full bg-accent hover:brightness-110 text-white font-black text-[11px] transition-all duration-300 uppercase tracking-[0.15em] disabled:opacity-50 flex items-center justify-center gap-1.5 shadow-lg shadow-accent/20"
            >
              <Wrench size={12} className={fixerRunning ? 'animate-spin' : ''} />
              {fixerRunning ? t.home.fixerRunning : t.home.fixerBtn}
            </button>
        </motion.div>

        <motion.div
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.35, ease: 'easeOut', delay: 0.09 }}
          className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] px-5 py-4 flex items-center gap-4"
        >
          <div className="w-9 h-9 rounded-lg bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
            <Package size={16} className="text-accent" />
          </div>
          <div className="flex-1 min-w-0">
            <h4 className="font-black text-sm text-white/90 tracking-tight truncate">
              {ti('Agregar Lua Manualmente', 'Add Lua Manually')}
            </h4>
            <p className="text-[11px] text-gray-500 font-medium truncate">
              {ti('Para juegos que la API de Ryuu no cubre.', "For games Ryuu's API doesn't cover.")}
            </p>
          </div>
           <button
             onClick={() => setShowManualLuaModal(true)}
             className="shrink-0 px-5 py-2 rounded-full bg-indigo-600 hover:brightness-110 text-white font-black text-[11px] transition-all duration-300 uppercase tracking-[0.15em] flex items-center justify-center gap-1.5 shadow-lg shadow-indigo-600/20"
           >
             <Download size={12} />
             {ti('Agregar', 'Add')}
           </button>
        </motion.div>

      </div>

      {/* Tools Section */}
      <div className="space-y-3 pt-1">
        <div className="flex items-center justify-center gap-3">
          <div className="h-px w-16 bg-white/10" />
          <h3 className="text-[10px] font-black uppercase tracking-[0.3em] text-gray-500 flex items-center gap-2">
            <Settings size={12} /> OPENSTEAMTOOL
          </h3>
          <div className="h-px w-16 bg-white/10" />
        </div>
        <motion.div
          className="flex flex-wrap justify-center gap-3"
          initial="hidden"
          animate="show"
          variants={{ hidden: {}, show: { transition: { staggerChildren: 0.07, delayChildren: 0.1 } } }}
        >
          <motion.div
            variants={{ hidden: { opacity: 0, y: 10 }, show: { opacity: 1, y: 0 } }}
            transition={{ duration: 0.3, ease: 'easeOut' }}
            whileHover={{ scale: 1.02 }}
          >
            <ToolButton icon={RefreshCw} label={t.home.restartSteam} color="text-indigo-400" onClick={handleRestartSteam} />
          </motion.div>
          <motion.div
            variants={{ hidden: { opacity: 0, y: 10 }, show: { opacity: 1, y: 0 } }}
            transition={{ duration: 0.3, ease: 'easeOut' }}
            whileHover={{ scale: 1.02 }}
          >
            <ToolButton icon={Download} label={t.home.installPlugin} color="text-emerald-400" onClick={handleInstallPlugin} disabled={pluginRunning} />
          </motion.div>

          <motion.div
            variants={{ hidden: { opacity: 0, y: 10 }, show: { opacity: 1, y: 0 } }}
            transition={{ duration: 0.3, ease: 'easeOut' }}
            whileHover={{ scale: 1.02 }}
          >
            <ToolButton icon={Wrench} label={t.home.repairPlugin} color="text-amber-400" onClick={handleRepairPlugin} disabled={pluginRunning} />
          </motion.div>

          <motion.div
            variants={{ hidden: { opacity: 0, y: 10 }, show: { opacity: 1, y: 0 } }}
            transition={{ duration: 0.3, ease: 'easeOut' }}
            whileHover={{ scale: 1.02 }}
          >
            <ToolButton
              icon={AlertTriangle}
              label={ti('Arreglar 0 B', 'Fix 0 B')}
              color="text-rose-400"
              onClick={() => setShowZeroByteConfirm(true)}
              disabled={cleaningZeroByte}
            />
          </motion.div>
        </motion.div>
      </div>

      {/* 0 B fix confirmation — portal'd like the other modals */}
      {createPortal(
        <AnimatePresence>
          {showZeroByteConfirm && (
            <motion.div
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              className="fixed inset-0 z-[80] flex items-center justify-center p-4"
              style={{ background: 'rgba(0,0,0,0.62)', backdropFilter: 'blur(10px)' }}
              onClick={() => { if (!cleaningZeroByte) setShowZeroByteConfirm(false); }}
            >
              <motion.div
                initial={{ scale: 0.97, opacity: 0, y: 12 }}
                animate={{ scale: 1, opacity: 1, y: 0 }}
                exit={{ scale: 0.97, opacity: 0, y: 12 }}
                transition={{ duration: 0.3, ease: 'easeOut' }}
                onClick={e => e.stopPropagation()}
                className="relative w-full max-w-[420px] rounded-2xl bg-[#0d0e12] border border-white/[0.08] shadow-2xl shadow-black/60 overflow-hidden p-6 space-y-5"
              >
                <div className="flex items-center gap-3">
                  <div className="w-11 h-11 shrink-0 flex items-center justify-center rounded-full bg-rose-500/10 border border-rose-500/25">
                    <AlertTriangle size={18} className="text-rose-400" />
                  </div>
                  <div className="min-w-0">
                    <span className="text-[9px] font-black uppercase tracking-[0.2em] text-rose-400">OpenSteamTool</span>
                    <h3 className="text-[17px] font-black text-white/90 tracking-tight leading-tight">
                      {ti('Arreglar juegos de 0 B', 'Fix 0 B games')}
                    </h3>
                  </div>
                </div>

                <div className="rounded-xl bg-amber-400/[0.07] border border-amber-400/25 px-4 py-3">
                  <p className="text-[13px] font-black text-amber-300">
                    {ti('Tienes que tener Steam cerrado para que funcione el fix.', 'Steam has to be closed for the fix to work.')}
                  </p>
                  <p className="text-[11px] font-medium text-gray-400 mt-1 leading-relaxed">
                    {ti(
                      'Steam reescribe su lista de bibliotecas al cerrarse, y si está abierto vuelve a dejar las entradas de 0 B.',
                      'Steam rewrites its library list when it exits, so if it is open the 0 B entries come right back.'
                    )}
                  </p>
                </div>

                {/* Live, so the user can close Steam and watch this turn green
                    instead of guessing whether it registered. */}
                <div className={`flex items-center gap-2.5 rounded-xl px-4 py-2.5 border ${isSteamRunning ? 'bg-red-500/[0.07] border-red-500/25' : 'bg-green-500/[0.07] border-green-500/25'}`}>
                  {isSteamRunning
                    ? <XCircle size={15} className="text-red-400 shrink-0" />
                    : <CheckCircle size={15} className="text-green-400 shrink-0" />}
                  <span className={`text-[12px] font-bold ${isSteamRunning ? 'text-red-300' : 'text-green-300'}`}>
                    {isSteamRunning
                      ? ti('Steam está abierto — ciérralo para continuar.', 'Steam is open — close it to continue.')
                      : ti('Steam está cerrado. Listo para arreglar.', 'Steam is closed. Ready to fix.')}
                  </span>
                </div>

                <p className="text-[11px] font-medium text-gray-600 leading-relaxed">
                  {ti(
                    'Solo se borran las entradas cuya carpeta del juego no existe o está vacía. Tus juegos instalados no se tocan.',
                    'Only entries whose game folder is missing or empty are removed. Installed games are left untouched.'
                  )}
                </p>

                <div className="flex gap-2.5 pt-0.5">
                  <button
                    onClick={async () => {
                      await handleCleanZeroByte();
                      setShowZeroByteConfirm(false);
                    }}
                    disabled={isSteamRunning || cleaningZeroByte}
                    className="flex-1 py-3 rounded-xl bg-rose-500 text-white text-[11px] font-black uppercase tracking-[0.15em] shadow-lg shadow-rose-500/25 hover:brightness-110 hover:-translate-y-0.5 transition-all duration-200 disabled:opacity-40 disabled:hover:translate-y-0 disabled:cursor-not-allowed flex items-center justify-center gap-1.5"
                  >
                    <Wrench size={12} className={cleaningZeroByte ? 'animate-spin' : ''} />
                    {cleaningZeroByte ? ti('Arreglando', 'Fixing') : ti('Arreglar', 'Fix')}
                  </button>
                  <button
                    onClick={() => setShowZeroByteConfirm(false)}
                    disabled={cleaningZeroByte}
                    className="px-5 py-3 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[11px] font-black uppercase tracking-[0.15em] text-gray-500 hover:text-white transition-all duration-200 disabled:opacity-40"
                  >
                    {ti('Cancelar', 'Cancel')}
                  </button>
                </div>
              </motion.div>
            </motion.div>
          )}
        </AnimatePresence>,
        document.body
      )}

      {/* Plugin Modal (Enhanced) — portal'd, same fix as the other modals */}
      {createPortal(
      <AnimatePresence>
        {pluginUpdate && !showPluginModal && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-[80] flex items-center justify-center p-4"
            style={{ background: 'rgba(0,0,0,0.62)', backdropFilter: 'blur(10px)' }}
            onClick={dismissPluginUpdate}
          >
            <motion.div
              initial={{ scale: 0.97, opacity: 0, y: 12 }}
              animate={{ scale: 1, opacity: 1, y: 0 }}
              transition={{ duration: 0.35, ease: 'easeOut' }}
              onClick={e => e.stopPropagation()}
              className="relative w-full max-w-[420px] rounded-2xl bg-[#0d0e12] border border-white/[0.08] shadow-2xl shadow-black/60 overflow-hidden"
            >
              {/* The one ambient glow this panel gets. */}
              <div className="absolute -top-16 -right-10 w-44 h-44 rounded-full bg-accent/15 blur-[70px] pointer-events-none" />

              <div className="relative px-6 pt-6 pb-5 space-y-5">
                <div className="flex items-center gap-3.5">
                  <div className="relative w-11 h-11 shrink-0 flex items-center justify-center rounded-full bg-accent/10 border border-accent/25">
                    <motion.span
                      className="absolute inset-0 rounded-full bg-accent/25"
                      animate={{ opacity: [0.5, 0.15, 0.5] }}
                      transition={{ duration: 2.4, repeat: Infinity, ease: 'easeInOut' }}
                    />
                    <RefreshCw size={17} className="relative z-10 text-accent" />
                  </div>
                  <div className="min-w-0">
                    <span className="text-[9px] font-black uppercase tracking-[0.2em] text-accent">
                      OpenSteamTool
                    </span>
                    <h3 className="text-[17px] font-black text-white/90 tracking-tight leading-tight">
                      {ti('Actualización del Plugin', 'Plugin Update')}
                    </h3>
                  </div>
                </div>

                <div className="rounded-xl bg-black/40 border border-white/10 px-4 py-3">
                  <p className="text-[9px] font-black uppercase tracking-widest text-gray-500">
                    {ti('Publicado', 'Published')}
                  </p>
                  <p className="text-[15px] font-black text-white/90 mt-0.5">
                    {publishedAgo(pluginUpdate.published_at, lang === 'es')}
                  </p>
                  <p className="text-[10px] font-medium text-gray-600 tabular-nums mt-0.5">
                    {(() => {
                      try {
                        return new Date(pluginUpdate.published_at).toLocaleString(
                          lang === 'es' ? 'es-ES' : 'en-US',
                          { dateStyle: 'long', timeStyle: 'short' }
                        );
                      } catch { return pluginUpdate.published_at; }
                    })()}
                  </p>
                </div>

                {/* Numbered because it really is a sequence, and knowing Steam
                    closes is the part people want warned about. */}
                <div className="space-y-2">
                  <p className="text-[9px] font-black uppercase tracking-widest text-gray-500">
                    {ti('Qué va a pasar', 'What happens')}
                  </p>
                  {[
                    ti('Se cierra Steam', 'Steam closes'),
                    ti('Se reemplazan los archivos del plugin', 'The plugin files are replaced'),
                    ti('Steam se vuelve a abrir', 'Steam reopens'),
                  ].map((step, i) => (
                    <div key={i} className="flex items-center gap-2.5">
                      <span className="w-5 h-5 shrink-0 rounded-full bg-white/[0.05] border border-white/10 flex items-center justify-center text-[9px] font-black text-gray-500 tabular-nums">
                        {i + 1}
                      </span>
                      <span className="text-[12px] font-medium text-gray-400">{step}</span>
                    </div>
                  ))}
                </div>

                <p className="text-[11px] font-medium text-gray-600 leading-relaxed">
                  {ti(
                    'Tus juegos instalados y tu configuración no se tocan.',
                    'Your installed games and settings are left untouched.'
                  )}
                </p>

                <div className="flex gap-2.5 pt-0.5">
                  <button
                    onClick={handleUpdatePlugin}
                    disabled={pluginRunning}
                    className="flex-1 py-3 rounded-xl bg-accent text-white text-[11px] font-black uppercase tracking-[0.15em] shadow-lg shadow-accent/25 hover:brightness-110 hover:-translate-y-0.5 transition-all duration-200 disabled:opacity-50 disabled:hover:translate-y-0"
                  >
                    {ti('Actualizar ahora', 'Update now')}
                  </button>
                  <button
                    onClick={dismissPluginUpdate}
                    className="px-5 py-3 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[11px] font-black uppercase tracking-[0.15em] text-gray-500 hover:text-white transition-all duration-200"
                  >
                    {ti('Ahora no', 'Not now')}
                  </button>
                </div>
              </div>
            </motion.div>
          </motion.div>
        )}

        {showPluginModal && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.25, ease: 'easeOut' }}
            className="fixed inset-0 z-[200] flex items-center justify-center bg-black/60 backdrop-blur-md"
          >
            <motion.div
              initial={{ opacity: 0, y: 12, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 12, scale: 0.98 }}
              transition={{ duration: 0.3, ease: 'easeOut' }}
              className="bg-[#0a0a0c]/95 border border-white/[0.08] rounded-2xl p-8 w-[600px] max-h-[80vh] flex flex-col shadow-2xl backdrop-blur-xl"
            >
              <div className="flex items-center justify-between mb-6">
                <div className="flex items-center gap-4">
                  <div className="w-10 h-10 rounded-xl bg-accent/15 border border-accent/20 flex items-center justify-center">
                    <Download size={20} className="text-accent" />
                  </div>
                  <h3 className="font-black text-white/90 uppercase tracking-widest text-lg">
                    {pluginRunning ? 'Instalando Plugin...' : 'Gestor de Plugin'}
                  </h3>
                </div>
                <button
                  onClick={() => setShowPluginModal(false)}
                  disabled={pluginRunning}
                  className="w-10 h-10 rounded-xl bg-white/[0.04] border border-white/[0.08] hover:bg-white/[0.07] flex items-center justify-center text-gray-400 hover:text-white transition-all disabled:opacity-30"
                >
                  <X size={20} />
                </button>
              </div>
              <div className="flex-1 overflow-y-auto bg-black/40 rounded-2xl p-6 font-mono text-[13px] leading-relaxed space-y-2 min-h-[300px] border border-white/[0.06]">
                {pluginLogs.length === 0 && (
                  <div className="flex items-center gap-3 text-gray-500">
                    <motion.div
                      className="w-2 h-2 bg-gray-500 rounded-full"
                      animate={{ opacity: [1, 0.3, 1] }}
                      transition={{ duration: 1.4, repeat: Infinity, ease: 'easeInOut' }}
                    />
                    Inicializando proceso seguro...
                  </div>
                )}
                {pluginLogs.map((line, i) => (
                  <motion.p
                    initial={{ opacity: 0, x: -10 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{ duration: 0.25, ease: 'easeOut' }}
                    key={i}
                    className={line.startsWith('✗') ? 'text-accent font-bold' : line.startsWith('✓') ? 'text-emerald-400' : 'text-gray-300'}
                  >
                    {line}
                  </motion.p>
                ))}
              </div>
              {pluginRunning && (
                <div className="mt-6 h-1.5 bg-white/[0.05] rounded-full overflow-hidden">
                  <motion.div
                    className="h-full bg-accent rounded-full"
                    animate={{ opacity: [1, 0.5, 1] }}
                    transition={{ duration: 1.2, repeat: Infinity, ease: 'easeInOut' }}
                  />
                </div>
              )}
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>,
      document.body
      )}

      {/* Manual Lua Add Modal — portal'd to document.body for the same
          reason as the Bypass modals: this view sits under the tab-switch
          wrapper's `<motion.div key={activeTab} animate={{ x: 0 }}>`, whose
          inline transform (set by Framer Motion even at rest) becomes the
          containing block for any `position: fixed` descendant, so this
          modal's backdrop was only covering the content pane instead of the
          real window — leaving the sidebar/header uncovered. */}
      {createPortal(
      <AnimatePresence>
        {showManualLuaModal && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.25, ease: 'easeOut' }}
            className="fixed inset-0 z-[200] flex items-center justify-center bg-black/60 backdrop-blur-md"
          >
            <motion.div
              initial={{ opacity: 0, y: 12, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 12, scale: 0.98 }}
              transition={{ duration: 0.3, ease: 'easeOut' }}
              className="relative overflow-hidden bg-[#0c0d11] border border-white/[0.08] rounded-2xl p-8 w-[520px] max-h-[80vh] flex flex-col shadow-2xl"
            >
              <div className="absolute top-0 left-0 right-0 h-[3px] bg-gradient-to-r from-indigo-500 via-violet-500 to-blue-400" />
              <div
                className="absolute inset-0 pointer-events-none opacity-[0.035]"
                style={{ backgroundImage: 'linear-gradient(rgba(255,255,255,0.6) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.6) 1px, transparent 1px)', backgroundSize: '26px 26px' }}
              />
              <div className="absolute -right-16 -top-16 w-56 h-56 bg-accent/10 blur-[70px] rounded-full pointer-events-none" />

              <div className="relative z-10 flex items-center justify-between mb-6">
                <div className="flex items-center gap-4">
                  <div className="relative w-10 h-10 shrink-0">
                    <motion.div
                      className="absolute inset-0 rounded-xl bg-accent blur-md"
                      animate={{ opacity: [0.5, 0.15, 0.5] }}
                      transition={{ duration: 2.2, repeat: Infinity, ease: 'easeInOut' }}
                    />
                    <div className="relative z-10 w-10 h-10 rounded-xl bg-accent/15 border border-accent/30 flex items-center justify-center">
                      <Package size={19} className="text-accent" />
                    </div>
                  </div>
                  <h3 className="font-black text-white/90 uppercase tracking-widest text-lg">
                    {ti('Agregar Lua Manualmente', 'Add Lua Manually')}
                  </h3>
                </div>
                <button
                  onClick={() => { setShowManualLuaModal(false); setManualLuaResult(null); }}
                  disabled={manualLuaBusy}
                  className="w-10 h-10 rounded-xl bg-white/[0.04] border border-white/[0.08] hover:bg-white/[0.07] flex items-center justify-center text-gray-400 hover:text-white transition-all disabled:opacity-30"
                >
                  <X size={20} />
                </button>
              </div>

              {!manualLuaResult ? (
                <div className="relative z-10 space-y-5">
                  <p className="text-[13px] text-gray-400 font-medium leading-relaxed">
                    {ti(
                      'Selecciona un archivo .zip o .rar con la configuración Lua del juego (para títulos que la API de Ryuu todavía no cubre).',
                      "Select a .zip or .rar file with the game's Lua config (for titles Ryuu's API doesn't cover yet)."
                    )}
                  </p>

                  <div
                    className={`relative rounded-2xl border-2 border-dashed transition-all duration-300 px-6 py-10 flex flex-col items-center text-center ${
                      manualLuaDragOver
                        ? 'border-accent bg-accent/[0.08] scale-[1.02]'
                        : 'border-white/[0.12] bg-white/[0.02] hover:border-white/20'
                    } ${manualLuaBusy ? 'pointer-events-none opacity-60' : ''}`}
                  >
                    <motion.div
                      animate={manualLuaDragOver ? { y: [-3, 3, -3] } : { y: 0 }}
                      transition={{ duration: 1.1, repeat: manualLuaDragOver ? Infinity : 0, ease: 'easeInOut' }}
                      className={`w-14 h-14 rounded-2xl border flex items-center justify-center mb-4 transition-colors duration-300 ${
                        manualLuaDragOver ? 'bg-accent border-accent/60 text-white' : 'bg-white/[0.05] border-white/10 text-gray-400'
                      }`}
                    >
                      {manualLuaBusy ? <RefreshCw size={22} className="animate-spin" /> : <UploadCloud size={22} />}
                    </motion.div>
                    <p className="text-sm font-black text-white/90 mb-1">
                      {manualLuaBusy
                        ? ti('Instalando...', 'Installing...')
                        : manualLuaDragOver
                          ? ti('Soltá el archivo acá', 'Drop the file here')
                          : ti('Arrastrá tu archivo .zip o .rar acá', 'Drag your .zip or .rar file here')}
                    </p>
                    {!manualLuaBusy && (
                      <>
                        <p className="text-[10px] text-gray-500 font-bold uppercase tracking-widest mb-4">{ti('o', 'or')}</p>
                        <button
                          onClick={handleAddManualLua}
                          className="px-5 py-2.5 bg-accent hover:brightness-110 text-white font-black text-[11px] rounded-xl transition-all duration-300 uppercase tracking-widest hover:-translate-y-0.5 flex items-center gap-2"
                        >
                          <Download size={13} />
                          {ti('Seleccionar archivo', 'Select file')}
                        </button>
                      </>
                    )}
                  </div>
                </div>
              ) : (
                <div className="flex-1 overflow-y-auto space-y-4">
                  <p className="text-[13px] text-gray-300 font-medium">{manualLuaResult.message}</p>
                  {manualLuaResult.games.length > 0 && (
                    <div className="space-y-2">
                      {manualLuaResult.games.map(g => (
                        <div key={g.id} className="flex items-center gap-3 bg-white/[0.04] border border-white/[0.08] rounded-xl p-3">
                          {g.image ? (
                            <img src={g.image} alt="" className="w-14 h-8 object-cover rounded-md shrink-0" />
                          ) : (
                            <div className="w-14 h-8 rounded-md bg-white/[0.06] shrink-0" />
                          )}
                          <div className="min-w-0">
                            <p className="text-sm font-bold text-white/90 truncate">{g.name}</p>
                            <p className="text-[10px] text-gray-500 font-semibold">#{g.id}</p>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                  <button
                    onClick={() => { setShowManualLuaModal(false); setManualLuaResult(null); }}
                    className="w-full px-6 py-3 bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-white font-black text-xs rounded-xl transition-all duration-300 uppercase tracking-[0.2em]"
                  >
                    {ti('Cerrar', 'Close')}
                  </button>
                </div>
              )}
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>,
      document.body
      )}

    </motion.div>
  );
};

// --- YouTube Player ---

const YouTubePlayer = ({ videoId, cinematic }: {
  videoId: string, cinematic: boolean, onToggleCinematic: () => void
}) => (
  <motion.div
    initial={{ opacity: 0 }}
    animate={{ opacity: 1 }}
    transition={{ duration: 0.3 }}
    className={`relative bg-black rounded-2xl overflow-hidden ${cinematic ? 'fixed inset-0 z-[100] rounded-none' : 'w-full h-full'}`}>
    <iframe
      src={`https://www.youtube-nocookie.com/embed/${videoId}?autoplay=1&rel=0&modestbranding=1&playsinline=1`}
      className="w-full h-full border-0"
      allow="autoplay; encrypted-media; picture-in-picture; storage-access"
      allowFullScreen
    />
  </motion.div>
);

// --- Game page helpers ---

/// Where one game's install stands (see the `install_readiness` command).
interface InstallReadiness {
  steam_running: boolean;
  plugin_installed: boolean;
  ticket_installed: boolean;
  manifests_missing: number;
  download: { state_flags: number; bytes_downloaded: number; bytes_to_download: number } | null;
  libraries: { path: string; free_bytes: number }[];
}

// Zero has to read as zero: this used to floor everything at "1 MB", so a
// download that had not started showed "1 MB / 3.0 GB" next to 0%.
const formatGameBytes = (n: number): string =>
  n >= 1_073_741_824
    ? `${(n / 1_073_741_824).toFixed(1)} GB`
    : n >= 1_048_576
      ? `${Math.round(n / 1_048_576)} MB`
      : n >= 1024
        ? `${Math.round(n / 1024)} KB`
        : '0 MB';

/// Steam's playtime, which it counts in minutes.
const formatPlaytime = (minutes: number): string =>
  minutes < 60 ? `${minutes} min` : `${Math.round(minutes / 60)} h`;

/// Per-game facts for the Library (see the `get_library_stats` command).
interface LibraryGameStats {
  last_played: number;
  playtime_minutes: number;
  downloading: boolean;
  progress: number;
  update_pending: boolean;
  manifests_missing: number;
  size_on_disk: number;
}

/// The PC's specs, read once per session: every game page compares against
/// them, and they do not change while the app is open.
let systemSpecsPromise: Promise<SystemSpecs | null> | null = null;
const loadSystemSpecs = (steamPath: string): Promise<SystemSpecs | null> => {
  if (!systemSpecsPromise) {
    systemSpecsPromise = invoke<SystemSpecs>('get_system_specs', { steamPath }).catch(() => {
      systemSpecsPromise = null;
      return null;
    });
  }
  return systemSpecsPromise;
};

/// Steam's review wording. The reviews API only answers in English.
const REVIEW_WORDS_ES: Record<string, string> = {
  'Overwhelmingly Positive': 'Extremadamente positivas',
  'Very Positive': 'Muy positivas',
  'Positive': 'Positivas',
  'Mostly Positive': 'Mayormente positivas',
  'Mixed': 'Variadas',
  'Mostly Negative': 'Mayormente negativas',
  'Negative': 'Negativas',
  'Very Negative': 'Muy negativas',
  'Overwhelmingly Negative': 'Extremadamente negativas',
};

/// Each interface language as Steam names it in `supported_languages`, and as
/// its speakers write it.
const STEAM_LANGUAGES: Record<Lang, { steam: string; native: string }> = {
  es: { steam: 'Spanish', native: 'Español' },
  en: { steam: 'English', native: 'English' },
  pt: { steam: 'Portuguese', native: 'Português' },
  fr: { steam: 'French', native: 'Français' },
  ru: { steam: 'Russian', native: 'Русский' },
};

/// A game's best place in Steam's charts, for its page.
function rankInCharts(rankings: SteamRankings | null, appId: string): CatalogRank | undefined {
  if (!rankings) return undefined;
  const sold = rankings.top_sellers.indexOf(appId);
  if (sold >= 0) return { kind: 'top_sellers', position: sold + 1 };
  const played = rankings.most_played.indexOf(appId);
  return played >= 0 ? { kind: 'most_played', position: played + 1 } : undefined;
}

/// Steam's own trailer.
///
/// Steam serves trailers only as HLS (and DASH) streams now. The webview does
/// not play HLS by itself, so hls.js feeds it to the <video>; where the
/// browser can play it natively, it is handed over directly. Workers stay off:
/// the app's content policy does not allow the blob workers hls.js would start.
const SteamTrailer = ({ src, poster, onFail }: { src: string; poster?: string | null; onFail: () => void }) => {
  const videoRef = useRef<HTMLVideoElement>(null);
  const failRef = useRef(onFail);
  failRef.current = onFail;

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    let cancelled = false;
    let hls: import('hls.js').default | null = null;

    if (video.canPlayType('application/vnd.apple.mpegurl')) {
      video.src = src;
    } else {
      import('hls.js')
        .then(({ default: Hls }) => {
          if (cancelled) return;
          if (!Hls.isSupported()) {
            failRef.current();
            return;
          }
          hls = new Hls({ enableWorker: false, capLevelToPlayerSize: true });
          hls.on(Hls.Events.ERROR, (_event, data) => {
            if (data.fatal) failRef.current();
          });
          hls.loadSource(src);
          hls.attachMedia(video);
        })
        .catch(() => failRef.current());
    }
    return () => {
      cancelled = true;
      hls?.destroy();
    };
  }, [src]);

  return (
    <video
      ref={videoRef}
      className="w-full h-full object-cover"
      controls
      autoPlay
      muted
      loop
      playsInline
      poster={poster ?? undefined}
      onError={() => failRef.current()}
    />
  );
};

/// Steam opens each block with its own "Minimum:" / "Recommended:" heading,
/// which repeated the label drawn right above it.
const stripRequirementsHeading = (html: string): string =>
  html.replace(/^\s*<strong>\s*(Minimum|Recommended|Mínimo|Recomendado)\s*:?\s*<\/strong>\s*(<br\s*\/?>)?/i, '');

/// One requirement against this PC: a mark, and the requirement itself.
const ReqCell = ({ ok, known, text }: { ok: boolean; known: boolean; text: string }) => (
  <span className="flex items-start gap-1.5">
    {!known
      ? <Circle size={12} className="text-gray-600 shrink-0 mt-0.5" />
      : ok
        ? <CheckCircle2 size={12} className="text-emerald-400 shrink-0 mt-0.5" />
        : <XCircle size={12} className="text-red-400 shrink-0 mt-0.5" />}
    <span className={known && !ok ? 'text-red-300' : 'text-gray-400'}>{text}</span>
  </span>
);

// --- Game Detail Full Screen ---

const GameDetailView = ({ game, steamPath, onClose, onInstall, onNameResolved, rank }: {
  game: Game, steamPath: string, onClose: () => void, onInstall: (id: string) => void | Promise<void>,
  onNameResolved: (id: string, name: string) => void,
  // Place in Steam's charts, when it has one.
  rank?: CatalogRank,
}) => {
  const { t, lang } = useLanguage();
  const tr = useTranslateInline();
  const [media, setMedia] = useState<SteamMedia>({ screenshots: [], trailer_mp4: null, trailer_webm: null, trailer_thumbnail: null, name: null, metacritic_score: null, minimum_reqs: null, recommended_reqs: null, short_description: null });
  const [loading, setLoading] = useState(true);
  const [youtubeId, setYoutubeId] = useState<string | null>(null);
  const [youtubeLoading, setYoutubeLoading] = useState(false);
  // Steam's own trailer URL sometimes just doesn't load (dead CDN link,
  // expired token, region hiccup) — the <video> below stayed stuck at 0:00
  // forever with no way out, since the YouTube-fallback effect only checked
  // whether Steam *claimed* to have a trailer URL, never whether it actually
  // played. Set on the video's onError so that effect can fall through to
  // YouTube instead.
  const [videoFailed, setVideoFailed] = useState(false);
  const [lightbox, setLightbox] = useState<{ src: string, idx: number } | null>(null);
  const [cinematic, setCinematic] = useState(false);
  const [showUnlockModal, setShowUnlockModal] = useState(false);
  const [showSavesModal, setShowSavesModal] = useState(false);

  const displayName = media.name ?? game.name?.trim() ?? `App ${game.id}`;
  const score = media.metacritic_score ?? game.metacritic?.score ?? null;

  // ── Live install state, what an install costs, and this PC against the game ──
  const [readiness, setReadiness] = useState<InstallReadiness | null>(null);
  const [gameSize, setGameSize] = useState<number>(game.size_bytes ?? 0);
  const [specs, setSpecs] = useState<SystemSpecs | null>(null);
  const [hubcapLeft, setHubcapLeft] = useState<number | null>(null);
  const [installing, setInstalling] = useState(false);
  const [showRawReqs, setShowRawReqs] = useState(false);
  const [showWorkshopModal, setShowWorkshopModal] = useState(false);

  // Re-read every few seconds while the page is open, so it follows Steam
  // starting, the ticket landing and the download moving. The page used to
  // show three fixed sentences whether or not any of it was already true.
  useEffect(() => {
    let alive = true;
    const read = () => {
      invoke<InstallReadiness>('install_readiness', { steamPath, appId: game.id })
        .then(r => { if (alive) setReadiness(r); })
        .catch(() => { });
    };
    read();
    const timer = window.setInterval(read, 3000);
    return () => { alive = false; clearInterval(timer); };
  }, [steamPath, game.id]);

  useEffect(() => {
    let alive = true;
    if (!game.size_bytes) {
      invoke<number>('get_game_size', { appId: game.id })
        .then(size => { if (alive && size > 0) setGameSize(size); })
        .catch(() => { });
    }
    loadSystemSpecs(steamPath).then(s => { if (alive) setSpecs(s); });
    const { catalogSource, hubcapApiKey } = readCatalogSource();
    if (catalogSource === 'hubcap') {
      fetchHubcapUsage(hubcapApiKey)
        .then(u => { if (alive) setHubcapLeft(u.can_make_requests ? u.remaining : 0); })
        .catch(() => { });
    }
    return () => { alive = false; };
  }, [game.id, game.size_bytes, steamPath]);

  const download = readiness?.download ?? null;
  const downloading = !!download && download.bytes_to_download > 0 && download.bytes_downloaded < download.bytes_to_download;
  const progress = downloading && download ? download.bytes_downloaded / download.bytes_to_download : 0;
  const pct = Math.round(progress * 100);
  const installedNow = game.status === 'Installed' || (!!download && (download.state_flags & 4) !== 0 && !downloading);
  const bestLibrary = readiness?.libraries[0] ?? null;
  const bestDrive = bestLibrary ? (bestLibrary.path.match(/^[A-Za-z]:/)?.[0] ?? bestLibrary.path) : '';
  const sourceName = readCatalogSource().catalogSource === 'hubcap' ? 'Hubcap' : 'Ryuu';

  // What an install involves, said before it starts.
  const facts = [
    gameSize > 0 ? formatGameBytes(gameSize) : null,
    game.drm === '' ? tr('Sin Denuvo', 'No Denuvo') : null,
    tr(`Desde ${sourceName}`, `From ${sourceName}`),
  ].filter((f): f is string => !!f);

  const warnings: { text: string; tone: 'red' | 'amber' }[] = [];
  if (!installedNow && gameSize > 0 && bestLibrary && bestLibrary.free_bytes < gameSize) {
    warnings.push({
      tone: 'red',
      text: tr(
        `No cabe: necesita ${formatGameBytes(gameSize)} y el disco con más espacio (${bestDrive}) tiene ${formatGameBytes(bestLibrary.free_bytes)} libres.`,
        `It does not fit: it needs ${formatGameBytes(gameSize)} and the roomiest drive (${bestDrive}) has ${formatGameBytes(bestLibrary.free_bytes)} free.`
      ),
    });
  }
  if (!installedNow && hubcapLeft === 0) {
    warnings.push({
      tone: 'amber',
      text: tr('No te quedan descargas de Hubcap hoy: esta instalación usará Ryuu.', 'No Hubcap downloads left today: this install will use Ryuu.'),
    });
  }
  if (game.drm) {
    warnings.push({
      tone: 'amber',
      text: tr(`Tiene ${game.drm}: puede necesitar un bypass para abrir.`, `It has ${game.drm}: it may need a bypass to run.`),
    });
  }

  type CheckState = 'ok' | 'bad' | 'pending';
  const checklist: { key: string; state: CheckState; label: string; hint?: string }[] = readiness ? [
    {
      key: 'steam',
      state: readiness.steam_running ? 'ok' : 'bad',
      label: readiness.steam_running ? tr('Steam está abierto', 'Steam is running') : tr('Steam está cerrado', 'Steam is closed'),
      hint: readiness.steam_running ? undefined : tr('Ábrelo para que la descarga pueda empezar.', 'Open it so the download can start.'),
    },
    {
      key: 'plugin',
      state: readiness.plugin_installed ? 'ok' : 'bad',
      label: readiness.plugin_installed ? tr('OpenSteamTool instalado', 'OpenSteamTool installed') : tr('Falta OpenSteamTool', 'OpenSteamTool is missing'),
      hint: readiness.plugin_installed ? undefined : tr('Instálalo con "Instalar Plugin" en Inicio.', 'Install it with "Install Plugin" on Home.'),
    },
    {
      key: 'ticket',
      state: !readiness.ticket_installed ? 'pending' : readiness.manifests_missing > 0 ? 'bad' : 'ok',
      label: !readiness.ticket_installed
        ? tr('Ticket y manifiestos sin preparar', 'Ticket and manifests not prepared')
        : readiness.manifests_missing > 0
          ? tr(`Faltan ${readiness.manifests_missing} manifiesto(s)`, `${readiness.manifests_missing} manifest(s) missing`)
          : tr('Ticket y manifiestos listos', 'Ticket and manifests ready'),
      hint: !readiness.ticket_installed
        ? tr('Se preparan al pulsar Instalar.', 'They are prepared when you press Install.')
        : readiness.manifests_missing > 0
          ? tr('Steam no podrá descargarlo así. Prueba a reinstalar; si sigue, cambia de fuente en Ajustes.', 'Steam cannot download it like this. Try reinstalling; if it persists, switch source in Settings.')
          : undefined,
    },
    {
      key: 'download',
      state: installedNow ? 'ok' : 'pending',
      label: installedNow
        ? tr('Descargado en Steam', 'Downloaded in Steam')
        : downloading
          ? tr(`Descargando en Steam · ${pct}%`, `Downloading in Steam · ${pct}%`)
          : tr('Descarga en Steam', 'Download in Steam'),
      hint: installedNow || downloading || !readiness.ticket_installed
        ? undefined
        : tr('En el aviso de Steam, elige el disco y pulsa Instalar.', "In Steam's dialog, pick a drive and press Install."),
    },
  ] : [];

  const reqRows = useMemo(
    () => (specs && media.minimum_reqs ? compareRequirements(specs, media.minimum_reqs, media.recommended_reqs, tr) : []),
    [specs, media.minimum_reqs, media.recommended_reqs, tr]
  );
  const verdict = requirementsVerdict(reqRows);
  const verdictInfo = {
    excellent: { text: tr('Tu PC cumple lo recomendado', 'Your PC meets the recommended specs'), tone: 'bg-emerald-500/10 border-emerald-500/30 text-emerald-300' },
    good: { text: tr('Tu PC cumple lo mínimo', 'Your PC meets the minimum'), tone: 'bg-amber-500/10 border-amber-500/30 text-amber-200' },
    poor: { text: tr('Tu PC no llega al mínimo', 'Your PC is below the minimum'), tone: 'bg-red-500/10 border-red-500/30 text-red-300' },
    unknown: { text: tr('No se pudo comparar del todo', 'Could not fully compare'), tone: 'bg-white/[0.05] border-white/10 text-gray-400' },
  }[verdict];

  // Developer · publisher · release date, under the title.
  const developerList = media.developers?.length ? media.developers : game.developers ?? [];
  const publisherList = (media.publishers ?? []).filter(p => !developerList.includes(p));
  const metaLine = [developerList.join(', '), publisherList.join(', '), media.release_date ?? ''].filter(Boolean).join(' · ');

  const reviewPct = media.reviews && media.reviews.total > 0 ? Math.round((media.reviews.positive / media.reviews.total) * 100) : null;
  const reviewWords = media.reviews
    ? (lang === 'es' ? REVIEW_WORDS_ES[media.reviews.description] ?? media.reviews.description : media.reviews.description)
    : '';
  const reviewTone = reviewPct === null ? '' : reviewPct >= 70
    ? 'bg-emerald-500/10 border-emerald-500/30 text-emerald-300'
    : reviewPct >= 40 ? 'bg-amber-500/10 border-amber-500/30 text-amber-200' : 'bg-red-500/10 border-red-500/30 text-red-300';

  // Whether the game speaks the interface's language — for most users here,
  // the first thing they want to know.
  const uiLanguage = STEAM_LANGUAGES[lang];
  const languageMatches = (media.languages ?? []).filter(l => l.name.startsWith(uiLanguage.steam));
  const languageSupport: 'audio' | 'text' | 'none' | null = !media.languages?.length
    ? null
    : languageMatches.some(l => l.audio) ? 'audio' : languageMatches.length > 0 ? 'text' : 'none';
  const languageLabel = languageSupport === 'audio'
    ? `${uiLanguage.native}: ${tr('textos y voces', 'text and voice', { pt: 'textos e vozes', fr: 'textes et voix', ru: 'текст и озвучка' })}`
    : languageSupport === 'text'
      ? `${uiLanguage.native}: ${tr('solo textos', 'text only', { pt: 'só textos', fr: 'texte seulement', ru: 'только текст' })}`
      : tr(`Sin ${uiLanguage.native.toLowerCase()}`, `No ${uiLanguage.native}`, { pt: `Sem ${uiLanguage.native}`, fr: `Pas de ${uiLanguage.native}`, ru: `Нет: ${uiLanguage.native}` });
  const languageTone = languageSupport === 'audio'
    ? 'bg-emerald-500/10 border-emerald-500/30 text-emerald-300'
    : languageSupport === 'text' ? 'bg-sky-500/10 border-sky-500/30 text-sky-300' : 'bg-amber-500/10 border-amber-500/30 text-amber-200';

  const openStorePage = () => {
    import('@tauri-apps/api/shell').then(({ open }) => {
      open(`https://store.steampowered.com/app/${game.id}`).catch(console.error);
    });
  };
  const startInstall = () => {
    if (installing) return;
    setInstalling(true);
    Promise.resolve(onInstall(game.id)).finally(() => setInstalling(false));
  };

  useEffect(() => {
    setVideoFailed(false);
    fetchSteamMedia(game.id, lang).then(m => {
      setMedia(m);
      setLoading(false);
      if (m.name) onNameResolved(game.id, m.name);
    });
  }, [game.id, lang]);

  // Once metadata loads, search YouTube automatically — as a fallback when
  // Steam has no trailer of its own, OR when its trailer URL exists but
  // failed to actually play (videoFailed) — and never for nsfw/+18 games.
  useEffect(() => {
    if (game.nsfw) return;
    if (loading || youtubeId || youtubeLoading) return;
    if ((media.trailer_hls || media.trailer_mp4 || media.trailer_webm) && !videoFailed) return;
    const name = media.name ?? game.name?.trim() ?? '';
    if (!name || name.startsWith('Game ID:')) return;
    setYoutubeLoading(true);
    invoke<string | null>('search_youtube_trailer', { gameName: name })
      .then(id => setYoutubeId(id ?? null))
      .catch(() => { })
      .finally(() => setYoutubeLoading(false));
  }, [loading, media.name, media.trailer_hls, media.trailer_mp4, media.trailer_webm, game.name, game.nsfw, videoFailed]);

  // Lightbox keyboard nav
  useEffect(() => {
    if (!lightbox) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setLightbox(null);
      if (e.key === 'ArrowRight') setLightbox(l => l && l.idx < media.screenshots.length - 1 ? { src: media.screenshots[l.idx + 1], idx: l.idx + 1 } : l);
      if (e.key === 'ArrowLeft') setLightbox(l => l && l.idx > 0 ? { src: media.screenshots[l.idx - 1], idx: l.idx - 1 } : l);
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [lightbox, media.screenshots]);

  // Esc closes the whole detail view — but only when the screenshot
  // lightbox isn't open, since that has its own Escape handler above that
  // should take precedence (close the lightbox first, not the whole page).
  useEffect(() => {
    if (lightbox) return;
    const handler = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [lightbox, onClose]);

  return (
    <motion.div
      initial={{ opacity: 0, x: 30 }}
      animate={{ opacity: 1, x: 0 }}
      exit={{ opacity: 0, x: 30 }}
      transition={{ duration: 0.25 }}
      className="fixed inset-0 bg-[#050507] z-40 flex flex-col overflow-hidden"
    >
      {/* Top bar */}
      <div className="flex items-center justify-between px-6 py-3 border-b border-white/[0.06] shrink-0 bg-[#050507]/80 backdrop-blur-xl">
        <motion.button
          whileHover={{ x: -2 }}
          onClick={onClose}
          title={`${t.detail.back} (Esc)`}
          className="group flex items-center gap-2.5 text-gray-500 hover:text-white/90 transition-colors font-bold text-sm"
        >
          <span className="w-7 h-7 rounded-full bg-white/[0.05] group-hover:bg-white/[0.1] border border-white/10 flex items-center justify-center transition-colors">
            <ArrowLeft size={13} />
          </span>
          {t.detail.back}
          <span className="text-[9px] text-gray-600 font-black bg-white/[0.04] border border-white/[0.08] px-1.5 py-0.5 rounded-md tracking-wider">ESC</span>
        </motion.button>
        <div className="flex items-center gap-3">
          {!loading && <span className="text-sm font-black text-white/90 truncate max-w-sm">{displayName}</span>}
          {/* Metacritic when it scored the game; otherwise Steam's own
              reviews. A grey "N/A" was the only thing shown for most games,
              which Metacritic never reviews. */}
          {!loading && (
            score !== null ? (
              <MetacriticBadge score={score} />
            ) : reviewPct !== null && media.reviews ? (
              <div
                title={tr(`${media.reviews.total.toLocaleString('es')} reseñas en Steam`, `${media.reviews.total.toLocaleString('en')} Steam reviews`)}
                className={`flex items-center gap-1.5 px-2.5 py-1 rounded-full border text-[10px] font-black uppercase tracking-wider ${reviewTone}`}
              >
                <ThumbsUp size={10} />
                {reviewPct}% Steam
              </div>
            ) : null
          )}
          <span className="text-[9px] text-gray-500 font-black bg-white/[0.05] border border-white/10 px-2.5 py-1 rounded-full uppercase tracking-widest">APP {game.id}</span>
        </div>
      </div>

      {/* Scrollable content */}
      <div className="flex-1 overflow-y-auto custom-scrollbar">
        <div className="w-full px-8 py-6 flex flex-col lg:flex-row gap-8 items-start">

          {/* ── LEFT: trailer/screenshot viewer + system requirements ── */}
          <div className="flex-[1.4] min-w-0 w-full space-y-5">
            <div className="relative w-full aspect-video rounded-[1.5rem] overflow-hidden border border-white/[0.1] shadow-2xl bg-black">
              <div className="absolute inset-x-0 top-0 h-[2px] bg-gradient-to-r from-transparent via-accent to-transparent z-20 pointer-events-none" />
              {loading ? (
                <div className="w-full h-full flex items-center justify-center bg-white/[0.02]">
                  <div className="flex flex-col items-center gap-4 text-gray-600">
                    <RefreshCw className="animate-spin" size={28} />
                    <span className="text-sm font-medium">{t.detail.loading}</span>
                  </div>
                </div>
              ) : game.nsfw ? (
                <div className="relative w-full h-full overflow-hidden">
                  <SteamImage
                    id={game.id}
                    fixedSrc={game.image}
                    alt={displayName}
                    className="w-full h-full object-cover opacity-60"
                  />
                  <div className="absolute inset-0 bg-gradient-to-t from-black via-transparent to-transparent" />
                  <div className="absolute bottom-6 left-6 flex items-center gap-3 text-gray-500">
                    <Play size={16} opacity={0.5} />
                    <span className="text-xs font-black uppercase tracking-wider">{t.detail.noTrailer}</span>
                  </div>
                </div>
              ) : media.trailer_hls && !videoFailed ? (
                <SteamTrailer
                  src={media.trailer_hls}
                  poster={media.trailer_thumbnail}
                  onFail={() => setVideoFailed(true)}
                />
              ) : (media.trailer_mp4 || media.trailer_webm) && !videoFailed ? (
                <video
                  key={media.trailer_mp4 ?? media.trailer_webm ?? undefined}
                  className="w-full h-full object-cover"
                  controls
                  autoPlay
                  muted
                  loop
                  poster={media.trailer_thumbnail ?? undefined}
                  onError={() => setVideoFailed(true)}
                >
                  {media.trailer_mp4 && <source src={media.trailer_mp4} type="video/mp4" />}
                  {media.trailer_webm && <source src={media.trailer_webm} type="video/webm" />}
                </video>
              ) : youtubeLoading ? (
                <div className="w-full h-full flex items-center justify-center bg-white/[0.02]">
                  <div className="flex flex-col items-center gap-4 text-gray-600">
                    <RefreshCw className="animate-spin" size={28} />
                    <span className="text-sm font-medium">{tr('Buscando trailer...', 'Searching for trailer...')}</span>
                  </div>
                </div>
              ) : youtubeId ? (
                <YouTubePlayer
                  videoId={youtubeId}
                  cinematic={cinematic}
                  onToggleCinematic={() => setCinematic(c => !c)}
                />
              ) : (
                <div className="relative w-full h-full overflow-hidden">
                  <SteamImage
                    id={game.id}
                    fixedSrc={game.image}
                    alt={displayName}
                    className="w-full h-full object-cover opacity-60"
                  />
                  <div className="absolute inset-0 bg-gradient-to-t from-black via-transparent to-transparent" />
                  <div className="absolute bottom-6 left-6 flex items-center gap-3 text-gray-500">
                    <Play size={16} opacity={0.5} />
                    <span className="text-xs font-black uppercase tracking-wider">{t.detail.noTrailer}</span>
                  </div>
                </div>
              )}
            </div>

            {/* ── REQUISITOS DEL SISTEMA ──
                Compared against this PC, the same way the Specs tab does. The
                raw Steam text stays one click away: it used to be the whole
                section, in English, with "MÍNIMO" above "Minimum:". */}
            {!loading && (
              (media.minimum_reqs || media.recommended_reqs) ? (
                <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-6 space-y-5">
                  <style dangerouslySetInnerHTML={{
                    __html: `
                    .system-reqs-content { color: #e4e4e7; font-size: 13px; line-height: 1.6; }
                    .system-reqs-content ul { list-style-type: none; padding-left: 0; margin: 0; }
                    .system-reqs-content li { margin-bottom: 8px; color: #e4e4e7; font-size: 13px; line-height: 1.6; }
                    .system-reqs-content strong { color: #ffffff; font-weight: 850; }
                  `}} />
                  <div className="flex items-center justify-between gap-3 flex-wrap border-b border-white/[0.06] pb-4">
                    <h3 className="text-[10px] font-black uppercase tracking-widest text-gray-500 flex items-center gap-2">
                      <Cpu size={12} className="text-gray-600" /> {tr('Requisitos del Sistema', 'System Requirements')}
                    </h3>
                    {reqRows.length > 0 && (
                      <span className={`px-3 py-1 rounded-full border text-[10px] font-black uppercase tracking-wider ${verdictInfo.tone}`}>
                        {verdictInfo.text}
                      </span>
                    )}
                  </div>

                  {reqRows.length > 0 && (
                    <div className="overflow-x-auto">
                      <table className="w-full text-left text-[12px]">
                        <thead>
                          <tr className="text-[9px] font-black uppercase tracking-widest text-gray-500">
                            <th className="pb-2 pr-4 font-black" />
                            <th className="pb-2 pr-4 font-black">{tr('Tu PC', 'Your PC')}</th>
                            <th className="pb-2 pr-4 font-black">{tr('Mínimo', 'Minimum')}</th>
                            <th className="pb-2 font-black text-accent">{tr('Recomendado', 'Recommended')}</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-white/[0.05]">
                          {reqRows.map(r => (
                            <tr key={r.kind} className="align-top">
                              <td className="py-2.5 pr-4 font-black text-white/80 whitespace-nowrap">{r.component}</td>
                              <td className="py-2.5 pr-4 text-gray-300">{r.userValue}</td>
                              <td className="py-2.5 pr-4"><ReqCell ok={r.meetsMinimum} known={r.comparable} text={r.minimum} /></td>
                              <td className="py-2.5"><ReqCell ok={r.meetsRecommended} known={r.comparable} text={r.recommended} /></td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  )}

                  {reqRows.length > 0 && (
                    <button
                      onClick={() => setShowRawReqs(s => !s)}
                      className="text-[10px] font-black uppercase tracking-widest text-gray-500 hover:text-white transition-colors"
                    >
                      {showRawReqs ? tr('Ocultar el texto de Steam', 'Hide Steam text') : tr('Ver el texto de Steam', 'Show Steam text')}
                    </button>
                  )}

                  {(showRawReqs || reqRows.length === 0) && (
                    <div className="grid sm:grid-cols-2 gap-5">
                      {media.minimum_reqs && (
                        <div className="space-y-2 rounded-xl bg-white/[0.02] border border-white/[0.05] p-4">
                          <span className="text-[9px] font-black uppercase tracking-widest text-gray-500">{tr('Mínimo', 'Minimum')}</span>
                          <div className="system-reqs-content" dangerouslySetInnerHTML={{ __html: sanitizeRequirementsHtml(stripRequirementsHeading(media.minimum_reqs)) }} />
                        </div>
                      )}
                      {media.recommended_reqs && (
                        <div className="space-y-2 rounded-xl bg-accent/[0.04] border border-accent/[0.12] p-4">
                          <span className="text-[9px] font-black uppercase tracking-widest text-accent">{tr('Recomendado', 'Recommended')}</span>
                          <div className="system-reqs-content" dangerouslySetInnerHTML={{ __html: sanitizeRequirementsHtml(stripRequirementsHeading(media.recommended_reqs)) }} />
                        </div>
                      )}
                    </div>
                  )}
                </div>
              ) : (
                <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-8 text-center">
                  <p className="text-sm text-gray-400 font-bold">
                    {tr('No se pudieron extraer los requisitos de hardware.', 'Failed to extract hardware requirements.')}
                  </p>
                  <p className="text-xs text-gray-600 mt-2">
                    {tr('Los datos de Steam no contienen campos de requisitos válidos para esta plataforma.', 'Steam data does not contain valid requirements fields for this platform.')}
                  </p>
                </div>
              )
            )}
          </div>

          {/* ── RIGHT: title, genres, action required ── */}
          <div className="flex-1 min-w-0 w-full space-y-5">
            <div className="space-y-2.5">
              <h1 className="text-4xl font-black tracking-tight leading-[1.05]">{displayName}</h1>
              {metaLine && (
                <p className="text-gray-500 text-sm font-semibold">{metaLine}</p>
              )}
              {(rank || reviewPct !== null || languageSupport) && (
                <div className="flex gap-2 flex-wrap pt-1">
                  {rank && (
                    <span className="flex items-center gap-1.5 px-3 py-1.5 rounded-full bg-accent text-white text-[10px] font-black uppercase tracking-wider shadow-md shadow-accent/25">
                      <Trophy size={11} />
                      #{rank.position}{' '}
                      {rank.kind === 'top_sellers'
                        ? tr('más vendido', 'top seller', { pt: 'mais vendido', fr: 'meilleure vente', ru: 'по продажам' })
                        : tr('más jugado', 'most played', { pt: 'mais jogado', fr: 'plus joué', ru: 'по игрокам' })}
                    </span>
                  )}
                  {reviewPct !== null && media.reviews && (
                    <span
                      title={tr(`${media.reviews.total.toLocaleString('es')} reseñas en Steam`, `${media.reviews.total.toLocaleString('en')} Steam reviews`)}
                      className={`flex items-center gap-1.5 px-3 py-1.5 rounded-full border text-[10px] font-black uppercase tracking-wider ${reviewTone}`}
                    >
                      <ThumbsUp size={11} />
                      {reviewWords} · {reviewPct}%
                    </span>
                  )}
                  {languageSupport && (
                    <span className={`flex items-center gap-1.5 px-3 py-1.5 rounded-full border text-[10px] font-black uppercase tracking-wider ${languageTone}`}>
                      <Languages size={11} />
                      {languageLabel}
                    </span>
                  )}
                </div>
              )}
              {game.genres && game.genres.length > 0 && (
                <div className="flex gap-2 flex-wrap pt-1">
                  {game.genres.map(g => (
                    <span key={g} className="px-3 py-1.5 rounded-full bg-[#0d0e12] hover:bg-white/[0.06] border border-white/[0.08] hover:border-white/15 text-[10px] font-black uppercase tracking-wider text-gray-300 hover:text-white transition-all select-none">{g}</span>
                  ))}
                </div>
              )}
              {media.short_description && (
                <p className="text-[13px] font-medium text-gray-300 leading-relaxed pt-2 max-w-2xl border-l-2 border-white/[0.08] pl-3.5">
                  {media.short_description}
                </p>
              )}
            </div>

            {/* ── INSTALAR ──
                The card used to open with "ACCIÓN REQUERIDA · INSTALAR
                MANIFIESTO" in red, which read as an error on a game nothing
                was wrong with. It now says where the install stands and what
                it will take, and follows it live. */}
            <div className="relative overflow-hidden rounded-[1.5rem] bg-[#0d0e12] border border-white/[0.08]">
              <div className="absolute inset-x-0 top-0 h-[2px] bg-gradient-to-r from-transparent via-accent to-transparent" />
              <div className="absolute top-0 right-0 w-48 h-48 bg-accent/10 blur-[70px] rounded-full pointer-events-none" />
              <div className="relative z-10 p-6 space-y-5">
                <div className="flex items-center gap-4">
                  <div className="w-12 h-12 shrink-0 flex items-center justify-center rounded-2xl bg-accent/15 border border-accent/30">
                    {downloading
                      ? <RefreshCw size={20} className="text-accent animate-spin" />
                      : installedNow
                        ? <CheckCircle2 size={20} className="text-accent" />
                        : <Download size={20} className="text-accent" />}
                  </div>
                  <div className="min-w-0">
                    <h3 className="text-lg font-black tracking-wide text-white/90 uppercase leading-tight">
                      {downloading
                        ? tr(`Descargando · ${pct}%`, `Downloading · ${pct}%`)
                        : installedNow
                          ? tr('Instalado', 'Installed')
                          : tr('Listo para instalar', 'Ready to install')}
                    </h3>
                    {facts.length > 0 && (
                      <p className="text-[11px] font-semibold text-gray-500 mt-1">{facts.join(' · ')}</p>
                    )}
                  </div>
                </div>

                {downloading && download && (
                  <div className="space-y-1.5">
                    <div className="h-2 w-full rounded-full bg-white/[0.06] overflow-hidden">
                      <motion.div
                        className="h-full rounded-full bg-accent"
                        initial={false}
                        animate={{ width: `${pct}%` }}
                        transition={{ duration: 0.6, ease: 'easeOut' }}
                      />
                    </div>
                    <p className="text-[10px] font-semibold text-gray-500 tabular-nums">
                      {formatGameBytes(download.bytes_downloaded)} / {formatGameBytes(download.bytes_to_download)}
                    </p>
                  </div>
                )}

                {warnings.map(w => (
                  <div
                    key={w.text}
                    className={`flex items-start gap-2.5 rounded-xl border px-3.5 py-2.5 text-[11px] font-semibold leading-snug ${
                      w.tone === 'red'
                        ? 'bg-red-500/[0.07] border-red-500/25 text-red-300'
                        : 'bg-amber-500/[0.07] border-amber-500/25 text-amber-200'
                    }`}
                  >
                    <AlertTriangle size={13} className="shrink-0 mt-0.5" />
                    <span>{w.text}</span>
                  </div>
                ))}

                <motion.button
                  whileHover={{ y: -2 }}
                  whileTap={{ scale: 0.97 }}
                  onClick={startInstall}
                  disabled={installing}
                  className="group relative w-full flex items-center justify-center gap-3 py-4 bg-accent hover:brightness-110 text-white font-black text-sm rounded-2xl transition-all duration-300 uppercase tracking-widest outline-none focus:outline-none shadow-lg shadow-accent/30 overflow-hidden disabled:opacity-60 disabled:pointer-events-none"
                >
                  <div className="absolute inset-0 -translate-x-full group-hover:translate-x-full transition-transform duration-700 bg-gradient-to-r from-transparent via-white/15 to-transparent pointer-events-none" />
                  <span className="relative z-10 w-7 h-7 rounded-full bg-white/15 flex items-center justify-center">
                    {installing ? <RefreshCw size={13} className="animate-spin" /> : <Download size={13} />}
                  </span>
                  <span className="relative z-10">
                    {installing
                      ? tr('Preparando…', 'Preparing…')
                      : installedNow
                        ? tr('Reinstalar', 'Reinstall')
                        : tr('Instalar', 'Install')}
                  </span>
                </motion.button>

                {/* Tools as one row of small buttons instead of two large
                    tiles with a single word each. */}
                <div className="flex flex-wrap gap-2">
                  {[
                    { key: 'dlc', icon: <Package size={13} />, label: tr('DLCs / Desbloquear', 'DLCs / Unlock'), onClick: () => setShowUnlockModal(true) },
                    { key: 'mods', icon: <Puzzle size={13} />, label: tr('Mods de Workshop', 'Workshop mods'), onClick: () => setShowWorkshopModal(true) },
                    ...(installedNow
                      ? [{ key: 'saves', icon: <Cloud size={13} />, label: tr('Partidas', 'Saves'), onClick: () => setShowSavesModal(true) }]
                      : []),
                    { key: 'steam', icon: <SteamIcon size={13} />, label: 'Steam', onClick: openStorePage },
                  ].map(tool => (
                    <button
                      key={tool.key}
                      onClick={tool.onClick}
                      className="flex items-center gap-2 px-3.5 py-2 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[10px] font-black uppercase tracking-widest text-gray-300 hover:text-white transition-colors"
                    >
                      {tool.icon}
                      {tool.label}
                    </button>
                  ))}
                </div>

                {checklist.length > 0 && (
                  <div className="border-t border-white/[0.06] pt-5 space-y-3">
                    <span className="text-[9px] font-black uppercase tracking-widest text-gray-500">{tr('Estado', 'Status')}</span>
                    {checklist.map(item => (
                      <div key={item.key} className="flex items-start gap-3">
                        {item.state === 'ok'
                          ? <CheckCircle2 size={16} className="text-emerald-400 shrink-0 mt-0.5" />
                          : item.state === 'bad'
                            ? <XCircle size={16} className="text-red-400 shrink-0 mt-0.5" />
                            : <Circle size={16} className="text-gray-600 shrink-0 mt-0.5" />}
                        <div className="min-w-0">
                          <p className={`text-xs font-bold ${
                            item.state === 'ok' ? 'text-white/85' : item.state === 'bad' ? 'text-red-300' : 'text-gray-400'
                          }`}>
                            {item.label}
                          </p>
                          {item.hint && (
                            <p className="text-[11px] font-medium text-gray-500 leading-snug mt-0.5">{item.hint}</p>
                          )}
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>

            {/* ── CAPTURAS ── */}
            {!loading && media.screenshots.length > 0 && (
              <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-6">
                <div className="mb-4 flex items-center justify-between">
                  <h3 className="text-[10px] font-black uppercase tracking-widest text-gray-500 flex items-center gap-2">
                    <Maximize2 size={12} className="text-gray-600" /> {t.detail.screenshots}
                  </h3>
                  <span className="text-[11px] text-gray-700 font-bold">{media.screenshots.length}</span>
                </div>
                {/* Scroll horizontal de capturas.

                    The tile draws at 220x124 and takes the 600x338 thumbnail;
                    the lightbox keeps the full 1920x1080. Using path_full for
                    both meant ~350 KB per tile — 7.7 MB across a game like
                    Cyberpunk's 22 shots — to fill a box an eighth that size. */}
                <div className="flex gap-3 overflow-x-auto pb-2 custom-scrollbar" style={{ scrollSnapType: 'x mandatory' }}>
                  {media.screenshots.map((src, i) => (
                    <motion.div
                      key={i}
                      whileHover={{ scale: 1.02 }}
                      className="relative shrink-0 overflow-hidden rounded-xl cursor-pointer border border-white/5 hover:border-white/20 transition-all group/ss shadow-lg"
                      style={{ width: '220px', height: '124px', scrollSnapAlign: 'start' }}
                      onClick={() => setLightbox({ src, idx: i })}
                    >
                      <img
                        src={media.screenshots_thumbs?.[i] ?? src}
                        alt={`Screenshot ${i + 1}`}
                        loading="lazy"
                        decoding="async"
                        className="w-full h-full object-cover"
                      />
                      <div className="absolute inset-0 bg-black/0 group-hover/ss:bg-black/30 transition-colors flex items-center justify-center">
                        <Maximize2 size={16} className="opacity-0 group-hover/ss:opacity-100 transition-opacity text-white drop-shadow-lg" />
                      </div>
                      <div className="absolute bottom-2 right-2 text-[10px] text-white/40 font-bold">{i + 1}</div>
                    </motion.div>
                  ))}
                </div>
              </div>
            )}

            {!loading && media.screenshots.length === 0 && (
              <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-6 flex items-center gap-3 text-gray-700">
                <Gamepad2 size={16} opacity={0.4} />
                <span className="text-sm">{t.detail.noScreenshots}</span>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Unlock / Cloud Saves modals */}
      <AnimatePresence>
        {showUnlockModal && (
          <UnlockModal game={game} steamPath={steamPath} onClose={() => setShowUnlockModal(false)} />
        )}
        {showSavesModal && (
          <CloudSavesModal game={game} steamPath={steamPath} onClose={() => setShowSavesModal(false)} />
        )}
        {showWorkshopModal && (
          <WorkshopModal game={game} steamPath={steamPath} onClose={() => setShowWorkshopModal(false)} />
        )}
      </AnimatePresence>

      {/* Screenshot Lightbox */}
      <AnimatePresence>
        {lightbox && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 bg-black/95 z-[60] flex items-center justify-center p-8"
            onClick={() => setLightbox(null)}
          >
            <motion.img
              key={lightbox.src}
              initial={{ scale: 0.93, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              transition={{ duration: 0.18 }}
              src={lightbox.src}
              className="max-w-[88vw] max-h-[88vh] rounded-xl object-contain shadow-2xl"
              onClick={e => e.stopPropagation()}
            />
            <button className="absolute top-5 right-5 w-9 h-9 bg-white/10 hover:bg-white/20 rounded-xl flex items-center justify-center transition-all" onClick={() => setLightbox(null)}>
              <X size={16} />
            </button>
            {lightbox.idx > 0 && (
              <button
                className="absolute left-5 top-1/2 -translate-y-1/2 w-11 h-11 bg-white/10 hover:bg-white/20 rounded-xl flex items-center justify-center transition-all"
                onClick={e => { e.stopPropagation(); setLightbox({ src: media.screenshots[lightbox.idx - 1], idx: lightbox.idx - 1 }); }}
              >
                <ArrowLeft size={18} />
              </button>
            )}
            {lightbox.idx < media.screenshots.length - 1 && (
              <button
                className="absolute right-5 top-1/2 -translate-y-1/2 w-11 h-11 bg-white/10 hover:bg-white/20 rounded-xl flex items-center justify-center transition-all"
                onClick={e => { e.stopPropagation(); setLightbox({ src: media.screenshots[lightbox.idx + 1], idx: lightbox.idx + 1 }); }}
              >
                <ArrowLeft size={18} className="rotate-180" />
              </button>
            )}
            <div className="absolute bottom-5 left-1/2 -translate-x-1/2 px-4 py-1.5 bg-white/5 rounded-full text-xs text-gray-500 font-bold border border-white/5">
              {lightbox.idx + 1} / {media.screenshots.length}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  );
};

// --- DLC Modal ---

interface DlcEntry {
  id: string;
  name: string;
}

/// A DLC's store capsule, or a plain box when Steam has none for it.
/// One DLC's little capsule.
///
/// It used to point straight at `capsule_184x69.jpg`, which 404s for every DLC
/// of a recent game — their art sits behind a hashed path only the store knows
/// (checked against Elden Ring Nightreign's five DLCs: all 404 on the fixed
/// URL, two answer with a real image through the store). So this goes through
/// the same resolver and disk cache the game covers use, which knows how to
/// ask. DLCs Steam has nothing at all for still fall back to the icon.
const DlcThumb = ({ id }: { id: string }) => {
  const [src, setSrc] = useState<string | null>(() => imageMemoryCache.get(id) ?? null);
  const [failed, setFailed] = useState(() => recentlyFailed(id));

  useEffect(() => {
    let cancelled = false;
    const hit = imageMemoryCache.get(id);
    if (hit) { setSrc(hit); setFailed(false); return; }
    setSrc(null);
    setFailed(recentlyFailed(id));
    if (recentlyFailed(id)) return;
    loadCoverArt(id).then(url => {
      if (cancelled) return;
      setSrc(url);
      setFailed(url === null);
    });
    return () => { cancelled = true; };
  }, [id]);

  if (failed || !src) {
    return (
      <div className="w-[92px] h-[43px] shrink-0 rounded-lg bg-white/[0.04] border border-white/[0.06] flex items-center justify-center overflow-hidden">
        {failed
          ? <Package size={14} className="text-gray-600" />
          : <div className="w-full h-full bg-white/[0.03] animate-pulse" />}
      </div>
    );
  }
  return (
    <img
      src={src}
      alt=""
      loading="lazy"
      decoding="async"
      onError={() => setFailed(true)}
      className="w-[92px] h-[43px] shrink-0 rounded-lg object-cover bg-white/[0.04] border border-white/[0.06]"
    />
  );
};

// --- Unlock Modal (DLC & DRM Patch Unlock) ---

type UnlockTab = 'dlcs' | 'unlocker' | 'steamless' | 'goldberg';
type UnlockerType = 'creamapi' | 'smokeapi' | 'uplayr1' | 'uplayr2' | 'koaloader';

const UnlockModal = ({
  game,
  steamPath,
  onClose,
}: {
  game: Game;
  steamPath: string;
  onClose: () => void;
}) => {
  const { notify } = useNotify();
  const tr = useTranslateInline();
  const [tab, setTab] = useState<UnlockTab>('dlcs');
  const [unlockerType, setUnlockerType] = useState<UnlockerType>('creamapi');
  const [gamePath, setGamePath] = useState('');
  const [exePath, setExePath] = useState('');
  const [dlcList, setDlcList] = useState<DlcEntry[]>([]);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [dlcLoading, setDlcLoading] = useState(false);
  const [dlcSearched, setDlcSearched] = useState(false);
  // What the game's ticket already has, to mark and to compare against.
  const [savedIds, setSavedIds] = useState<Set<string>>(new Set());
  const [dlcFilter, setDlcFilter] = useState('');
  const [savingTicket, setSavingTicket] = useState(false);
  const [busy, setBusy] = useState(false);
  const [drm, setDrm] = useState<DrmCheckResult | null>(null);

  useEffect(() => {
    if (!gamePath) return;
    invoke<string>('detect_unlocker_type', { gamePath }).then(dt => {
      if (dt === 'creamapi' || dt === 'smokeapi' || dt === 'uplayr1' || dt === 'uplayr2') setUnlockerType(dt);
    }).catch(() => { });
  }, [gamePath]);

  useEffect(() => {
    if (gamePath && !exePath) setExePath(`${gamePath}\\${game.name}.exe`);
  }, [gamePath, game.name]);

  useEffect(() => {
    invoke<string>('check_game_drm', { appId: game.id }).then(raw => {
      try {
        const parsed: DrmCheckResult = JSON.parse(raw);
        setDrm(parsed);
      } catch { }
    }).catch(() => { });
  }, [game.id]);

  const browseFolder = async () => {
    const selected = await open({ directory: true, multiple: false, defaultPath: gamePath || steamPath });
    if (selected && typeof selected === 'string') setGamePath(selected);
  };

  const browseExe = async () => {
    const selected = await open({ directory: false, multiple: false, filters: [{ name: 'Executables', extensions: ['exe'] }], defaultPath: exePath || steamPath });
    if (selected && typeof selected === 'string') setExePath(selected);
  };

  const fetchDlcs = async () => {
    setDlcLoading(true);
    setDlcSearched(true);
    try {
      const [result, saved] = await Promise.all([
        invoke<{ cached: boolean; items: DlcEntry[] }>('get_dlcs', { appId: game.id, steamPath }),
        invoke<string[]>('get_saved_dlcs', { steamPath, appId: game.id }).catch(() => [] as string[]),
      ]);
      setDlcList(result.items);
      setSavedIds(new Set(saved));
      // What the ticket already has when it has anything; every DLC on a
      // first visit, which is what saving meant before.
      setSelectedIds(new Set(saved.length > 0 ? saved : result.items.map(d => d.id)));
    } catch (err) {
      notify(tr(`Error al buscar DLCs: ${err}`, `Error fetching DLCs: ${err}`), 'error');
    } finally {
      setDlcLoading(false);
    }
  };

  // Loaded on open. The tab used to sit empty behind a "Buscar DLCs" button
  // for a list the backend already keeps cached.
  useEffect(() => { void fetchDlcs(); }, [game.id]);

  const selectedInList = dlcList.filter(d => selectedIds.has(d.id)).length;
  const allSelected = dlcList.length > 0 && selectedInList === dlcList.length;
  const toggleAllDlcs = () => {
    setSelectedIds(prev => {
      const next = new Set(prev);
      for (const d of dlcList) {
        if (allSelected) next.delete(d.id); else next.add(d.id);
      }
      return next;
    });
  };
  const visibleDlcs = dlcFilter.trim()
    ? dlcList.filter(d => d.name.toLowerCase().includes(dlcFilter.trim().toLowerCase()) || d.id.startsWith(dlcFilter.trim()))
    : dlcList;
  const dlcDirty = selectedIds.size !== savedIds.size || Array.from(selectedIds).some(id => !savedIds.has(id));

  const toggleDlc = (id: string) => {
    setSelectedIds(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  };

  const requireGamePath = () => {
    if (!gamePath) {
      notify(tr('Selecciona la ruta del juego primero', 'Please select game path first'), 'error');
      return false;
    }
    return true;
  };

  // Alternate destination for the same DLC selection above: instead of (or
  // alongside) patching the game's own steam_api.dll via an unlocker, this
  // writes the picked DLC ids straight into the SteamTools ticket (config/lua)
  // — the same mechanism the catalog's "Instalar" flow uses. No unlocker DLL,
  // no game folder needed, works for anything already ticket-injected.
  const handleSaveToSteamTools = async () => {
    setSavingTicket(true);
    try {
      await invoke('update_dlcs', { steamPath, appId: game.id, dlcIds: Array.from(selectedIds) });
      setSavedIds(new Set(selectedIds));
      notify(tr(`DLCs guardados en el ticket para ${displayName}`, `DLCs saved to the ticket for ${displayName}`), 'success');
    } catch (err) {
      notify(`Error: ${err}`, 'error');
    } finally {
      setSavingTicket(false);
    }
  };

  const handleInject = async () => {
    if (!requireGamePath()) return;
    setBusy(true);
    try {
      let msg: string;
      if (unlockerType === 'creamapi') {
        const dlcs: [string, string][] = dlcList.filter(d => selectedIds.has(d.id)).map(d => [d.id, d.name]);
        msg = await invoke<string>('apply_creamapi', { gamePath, appid: game.id, dlcs });
      } else if (unlockerType === 'smokeapi') {
        msg = await invoke<string>('apply_smokeapi', { gamePath, appid: game.id, dlcIds: Array.from(selectedIds) });
      } else if (unlockerType === 'koaloader') {
        msg = await invoke<string>('apply_koaloader', { gamePath, appid: game.id, dlcIds: Array.from(selectedIds) });
      } else {
        msg = await invoke<string>('apply_uplay_unlocker', { gamePath, isR2: unlockerType === 'uplayr2' });
      }
      notify(msg, 'success');
    } catch (err) {
      notify(`Error: ${err}`, 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleUninstall = async () => {
    if (!requireGamePath()) return;
    setBusy(true);
    try {
      let msg: string;
      if (unlockerType === 'creamapi') msg = await invoke<string>('uninstall_creamapi', { gamePath });
      else if (unlockerType === 'smokeapi') msg = await invoke<string>('uninstall_smokeapi', { gamePath });
      else if (unlockerType === 'koaloader') msg = await invoke<string>('uninstall_koaloader', { gamePath });
      else msg = await invoke<string>('uninstall_uplay_unlocker', { gamePath });
      notify(msg, 'success');
    } catch (err) {
      notify(`Error: ${err}`, 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleSteamless = async () => {
    if (!exePath) {
      notify(tr('Selecciona el ejecutable primero', 'Please select executable first'), 'error');
      return;
    }
    setBusy(true);
    try {
      const msg = await invoke<string>('run_steamless', { exePath });
      notify(msg, 'success');
    } catch (err) {
      notify(`Error: ${err}`, 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleGoldbergApply = async () => {
    if (!requireGamePath()) return;
    setBusy(true);
    try {
      const apiKey = localStorage.getItem('rl_steam_api_key') ?? '';
      const msg = await invoke<string>('apply_goldberg', { gameFolder: gamePath, appId: game.id, steamApiKey: apiKey });
      notify(msg, 'success');
    } catch (err) {
      notify(`Error: ${err}`, 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleGoldbergRestore = async () => {
    if (!requireGamePath()) return;
    setBusy(true);
    try {
      const msg = await invoke<string>('uninstall_goldberg', { gameFolder: gamePath });
      notify(msg, 'success');
    } catch (err) {
      notify(`Error: ${err}`, 'error');
    } finally {
      setBusy(false);
    }
  };

  const rawName = game.name?.trim() ?? '';
  const displayName = rawName && !rawName.startsWith('Game ID:') ? rawName : `App ${game.id}`;

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      className="fixed inset-0 bg-black/80 backdrop-blur-sm z-50 flex items-center justify-center p-6"
      onClick={onClose}
    >
      <motion.div
        initial={{ scale: 0.92, opacity: 0 }}
        animate={{ scale: 1, opacity: 1 }}
        exit={{ scale: 0.92, opacity: 0 }}
        transition={{ duration: 0.2 }}
        className="bg-[#0d0e12] border border-white/[0.08] rounded-2xl w-full max-w-xl flex flex-col overflow-hidden shadow-2xl shadow-black/60"
        style={{ maxHeight: '85vh' }}
        onClick={e => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-5 border-b border-white/[0.06] shrink-0">
          <div className="flex items-center gap-3">
            <div className="w-9 h-9 rounded-full bg-accent/15 border border-accent/30 flex items-center justify-center shrink-0">
              <Package size={16} className="text-accent" />
            </div>
            <div>
              <h3 className="font-black text-base truncate max-w-xs">{displayName}</h3>
              <p className="text-[10px] text-gray-500 font-bold uppercase tracking-wider">
                DLC Manager · APP {game.id}
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="w-8 h-8 bg-white/5 hover:bg-white/10 rounded-xl flex items-center justify-center transition-all text-gray-400 hover:text-white"
          >
            <X size={15} />
          </button>
        </div>

        {/* Mini tabs */}
        <div className="flex gap-1.5 px-6 pt-4 shrink-0">
          {(['dlcs', 'unlocker', 'steamless', 'goldberg'] as UnlockTab[]).map(tKey => (
            <button
              key={tKey}
              onClick={() => setTab(tKey)}
              className={`px-3.5 py-1.5 rounded-full border text-[9px] font-black uppercase tracking-widest transition-colors duration-300 ${tab === tKey ? 'bg-accent border-accent text-white shadow-md shadow-accent/20' : 'bg-white/[0.05] border-white/10 text-gray-500 hover:text-white hover:bg-white/[0.09]'}`}
            >
              {tKey === 'dlcs' ? 'DLCs' : tKey === 'unlocker' ? 'Unlockers' : tKey === 'steamless' ? 'SteamStub' : 'Goldberg'}
            </button>
          ))}
        </div>

        {/* DLCs tab: same layout the old standalone DLC Manager modal had —
            full-height scrollable list + a fixed footer with Buscar/Guardar,
            instead of the compact inline version the other tabs use. */}
        {tab === 'dlcs' && (
          <>
            {dlcList.length > 0 && !dlcLoading && (
              <div className="flex items-center gap-2 px-6 pt-4 shrink-0">
                {dlcList.length > 6 ? (
                  <div className="relative flex-1">
                    <Search size={13} className="absolute left-3 top-1/2 -translate-y-1/2 text-gray-600 pointer-events-none" />
                    <input
                      value={dlcFilter}
                      onChange={e => setDlcFilter(e.target.value)}
                      placeholder={tr('Buscar DLC…', 'Search DLC…')}
                      spellCheck={false}
                      className="w-full bg-black/40 border border-white/10 rounded-xl py-2 pl-9 pr-3 text-xs text-white/90 placeholder:text-gray-600 focus:outline-none focus:border-accent/50 focus:ring-1 focus:ring-accent/25 transition-all"
                    />
                  </div>
                ) : <div className="flex-1" />}
                <button
                  onClick={toggleAllDlcs}
                  className="shrink-0 px-3.5 py-2 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[10px] font-black uppercase tracking-widest text-gray-300 hover:text-white transition-colors"
                >
                  {allSelected ? tr('Ninguno', 'None') : tr('Todos', 'All')}
                </button>
              </div>
            )}
            <div className="flex-1 overflow-y-auto custom-scrollbar px-6 py-4 space-y-2">
              {dlcList.length === 0 && !dlcLoading && dlcSearched && (
                <div className="flex flex-col items-center justify-center py-12 gap-3 text-gray-500">
                  <Package size={36} opacity={0.3} />
                  <p className="text-sm font-medium">{tr('No se encontraron DLCs para este juego', 'No DLCs found for this game')}</p>
                  <p className="text-xs text-gray-600 text-center max-w-xs">
                    {tr('Es posible que los DLCs estén incluidos en el juego base o no estén disponibles en la tienda de Steam.', 'DLCs may be bundled into the base game, or not available on the Steam store.')}
                  </p>
                </div>
              )}
              {dlcLoading && Array.from({ length: 4 }).map((_, i) => (
                <div key={i} className="flex items-center gap-3 p-2 pr-3 rounded-xl border border-white/[0.06] bg-white/[0.02]">
                  <div className="w-[92px] h-[43px] rounded-lg bg-white/[0.05] animate-pulse shrink-0" />
                  <div className="flex-1 space-y-2">
                    <div className="h-3 w-2/3 rounded bg-white/[0.06] animate-pulse" />
                    <div className="h-2 w-1/4 rounded bg-white/[0.04] animate-pulse" />
                  </div>
                </div>
              ))}
              {!dlcLoading && visibleDlcs.map(dlc => {
                const on = selectedIds.has(dlc.id);
                return (
                  <button
                    key={dlc.id}
                    onClick={() => toggleDlc(dlc.id)}
                    className={`w-full flex items-center gap-3 p-2 pr-3 rounded-xl border text-left transition-colors duration-200 ${
                      on ? 'bg-accent/[0.07] border-accent/30' : 'bg-white/[0.02] border-white/[0.06] hover:bg-white/[0.05]'
                    }`}
                  >
                    <DlcThumb id={dlc.id} />
                    <div className="flex-1 min-w-0">
                      <p className={`text-[13px] font-bold truncate transition-colors ${on ? 'text-white' : 'text-gray-400'}`}>{dlc.name}</p>
                      <p className="text-[10px] text-gray-600 font-bold uppercase tracking-wider">
                        ID {dlc.id}
                        {savedIds.has(dlc.id) && (
                          <span className="ml-2 text-emerald-400">· {tr('Activo en el ticket', 'Active in ticket')}</span>
                        )}
                      </p>
                    </div>
                    <span className={`w-5 h-5 shrink-0 rounded-md border flex items-center justify-center transition-colors ${
                      on ? 'bg-accent border-accent' : 'border-white/20 bg-black/30'
                    }`}>
                      {on && <Check size={12} strokeWidth={3} className="text-white" />}
                    </span>
                  </button>
                );
              })}
              {!dlcLoading && dlcList.length > 0 && visibleDlcs.length === 0 && (
                <p className="text-xs text-gray-600 text-center py-8">{tr('Ningún DLC coincide con la búsqueda.', 'No DLC matches the search.')}</p>
              )}
            </div>

            <div className="px-6 py-4 border-t border-white/[0.06] shrink-0 flex gap-3 items-center">
              <button
                onClick={fetchDlcs}
                disabled={dlcLoading}
                title={tr('Volver a pedir la lista a Steam', 'Fetch the list from Steam again')}
                className="w-10 h-10 shrink-0 flex items-center justify-center rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-400 hover:text-white transition-colors disabled:opacity-50"
              >
                <RefreshCw size={14} className={dlcLoading ? 'animate-spin' : ''} />
              </button>
              {dlcList.length > 0 && (
                <span className="text-[11px] text-gray-500 font-bold tabular-nums">
                  {selectedInList} / {dlcList.length} {tr('seleccionados', 'selected')}
                </span>
              )}
              <div className="flex-1" />
              <button
                onClick={handleSaveToSteamTools}
                disabled={savingTicket || dlcList.length === 0 || !dlcDirty}
                className="flex items-center gap-2 px-6 py-2.5 bg-accent hover:brightness-110 text-white font-black text-xs rounded-xl uppercase tracking-widest transition-all shadow-lg shadow-accent/25 disabled:opacity-40 disabled:pointer-events-none"
              >
                {savingTicket ? <RefreshCw size={13} className="animate-spin" /> : <CheckCircle size={13} />}
                {dlcDirty ? tr('Guardar en el ticket', 'Save to ticket') : tr('Guardado', 'Saved')}
              </button>
            </div>
          </>
        )}

        {/* Body (other tabs) */}
        {tab !== 'dlcs' && (
        <div className="flex-1 overflow-y-auto custom-scrollbar p-6 space-y-6">
          {tab === 'unlocker' && (
            <div className="space-y-4">
              <div className="space-y-2">
                <span className="text-[10px] font-black uppercase tracking-wider text-gray-400">{tr('Seleccionar Emulador/Unlocker', 'Select Emulator/Unlocker')}</span>
                <div className="grid grid-cols-2 sm:grid-cols-5 gap-2">
                  {(['creamapi', 'smokeapi', 'uplayr1', 'uplayr2', 'koaloader'] as UnlockerType[]).map(u => (
                    <button
                      key={u}
                      onClick={() => setUnlockerType(u)}
                      title={u === 'koaloader' ? tr('Respaldo cuando CreamAPI/SmokeAPI directo no funciona — no toca steam_api.dll', 'Fallback for when direct CreamAPI/SmokeAPI doesn\'t work — never touches steam_api.dll') : undefined}
                      className={`py-2.5 px-3 rounded-xl border text-[9px] font-black uppercase tracking-wider transition-all duration-300 ${unlockerType === u ? 'bg-gradient-to-r from-blue-600/15 to-indigo-600/15 border-blue-500/50 text-blue-400 shadow-[0_0_15px_rgba(59,130,246,0.12)]' : 'bg-[#07080d]/40 border-white/5 text-gray-500 hover:text-gray-300 hover:bg-white/5 hover:border-white/10'}`}
                    >
                      {u === 'creamapi' ? 'CreamAPI' : u === 'smokeapi' ? 'SmokeAPI' : u === 'uplayr1' ? 'Uplay R1' : u === 'uplayr2' ? 'Uplay R2' : 'Koaloader'}
                    </button>
                  ))}
                </div>
              </div>

              <div className="space-y-2">
                <span className="text-[10px] font-black uppercase tracking-wider text-gray-400">{tr('Carpeta de Instalación del Juego', 'Game Installation Folder')}</span>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={gamePath}
                    onChange={e => setGamePath(e.target.value)}
                    className="flex-1 bg-[#07080d]/40 border border-white/8 focus:border-accent/40 rounded-xl py-2.5 px-4 text-xs outline-none transition-all font-mono text-gray-300 shadow-inner"
                    placeholder="C:\Program Files (x86)\Steam\steamapps\common\..."
                  />
                  <button
                    onClick={browseFolder}
                    className="px-4 bg-white/5 hover:bg-white/10 border border-white/5 hover:border-white/10 rounded-xl text-gray-400 hover:text-white transition-all flex items-center justify-center group"
                  >
                    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" className="text-gray-400 group-hover:text-white transition-colors">
                      <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
                    </svg>
                  </button>
                </div>
              </div>

              {(unlockerType === 'creamapi' || unlockerType === 'smokeapi' || unlockerType === 'koaloader') && (
                <div className="flex items-center justify-between gap-3 p-3.5 rounded-2xl bg-white/5 border border-white/10">
                  <p className="text-[10px] text-gray-400 leading-relaxed">
                    {dlcList.length > 0
                      ? tr(
                          `Se van a inyectar los ${selectedIds.size} DLC(s) marcados en la pestaña "DLCs".`,
                          `The ${selectedIds.size} DLC(s) checked in the "DLCs" tab will be injected.`
                        )
                      : tr('Elegí los DLCs a inyectar en la pestaña "DLCs".', 'Pick which DLCs to inject in the "DLCs" tab.')}
                  </p>
                  <button
                    onClick={() => setTab('dlcs')}
                    className="shrink-0 px-3 py-1.5 bg-white/5 hover:bg-white/10 border border-white/10 rounded-lg text-[9px] font-black uppercase tracking-wider text-gray-300 hover:text-white transition-all"
                  >
                    {tr('Ir a DLCs', 'Go to DLCs')}
                  </button>
                </div>
              )}

              {drm?.status === 'denuvo' && (
                <div className="p-4 rounded-2xl bg-red-600/15 border border-red-500/40 text-xs space-y-2">
                  <div className="flex items-center gap-2 text-red-400 font-bold">
                    <AlertTriangle size={16} />
                    <span>{tr('DRM Denuvo Detectado', 'Denuvo DRM Detected')}</span>
                  </div>
                  <p className="text-gray-300 leading-relaxed">
                    {tr(
                      'Este juego usa Denuvo. Los unlockers normales (CreamAPI, SmokeAPI) no funcionan con Denuvo. Busca un crack o parche específico para este juego.',
                      'This game uses Denuvo. Standard unlockers (CreamAPI, SmokeAPI) do NOT work with Denuvo. Look for a dedicated crack or patch for this game.'
                    )}
                  </p>
                </div>
              )}
              {drm?.status === 'third_party' && (
                <div className="p-4 rounded-2xl bg-amber-600/15 border border-amber-500/40 text-xs space-y-2">
                  <div className="flex items-center gap-2 text-amber-400 font-bold">
                    <AlertOctagon size={16} />
                    <span>{tr('Requiere Cuenta Externa', 'Third-Party Account Required')}</span>
                  </div>
                  <p className="text-gray-300 leading-relaxed">{drm.message}</p>
                </div>
              )}
              {drm?.status === 'drm_detected' && (
                <div className="p-4 rounded-2xl bg-amber-600/15 border border-amber-500/40 text-xs space-y-2">
                  <div className="flex items-center gap-2 text-amber-400 font-bold">
                    <AlertOctagon size={16} />
                    <span>{tr('DRM Detectado', 'DRM Detected')}</span>
                  </div>
                  <p className="text-gray-300 leading-relaxed">{drm.message}</p>
                </div>
              )}

              <div className="flex gap-2">
                <button
                  onClick={handleInject}
                  disabled={busy || drm?.status === 'denuvo'}
                  className="flex-1 py-3.5 bg-gradient-to-r from-blue-600 to-indigo-600 hover:from-blue-500 hover:to-indigo-500 disabled:opacity-50 text-white font-black text-xs rounded-xl uppercase tracking-widest transition-all duration-300 flex items-center justify-center gap-2 shadow-lg shadow-blue-500/10 hover:shadow-blue-500/20 hover:scale-[1.01] active:scale-[0.99] disabled:scale-100 disabled:shadow-none"
                >
                  <CheckCircle2 size={14} className={busy ? 'animate-spin' : ''} />
                  {tr('Inyectar', 'Inject')}
                </button>
                <button
                  onClick={handleUninstall}
                  disabled={busy}
                  className="flex-[0.4] py-3.5 bg-red-600/20 hover:bg-red-600/30 border border-red-500/30 hover:border-red-500/50 disabled:opacity-50 text-red-400 font-black text-xs rounded-xl uppercase tracking-widest transition-all duration-300 flex items-center justify-center gap-2 hover:scale-[1.01] active:scale-[0.99] disabled:scale-100"
                >
                  <Trash2 size={14} />
                  {tr('Desinstalar', 'Uninstall')}
                </button>
              </div>
            </div>
          )}

          {tab === 'steamless' && (
            <div className="space-y-4">
              <div className="p-4 rounded-2xl bg-white/5 border border-white/10 text-xs text-gray-400 leading-relaxed">
                <p>
                  <strong>Steamless</strong>{' '}
                  {tr(
                    'es una herramienta automatizada para remover la protección de SteamStub DRM en archivos ejecutables. Recomendamos hacer una copia de seguridad del juego antes de continuar.',
                    'automatically unpacks and removes SteamStub DRM from your game executables. It is highly recommended to backup your game file first.'
                  )}
                </p>
              </div>
              <div className="space-y-2">
                <span className="text-[10px] font-black uppercase tracking-wider text-gray-400">{tr('Seleccionar archivo Ejecutable (.exe)', 'Select Executable File (.exe)')}</span>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={exePath}
                    onChange={e => setExePath(e.target.value)}
                    className="flex-1 bg-[#07080d]/40 border border-white/8 focus:border-accent/40 rounded-xl py-2.5 px-4 text-xs outline-none transition-all font-mono text-gray-300 shadow-inner"
                    placeholder="C:\...\game.exe"
                  />
                  <button
                    onClick={browseExe}
                    className="px-4 bg-white/5 hover:bg-white/10 border border-white/5 hover:border-white/10 rounded-xl text-gray-400 hover:text-white transition-all flex items-center justify-center group"
                  >
                    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" className="text-gray-400 group-hover:text-white transition-colors">
                      <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
                    </svg>
                  </button>
                </div>
              </div>
              <button
                onClick={handleSteamless}
                disabled={busy}
                className="w-full py-3.5 bg-gradient-to-r from-purple-600 to-fuchsia-600 hover:from-purple-500 hover:to-fuchsia-500 disabled:opacity-50 text-white font-black text-xs rounded-xl uppercase tracking-widest transition-all duration-300 flex items-center justify-center gap-2 shadow-lg shadow-purple-500/10 hover:shadow-purple-500/20 hover:scale-[1.01] active:scale-[0.99] disabled:scale-100 disabled:shadow-none"
              >
                <ShieldOff size={14} className={busy ? 'animate-spin' : ''} />
                {tr('Remover SteamStub DRM', 'Remove SteamStub DRM')}
              </button>
            </div>
          )}

          {tab === 'goldberg' && (
            <div className="space-y-4">
              <div className="p-4 rounded-2xl bg-white/5 border border-white/10 text-xs text-gray-400 leading-relaxed">
                <p>
                  <strong>Goldberg Emulator</strong>{' '}
                  {tr(
                    'reemplaza steam_api.dll para engañar a Steam y permitir jugar sin conexión o sin cuenta. Útil para juegos que requieren Steamworks DRM pero no tienen SteamStub.',
                    'replaces steam_api.dll to trick Steam into letting you play offline or without an account. Useful for games that require Steamworks DRM but do not have SteamStub.'
                  )}
                </p>
              </div>
              <div className="space-y-2">
                <span className="text-[10px] font-black uppercase tracking-wider text-gray-400">{tr('Carpeta de Instalación del Juego', 'Game Installation Folder')}</span>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={gamePath}
                    onChange={e => setGamePath(e.target.value)}
                    className="flex-1 bg-[#07080d]/40 border border-white/8 focus:border-accent/40 rounded-xl py-2.5 px-4 text-xs outline-none transition-all font-mono text-gray-300 shadow-inner"
                    placeholder="C:\Program Files (x86)\Steam\steamapps\common\..."
                  />
                  <button
                    onClick={browseFolder}
                    className="px-4 bg-white/5 hover:bg-white/10 border border-white/5 hover:border-white/10 rounded-xl text-gray-400 hover:text-white transition-all flex items-center justify-center group"
                  >
                    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" className="text-gray-400 group-hover:text-white transition-colors">
                      <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
                    </svg>
                  </button>
                </div>
              </div>
              <div className="p-3 rounded-2xl bg-amber-600/10 border border-amber-500/20 text-xs text-amber-400">
                {tr('Nota: Si el juego tiene SteamStub DRM, ejecutá SteamStub (Steamless) ANTES de aplicar Goldberg.', 'Note: If the game has SteamStub DRM, run Steamless BEFORE applying Goldberg.')}
              </div>
              {!(localStorage.getItem('rl_steam_api_key') ?? '').trim() && (
                <div className="p-3 rounded-2xl bg-blue-600/10 border border-blue-500/20 text-xs text-blue-300">
                  {tr(
                    'Sin una Steam Web API Key no se generará la lista de logros — el juego correrá, pero los logros no se podrán rastrear.',
                    "Without a Steam Web API Key, the achievement list won't be generated — the game will run, but achievements can't be tracked."
                  )}
                </div>
              )}
              <div className="flex gap-2">
                <button
                  onClick={handleGoldbergApply}
                  disabled={busy}
                  className="flex-1 py-3.5 bg-gradient-to-r from-emerald-600 to-teal-600 hover:from-emerald-500 hover:to-teal-500 disabled:opacity-50 text-white font-black text-xs rounded-xl uppercase tracking-widest transition-all duration-300 flex items-center justify-center gap-2 shadow-lg shadow-emerald-500/10 hover:shadow-emerald-500/20 hover:scale-[1.01] active:scale-[0.99] disabled:scale-100 disabled:shadow-none"
                >
                  <CheckCircle2 size={14} className={busy ? 'animate-spin' : ''} />
                  {tr('Aplicar Goldberg', 'Apply Goldberg')}
                </button>
                <button
                  onClick={handleGoldbergRestore}
                  disabled={busy}
                  className="flex-[0.4] py-3.5 bg-red-600/20 hover:bg-red-600/30 border border-red-500/30 hover:border-red-500/50 disabled:opacity-50 text-red-400 font-black text-xs rounded-xl uppercase tracking-widest transition-all duration-300 flex items-center justify-center gap-2 hover:scale-[1.01] active:scale-[0.99] disabled:scale-100"
                >
                  <Trash2 size={14} />
                  {tr('Restaurar', 'Restore')}
                </button>
              </div>
            </div>
          )}
        </div>
        )}
      </motion.div>
    </motion.div>
  );
};

// --- Game Card ---

/// Hover previews: the store's own few-second clip per game, asked for once.
const hoverTrailerCache = new Map<string, Promise<string | null>>();
const getHoverTrailer = (appId: string): Promise<string | null> => {
  let pending = hoverTrailerCache.get(appId);
  if (!pending) {
    pending = invoke<string | null>('get_hover_trailer', { appId }).catch(() => {
      // A failed lookup is tried again next time, not remembered as "none".
      hoverTrailerCache.delete(appId);
      return null;
    });
    hoverTrailerCache.set(appId, pending);
  }
  return pending;
};

/// How long the pointer has to rest on a card before its clip starts. Long
/// enough that sweeping across the grid downloads nothing: each clip is
/// around 2.5 MB.
const HOVER_PREVIEW_DELAY_MS = 650;

/// A game's place in one of Steam's charts.
type CatalogRank = { kind: 'top_sellers' | 'most_played'; position: number };

/// Catalog tags also come in Chinese, Russian and so on; the card shows only
/// the ones readable next to the rest of the interface.
const LATIN_GENRE = /^[ -~À-ɏ]+$/;

const GameCard = memo(({ game, onSelect, isLibrary, onInstall, steamPath, onUninstall, isPinned, onTogglePin, installSource, rank, libraryStats }: {
  game: Game;
  onSelect?: (game: Game) => void;
  isLibrary?: boolean;
  onInstall?: (id: string) => void;
  steamPath?: string;
  onUninstall?: (id: string) => void;
  isPinned?: boolean;
  onTogglePin?: (id: string, pinned: boolean) => void;
  // "ryuu" or "hubcap": the service that delivered this game's ticket.
  installSource?: string;
  // Place in Steam's charts, shown in the hover panel.
  rank?: CatalogRank;
  // Play history and install state, in the Library.
  libraryStats?: LibraryGameStats;
}) => {
  const { t, lang } = useLanguage();
  const ti = useTranslateInline();
  const { notify } = useNotify();
  const [showSavesModal, setShowSavesModal] = useState(false);

  // Hover preview. Never for +18 games, and not with the system's data saver
  // on. Leaving the card unmounts the <video>, which stops its download.
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [previewPlaying, setPreviewPlaying] = useState(false);
  const hoverTimer = useRef<number | null>(null);
  const hovering = useRef(false);
  const startPreview = () => {
    hovering.current = true;
    const saveData = (navigator as Navigator & { connection?: { saveData?: boolean } }).connection?.saveData;
    if (game.nsfw || saveData) return;
    if (hoverTimer.current) clearTimeout(hoverTimer.current);
    hoverTimer.current = window.setTimeout(() => {
      getHoverTrailer(game.id).then(url => {
        if (hovering.current && url) setPreviewUrl(url);
      });
    }, HOVER_PREVIEW_DELAY_MS);
  };
  const stopPreview = () => {
    hovering.current = false;
    if (hoverTimer.current) {
      clearTimeout(hoverTimer.current);
      hoverTimer.current = null;
    }
    setPreviewUrl(null);
    setPreviewPlaying(false);
  };
  useEffect(() => () => { if (hoverTimer.current) clearTimeout(hoverTimer.current); }, []);

  const shownGenres = (game.genres ?? []).filter(g => LATIN_GENRE.test(g)).slice(0, 3);

  // ── Library: Play up front, everything else behind "⋯" ──
  // Six identical grey icons, Uninstall among them, meant guessing which was
  // which and one slip away from removing a game.
  const [menuOpen, setMenuOpen] = useState(false);
  const [menuPos, setMenuPos] = useState<{ right: number; top?: number; bottom?: number } | null>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const toggleMenu = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (menuOpen) {
      setMenuOpen(false);
      return;
    }
    const rect = menuButtonRef.current?.getBoundingClientRect();
    if (!rect) return;
    // Opens upward unless the card sits too close to the top of the window.
    setMenuPos(rect.top > 300
      ? { right: window.innerWidth - rect.right, bottom: window.innerHeight - rect.top + 8 }
      : { right: window.innerWidth - rect.right, top: rect.bottom + 8 });
    setMenuOpen(true);
  };
  useEffect(() => {
    if (!menuOpen) return;
    const onPointer = (e: MouseEvent) => {
      const target = e.target as Node;
      if (menuRef.current?.contains(target) || menuButtonRef.current?.contains(target)) return;
      setMenuOpen(false);
    };
    const close = () => setMenuOpen(false);
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setMenuOpen(false); };
    document.addEventListener('mousedown', onPointer);
    window.addEventListener('scroll', close, true);
    window.addEventListener('resize', close);
    window.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onPointer);
      window.removeEventListener('scroll', close, true);
      window.removeEventListener('resize', close);
      window.removeEventListener('keydown', onKey);
    };
  }, [menuOpen]);

  const launchGame = async (e?: React.MouseEvent) => {
    e?.stopPropagation();
    try {
      await invoke('launch_game', { appId: game.id, steamPath: steamPath ?? '', appName: game.name });
      notify(`${game.name}: ${ti('abriendo...', 'launching...')}`, 'success');
    } catch (err) {
      notify(`${ti('No se pudo abrir el juego', 'Could not launch the game')}: ${err}`, 'error');
    }
  };
  const toggleVersionLock = async () => {
    const next = !isPinned;
    try {
      await invoke('set_manifest_pinned', { appId: game.id, pinned: next });
      onTogglePin?.(game.id, next);
    } catch (err) {
      console.error('Pin error:', err);
    }
  };
  type MenuItem = { key: string; icon: React.ReactNode; label: string; onClick: () => void; danger?: boolean } | 'sep';
  const menuItems: MenuItem[] = [
    { key: 'saves', icon: <Cloud size={14} />, label: ti('Partidas', 'Saves'), onClick: () => setShowSavesModal(true) },
    { key: 'dlc', icon: <Package size={14} />, label: ti('DLCs / Desbloquear', 'DLCs / Unlock'), onClick: () => setShowUnlockModal(true) },
    { key: 'mods', icon: <Puzzle size={14} />, label: ti('Mods de Workshop', 'Workshop mods'), onClick: () => setShowWorkshopModal(true) },
    {
      key: 'pin',
      icon: isPinned ? <PinOff size={14} /> : <Pin size={14} />,
      label: isPinned ? ti('Permitir actualizaciones', 'Allow updates') : ti('Fijar versión', 'Lock version'),
      onClick: () => { void toggleVersionLock(); },
    },
    { key: 'repair', icon: <RefreshCw size={14} />, label: ti('Reparar / reinstalar', 'Repair / reinstall'), onClick: () => onInstall?.(game.id) },
    // Desinstalar removes the real appmanifest_<id>.acf Steam itself created —
    // correct for a game Ragnarok installed, but it would corrupt Steam's own
    // tracking of a game the user owns and installed outside Ragnarok.
    ...(!game.foreign
      ? ['sep' as const, { key: 'uninstall', icon: <Trash2 size={14} />, label: ti('Desinstalar', 'Uninstall'), onClick: () => setShowUninstallModal(true), danger: true }]
      : []),
  ];

  const playedAgo = libraryStats && libraryStats.last_played > 0
    ? publishedAgo(new Date(libraryStats.last_played * 1000).toISOString(), lang === 'es')
    : null;
  const playedLine = playedAgo
    ? `${ti(`Jugado ${playedAgo}`, `Played ${playedAgo}`)}${libraryStats && libraryStats.playtime_minutes > 0 ? ` · ${formatPlaytime(libraryStats.playtime_minutes)}` : ''}`
    : ti('Sin jugar todavía', 'Not played yet');

  const statusBadge: { text: string; tone: string; title: string; repair?: boolean } | null = !isLibrary || !libraryStats
    ? null
    : libraryStats.downloading
      ? {
          text: ti(`Descargando ${Math.round(libraryStats.progress * 100)}%`, `Downloading ${Math.round(libraryStats.progress * 100)}%`),
          tone: 'bg-accent/25 border-accent/60 text-white',
          title: ti('Steam lo está descargando.', 'Steam is downloading it.'),
        }
      : libraryStats.manifests_missing > 0
        ? {
            text: ti('Falta manifiesto · Reparar', 'Missing manifest · Repair'),
            tone: 'bg-red-500/25 border-red-400/60 text-red-100 hover:bg-red-500/40',
            title: ti('Steam no podrá actualizarlo ni verificarlo. Pulsa para volver a prepararlo.', 'Steam cannot update or verify it. Click to prepare it again.'),
            repair: true,
          }
        : libraryStats.update_pending
          ? {
              text: ti('Actualización pendiente', 'Update pending'),
              tone: 'bg-amber-500/20 border-amber-400/50 text-amber-200',
              title: ti('Steam tiene una actualización en cola para este juego.', 'Steam has an update queued for this game.'),
            }
          : null;
  const updatedLabel = game.updated_at && Number.isFinite(new Date(game.updated_at).getTime())
    ? publishedAgo(game.updated_at, lang === 'es')
    : null;
  const [showUninstallModal, setShowUninstallModal] = useState(false);
  // Apagado por defecto: borrar la carpeta del juego es irreversible y puede
  // ser de decenas de GB, asi que se pide explicitamente en vez de asumirlo.
  const [deleteGameFiles, setDeleteGameFiles] = useState(false);
  const [showUnlockModal, setShowUnlockModal] = useState(false);
  const [showWorkshopModal, setShowWorkshopModal] = useState(false);


  const rawName = game.name?.trim() ?? '';
  const isLoadingName = !rawName || rawName.startsWith('Game ID:');
  const displayName = isLoadingName ? `App ${game.id}` : rawName;

  const handleUninstall = async () => {
    try {
      // El backend devuelve que paso realmente con los archivos: si se
      // borraron y cuanto se libero, o donde siguen estando. Antes se decia
      // "desinstalado" a secas mientras la carpeta seguia intacta en disco.
      const msg = await invoke<string>('delete_game', {
        appId: game.id,
        steamPath: steamPath ?? DEFAULT_STEAM_PATH,
        deleteFiles: deleteGameFiles,
      });
      notify(msg, deleteGameFiles ? 'success' : 'warning');
      onUninstall?.(game.id);
      setShowUninstallModal(false);
      setDeleteGameFiles(false);
    } catch (err) {
      notify(`Error al desinstalar: ${err}`, 'error');
    }
  };

  const formatSize = (bytes?: number): string | null => {
    if (!bytes || bytes === 0) return null;
    if (bytes >= 1_073_741_824) return `${(bytes / 1_073_741_824).toFixed(1)} GB`;
    if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(0)} MB`;
    return `${(bytes / 1024).toFixed(0)} KB`;
  };

  const sizeLabel = formatSize(game.size_bytes || libraryStats?.size_on_disk);

  return (
    <>
      <motion.div
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        whileHover={{ scale: 1.02, y: -3 }}
        whileTap={{ scale: 0.96 }}
        transition={{ duration: 0.35, ease: 'easeOut', scale: { type: 'spring', stiffness: 400, damping: 20 } }}
        className="relative bg-[#0d0e12] border border-white/[0.08] rounded-2xl overflow-hidden group hover:border-accent/40 transition-colors duration-300 cursor-pointer shadow-lg"
        style={{
          // `contentVisibility: auto` deja que el navegador se saltee el
          // layout y el pintado de las tarjetas que están fuera de pantalla.
          // Importa acá porque la grilla usa scroll infinito: después de unos
          // cuantos scrolls hay 300+ tarjetas montadas, y sin esto las 300 se
          // siguen midiendo y pintando en cada cuadro aunque no se vean.
          //
          // `containIntrinsicSize` con `auto` reserva el alto mientras la
          // tarjeta está saltada y recuerda el real una vez que se pintó, así
          // la barra de scroll no pega saltos al subir y bajar.
          contentVisibility: 'auto',
          containIntrinsicSize: 'auto 236px',
        }}
        onClick={() => onSelect?.(game)}
        onMouseEnter={startPreview}
        onMouseLeave={stopPreview}
      >
        {/* Ambient hover glow */}
        <div className="pointer-events-none absolute -inset-px rounded-2xl opacity-0 group-hover:opacity-100 transition-opacity duration-500 bg-accent/[0.08] blur-xl -z-10" />
        {/* Ring flash on tap/select */}
        <motion.div
          className="pointer-events-none absolute inset-0 rounded-2xl border-2 border-accent z-40 opacity-0"
          whileTap={{ opacity: [0, 1, 0] }}
          transition={{ duration: 0.4 }}
        />

        <div className="relative h-40 w-full bg-black/40 overflow-hidden">
          <CachedImage
            appId={game.id}
            alt={displayName}
            className="absolute inset-0 w-full h-full object-cover transition-transform duration-700 ease-out group-hover:scale-110 z-10"
          />
          {previewUrl && (
            <video
              key={previewUrl}
              src={previewUrl}
              muted
              autoPlay
              loop
              playsInline
              preload="auto"
              onPlaying={() => setPreviewPlaying(true)}
              onError={() => setPreviewUrl(null)}
              className={`absolute inset-0 w-full h-full object-cover z-[15] transition-opacity duration-500 ${previewPlaying ? 'opacity-100' : 'opacity-0'}`}
            />
          )}
          <div className="absolute inset-0 bg-gradient-to-t from-black via-black/50 to-black/10 z-20" />
          {/* Library: the whole artwork is a Play button on hover. */}
          {isLibrary && (
            <button
              onClick={launchGame}
              title={ti('Jugar', 'Play')}
              className="absolute inset-0 z-[35] flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity duration-300"
            >
              <span className="w-14 h-14 rounded-full bg-accent shadow-xl shadow-accent/40 flex items-center justify-center scale-90 group-hover:scale-100 transition-transform duration-300">
                <Play size={22} fill="currentColor" className="text-white ml-0.5" />
              </span>
            </button>
          )}
          {isLibrary && libraryStats?.downloading && (
            <div className="absolute bottom-0 left-0 right-0 h-1 bg-white/10 z-[36]">
              <div className="h-full bg-accent transition-[width] duration-700" style={{ width: `${Math.round(libraryStats.progress * 100)}%` }} />
            </div>
          )}
          {/* Top-left labels in one column, so they stack instead of
              landing on each other. */}
          <div className="absolute top-3 left-3 z-[40] flex flex-col items-start gap-1.5">
            {game.is_new && (
              <span className="px-2.5 py-1 bg-accent text-white text-[9px] font-black uppercase tracking-widest rounded-full shadow-lg shadow-accent/30">
                NUEVO
              </span>
            )}
            {game.foreign && (
              <span
                title="Juego de Steam ajeno al catálogo — instalado por vos, no por Ragnarok"
                className="px-2.5 py-1 bg-white/10 backdrop-blur-sm border border-white/20 text-white text-[9px] font-black uppercase tracking-widest rounded-full"
              >
                Steam
              </span>
            )}
            {/* Which service installed it. Left off Steam-owned games: those
                were never installed from Ryuu or Hubcap, and a label claiming
                otherwise would be wrong. */}
            {(isLibrary && installSource && !game.foreign) || (isLibrary && isPinned) ? (
              <div className="flex items-center gap-1.5">
                {isLibrary && installSource && !game.foreign && (
                  <span
                    title={installSource === 'hubcap' ? 'Instalado desde Hubcap' : 'Instalado desde Ryuu'}
                    className={`px-2.5 py-1 backdrop-blur-sm border text-[9px] font-black uppercase tracking-widest rounded-full ${
                      installSource === 'hubcap'
                        ? 'bg-emerald-500/15 border-emerald-400/40 text-emerald-300'
                        : 'bg-sky-500/15 border-sky-400/40 text-sky-300'
                    }`}
                  >
                    {installSource === 'hubcap' ? 'Hubcap' : 'Ryuu'}
                  </span>
                )}
                {isLibrary && isPinned && (
                  <span
                    title={ti('Versión fijada: Steam no la actualiza.', 'Version locked: Steam will not update it.')}
                    className="flex items-center gap-1 px-2 py-1 backdrop-blur-sm border text-[9px] font-black uppercase tracking-widest rounded-full bg-amber-500/15 border-amber-400/40 text-amber-300"
                  >
                    <Pin size={9} />
                    {ti('Versión fijada', 'Locked')}
                  </span>
                )}
              </div>
            ) : null}
            {statusBadge && (
              <button
                onClick={e => {
                  e.stopPropagation();
                  if (statusBadge.repair) onInstall?.(game.id);
                }}
                title={statusBadge.title}
                className={`px-2.5 py-1 backdrop-blur-sm border text-[9px] font-black uppercase tracking-widest rounded-full transition-colors ${statusBadge.tone} ${statusBadge.repair ? 'cursor-pointer' : 'cursor-default'}`}
              >
                {statusBadge.text}
              </button>
            )}
          </div>
          {game.metacritic && (
            <div className="absolute top-3 right-3 z-30">
              <MetacriticBadge score={game.metacritic.score} compact />
            </div>
          )}
          {/* Semantic colour on purpose: this is information about the game,
              not decoration, so it keeps the amber the rest of the app uses
              for warnings instead of the neutral treatment badges get. */}
          {typeof game.drm === 'string' && game.drm.length > 0 ? (
            <div
              className={`absolute right-3 z-30 ${game.metacritic ? 'top-11' : 'top-3'}`}
              title="Este juego incluye Denuvo"
            >
              <span className="px-2.5 py-1 bg-amber-500/15 backdrop-blur-sm border border-amber-500/40 text-amber-300 text-[9px] font-black uppercase tracking-widest rounded-full">
                {game.drm}
              </span>
            </div>
          ) : null}

          {/* Title + metadata overlaid on the artwork, not in a separate panel below */}
          <div className="absolute bottom-0 left-0 right-0 z-30 p-3">
            {isLoadingName ? (
              <div className="space-y-1.5">
                <div className="h-4 w-3/4 rounded-lg bg-white/10 animate-pulse" />
                <div className="h-2.5 w-1/2 rounded-lg bg-white/[0.06] animate-pulse" />
              </div>
            ) : (
              <h4 className="font-black text-[13px] tracking-tight text-white uppercase truncate drop-shadow-[0_2px_8px_rgba(0,0,0,0.8)] group-hover:text-accent transition-colors duration-300">{displayName}</h4>
            )}
            <div className="flex items-center justify-between gap-2 mt-1">
              <p className="text-[10px] text-gray-300 font-semibold uppercase tracking-wider truncate drop-shadow-[0_1px_4px_rgba(0,0,0,0.8)]">
                {isLibrary ? playedLine : (game.developers?.join(', ') || `ID: ${game.id}`)}
              </p>
              {sizeLabel && (
                <div className="flex items-center gap-1 shrink-0">
                  <HardDrive size={10} className="text-gray-400" />
                  <span className="text-[10px] text-gray-300 font-semibold">{sizeLabel}</span>
                </div>
              )}
            </div>
            {/* Slides up on hover: what used to need the detail page. */}
            {/* Catalog facts; the Library card already says what matters there. */}
            {!isLibrary && (rank || shownGenres.length > 0 || updatedLabel) && (
              <div className="grid grid-rows-[0fr] group-hover:grid-rows-[1fr] transition-[grid-template-rows] duration-300 ease-out">
                <div className="overflow-hidden">
                  <div className="flex flex-wrap items-center gap-1 pt-2">
                    {rank && (
                      <span className="px-2 py-0.5 rounded-full bg-accent text-white text-[9px] font-black uppercase tracking-widest shadow-md shadow-accent/30">
                        #{rank.position}{' '}
                        {rank.kind === 'top_sellers'
                          ? ti('más vendido', 'top seller', { pt: 'mais vendido', fr: 'meilleure vente', ru: 'по продажам' })
                          : ti('más jugado', 'most played', { pt: 'mais jogado', fr: 'plus joué', ru: 'по игрокам' })}
                      </span>
                    )}
                    {shownGenres.map(genre => (
                      <span
                        key={genre}
                        className="px-2 py-0.5 rounded-full bg-white/10 border border-white/15 backdrop-blur-sm text-[9px] font-bold uppercase tracking-widest text-gray-200"
                      >
                        {genre}
                      </span>
                    ))}
                  </div>
                  {updatedLabel && (
                    <p className="text-[9px] font-semibold text-gray-400 mt-1 drop-shadow-[0_1px_4px_rgba(0,0,0,0.8)]">
                      {ti(`Ticket actualizado ${updatedLabel}`, `Ticket updated ${updatedLabel}`)}
                    </p>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>
        <div className={isLibrary ? "p-3" : "p-0"}>
          {isLibrary && (
            <div className="flex gap-2">
              {/* Requested by a user: after downloading, Ragnarok is where
                  they already are, and opening Steam just to press Play is
                  the only reason left to leave it. Steam still has to be
                  running — it is what actually starts the game. */}
              <button
                onClick={launchGame}
                className="flex-1 flex items-center justify-center gap-2 py-2.5 rounded-xl bg-accent hover:brightness-110 text-white text-[11px] font-black uppercase tracking-widest shadow-md shadow-accent/25 transition-all"
              >
                <Play size={13} fill="currentColor" />
                {ti('Jugar', 'Play')}
              </button>
              <button
                ref={menuButtonRef}
                onClick={toggleMenu}
                title={ti('Más opciones', 'More options')}
                className={`w-11 shrink-0 flex items-center justify-center rounded-xl border transition-colors ${
                  menuOpen
                    ? 'bg-white/[0.12] border-white/20 text-white'
                    : 'bg-white/[0.06] border-white/10 text-gray-400 hover:text-white hover:bg-white/[0.1]'
                }`}
              >
                <MoreHorizontal size={16} />
              </button>
            </div>
          )}
        </div>
      </motion.div>

      {/* The "⋯" menu, portalled so the card's overflow-hidden cannot clip it. */}
      {menuOpen && menuPos && createPortal(
        <motion.div
          ref={menuRef}
          initial={{ opacity: 0, y: menuPos.bottom !== undefined ? 6 : -6, scale: 0.97 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          transition={{ duration: 0.15, ease: 'easeOut' }}
          style={{ position: 'fixed', right: menuPos.right, top: menuPos.top, bottom: menuPos.bottom, zIndex: 80 }}
          className="w-56 rounded-xl bg-[#0d0e12] border border-white/[0.08] shadow-2xl shadow-black/70 p-1.5"
          onClick={e => e.stopPropagation()}
        >
          {menuItems.map((item, i) => item === 'sep' ? (
            <div key={`sep-${i}`} className="my-1 h-px bg-white/[0.06]" />
          ) : (
            <button
              key={item.key}
              onClick={() => { setMenuOpen(false); item.onClick(); }}
              className={`w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-[12px] font-semibold text-left transition-colors ${
                item.danger ? 'text-red-400 hover:bg-red-500/10' : 'text-gray-300 hover:text-white hover:bg-white/[0.06]'
              }`}
            >
              {item.icon}
              {item.label}
            </button>
          ))}
        </motion.div>,
        document.body
      )}

      <AnimatePresence>
        {showSavesModal && (
          <CloudSavesModal
            game={game}
            steamPath={steamPath ?? 'C:\\Program Files (x86)\\Steam'}
            onClose={() => setShowSavesModal(false)}
          />
        )}
        {showUnlockModal && (
          <UnlockModal
            game={game}
            steamPath={steamPath ?? 'C:\\Program Files (x86)\\Steam'}
            onClose={() => setShowUnlockModal(false)}
          />
        )}
        {showWorkshopModal && (
          <WorkshopModal
            game={game}
            steamPath={steamPath ?? 'C:\\Program Files (x86)\\Steam'}
            onClose={() => setShowWorkshopModal(false)}
          />
        )}
        {showUninstallModal && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-[60] flex items-center justify-center p-4"
            style={{ background: 'rgba(0,0,0,0.75)', backdropFilter: 'blur(8px)' }}
            onClick={(e) => { e.stopPropagation(); setShowUninstallModal(false); }}
          >
            <motion.div
              initial={{ scale: 0.95, opacity: 0, y: 8 }}
              animate={{ scale: 1, opacity: 1, y: 0 }}
              exit={{ scale: 0.95, opacity: 0, y: 8 }}
              transition={{ duration: 0.25, ease: 'easeOut' }}
              className="relative w-full max-w-sm bg-[#0a0a0c] border border-white/[0.08] rounded-2xl shadow-2xl overflow-hidden p-6 text-center backdrop-blur-xl"
              onClick={e => e.stopPropagation()}
            >
              <div className="w-16 h-16 mx-auto bg-accent/10 rounded-2xl flex items-center justify-center mb-4 border border-accent/20">
                <Trash2 size={28} className="text-accent" />
              </div>
              <h3 className="text-base font-black text-white/90 uppercase tracking-wider mb-2">¿Desinstalar Juego?</h3>
              <p className="text-[13px] text-gray-400 font-medium mb-4 leading-relaxed">
                <strong className="text-white/80">{displayName}</strong> se va a quitar de Steam. Esta acción no se puede deshacer.
              </p>

              {/* Quitar el juego de Steam y borrarlo del disco son dos cosas
                  distintas, y hasta ahora sólo pasaba la primera: Steam lo
                  daba por desinstalado y la carpeta seguía ocupando el mismo
                  espacio, sin que nada lo dijera. */}
              <label className="flex items-start gap-3 text-left mb-6 p-3 rounded-xl bg-white/[0.03] border border-white/[0.06] cursor-pointer hover:bg-white/[0.05] transition-colors">
                <input
                  type="checkbox"
                  checked={deleteGameFiles}
                  onChange={e => setDeleteGameFiles(e.target.checked)}
                  className="mt-0.5 w-4 h-4 shrink-0 accent-accent cursor-pointer"
                />
                <span className="min-w-0">
                  <span className="block text-[12px] font-black text-white/85">Borrar también los archivos del disco</span>
                  <span className="block text-[11px] text-gray-500 font-medium leading-relaxed mt-0.5">
                    Sin esto, el juego se quita de Steam pero la carpeta sigue ocupando espacio.
                  </span>
                </span>
              </label>
              <div className="flex gap-3">
                <button
                  onClick={() => setShowUninstallModal(false)}
                  className="flex-1 py-3 bg-white/[0.04] border border-white/[0.08] hover:bg-white/[0.07] text-white/90 rounded-xl font-bold transition-all text-[11px] uppercase tracking-wider"
                >
                  Cancelar
                </button>
                <button
                  onClick={handleUninstall}
                  className="flex-1 py-3 bg-accent hover:brightness-110 text-white rounded-xl font-black transition-all duration-200 hover:-translate-y-0.5 text-[11px] uppercase tracking-wider"
                >
                  {deleteGameFiles ? 'Desinstalar y borrar' : 'Desinstalar'}
                </button>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  );
});

// --- Search header ---

// Normalizes text for fuzzy search: removes accents, lowercases, trims extra spaces
const normalizeSearch = (s: string): string =>
  s
    .toLowerCase()
    .normalize('NFD')
    .replace(/[\u0300-\u036f]/g, '') // strip accent marks
    .replace(/[^a-z0-9\s]/g, '')     // REMOVE special chars (don't replace with space)
    .replace(/\s+/g, ' ')            // collapse multiple spaces
    .trim();

// Returns true if ALL words in the query appear somewhere in the target string
const matchesSearch = (target: string, query: string): boolean => {
  const t = normalizeSearch(target);
  const words = normalizeSearch(query).split(' ').filter(Boolean);
  return words.length > 0 && words.every(w => t.includes(w));
};

const SearchBar = ({
  placeholder, onSearch,
  // Category props (optional — only used in CatalogView)
  categories, selectedCategory, onCategoryChange, dropdownOpen, onDropdownToggle, dropdownRef,
}: {
  placeholder?: string;
  onSearch?: (v: string) => void;
  categories?: string[];
  selectedCategory?: string | null;
  onCategoryChange?: (cat: string | null | ((prev: string | null) => string | null)) => void;
  dropdownOpen?: boolean;
  onDropdownToggle?: () => void;
  dropdownRef?: React.RefObject<HTMLDivElement>;
}) => {
  const ti = useTranslateInline();
  const [value, setValue] = useState('');
  // Filters the category list itself, not the games. With 40+ categories the
  // two-column scroller was the slowest way to reach one you already knew
  // the name of.
  const [catQuery, setCatQuery] = useState('');
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const clear = () => { setValue(''); onSearch?.(''); };
  const change = (v: string) => {
    setValue(v);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => onSearch?.(v), 200);
  };

  // Genre emoji/icon map
  const GENRE_ICONS: Record<string, string> = {
    'Action': '⚔️', 'Adventure': '🗺️', 'RPG': '🧙', 'Strategy': '♟️',
    'Simulation': '🎮', 'Sports': '⚽', 'Racing': '🏎️', 'Casual': '🎲',
    'Indie': '🎨', 'Free to Play': '🆓', 'Early Access': '🔓',
    'Massively Multiplayer': '🌐', 'Animation & Modeling': '🖥️',
    'Video Production': '🎬', 'Photo Editing': '📷', 'Utilities': '🔧',
    'Game Development': '👾', 'Education': '📚', 'Software Training': '📖',
    'Audio Production': '🎵', 'Web Publishing': '🌍',
  };

  const hasCategories = categories && categories.length > 0;

  const visibleCategories = useMemo(() => {
    const q = catQuery.trim().toLowerCase();
    if (!q) return categories ?? [];
    return (categories ?? []).filter(c => c.toLowerCase().includes(q));
  }, [categories, catQuery]);

  return (
    <div className="mb-6 relative" ref={dropdownRef}>
      {/* Search row */}
      <div className="relative">
        <div className="absolute inset-y-0 left-4 flex items-center pointer-events-none text-gray-500">
          <Search size={16} />
        </div>
        <input
          type="text" value={value}
          placeholder={placeholder ?? 'Search...'}
          className={`w-full bg-black/40 border border-white/10 rounded-xl py-3 pl-12 text-sm text-white/90 placeholder:text-gray-500 focus:outline-none focus:border-accent/50 focus:ring-1 focus:ring-accent/25 transition-all duration-200 ${
            hasCategories ? 'pr-32' : 'pr-12'
          }`}
          onChange={e => change(e.target.value)}
          spellCheck={false}
          autoComplete="off"
        />

        {/* Right side: clear button + category filter button */}
        <div className="absolute inset-y-0 right-2 flex items-center gap-1">
          {value && (
            <button onClick={clear} className="p-1.5 text-gray-500 hover:text-white transition-colors">
              <X size={15} />
            </button>
          )}

          {/* Category filter button — sits INSIDE the search bar */}
          {hasCategories && (
            <button
              onClick={onDropdownToggle}
              className={`flex items-center gap-1.5 px-3 py-1.5 rounded-full text-[10px] font-black uppercase tracking-wider transition-all duration-200 border ${
                selectedCategory
                  ? 'bg-accent border-accent/40 text-white shadow-md shadow-accent/20'
                  : 'bg-white/[0.05] border-white/10 text-gray-400 hover:text-white hover:bg-white/[0.09]'
              }`}
            >
              <span className="text-[13px] leading-none">
                {selectedCategory ? (GENRE_ICONS[selectedCategory] ?? '🎯') : '🎯'}
              </span>
              <span className="max-w-[72px] truncate">
                {selectedCategory ?? ti('Filtro', 'Filter')}
              </span>
              <svg
                width="10" height="10" viewBox="0 0 10 10" fill="none"
                stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"
                style={{ transform: dropdownOpen ? 'rotate(180deg)' : 'rotate(0deg)', transition: 'transform 0.2s ease', flexShrink: 0 }}
              >
                <path d="M2 3.5l3 3 3-3" />
              </svg>
            </button>
          )}
        </div>
      </div>

      {/* ── Dropdown panel ── */}
      {hasCategories && dropdownOpen && (
        <motion.div
          initial={{ opacity: 0, y: -6 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -6 }}
          transition={{ duration: 0.2, ease: 'easeOut' }}
          className="absolute left-0 right-0 top-[calc(100%+6px)] z-50 rounded-2xl overflow-hidden bg-[#0d0e12] border border-white/[0.08] shadow-2xl shadow-black/70"
        >
          {/* Header — one place that says what is selected, instead of the
              old banner row and footer both repeating it. */}
          <div className="px-5 pt-4 pb-3 flex items-center gap-3 border-b border-white/[0.06]">
            <span className="w-1.5 h-1.5 rounded-full shrink-0 bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />
            <span className="text-[11px] font-black uppercase tracking-[0.2em] text-white/80">
              {ti('Categorías', 'Categories')}
            </span>
            {selectedCategory && (
              <span className="min-w-0 flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-accent/10 border border-accent/25">
                <span className="text-[9px] font-black uppercase tracking-widest text-accent truncate">
                  {selectedCategory}
                </span>
              </span>
            )}
            {selectedCategory && (
              <button
                onClick={() => onCategoryChange?.(null)}
                className="ml-auto shrink-0 text-[10px] font-black uppercase tracking-wider text-white/35 hover:text-accent transition-colors"
              >
                {ti('Quitar', 'Clear')}
              </button>
            )}
          </div>

          {/* Filter the categories themselves */}
          <div className="px-3 pt-3">
            <div className="relative">
              <div className="absolute inset-y-0 left-3 flex items-center pointer-events-none text-gray-600">
                <Search size={13} />
              </div>
              <input
                type="text"
                value={catQuery}
                onChange={e => setCatQuery(e.target.value)}
                placeholder={ti('Buscar categoría...', 'Search category...')}
                spellCheck={false}
                autoComplete="off"
                className="w-full bg-black/40 border border-white/10 rounded-xl py-2 pl-9 pr-8 text-[12px] text-white/90 placeholder:text-gray-600 focus:outline-none focus:border-accent/50 focus:ring-1 focus:ring-accent/25 transition-all duration-200"
              />
              {catQuery && (
                <button
                  onClick={() => setCatQuery('')}
                  className="absolute inset-y-0 right-2 flex items-center text-gray-600 hover:text-white transition-colors"
                >
                  <X size={13} />
                </button>
              )}
            </div>
          </div>

          {/* Categories 2-column grid */}
          <motion.div
            initial="hidden"
            animate="show"
            variants={{ show: { transition: { staggerChildren: 0.015 } } }}
            className="p-3 grid grid-cols-2 gap-1.5 max-h-72 overflow-y-auto"
          >
            {/* All button — hidden while filtering, since it is not a match */}
            {!catQuery.trim() && (
              <button
                onClick={() => { onCategoryChange?.(null); }}
                className={`col-span-2 flex items-center gap-3 px-4 py-3 rounded-xl text-[11px] font-black uppercase tracking-wider transition-all duration-150 border ${
                  !selectedCategory
                    ? 'bg-accent/10 border-accent/30 text-accent'
                    : 'bg-white/[0.03] border-white/[0.07] text-white/50 hover:bg-white/[0.06] hover:-translate-y-px'
                }`}
              >
                <span className="text-base">🌐</span>
                <span>{ti('Todos los juegos', 'All games')}</span>
                {!selectedCategory && <span className="ml-auto w-1.5 h-1.5 rounded-full shrink-0 bg-accent" />}
              </button>
            )}

            {visibleCategories.map(cat => (
              <motion.button
                key={cat}
                variants={{ hidden: { opacity: 0, y: 6 }, show: { opacity: 1, y: 0 } }}
                transition={{ duration: 0.2, ease: 'easeOut' }}
                onClick={() => { onCategoryChange?.(prev => prev === cat ? null : cat); }}
                title={cat}
                className={`flex items-center gap-2.5 px-3.5 py-3 rounded-xl text-[11px] font-black uppercase tracking-wider transition-colors duration-150 border ${
                  selectedCategory === cat
                    ? 'bg-accent/10 border-accent/30 text-accent'
                    : 'bg-white/[0.03] border-white/[0.07] text-white/55 hover:bg-white/[0.06] hover:text-white/80'
                }`}
              >
                <span className="text-[15px] leading-none shrink-0">{GENRE_ICONS[cat] ?? '🎮'}</span>
                <span className="truncate">{cat}</span>
                {selectedCategory === cat && (
                  <span className="ml-auto w-1.5 h-1.5 rounded-full shrink-0 bg-accent" />
                )}
              </motion.button>
            ))}

            {visibleCategories.length === 0 && (
              <div className="col-span-2 px-4 py-8 text-center">
                <p className="text-[11px] font-black uppercase tracking-widest text-gray-600">
                  {ti('Sin coincidencias', 'No matches')}
                </p>
                <p className="text-[11px] text-gray-600 font-medium mt-1.5">
                  {ti(`Ninguna categoría contiene "${catQuery}".`, `No category contains "${catQuery}".`)}
                </p>
              </div>
            )}
          </motion.div>

          {/* Footer */}
          <div className="px-5 py-3 border-t border-white/[0.06] bg-black/20">
            <span className="text-[10px] font-bold text-gray-600 tabular-nums">
              {catQuery.trim()
                ? ti(
                    `${visibleCategories.length} de ${categories!.length} categorías`,
                    `${visibleCategories.length} of ${categories!.length} categories`
                  )
                : ti(
                    `${categories!.length} categorías disponibles`,
                    `${categories!.length} categories available`
                  )}
            </span>
          </div>
        </motion.div>
      )}

    </div>
  );
};

// --- Catalog ---

/// Las categorías que Steam usa para software y medios, no para juegos.
/// Aparecen en el catálogo porque comparten el mismo campo de etiquetas.
const SOFTWARE_CATEGORIES = new Set([
  'utilities', 'web publishing', 'video production', 'photo editing',
  'audio production', 'animation & modeling', 'design & illustration',
  'education', 'software training', 'accounting', 'game development',
  'movie', 'documentary', 'short', 'tutorial', 'episodic',
]);

/// Versión del formato de la caché de Denuvo. Subirla es la forma de tirar
/// lo guardado cuando cambia qué significa el valor.
const DRM_CACHE_KEY = 'rl_drm_v2';

const PAGE_SIZE = 30;

/// Steam's own charts, as AppIDs in rank order (see `get_steam_rankings`).
type SteamRankings = { top_sellers: string[]; most_played: string[]; updated_at: number };
type RankingView = 'top_sellers' | 'most_played' | 'all';
const RANKING_VIEW_KEY = 'rl_catalog_ranking';

const CatalogView = memo(({ games, onSelect, interstitialAd, interstitialAdEvery = 6, onNeedDrm, rankings }: {
  games: Game[], onSelect: (game: Game) => void,
  // Steam's best sellers and most played. Only Games passes them: both
  // catalogs list every title they can get a ticket for, which by count is
  // mostly games nobody looks for, and these put the ones people actually
  // buy and play first.
  rankings?: SteamRankings,
  // Llamado con los AppIDs visibles cuyo estado de DRM todavía no se conoce.
  // Se resuelve arriba, donde vive `games`, para que el resultado quede en un
  // solo lugar y lo aprovechen también la Librería y el panel de detalle.
  onNeedDrm?: (ids: string[]) => void,
  // Optional ad node repeated every N cards in the grid — only Zona +18
  // passes this (higher CPM there), Games/RAGNAROK LEGENDS leave it unset
  // so this component's normal behavior is completely unchanged for them.
  interstitialAd?: React.ReactNode, interstitialAdEvery?: number,
}) => {
  const { t, lang } = useLanguage();
  const es = lang === 'es';
  const [search, setSearch] = useState('');
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null);
  const [page, setPage] = useState(1);
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const loaderRef = useRef<HTMLDivElement>(null);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const q = search.trim();

  // Close dropdown when clicking outside
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

  // Derive unique categories from all games that have resolved names
  const categories = useMemo(() => {
    // Las etiquetas llegan crudas del catálogo, y ahí vienen tres clases de
    // basura que hacían que la lista tuviera 44 entradas:
    //
    //   1. El mismo género repetido en otros idiomas — 动作 es "Action",
    //      策略 es "Strategy", 模拟 es "Simulation", "Strateji" es "Strategy"
    //      en turco. Elegir uno filtraba un puñado de juegos en vez del
    //      género entero.
    //   2. Las categorías de software de Steam (Utilities, Web Publishing,
    //      Video Production...), que no son géneros de juegos.
    //   3. Etiquetas de cola larga que aparecen en un par de títulos.
    //
    // Las tres se van con reglas distintas, y las tres hacen falta: la de
    // idioma no atrapa "Strateji", y la de conteo no atrapa "Utilities".
    const counts = new Map<string, number>();
    games.forEach(g => {
      g.genres?.forEach(genre => {
        counts.set(genre, (counts.get(genre) ?? 0) + 1);
      });
    });

    return Array.from(counts.entries())
      .filter(([name, count]) => {
        // 1. Sólo alfabeto latino. Un género que ya existe en inglés no
        //    necesita su duplicado en chino, ruso o japonés.
        if (!/^[ -~À-ɏ]+$/.test(name)) return false;
        // 2. Categorías de software, no de juegos.
        if (SOFTWARE_CATEGORIES.has(name.toLowerCase())) return false;
        // 3. Cola larga. Con ~70.000 juegos en el catálogo, algo que aparece
        //    en menos de 25 es ruido — incluidas las traducciones en alfabeto
        //    latino que la regla 1 deja pasar.
        return count >= 25;
      })
      .map(([name]) => name)
      .sort();
  }, [games]);

  // How many games still don't have a real name yet (still showing "Game
  // ID: X" while names resolve in the background, a few at a time so as not
  // to hit Steam's rate limits) — search-by-name silently skips these, which
  // otherwise looks like the search is broken/missing games that do exist.
  const unresolvedCount = useMemo(
    () => games.filter(g => { const n = g.name?.trim() ?? ''; return !n || n.startsWith('Game ID:'); }).length,
    [games]
  );

  const [rankingView, setRankingView] = useState<RankingView>(() => {
    try {
      const saved = localStorage.getItem(RANKING_VIEW_KEY);
      return saved === 'most_played' || saved === 'all' ? saved : 'top_sellers';
    } catch {
      return 'top_sellers';
    }
  });
  const chooseRankingView = (view: RankingView) => {
    setRankingView(view);
    try { localStorage.setItem(RANKING_VIEW_KEY, view); } catch { }
  };

  // The chosen chart, limited to games this catalog actually has, in Steam's
  // order. Null means show the whole catalog.
  const rankedList = useMemo(() => {
    if (!rankings || rankingView === 'all') return null;
    const order = rankings[rankingView];
    if (!order?.length) return null;
    const byId = new Map(games.map(g => [g.id, g]));
    return order.map(id => byId.get(id)).filter((g): g is Game => !!g);
  }, [games, rankings, rankingView]);

  const rankedCounts = useMemo(() => {
    if (!rankings) return null;
    const ids = new Set(games.map(g => g.id));
    return {
      top_sellers: rankings.top_sellers.filter(id => ids.has(id)).length,
      most_played: rankings.most_played.filter(id => ids.has(id)).length,
    };
  }, [games, rankings]);

  // Each card's place in Steam's charts, for its hover panel: the chart being
  // viewed first, the other one for games it does not list.
  const rankOf = useMemo(() => {
    const out = new Map<string, CatalogRank>();
    if (!rankings) return out;
    const preferred: CatalogRank['kind'] = rankingView === 'most_played' ? 'most_played' : 'top_sellers';
    const other: CatalogRank['kind'] = preferred === 'top_sellers' ? 'most_played' : 'top_sellers';
    // The preferred chart goes last so it overwrites.
    for (const kind of [other, preferred]) {
      rankings[kind].forEach((id, i) => out.set(id, { kind, position: i + 1 }));
    }
    return out;
  }, [rankings, rankingView]);

  // Reset pagination when filters change (not when game metadata/DRM updates in background)
  useEffect(() => { setPage(1); }, [q, selectedCategory, rankingView]);

  const filtered = useMemo(() => {
    // A search always covers the whole catalog: a chart is a starting point,
    // and a game someone types the name of must never be hidden by it.
    let list = rankedList && rankedList.length > 0 && !q ? rankedList : games;
    // Filter by category
    if (selectedCategory) {
      list = list.filter(g => g.genres?.includes(selectedCategory));
    }
    // Then filter by search query
    if (!q) return list;
    return list.filter(g => {
      const rawName = g.name?.trim() ?? '';
      const isUnresolved = !rawName || rawName.startsWith('Game ID:');
      if (g.id.startsWith(q.trim())) return true;
      if (isUnresolved) return false;
      return matchesSearch(rawName, q);
    });
  }, [games, q, selectedCategory, rankedList]);

  // Only render up to page * PAGE_SIZE cards
  const visible = useMemo(() => filtered.slice(0, page * PAGE_SIZE), [filtered, page]);
  const hasMore = visible.length < filtered.length;

  // El aviso de DRM se resuelve por página, no para las ~70.000 filas del
  // catálogo: sólo se preguntan los AppIDs que están efectivamente en
  // pantalla y de los que todavía no se sabe nada. El resultado se cachea,
  // así que cada juego se consulta una vez y nunca más.
  useEffect(() => {
    if (!onNeedDrm) return;
    // Se manda todo lo visible, no sólo lo que no tiene etiqueta: el
    // catálogo sólo sabe decir "DRM" genérico, y filtrar acá por
    // `drm === undefined` hacía que esa respuesta pobre ganara para siempre
    // sobre la de Steam, que sabe si es Denuvo, GuardIT o Ubisoft. Quién
    // pregunta y quién no lo decide el resolutor, que es el que ve la caché.
    const ids = visible.map(g => g.id);
    if (ids.length === 0) return;
    const timer = setTimeout(() => onNeedDrm(ids), 400);
    return () => clearTimeout(timer);
  }, [visible, onNeedDrm]);

  // IntersectionObserver: load more when the sentinel enters the viewport
  useEffect(() => {
    if (!loaderRef.current || !hasMore) return;
    const observer = new IntersectionObserver(
      (entries) => { if (entries[0].isIntersecting) setPage(p => p + 1); },
      { rootMargin: '400px' }
    );
    observer.observe(loaderRef.current);
    return () => observer.disconnect();
  }, [hasMore, visible.length]);

  return (
    <div>
      {/* Unified search bar with integrated category dropdown */}
      <SearchBar
        placeholder={t.catalog.search}
        onSearch={setSearch}
        categories={categories}
        selectedCategory={selectedCategory}
        onCategoryChange={(cat) => {
          if (typeof cat === 'function') {
            setSelectedCategory(cat);
          } else {
            setSelectedCategory(cat);
          }
          setDropdownOpen(false);
        }}
        dropdownOpen={dropdownOpen}
        onDropdownToggle={() => setDropdownOpen(o => !o)}
        dropdownRef={dropdownRef}
      />

      {rankings && rankedCounts && (
        <div className="flex items-center gap-2 mb-5 flex-wrap">
          {([
            { id: 'top_sellers', label: es ? 'Más vendidos' : 'Top sellers', count: rankedCounts.top_sellers },
            { id: 'most_played', label: es ? 'Más jugados' : 'Most played', count: rankedCounts.most_played },
            { id: 'all', label: es ? 'Todos' : 'All', count: games.length },
          ] as { id: RankingView; label: string; count: number }[]).map(option => {
            const active = rankingView === option.id;
            return (
              <button
                key={option.id}
                onClick={() => chooseRankingView(option.id)}
                className={`relative flex items-center gap-1.5 px-4 py-2 rounded-full border text-[10px] font-black uppercase tracking-widest transition-colors duration-200 ${
                  active
                    ? 'border-accent/40 text-white'
                    : 'bg-white/[0.05] border-white/10 text-gray-400 hover:text-white hover:bg-white/[0.09]'
                }`}
              >
                {active && (
                  <motion.span
                    layoutId="catalog-ranking-pill"
                    className="absolute inset-0 rounded-full bg-accent shadow-md shadow-accent/20"
                    transition={{ duration: 0.25, ease: 'easeOut' }}
                  />
                )}
                <span className="relative">{option.label}</span>
                <span className={`relative tabular-nums ${active ? 'text-white/70' : 'text-gray-600'}`}>
                  {option.count.toLocaleString(es ? 'es' : 'en')}
                </span>
              </button>
            );
          })}
          {q && rankingView !== 'all' && (
            <span className="text-[10px] font-semibold text-gray-500">
              {es ? 'La búsqueda incluye todo el catálogo.' : 'Search covers the whole catalog.'}
            </span>
          )}
        </div>
      )}

      {q && <p className="text-xs text-gray-500 font-bold uppercase tracking-wider mb-2">{t.catalog.results(filtered.length, search.trim())}</p>}
      {q && unresolvedCount > 0 && (
        <div className="flex items-center gap-2.5 mb-4 px-3.5 py-2.5 rounded-xl bg-accent/[0.06] border border-accent/[0.15] text-accent">
          <RefreshCw size={12} className="animate-spin shrink-0" />
          <span className="text-[11px] font-semibold leading-snug">
            {es
              ? `Todavía se están resolviendo los nombres de ${unresolvedCount.toLocaleString('es')} juegos — la búsqueda por nombre puede no encontrarlos hasta que terminen.`
              : `Still resolving names for ${unresolvedCount.toLocaleString('en')} games — searching by name may not find them until that finishes.`}
          </span>
        </div>
      )}
      {/* Was capped at 3 columns, so on a wide window each card stretched to
          ~590px — hence "las cards son muy grandes". More columns as the
          window grows keeps each card a sane size instead. */}
      <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4">
        {visible.map((game, i) => (
          <React.Fragment key={game.id}>
            <GameCard game={game} onSelect={onSelect} rank={rankOf.get(game.id)} />
            {interstitialAd && (i + 1) % interstitialAdEvery === 0 && (
              <div className="col-span-2 md:col-span-3 xl:col-span-4 2xl:col-span-5 flex justify-center py-2">
                {interstitialAd}
              </div>
            )}
          </React.Fragment>
        ))}
      </div>
      {/* Sentinel for infinite scroll */}
      {hasMore && (
        <div ref={loaderRef} className="flex justify-center py-8">
          <div className="flex items-center gap-3 text-gray-500 text-xs font-bold uppercase tracking-widest">
            <div className="w-4 h-4 border-2 border-gray-600 border-t-white rounded-full animate-spin" />
            Cargando más juegos...
          </div>
        </div>
      )}
    </div>
  );
});

// Sidebar/section icon for the Emulators tab: an img standing in for a lucide
// icon, so it drops straight into SidebarItem's `icon` slot. The source art
// shipped with a fake transparency checkerboard baked into its pixels, so the
// asset here is the cropped icon with real alpha on its rounded corners.
export const SwitchEmulatorsIcon = ({ size, className }: { size?: number; className?: string }) => (
  <img
    src={switchEmulatorsIconImg}
    alt=""
    style={{ width: size ?? 17, height: size ?? 17 }}
    className={`object-contain shrink-0 ${className ?? ''}`}
  />
);

type LibrarySort = 'recent' | 'name' | 'size' | 'playtime';
type LibraryFilter = 'all' | 'ryuu' | 'hubcap' | 'problems';
const LIBRARY_SORT_KEY = 'rl_library_sort';

/// A small pill in the Library toolbar.
const LibraryPill = ({ active, onClick, children, layoutId }: {
  active: boolean; onClick: () => void; children: React.ReactNode; layoutId: string;
}) => (
  <button
    onClick={onClick}
    className={`relative flex items-center gap-1.5 px-3.5 py-1.5 rounded-full border text-[10px] font-black uppercase tracking-widest transition-colors duration-200 ${
      active ? 'border-accent/40 text-white' : 'bg-white/[0.05] border-white/10 text-gray-400 hover:text-white hover:bg-white/[0.09]'
    }`}
  >
    {active && (
      <motion.span
        layoutId={layoutId}
        className="absolute inset-0 rounded-full bg-accent shadow-md shadow-accent/20"
        transition={{ duration: 0.25, ease: 'easeOut' }}
      />
    )}
    <span className="relative flex items-center gap-1.5">{children}</span>
  </button>
);

const LibraryView = memo(({ games, onInstall, steamPath, onUninstall, onNeedDrm }: {
  games: Game[];
  onInstall: (id: string) => void | Promise<void>;
  steamPath: string;
  onUninstall: (id: string) => void;
  onNeedDrm?: (ids: string[]) => void;
}) => {
  const { t, lang } = useLanguage();
  const ti = useTranslateInline();
  const { notify } = useNotify();
  const [search, setSearch] = useState('');
  const [page, setPage] = useState(1);
  const loaderRef = useRef<HTMLDivElement>(null);
  const [pinnedManifests, setPinnedManifests] = useState<Set<string>>(new Set());

  useEffect(() => {
    invoke<string[]>('get_pinned_manifests')
      .then(res => setPinnedManifests(new Set(res || [])))
      .catch(() => setPinnedManifests(new Set()));
  }, []);

  // Source labels. Re-read whenever the number of installed games changes, so
  // a game installed a moment ago gets its label without leaving the tab.
  const [installSources, setInstallSources] = useState<Record<string, string>>({});
  const installed = useMemo(() => games.filter((g) => g.status === 'Installed'), [games]);
  const installedCount = installed.length;
  useEffect(() => {
    if (!steamPath) return;
    invoke<Record<string, string>>('get_install_sources', { steamPath })
      .then(res => setInstallSources(res || {}))
      .catch(() => setInstallSources({}));
  }, [steamPath, installedCount]);

  // Play history and install state for every card. Re-read every few seconds
  // while the tab is open, so a download's progress moves on its own.
  const [stats, setStats] = useState<Record<string, LibraryGameStats>>({});
  useEffect(() => {
    if (!steamPath) return;
    let alive = true;
    const load = () => {
      invoke<Record<string, LibraryGameStats>>('get_library_stats', { steamPath })
        .then(s => { if (alive) setStats(s || {}); })
        .catch(() => { });
    };
    load();
    const timer = window.setInterval(load, 5000);
    return () => { alive = false; clearInterval(timer); };
  }, [steamPath]);

  const handleTogglePin = useCallback((id: string, pinned: boolean) => {
    setPinnedManifests(prev => {
      const next = new Set(prev);
      if (pinned) next.add(id); else next.delete(id);
      return next;
    });
  }, []);

  const [sort, setSort] = useState<LibrarySort>(() => {
    try {
      const saved = localStorage.getItem(LIBRARY_SORT_KEY);
      return saved === 'name' || saved === 'size' || saved === 'playtime' ? saved : 'recent';
    } catch {
      return 'recent';
    }
  });
  const chooseSort = (next: LibrarySort) => {
    setSort(next);
    try { localStorage.setItem(LIBRARY_SORT_KEY, next); } catch { }
  };
  const [filter, setFilter] = useState<LibraryFilter>('all');

  const hasProblem = useCallback((g: Game) => {
    const s = stats[g.id];
    return !!s && (s.manifests_missing > 0 || s.update_pending);
  }, [stats]);
  const problemCount = useMemo(() => installed.filter(hasProblem).length, [installed, hasProblem]);
  const hubcapCount = useMemo(() => installed.filter(g => installSources[g.id] === 'hubcap').length, [installed, installSources]);

  // "Reparar todos": what needs no network first, for every game at once —
  // manifests back from the vault, and ACFs that keep Steam retrying an
  // update — then the games still missing a manifest, prepared again one at
  // a time.
  const [repairing, setRepairing] = useState<{ done: number; total: number } | null>(null);
  const repairAll = async () => {
    const targets = installed.filter(hasProblem);
    if (targets.length === 0 || repairing) return;
    setRepairing({ done: 0, total: targets.length });
    try {
      await invoke('check_manifest_integrity', { steamPath, repair: true }).catch(() => { });
      const fresh = await invoke<Record<string, LibraryGameStats>>('get_library_stats', { steamPath }).catch(() => stats);
      setStats(fresh);
      const still = targets.filter(g => (fresh[g.id]?.manifests_missing ?? 0) > 0);
      let done = targets.length - still.length;
      setRepairing({ done, total: targets.length });
      for (const game of still) {
        await Promise.resolve(onInstall(game.id));
        done += 1;
        setRepairing({ done, total: targets.length });
      }
      const after = await invoke<Record<string, LibraryGameStats>>('get_library_stats', { steamPath }).catch(() => fresh);
      setStats(after);
      const left = targets.filter(g => {
        const s = after[g.id];
        return !!s && (s.manifests_missing > 0 || s.update_pending);
      }).length;
      notify(
        left === 0
          ? ti(`Reparación terminada: ${targets.length} juego(s) en orden.`, `Repair finished: ${targets.length} game(s) fixed.`)
          : ti(
              `Reparación terminada. ${left} juego(s) siguen con avisos: si es una actualización pendiente, Steam la hará al abrirlo.`,
              `Repair finished. ${left} game(s) still show a notice: if it is a pending update, Steam will run it when opened.`
            ),
        left === 0 ? 'success' : 'info'
      );
    } finally {
      setRepairing(null);
    }
  };

  const q = search.trim();
  // Uses the same robust matchesSearch logic as the catalog.
  const filtered = useMemo(() => {
    let list = installed;
    if (q) {
      list = list.filter((g) => {
        const rawName = g.name?.trim() ?? '';
        const isUnresolved = !rawName || rawName.startsWith('Game ID:');
        if (g.id.startsWith(q)) return true;
        if (isUnresolved) return false;
        return matchesSearch(rawName, q);
      });
    }
    if (filter === 'ryuu') list = list.filter(g => !g.foreign && installSources[g.id] !== 'hubcap');
    else if (filter === 'hubcap') list = list.filter(g => installSources[g.id] === 'hubcap');
    else if (filter === 'problems') list = list.filter(hasProblem);

    const nameOf = (g: Game) => (g.name ?? '').toLowerCase();
    const sizeOf = (g: Game) => g.size_bytes || stats[g.id]?.size_on_disk || 0;
    const sorted = [...list];
    if (sort === 'name') sorted.sort((a, b) => nameOf(a).localeCompare(nameOf(b)));
    else if (sort === 'size') sorted.sort((a, b) => sizeOf(b) - sizeOf(a));
    else if (sort === 'playtime') sorted.sort((a, b) => (stats[b.id]?.playtime_minutes ?? 0) - (stats[a.id]?.playtime_minutes ?? 0));
    else sorted.sort((a, b) => ((stats[b.id]?.last_played ?? 0) - (stats[a.id]?.last_played ?? 0)) || nameOf(a).localeCompare(nameOf(b)));
    return sorted;
  }, [installed, q, filter, sort, stats, installSources, hasProblem]);


  // Paged the same way the catalog is. Every GameCard fires its own backend
  // calls on mount (cover art, pinned-manifest lookup), so rendering the whole
  // list at once meant a couple of hundred invokes the moment this tab opened.
  // Reset pagination when the list itself changes shape, not on background stats.
  useEffect(() => { setPage(1); }, [q, filter, sort]);
  const visible = useMemo(() => filtered.slice(0, page * PAGE_SIZE), [filtered, page]);
  const hasMore = visible.length < filtered.length;

  useEffect(() => {
    if (!loaderRef.current || !hasMore) return;
    const observer = new IntersectionObserver(
      (entries) => { if (entries[0].isIntersecting) setPage(p => p + 1); },
      { rootMargin: '400px' }
    );
    observer.observe(loaderRef.current);
    return () => observer.disconnect();
  }, [hasMore, visible.length]);

  // El aviso de DRM también hace falta acá: un juego instalado se ve en la
  // Librería y puede no haber pasado nunca por el catálogo — Crimson Desert
  // es justo ese caso. Si sólo lo pidiera CatalogView, esos no tendrían
  // etiqueta nunca.
  useEffect(() => {
    if (!onNeedDrm) return;
    const ids = visible.map(g => g.id);
    if (ids.length === 0) return;
    const timer = setTimeout(() => onNeedDrm(ids), 400);
    return () => clearTimeout(timer);
  }, [visible, onNeedDrm]);

  const sortOptions: { id: LibrarySort; label: string }[] = [
    { id: 'recent', label: ti('Recientes', 'Recent') },
    { id: 'name', label: ti('Nombre', 'Name') },
    { id: 'size', label: ti('Tamaño', 'Size') },
    { id: 'playtime', label: ti('Horas jugadas', 'Playtime') },
  ];

  return (
    <div className="space-y-5">
      <div className="flex items-center justify-between gap-4">
        <div className="flex-1">
          <SearchBar placeholder={t.library.search} onSearch={setSearch} />
        </div>
        <div className="shrink-0 flex items-center gap-2 px-4 py-2.5 rounded-full bg-[#0d0e12] border border-white/[0.08] mb-6">
          <span className="w-1.5 h-1.5 rounded-full bg-accent" />
          <span className="text-[11px] font-black text-white/90">{installedCount}</span>
          <span className="text-[9px] font-black uppercase tracking-widest text-gray-500">{t.sidebar.games}</span>
        </div>
      </div>

      {/* Sort and filter. There used to be a search box and nothing else. */}
      <div className="flex items-center justify-between gap-3 flex-wrap -mt-6">
        <div className="flex items-center gap-1.5 flex-wrap">
          <span className="text-[9px] font-black uppercase tracking-widest text-gray-600 mr-1">{ti('Ordenar', 'Sort')}</span>
          {sortOptions.map(o => (
            <LibraryPill key={o.id} active={sort === o.id} onClick={() => chooseSort(o.id)} layoutId="library-sort-pill">
              {o.label}
            </LibraryPill>
          ))}
        </div>
        <div className="flex items-center gap-1.5 flex-wrap">
          <LibraryPill active={filter === 'all'} onClick={() => setFilter('all')} layoutId="library-filter-pill">
            {ti('Todos', 'All')}
          </LibraryPill>
          <LibraryPill active={filter === 'ryuu'} onClick={() => setFilter('ryuu')} layoutId="library-filter-pill">
            Ryuu
          </LibraryPill>
          {hubcapCount > 0 && (
            <LibraryPill active={filter === 'hubcap'} onClick={() => setFilter('hubcap')} layoutId="library-filter-pill">
              Hubcap
            </LibraryPill>
          )}
          {problemCount > 0 && (
            <LibraryPill active={filter === 'problems'} onClick={() => setFilter('problems')} layoutId="library-filter-pill">
              <AlertTriangle size={11} />
              {ti('Con problemas', 'Needs attention')}
              <span className="tabular-nums opacity-70">{problemCount}</span>
            </LibraryPill>
          )}
          {problemCount > 0 && (
            <button
              onClick={repairAll}
              disabled={!!repairing}
              className="flex items-center gap-1.5 px-3.5 py-1.5 rounded-full bg-red-500/15 border border-red-400/40 text-red-200 hover:bg-red-500/25 text-[10px] font-black uppercase tracking-widest transition-colors disabled:opacity-70"
            >
              {repairing ? <RefreshCw size={11} className="animate-spin" /> : <Wrench size={11} />}
              {repairing
                ? ti(`Reparando ${repairing.done}/${repairing.total}`, `Repairing ${repairing.done}/${repairing.total}`)
                : ti('Reparar todos', 'Repair all')}
            </button>
          )}
        </div>
      </div>

      {filtered.length === 0 && (
        <div className="flex flex-col items-center justify-center py-16 gap-3 text-gray-600">
          <Library size={32} opacity={0.4} />
          <p className="text-sm font-medium">
            {q ? ti('Ningún juego coincide con la búsqueda.', 'No game matches the search.') : ti('No hay juegos en esta vista.', 'No games in this view.')}
          </p>
        </div>
      )}

      <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4">
        {visible.map((game) => (
          <GameCard
            key={game.id}
            game={game}
            isLibrary
            onInstall={onInstall}
            steamPath={steamPath}
            onUninstall={onUninstall}
            isPinned={pinnedManifests.has(game.id)}
            onTogglePin={handleTogglePin}
            installSource={installSources[game.id]}
            libraryStats={stats[game.id]}
          />
        ))}
      </div>
      {hasMore && (
        <div ref={loaderRef} className="flex justify-center py-8">
          <div className="flex items-center gap-3 text-gray-500 text-xs font-bold uppercase tracking-widest">
            <div className="w-4 h-4 border-2 border-gray-600 border-t-white rounded-full animate-spin" />
            {lang === 'es' ? 'Cargando más juegos...' : 'Loading more games...'}
          </div>
        </div>
      )}
    </div>
  );
});


// --- Cloud Saves Modal ---

const CloudSavesModal = memo(({ game, steamPath, onClose }: {
  game: Game;
  steamPath: string;
  onClose: () => void;
}) => {
  const { t } = useLanguage();
  const { notify } = useNotify();
  const [connected, setConnected] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [backups, setBackups] = useState<BackupInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [backing, setBacking] = useState(false);
  const [restoringId, setRestoringId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const autoSyncKey = `autosync_${game.id}`;
  const [autoSync, setAutoSync] = useState(() => localStorage.getItem(autoSyncKey) === 'true');
  const displayName = game.name?.trim() && !game.name.startsWith('Game ID:') ? game.name : `App ${game.id}`;

  const toggleAutoSync = () => {
    const val = !autoSync;
    setAutoSync(val);
    localStorage.setItem(autoSyncKey, String(val));
    notify(val ? 'Sincronización automática activada' : 'Sincronización automática desactivada', 'info');
  };

  const formatSize = (bytes: number) => {
    if (bytes >= 1_073_741_824) return `${(bytes / 1_073_741_824).toFixed(1)} GB`;
    if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(0)} MB`;
    if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    return `${bytes} B`;
  };

  const formatDate = (iso: string) => {
    if (!iso) return '';
    try { return new Date(iso).toLocaleString(); } catch { return iso; }
  };

  const loadState = async () => {
    setLoading(true);
    try {
      const ok = await invoke<boolean>('is_gdrive_connected');
      setConnected(ok);
      if (ok) {
        const list = await invoke<BackupInfo[]>('list_cloud_backups', { appId: game.id });
        setBackups(list);
      }
    } catch (e) {
      notify(`Error: ${e}`, 'error');
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { loadState(); }, []);

  const handleConnect = async () => {
    setConnecting(true);
    try {
      await invoke('auth_google_drive');
      setConnected(true);
      const list = await invoke<BackupInfo[]>('list_cloud_backups', { appId: game.id });
      setBackups(list);
    } catch (e) {
      notify(`Error al conectar: ${e}`, 'error');
    } finally {
      setConnecting(false);
    }
  };

  const handleBackup = async () => {
    setBacking(true);
    try {
      await invoke('backup_game_saves', { appId: game.id, steamPath });
      notify(t.saves.backupOk, 'success');
      const localModTime = await invoke<number>('get_save_modified_time', { appId: game.id, steamPath });
      localStorage.setItem(`last_auto_backup_${game.id}`, String(localModTime));
      const list = await invoke<BackupInfo[]>('list_cloud_backups', { appId: game.id });
      setBackups(list);
    } catch (e) {
      notify(`Error: ${e}`, 'error');
    } finally {
      setBacking(false);
    }
  };

  const handleRestore = async (backupId: string) => {
    setRestoringId(backupId);
    try {
      await invoke('restore_game_saves', { appId: game.id, backupId, steamPath });
      notify(t.saves.restoreOk, 'success');
      // Best-effort, same as after install: without this, Steam's own cloud
      // sync for this app is still active, and can decide its own (older or
      // empty) cloud record is authoritative and silently overwrite the save
      // we just restored the next time the game launches — the restore
      // "works" (files land on disk) but the progress disappears again the
      // moment you open the game. Reported by a user restoring on a second
      // PC: backup listed fine, restore ran with no error, but nothing came
      // back once the game opened.
      invoke('disable_steam_cloud_for_app', { steamPath, appId: game.id }).catch(() => {});
    } catch (e) {
      notify(`Error: ${e}`, 'error');
    } finally {
      setRestoringId(null);
    }
  };

  const handleDeleteBackup = async (backupId: string) => {
    setDeletingId(backupId);
    try {
      await invoke('delete_cloud_backup', { backupId });
      notify("Backup eliminado con éxito", 'success');
      const list = await invoke<BackupInfo[]>('list_cloud_backups', { appId: game.id });
      setBackups(list);
    } catch (e) {
      notify(`Error al eliminar: ${e}`, 'error');
    } finally {
      setDeletingId(null);
    }
  };

  const handleDisconnect = async () => {
    try {
      await invoke('disconnect_gdrive');
      setConnected(false);
      setBackups([]);
      notify('Cuenta desconectada', 'info');
    } catch (e) {
      notify(`Error: ${e}`, 'error');
    }
  };

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,0.75)', backdropFilter: 'blur(8px)' }}
      onClick={onClose}
    >
      <motion.div
        initial={{ scale: 0.92, opacity: 0 }}
        animate={{ scale: 1, opacity: 1 }}
        exit={{ scale: 0.92, opacity: 0 }}
        className="relative w-full max-w-[440px] bg-[#0d0e12] border border-white/[0.08] rounded-2xl shadow-2xl shadow-black/50 overflow-hidden"
        onClick={e => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-5 border-b border-white/[0.06]">
          <div className="flex items-center gap-3">
            <div className="w-10 h-10 rounded-full bg-accent/10 border border-accent/25 flex items-center justify-center shrink-0">
              <Cloud size={16} className="text-accent" />
            </div>
            <div>
              <h3 className="font-black text-sm text-white uppercase tracking-wider">{t.saves.title}</h3>
              <p className="text-[10px] text-gray-500 font-bold truncate max-w-[220px]">{displayName}</p>
            </div>
          </div>
          <button onClick={onClose} className="text-gray-500 hover:text-white transition-colors">
            <X size={18} />
          </button>
        </div>

        <div className="p-6 space-y-4">
          {/* Connect section */}
          {!connected ? (
            <button
              onClick={handleConnect}
              disabled={connecting}
              className="w-full py-3 bg-accent/10 hover:bg-accent/20 border border-accent/30 text-accent disabled:opacity-40 font-black text-[12px] rounded-xl transition-all uppercase tracking-widest flex items-center justify-center gap-2 hover:-translate-y-0.5"
            >
              {connecting ? <RefreshCw size={14} className="animate-spin" /> : <Cloud size={14} />}
              {connecting ? t.saves.connecting : t.saves.connect}
            </button>
          ) : (
            <>
              <div className="flex flex-col gap-3">
                <div className="flex items-center justify-between bg-white/[0.03] p-4 rounded-xl border border-white/[0.07]">
                  <div className="flex flex-col gap-1.5">
                    <div className="flex items-center gap-1.5 text-emerald-400/90">
                      <CheckCircle size={14} />
                      <span className="text-[11px] font-black uppercase tracking-wider">{t.saves.connected}</span>
                    </div>
                    <button
                      onClick={handleDisconnect}
                      className="text-left text-[9px] text-gray-500 hover:text-red-400 transition-colors uppercase tracking-wider font-bold"
                    >
                      Desconectar cuenta
                    </button>
                  </div>
                  <button
                    onClick={handleBackup}
                    disabled={backing}
                    className="px-5 py-2.5 bg-accent hover:brightness-110 disabled:opacity-40 text-white font-black text-[11px] rounded-xl transition-all uppercase tracking-widest flex items-center gap-2 hover:-translate-y-0.5"
                  >
                    {backing ? <RefreshCw size={14} className="animate-spin" /> : <UploadCloud size={14} />}
                    {backing ? t.saves.backing : t.saves.backup}
                  </button>
                </div>
                
                {/* Auto Sync Toggle */}
                <div className="flex items-center justify-between bg-white/[0.03] p-3.5 rounded-xl border border-white/[0.07]">
                  <div className="flex items-center gap-2.5">
                    <div className={`w-8 h-8 rounded-lg flex items-center justify-center shrink-0 transition-colors ${autoSync ? 'bg-accent/15 text-accent border border-accent/30' : 'bg-white/[0.05] text-gray-500 border border-white/10'}`}>
                      <RefreshCw size={14} className={autoSync ? 'animate-spin-slow' : ''} />
                    </div>
                    <div>
                      <p className="text-[11px] font-black text-white uppercase tracking-wider">Sincronización Automática</p>
                      <p className="text-[9px] text-gray-500 font-bold mt-0.5">Sube la partida al abrir el launcher</p>
                    </div>
                  </div>
                  <button 
                    onClick={toggleAutoSync}
                    className={`w-11 h-6 rounded-full transition-colors relative shrink-0 ${autoSync ? 'bg-accent' : 'bg-white/10'}`}
                  >
                    <div className={`absolute top-1 bottom-1 w-4 bg-white rounded-full transition-all shadow-sm ${autoSync ? 'right-1' : 'left-1'}`} />
                  </button>
                </div>
              </div>

              {/* Backups list */}
              {loading ? (
                <div className="space-y-2">
                  {[1, 2].map(i => (
                    <div key={i} className="h-12 rounded-xl bg-white/5 animate-pulse" />
                  ))}
                </div>
              ) : backups.length === 0 ? (
                <p className="text-center text-gray-500 text-[11px] py-4">{t.saves.noBackups}</p>
              ) : (
                <div className="space-y-2.5 max-h-52 overflow-y-auto pr-1">
                  {backups.map(b => (
                    <div key={b.id} className="group flex items-center justify-between bg-white/5 hover:bg-white/10 border border-white/5 hover:border-white/10 rounded-xl p-3 gap-3 transition-all">
                      <div className="flex items-center gap-3 min-w-0">
                        <div className="w-8 h-8 rounded-lg bg-accent/10 flex items-center justify-center shrink-0 border border-accent/25">
                          <Cloud size={14} className="text-blue-400" />
                        </div>
                        <div className="min-w-0">
                          <p className="text-[11px] font-bold text-white truncate group-hover:text-blue-400 transition-colors">
                            {formatDate(b.created_at)}
                          </p>
                          {b.size_bytes > 0 && (
                            <p className="text-[9px] text-gray-400 mt-0.5 uppercase tracking-wider">
                              {formatSize(b.size_bytes)}
                            </p>
                          )}
                        </div>
                      </div>
                      <div className="flex items-center gap-2">
                        <button
                          onClick={() => handleRestore(b.id)}
                          disabled={restoringId === b.id || deletingId === b.id}
                          className="shrink-0 px-3 py-1.5 bg-white/[0.06] hover:bg-accent/20 border border-white/10 hover:border-accent/40 rounded-lg text-gray-300 hover:text-white transition-all disabled:opacity-40 flex items-center gap-1.5 text-[9px] font-black uppercase tracking-wider"
                        >
                          {restoringId === b.id
                            ? <RefreshCw size={12} className="animate-spin" />
                            : <DownloadCloud size={12} />}
                          Restaurar
                        </button>
                        <button
                          onClick={() => handleDeleteBackup(b.id)}
                          disabled={deletingId === b.id || restoringId === b.id}
                          className="shrink-0 p-1.5 bg-white/5 hover:bg-red-500/20 border border-white/5 hover:border-red-500/30 rounded-lg text-gray-400 hover:text-red-400 transition-all disabled:opacity-50"
                        >
                          {deletingId === b.id
                            ? <RefreshCw size={12} className="animate-spin" />
                            : <Trash2 size={12} />}
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      </motion.div>
    </motion.div>
  );
});

// Sentinel to show credentials warning in the UI
const GOOGLE_CREDENTIALS_PLACEHOLDER =
  typeof window !== 'undefined' &&
  // This flag is set if the Rust side still has the placeholder credentials.
  // We detect it by checking a constant baked into a dummy invoke response,
  // but for simplicity we default to true so the warning always shows until
  // the developer replaces the constants in cloud_saves.rs.
  true;

// --- Online Games ---

const ONLINE_SITES = [
  {
    name: 'Online-Fix.me',
    host: 'online-fix.me',
    desc: 'Base de datos global especializada en emulación de servidores oficiales. Habilita invitaciones directas, matchmaking real (Steam/Epic) y lobbies seguros.',
    url: 'https://online-fix.me/',
    tags: ['Multiplayer', 'Fixes de Red'],
    icon: Globe,
    bar: 'from-blue-500 via-blue-500/30',
    tint: 'bg-blue-500/10 border-blue-500/25 text-blue-400',
    button: 'bg-blue-500 shadow-blue-500/25',
    hover: 'hover:border-blue-500/40',
  },
  {
    name: 'FreeTP.org',
    host: 'freetp.org',
    desc: 'Plataforma comunitaria enfocada en cooperativo directo y parches LAN. Ofrece compatibilidad con redes virtuales y emuladores para conectarte con tus amigos.',
    url: 'https://freetp.org/',
    tags: ['Co-Op', 'Juegos Completos'],
    icon: Download,
    bar: 'from-emerald-500 via-emerald-500/30',
    tint: 'bg-emerald-500/10 border-emerald-500/25 text-emerald-400',
    button: 'bg-emerald-500 shadow-emerald-500/25',
    hover: 'hover:border-emerald-500/40',
  },
];

const OnlineGamesView = () => {
  const ti = useTranslateInline();
  const [copied, setCopied] = useState(false);
  const handleCopy = () => {
    navigator.clipboard.writeText('-onlinefix');
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="space-y-6 max-w-6xl">
      {/* Header */}
      <motion.div
        initial={{ opacity: 0, y: 14 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.4, ease: 'easeOut' }}
        className="relative overflow-hidden rounded-2xl bg-[#0d0e12] border border-white/[0.08] px-7 py-6 flex items-center gap-5"
      >
        <div className="absolute top-0 left-0 right-0 h-[2px] bg-gradient-to-r from-accent via-accent/30 to-transparent" />
        <div
          className="absolute inset-0 opacity-[0.035] pointer-events-none"
          style={{ backgroundImage: 'linear-gradient(rgba(255,255,255,0.6) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.6) 1px, transparent 1px)', backgroundSize: '28px 28px' }}
        />
        <div className="relative w-14 h-14 rounded-2xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
          <Globe size={26} className="text-accent" />
        </div>
        <div className="relative min-w-0 flex-1">
          <h3 className="text-xl font-black text-white/90 tracking-tight">Juegos Online</h3>
          <p className="font-medium text-sm text-gray-400 mt-0.5">Plataformas para jugar online con parches de red.</p>
        </div>
        <span className="relative hidden md:inline-flex items-center gap-2 px-3 py-1.5 rounded-full bg-white/[0.04] border border-white/10 text-[10px] font-black uppercase tracking-widest text-gray-400">
          <span className="w-1.5 h-1.5 rounded-full bg-accent" /> {ONLINE_SITES.length} {ti('sitios', 'sites')}
        </span>
      </motion.div>

      {/* Site cards */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-5">
        {ONLINE_SITES.map((site, i) => (
          <motion.div
            key={site.name}
            initial={{ opacity: 0, y: 14 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.4, delay: 0.06 + i * 0.06, ease: 'easeOut' }}
            className={`group relative overflow-hidden rounded-2xl bg-[#0d0e12] border border-white/[0.08] ${site.hover} p-6 flex flex-col gap-4 transition-colors`}
          >
            <div className={`absolute top-0 left-0 right-0 h-[2px] bg-gradient-to-r ${site.bar} to-transparent`} />
            <span className="absolute top-4 right-5 text-5xl font-black text-white/[0.035] select-none leading-none">
              {String(i + 1).padStart(2, '0')}
            </span>

            <div className="flex items-center gap-3.5">
              <div className={`w-12 h-12 rounded-xl border ${site.tint} flex items-center justify-center shrink-0`}>
                <site.icon size={20} />
              </div>
              <div className="min-w-0">
                <h4 className="text-base font-black text-white/90 leading-tight">{site.name}</h4>
                <p className="text-[11px] text-gray-500 font-mono mt-0.5">{site.host}</p>
              </div>
            </div>

            <div className="flex flex-wrap gap-2">
              {site.tags.map(tag => (
                <span key={tag} className="px-2.5 py-1 rounded-md text-[9px] font-black uppercase tracking-widest bg-white/[0.04] border border-white/[0.08] text-gray-400">{tag}</span>
              ))}
            </div>

            <p className="text-sm text-gray-400 font-medium leading-relaxed flex-1">{site.desc}</p>

            <a
              href={site.url}
              target="_blank"
              rel="noopener noreferrer"
              className={`self-start inline-flex items-center gap-2 px-5 py-2.5 rounded-xl ${site.button} hover:brightness-110 text-white text-[11px] font-black uppercase tracking-widest transition-all duration-200 hover:-translate-y-0.5 shadow-lg`}
            >
              Ir al Sitio <ArrowUpRight size={14} className="transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5" />
            </a>
          </motion.div>
        ))}
      </div>

      {/* OnlineFix / OpenSteamTool */}
      <motion.div
        initial={{ opacity: 0, y: 14 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.4, delay: 0.16, ease: 'easeOut' }}
        className="relative overflow-hidden rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-6"
      >
        <div className="absolute top-0 left-0 right-0 h-[2px] bg-gradient-to-r from-emerald-500 via-emerald-500/30 to-transparent" />
        <div className="grid grid-cols-1 xl:grid-cols-[minmax(0,1fr)_minmax(0,1.25fr)] gap-6 items-start">
          <div className="space-y-5">
            <div className="flex items-center gap-3.5">
              <div className="w-12 h-12 rounded-xl border bg-emerald-500/10 border-emerald-500/25 text-emerald-400 flex items-center justify-center shrink-0">
                <Users size={20} />
              </div>
              <div className="min-w-0">
                <h4 className="text-base font-black text-white/90 leading-tight">OnlineFix (OpenSteamTool)</h4>
                <p className="text-[11px] text-gray-500 font-medium mt-0.5">{ti('Opciones de lanzamiento de Steam', 'Steam launch options')}</p>
              </div>
            </div>

            <p className="text-sm text-gray-400 font-medium leading-relaxed">
              {ti(
                'Para jugar en línea o en cooperativo en algunos juegos con matchmaking por lobby, agregá este código a las Opciones de lanzamiento del juego en Steam (clic derecho en el juego → Propiedades → General). Solo un juego puede tenerlo activo a la vez.',
                "To play online or co-op in some lobby-matchmaking games, add this flag to the game's launch options in Steam (right-click the game → Properties → General). Only one game can have it at a time."
              )}
            </p>

            <div className="flex items-center gap-2 p-1.5 pl-4 rounded-xl bg-black/40 border border-white/10">
              <span className="text-gray-600 font-mono text-sm select-none">$</span>
              <code className="flex-1 text-sm text-accent font-mono font-bold">-onlinefix</code>
              <button
                onClick={handleCopy}
                className={`px-4 py-2 rounded-lg text-[11px] font-black uppercase tracking-widest flex items-center gap-2 transition-all duration-200 shrink-0 ${copied ? 'bg-emerald-500 text-white' : 'bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-white'}`}
              >
                {copied ? <CheckCircle2 size={14} /> : <Copy size={14} />} {copied ? ti('Copiado', 'Copied') : ti('Copiar', 'Copy')}
              </button>
            </div>
          </div>

          {/* Real screenshot of Steam's own Properties → General panel, with
              the launch-options field highlighted — replaced an earlier
              hand-built CSS recreation of this same dialog. */}
          <div className="relative rounded-xl overflow-hidden border border-white/10 shadow-2xl select-none">
            <img src={onlineFixExample} alt="Steam Launch Options example" className="w-full h-auto block" />
            <div
              className="absolute border-2 border-emerald-500 rounded-lg shadow-[0_0_0_4px_rgba(16,185,129,0.15)] pointer-events-none"
              style={{ left: '25.5%', top: '77.5%', width: '30%', height: '6.5%' }}
            />
            <span
              className="absolute px-2 py-0.5 rounded-full bg-emerald-500 text-white text-[9px] font-black uppercase shadow-lg"
              style={{ left: 'calc(25.5% + 30%)', top: '75%', transform: 'translate(-50%, 0)' }}
            >
              Aquí
            </span>
          </div>
        </div>
      </motion.div>
    </div>
  );
};



// --- Fix Dashboard ---

const LOG_ICONS: Record<string, string> = {
  '✔': 'text-green-400',
  '✗': 'text-accent',
  '⚠': 'text-yellow-400',
  '⟳': 'text-accent',
  '━': 'text-purple-400',
};

function logColor(line: string): string {
  for (const [ch, cls] of Object.entries(LOG_ICONS)) {
    if (line.startsWith(ch)) return cls;
  }
  return 'text-gray-400';
}

const FixView = ({ steamPath }: { steamPath: string }) => {
  const { t } = useLanguage();
  const ti = useTranslateInline();
  const tf = t.fix;
  const [appId, setAppId] = useState('');
  const [running, setRunning] = useState(false);
  const [log, setLog] = useState<string[]>([]);
  const [status, setStatus] = useState<'idle' | 'ok' | 'error'>('idle');
  const logRef = useRef<HTMLDivElement>(null);

  // Auto-scroll log
  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [log]);

  const handleRepair = async () => {
    if (!appId.trim()) return;
    setRunning(true);
    setStatus('idle');
    setLog([]);

    // Subscribe to streaming log events from Rust
    const unlisten = await listen<string>('repair_log', (event) => {
      setLog(prev => [...prev, event.payload]);
    });

    try {
      await invoke('deep_repair', { appId: appId.trim(), steamPath });
      setStatus('ok');
    } catch (err) {
      setLog(prev => [...prev, `✗ ${err}`]);
      setStatus('error');
    } finally {
      unlisten();
      setRunning(false);
    }
  };

  const [runningPrereqs, setRunningPrereqs] = useState(false);
  const handlePrerequisites = async () => {
    if (!appId.trim()) return;
    setRunningPrereqs(true);
    setStatus('idle');
    setLog([]);

    const unlisten = await listen<string>('repair_log', (event) => {
      setLog(prev => [...prev, event.payload]);
    });

    try {
      const result = await invoke<string>('install_prerequisites', { appId: appId.trim(), steamPath });
      setLog(prev => [...prev, `✔ ${result}`]);
      setStatus('ok');
    } catch (err) {
      setLog(prev => [...prev, `✗ ${err}`]);
      setStatus('error');
    } finally {
      unlisten();
      setRunningPrereqs(false);
    }
  };

  return (
    <div className="max-w-2xl space-y-6 animate-in fade-in slide-in-from-bottom-4 duration-700">
      {/* Header card */}
      <div className="relative overflow-hidden rounded-3xl border border-white/10 shadow-2xl">
        <div className="absolute inset-0 bg-gradient-to-br from-[#1a1b26] to-[#0f1016]" />
        <div className="absolute -right-10 -top-10 w-48 h-48 bg-accent/10 blur-[60px] rounded-full" />
        <div className="relative z-10 p-7 space-y-5">
          <div className="flex items-center gap-5">
            <div className="w-14 h-14 rounded-2xl bg-gradient-to-br from-accent to-red-800 flex items-center justify-center shrink-0 shadow-lg shadow-accent/30">
              <Wrench size={24} className="text-white" />
            </div>
            <div>
              <h3 className="font-black text-xl text-white tracking-tight">{tf.title}</h3>
              <p className="text-sm text-gray-400 mt-1 leading-relaxed">{tf.subtitle}</p>
            </div>
          </div>
          {/* Step pills */}
          <div className="flex flex-wrap gap-2">
            {[tf.step1, tf.step2, tf.step3, tf.step4].map((s, i) => (
              <div key={i} className="flex items-center gap-2 px-3 py-2 rounded-xl bg-accent/5 border border-accent/20">
                <span className="w-5 h-5 rounded-full bg-accent text-white text-[10px] font-black flex items-center justify-center shrink-0 shadow-md shadow-accent/40">{i + 1}</span>
                <span className="text-[10px] font-black text-gray-300 uppercase tracking-wider">{s}</span>
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* Inputs */}
      <div className="bg-[#12131a] border border-white/8 rounded-3xl p-6 space-y-5 shadow-2xl">
        <div className="space-y-2">
          <label className="text-[10px] font-black uppercase tracking-[0.3em] text-gray-500">{tf.appIdLabel}</label>
          <input
            type="text"
            value={appId}
            // A pasted link becomes its id. Stripping non-digits alone looked
            // like it handled URLs, but it just glues every digit in the text
            // together — a store link with a year or a version in it produced
            // a number that belongs to no game at all.
            onChange={e => {
              const raw = e.target.value;
              const parsed = parseSteamAppId(raw);
              setAppId(parsed ?? raw.replace(/\D/g, ''));
            }}
            placeholder={tf.appIdPlaceholder}
            disabled={running}
            className="w-full bg-[#1a1b26] border border-white/8 focus:border-accent/40 rounded-2xl py-3.5 px-5 text-sm outline-none transition-all font-mono placeholder:text-gray-700 disabled:opacity-50 text-gray-200 shadow-inner"
          />
        </div>
        <div className="space-y-2">
          <label className="text-[10px] font-black uppercase tracking-[0.3em] text-gray-500">{tf.steamPath}</label>
          <div className="w-full bg-[#1a1b26] border border-white/8 rounded-2xl py-3.5 px-5 text-sm font-mono text-gray-500 select-all truncate shadow-inner">
            {steamPath}
          </div>
        </div>
        <button
          onClick={handleRepair}
          disabled={running || runningPrereqs || !appId.trim()}
          className="w-full py-4 bg-gradient-to-r from-accent to-red-700 hover:from-red-600 hover:to-red-800 disabled:opacity-30 disabled:cursor-not-allowed text-white font-black text-sm rounded-2xl transition-all duration-300 uppercase tracking-widest flex items-center justify-center gap-3 shadow-xl shadow-accent/20 hover:-translate-y-0.5"
        >
          <Wrench size={16} className={running ? 'animate-spin' : ''} />
          {running ? tf.repairing : tf.startBtn}
        </button>
        <button
          onClick={handlePrerequisites}
          disabled={running || runningPrereqs || !appId.trim()}
          title={ti('Busca e instala VC++/DirectX/EAC/PlayStation SDK que el juego venga necesitando', "Finds and runs bundled VC++/DirectX/EAC/PlayStation SDK installers the game needs")}
          className="w-full py-3.5 bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 disabled:opacity-30 disabled:cursor-not-allowed text-white font-black text-xs rounded-xl transition-all duration-300 uppercase tracking-widest flex items-center justify-center gap-2.5"
        >
          <Package size={15} className={runningPrereqs ? 'animate-spin' : ''} />
          {runningPrereqs
            ? ti('Instalando prerequisitos...', 'Installing prerequisites...')
            : ti('Instalar prerequisitos (VC++/DirectX/EAC)', 'Install prerequisites (VC++/DirectX/EAC)')}
        </button>
      </div>

      {/* Log terminal */}
      {log.length > 0 && (
        <div className="bg-[#0a0a0c] border border-white/5 rounded-3xl overflow-hidden">
          {/* Terminal header */}
          <div className="flex items-center justify-between px-5 py-3 border-b border-white/5 bg-[#0f1016]">
            <div className="flex items-center gap-2">
              <div className="flex gap-1.5">
                <div className="w-3 h-3 rounded-full bg-accent/" />
                <div className="w-3 h-3 rounded-full bg-yellow-500/60" />
                <div className="w-3 h-3 rounded-full bg-green-500/60" />
              </div>
              <span className="text-[10px] font-mono text-gray-600 ml-2">repair_log — app {appId}</span>
            </div>
            <div className="flex items-center gap-3">
              {status === 'ok' && (
                <span className="flex items-center gap-1.5 text-[10px] font-black text-green-400 uppercase tracking-wider">
                  <CheckCircle size={11} /> {tf.done}
                </span>
              )}
              {status === 'error' && (
                <span className="flex items-center gap-1.5 text-[10px] font-black text-accent uppercase tracking-wider">
                  <AlertTriangle size={11} /> {tf.error}
                </span>
              )}
              {running && (
                <RefreshCw size={12} className="text-accent animate-spin" />
              )}
              <button
                onClick={() => setLog([])}
                className="text-[10px] font-bold text-gray-600 hover:text-gray-400 uppercase tracking-wider transition-colors"
              >
                {tf.clearLog}
              </button>
            </div>
          </div>

          {/* Log lines */}
          <div
            ref={logRef}
            className="p-5 space-y-1 font-mono text-xs max-h-80 overflow-y-auto custom-scrollbar"
          >
            {log.map((line, i) => (
              <motion.div
                key={i}
                initial={{ opacity: 0, x: -6 }}
                animate={{ opacity: 1, x: 0 }}
                transition={{ duration: 0.12 }}
                className={`leading-relaxed ${logColor(line)}`}
              >
                {line}
              </motion.div>
            ))}
            {running && (
              <div className="flex items-center gap-2 text-gray-600">
                <span className="animate-pulse">▋</span>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

// --- Settings ---

/// Hubcap keys are `smm_` followed by 96 lowercase hex characters.
const HUBCAP_KEY_RE = /^smm_[0-9a-f]{96}$/;

/// Fired whenever Hubcap usage may have changed — an install, a new key, a
/// switch of source — so every place showing the count refreshes at once.
const HUBCAP_USAGE_EVENT = 'rl-hubcap-usage-refresh';

type HubcapUsage = {
  daily_usage: number;
  daily_limit: number;
  remaining: number;
  can_make_requests: boolean;
  expires_at: string | null;
  expired: boolean;
  days_left: number | null;
  username: string | null;
};

/// Part of the backend's message for a 401/403, which is what Hubcap answers
/// for a key that has expired or was revoked.
const HUBCAP_KEY_REJECTED = 'rechazó la clave';

/// The source choice as stored. One choice drives both the catalog and the
/// downloads, because a game listed by one catalog is not guaranteed to exist
/// in the other.
function readSourceSettings(): { wantsHubcap: boolean; hubcapApiKey: string } {
  let stored: string | null = null;
  let legacy: string | null = null;
  let key = '';
  try {
    stored = localStorage.getItem('rl_catalog_source');
    // Written by the earlier three-way selector; honoured until replaced.
    legacy = localStorage.getItem('rl_manifest_source');
    key = (localStorage.getItem('rl_hubcap_api_key') ?? '').trim();
  } catch { }
  return { wantsHubcap: (stored ?? legacy) === 'hubcap', hubcapApiKey: key };
}

/// The catalog to load. Hubcap without a usable key is reported as Ryuu:
/// otherwise a mistyped key would leave the Library empty.
function readCatalogSource(): { catalogSource: 'ryuu' | 'hubcap'; hubcapApiKey: string } {
  const { wantsHubcap, hubcapApiKey } = readSourceSettings();
  const catalogSource = wantsHubcap && HUBCAP_KEY_RE.test(hubcapApiKey) ? 'hubcap' : 'ryuu';
  return { catalogSource, hubcapApiKey };
}

/// Where installs take their ticket from. Read at the moment of each install
/// rather than captured once, so a change in Ajustes applies to the very next
/// game without a restart. "Ryuu" maps to the backend's automatic order, which
/// still lets a set Hubcap key rescue a ticket Ryuu left incomplete.
function readTicketSourceSettings(): { manifestSource: 'auto' | 'hubcap'; hubcapApiKey: string } {
  const { catalogSource, hubcapApiKey } = readCatalogSource();
  return { manifestSource: catalogSource === 'hubcap' ? 'hubcap' : 'auto', hubcapApiKey };
}

let hubcapUsageCache: { key: string; at: number; value: HubcapUsage } | null = null;

/// Usage for a key, cached for a minute. The header badge remounts on every tab
/// change, and that must not turn into a request per click.
async function fetchHubcapUsage(key: string, fresh = false): Promise<HubcapUsage> {
  if (!fresh && hubcapUsageCache && hubcapUsageCache.key === key && Date.now() - hubcapUsageCache.at < 60_000) {
    return hubcapUsageCache.value;
  }
  const value = await invoke<HubcapUsage>('hubcap_usage', { key });
  hubcapUsageCache = { key, at: Date.now(), value };
  return value;
}

/// "Hubcap · 18/25" beside the page title while Hubcap is the chosen source.
const HubcapUsageBadge = () => {
  const ti = useTranslateInline();
  const [usage, setUsage] = useState<HubcapUsage | null>(null);
  // Expired, or refused by Hubcap — either way it has to be renewed.
  const [keyExpired, setKeyExpired] = useState(false);

  useEffect(() => {
    let alive = true;
    const load = (fresh: boolean) => {
      const { catalogSource, hubcapApiKey } = readCatalogSource();
      if (catalogSource !== 'hubcap') {
        setUsage(null);
        setKeyExpired(false);
        return;
      }
      fetchHubcapUsage(hubcapApiKey, fresh)
        .then(u => { if (alive) { setUsage(u); setKeyExpired(u.expired); } })
        .catch(err => {
          if (!alive) return;
          setUsage(null);
          setKeyExpired(String(err).includes(HUBCAP_KEY_REJECTED));
        });
    };
    load(false);
    const onRefresh = () => load(true);
    window.addEventListener(HUBCAP_USAGE_EVENT, onRefresh);
    return () => {
      alive = false;
      window.removeEventListener(HUBCAP_USAGE_EVENT, onRefresh);
    };
  }, []);

  if (keyExpired) {
    return (
      <span
        title={ti(
          'Renueva la clave en hubcapmanifest.com y pégala en Ajustes → Fuente de juegos. Mientras tanto las instalaciones usan Ryuu.',
          'Renew the key at hubcapmanifest.com and paste it in Settings → Game source. Installs use Ryuu meanwhile.'
        )}
        className="ml-2 inline-flex items-center gap-2 pl-1.5 pr-3 py-1 rounded-full border bg-red-500/10 border-red-500/30"
      >
        <span className="w-6 h-6 rounded-full flex items-center justify-center shrink-0 bg-red-500/20 text-red-300">
          <AlertTriangle size={12} />
        </span>
        <span className="text-[10px] font-black uppercase tracking-widest text-red-300">
          {ti('Clave de Hubcap vencida', 'Hubcap key expired')}
        </span>
      </span>
    );
  }
  if (!usage) return null;
  const empty = usage.remaining <= 0 || !usage.can_make_requests;
  const ratio = usage.daily_limit > 0 ? Math.max(0, Math.min(1, usage.remaining / usage.daily_limit)) : 0;
  // Colour carries the state at a glance — plenty, running low, none left —
  // and matches the green Hubcap label on Library cards. The app's red accent
  // made a full allowance read like an error.
  const tone = empty
    ? { box: 'bg-red-500/10 border-red-500/30', text: 'text-red-300', bar: 'bg-red-400', icon: 'bg-red-500/20 text-red-300' }
    : ratio <= 0.3
      ? { box: 'bg-amber-500/10 border-amber-400/30', text: 'text-amber-200', bar: 'bg-amber-400', icon: 'bg-amber-500/20 text-amber-300' }
      : { box: 'bg-emerald-500/10 border-emerald-400/30', text: 'text-emerald-200', bar: 'bg-emerald-400', icon: 'bg-emerald-500/20 text-emerald-300' };
  return (
    <span
      title={empty
        ? ti('No quedan descargas de Hubcap hoy: las instalaciones usan Ryuu.', 'No Hubcap downloads left today: installs use Ryuu.')
        : ti(`Te quedan ${usage.remaining} descargas de Hubcap hoy.`, `${usage.remaining} Hubcap downloads left today.`)}
      className={`ml-2 inline-flex items-center gap-2.5 pl-1.5 pr-3 py-1 rounded-full border ${tone.box}`}
    >
      <span className={`w-6 h-6 rounded-full flex items-center justify-center shrink-0 ${tone.icon}`}>
        <Download size={12} />
      </span>
      <span className="flex flex-col items-start leading-none gap-1">
        <span className="text-[8px] font-black uppercase tracking-[0.2em] text-white/50">
          {ti('Hubcap · hoy', 'Hubcap · today')}
        </span>
        <span className={`text-[13px] font-black tabular-nums ${tone.text}`}>
          {usage.remaining}
          <span className="text-[10px] font-bold text-white/40"> / {usage.daily_limit}</span>
        </span>
      </span>
      <span className="w-12 h-1.5 rounded-full bg-white/10 overflow-hidden">
        <span className={`block h-full rounded-full ${tone.bar}`} style={{ width: `${Math.round(ratio * 100)}%` }} />
      </span>
    </span>
  );
};

type CatalogProgress = { games: number; page: number; waiting_secs: number; checkpoint: boolean; done: boolean };

/// Shown in the header while the Hubcap catalog is loading. The full list takes
/// minutes, mostly waiting out Hubcap's rate limit, and without this the switch
/// looked like it had simply not worked.
const CatalogLoadStatus = () => {
  const ti = useTranslateInline();
  const [progress, setProgress] = useState<CatalogProgress | null>(null);

  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    listen<CatalogProgress>('catalog_progress', e => {
      if (alive) setProgress(e.payload.done ? null : e.payload);
    }).then(fn => {
      if (alive) unlisten = fn;
      else fn();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  if (!progress) return null;
  return (
    <div className="flex items-center gap-2.5 px-4 py-2 rounded-full bg-emerald-500/10 border border-emerald-400/30">
      <div className="w-3.5 h-3.5 border-2 border-emerald-400/30 border-t-emerald-300 rounded-full animate-spin shrink-0" />
      <span className="text-[11px] font-black uppercase tracking-widest text-emerald-300 whitespace-nowrap">
        {ti('Cargando catálogo de Hubcap', 'Loading Hubcap catalog')}
      </span>
      <span className="text-[11px] font-bold tabular-nums text-emerald-200/80 whitespace-nowrap">
        {progress.games.toLocaleString()} {ti('juegos', 'games')}
      </span>
      {progress.waiting_secs > 0 && (
        <span className="text-[10px] font-medium text-amber-300/90 whitespace-nowrap">
          · {ti(`Hubcap pidió esperar ${progress.waiting_secs} s`, `Hubcap asked to wait ${progress.waiting_secs}s`)}
        </span>
      )}
    </div>
  );
};

/// What Games shows while no catalog has loaded yet.
///
/// Until one arrives, `games` holds only the installed games, so Games used
/// to list the Library a second time — reported as the Library "duplicated"
/// into Games while the Hubcap catalog downloaded.
const CatalogPending = ({ syncing }: { syncing: boolean }) => {
  const ti = useTranslateInline();
  return (
    <div className="flex flex-col items-center justify-center py-24 text-center">
      {syncing
        ? <div className="w-8 h-8 border-2 border-emerald-400/30 border-t-emerald-300 rounded-full animate-spin mb-4" />
        : <AlertTriangle size={32} className="text-amber-400 mb-4" />}
      <p className="text-sm font-black uppercase tracking-widest text-gray-200">
        {syncing ? ti('Cargando catálogo…', 'Loading catalog…') : ti('No se pudo cargar el catálogo', 'The catalog could not be loaded')}
      </p>
      <p className="text-xs text-gray-500 mt-2 max-w-md leading-relaxed">
        {syncing
          ? ti('Los juegos van apareciendo aquí a medida que llegan. Tus juegos instalados siguen en Biblioteca.', 'Games show up here as they arrive. Your installed games are still in Library.')
          : ti('Revisa tu conexión o la clave de Hubcap en Ajustes y vuelve a sincronizar. Tus juegos instalados siguen en Biblioteca.', 'Check your connection or your Hubcap key in Settings and sync again. Your installed games are still in Library.')}
      </p>
    </div>
  );
};

// Settings card for the on-screen achievement popup (see the "ach_toast"
// window in main.rs). Settings live in the backend so the watcher, which runs
// there, can read them without going through the webview.
const AchievementToastSettingsCard = () => {
  const ti = useTranslateInline();
  const [enabled, setEnabled] = useState(true);
  const [sound, setSound] = useState(true);
  const [customName, setCustomName] = useState<string | null>(null);
  const [soundError, setSoundError] = useState('');

  useEffect(() => {
    invoke<{ enabled: boolean; sound: boolean; custom_sound_name?: string | null }>('get_achievement_toast_settings')
      .then((v) => { setEnabled(v.enabled); setSound(v.sound); setCustomName(v.custom_sound_name ?? null); })
      .catch(() => {});
  }, []);

  const pickSound = async () => {
    setSoundError('');
    try {
      const picked = await open({
        multiple: false,
        filters: [{ name: 'Audio', extensions: ['mp3', 'wav', 'ogg', 'flac', 'm4a', 'aac'] }],
      });
      if (typeof picked !== 'string') return;
      const name = await invoke<string>('set_custom_achievement_sound', { path: picked });
      setCustomName(name);
      invoke('preview_achievement_sound').catch(() => {});
    } catch (e) {
      setSoundError(String(e));
    }
  };

  const resetSound = () => {
    setSoundError('');
    invoke('clear_custom_achievement_sound').then(() => setCustomName(null)).catch((e) => setSoundError(String(e)));
  };

  const save = (nextEnabled: boolean, nextSound: boolean) => {
    setEnabled(nextEnabled);
    setSound(nextSound);
    invoke('set_achievement_toast_settings', { enabled: nextEnabled, sound: nextSound }).catch(() => {});
  };

  const pill = (on: boolean) =>
    `px-4 py-2 rounded-full font-black text-xs uppercase tracking-widest transition-all duration-300 hover:-translate-y-0.5 ${
      on
        ? 'bg-accent text-white shadow-lg shadow-accent/25'
        : 'bg-white/[0.04] text-gray-400 hover:bg-white/[0.07] hover:text-white border border-white/[0.08]'
    }`;

  return (
    <div className="space-y-2.5 break-inside-avoid mb-7">
      <h3 className="flex items-center gap-2.5 text-[11px] font-black uppercase tracking-[0.25em] text-gray-300 px-1"><span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />{ti('Avisos de logros', 'Achievement popups')}</h3>
      <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] hover:border-white/[0.14] transition-colors duration-300 px-6 py-5 flex items-center justify-between gap-4 flex-wrap">
        <div className="flex items-center gap-3.5">
          <div className="w-10 h-10 rounded-xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
            <Trophy size={16} className="text-accent" />
          </div>
          <div>
            <h4 className="font-black text-white/90 text-[15px] tracking-tight">{ti('Aviso al desbloquear', 'Unlock popup')}</h4>
            <p className="text-xs font-medium text-gray-500 mt-0.5">
              {ti('Se ve sobre juegos en ventana o sin bordes, no en pantalla completa exclusiva', 'Shows over windowed or borderless games, not exclusive fullscreen')}
            </p>
          </div>
        </div>
        <div className="flex gap-2 shrink-0 flex-wrap">
          <button onClick={() => save(!enabled, sound)} className={pill(enabled)}>{ti('Aviso', 'Popup')}</button>
          <button onClick={() => save(enabled, !sound)} className={pill(sound)}>{ti('Sonido', 'Sound')}</button>
          <button
            onClick={() => invoke('test_achievement_toast').catch(() => {})}
            className="px-4 py-2 rounded-full font-black text-xs uppercase tracking-widest bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 transition-all duration-300 hover:-translate-y-0.5"
          >
            {ti('Probar', 'Test')}
          </button>
          <button
            onClick={() => invoke('test_achievement_toast', { count: 6 }).catch(() => {})}
            className="px-4 py-2 rounded-full font-black text-xs uppercase tracking-widest bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 transition-all duration-300 hover:-translate-y-0.5"
          >
            {ti('Probar ráfaga', 'Test burst')}
          </button>
        </div>
        <div className="basis-full flex items-center gap-2 flex-wrap pt-1">
          <span className="text-[10px] font-black uppercase tracking-widest text-gray-500 mr-1">{ti('Sonido', 'Sound')}</span>
          <span className="text-xs font-semibold text-gray-300 truncate max-w-[16rem]">
            {customName ?? ti('Xbox Rare Achievement (predeterminado)', 'Xbox Rare Achievement (default)')}
          </span>
          <div className="flex gap-2 ml-auto flex-wrap">
            <button onClick={pickSound} className={pill(false)}>{ti('Elegir sonido', 'Choose sound')}</button>
            {customName && <button onClick={resetSound} className={pill(false)}>{ti('Restaurar', 'Reset')}</button>}
            <button onClick={() => invoke('preview_achievement_sound').catch(() => {})} className={pill(false)}>{ti('Escuchar', 'Listen')}</button>
          </div>
          {soundError && <p className="basis-full text-[11px] font-semibold text-red-300">{soundError}</p>}
        </div>
      </div>
    </div>
  );
};

const SettingsView = ({
  pendingUpdate,
  onShowUpdate,
  onSyncCatalog,
  steamPath,
  sidebarMode,
  setSidebarMode,
  games,
}: {
  pendingUpdate: UpdateInfo | null;
  // Takes the info when the caller has just fetched it: the parent cannot
  // show the overlay without it.
  onShowUpdate: (info?: UpdateInfo) => void;
  onSyncCatalog: () => Promise<void>;
  steamPath: string;
  sidebarMode: 'hover' | 'arrow';
  setSidebarMode: (m: 'hover' | 'arrow') => void;
  games: Game[];
}) => {
  const { t, lang, setLang } = useLanguage();
  const ti = useTranslateInline();

  const { notify } = useNotify();
  const { theme, setTheme, accent, setAccent, bgPath, setBgPath } = useTheme();
  const [checking, setChecking] = useState(false);
  const [checkedInfo, setCheckedInfo] = useState<UpdateInfo | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [bgInput, setBgInput] = useState(bgPath);

  // Cover-art cache, shown so the space it takes is visible rather than a
  // mystery: it reached 3.1 GB on a user's machine before it had a ceiling.
  const [imageCache, setImageCache] = useState<{ files: number; bytes: number } | null>(null);
  const [clearingCache, setClearingCache] = useState(false);
  const [catalogSource, setCatalogSource] = useState<'ryuu' | 'hubcap'>(() => (readSourceSettings().wantsHubcap ? 'hubcap' : 'ryuu'));
  const [hubcapKey, setHubcapKey] = useState(() => readSourceSettings().hubcapApiKey);
  const [showHubcapKey, setShowHubcapKey] = useState(false);
  const [hubcapUsage, setHubcapUsage] = useState<HubcapUsage | null>(null);
  const [hubcapUsageError, setHubcapUsageError] = useState<string | null>(null);
  const [checkingHubcap, setCheckingHubcap] = useState(false);
  const hubcapKeyValid = HUBCAP_KEY_RE.test(hubcapKey);

  // Uses the stats route, which spends none of the key's daily downloads —
  // checking a key must never cost the user one.
  const loadHubcapUsage = useCallback(async (fresh: boolean) => {
    if (!HUBCAP_KEY_RE.test(hubcapKey)) {
      setHubcapUsage(null);
      return;
    }
    setCheckingHubcap(true);
    setHubcapUsageError(null);
    try {
      setHubcapUsage(await fetchHubcapUsage(hubcapKey, fresh));
    } catch (e) {
      setHubcapUsage(null);
      setHubcapUsageError(`${e}`);
    } finally {
      setCheckingHubcap(false);
    }
  }, [hubcapKey]);

  useEffect(() => {
    if (catalogSource === 'hubcap') loadHubcapUsage(false);
  }, [catalogSource, loadHubcapUsage]);

  // The catalog on screen has to match the choice, so switching reloads it.
  const chooseCatalogSource = async (value: 'ryuu' | 'hubcap') => {
    // "Already selected" is not the same as "already showing". An earlier
    // version of this screen could leave Hubcap chosen with Ryuu's list still
    // loaded, and returning here made the button look dead.
    let loaded: string | null = null;
    try { loaded = localStorage.getItem('rl_loaded_catalog_source'); } catch { }
    if (value === catalogSource && loaded === value) return;
    setCatalogSource(value);
    try {
      localStorage.setItem('rl_catalog_source', value);
      localStorage.removeItem('rl_manifest_source');
    } catch { }
    window.dispatchEvent(new Event(HUBCAP_USAGE_EVENT));
    if (value === 'hubcap' && !HUBCAP_KEY_RE.test(hubcapKey)) return;
    notify(
      value === 'hubcap'
        ? ti(
            'Cambiando al catálogo de Hubcap. La primera descarga tarda unos minutos (el progreso se ve arriba) y queda guardada; tu Biblioteca no se toca.',
            'Switching to the Hubcap catalog. The first download takes a few minutes (progress shows at the top) and is saved; your Library is left alone.'
          )
        : ti('Cambiando al catálogo de Ryuu…', 'Switching to the Ryuu catalog…'),
      'info'
    );
    try { await onSyncCatalog(); } catch { }
  };

  const checkHubcapKey = async () => {
    await loadHubcapUsage(true);
    window.dispatchEvent(new Event(HUBCAP_USAGE_EVENT));
    if (catalogSource === 'hubcap') {
      try { await onSyncCatalog(); } catch { }
    }
  };

  const refreshImageCache = useCallback(async () => {
    try {
      const [files, bytes] = await invoke<[number, number]>('image_cache_stats');
      setImageCache({ files, bytes });
    } catch {
      setImageCache(null);
    }
  }, []);

  useEffect(() => { void refreshImageCache(); }, [refreshImageCache]);

  // Google Drive accounts
  const [gdriveAccounts, setGdriveAccounts] = useState<string[]>([]);
  const [defaultGdriveAccount, setDefaultGdriveAccount] = useState<string | null>(null);
  const [addingAccount, setAddingAccount] = useState(false);
  const [gdriveConnected, setGdriveConnected] = useState(false);
  const [saveMode, setSaveModeRaw] = useState<'local' | 'cloud'>(
    () => (localStorage.getItem('rl_save_mode') as 'local' | 'cloud') ?? 'local'
  );
  const setSaveMode = (m: 'local' | 'cloud') => { setSaveModeRaw(m); localStorage.setItem('rl_save_mode', m); };

  const loadGdriveAccounts = useCallback(async () => {
    const list = await invoke<string[]>('list_gdrive_accounts').catch(() => []);
    setGdriveAccounts(list);
    const def = await invoke<string | null>('get_default_gdrive_account').catch(() => null);
    setDefaultGdriveAccount(def);
    const connected = await invoke<boolean>('is_gdrive_connected').catch(() => false);
    setGdriveConnected(connected);
  }, []);

  useEffect(() => { loadGdriveAccounts(); }, [loadGdriveAccounts]);

  const handleAddAccount = async () => {
    setAddingAccount(true);
    try {
      const email = await invoke<string>('add_gdrive_account');
      notify(ti(`Cuenta ${email} añadida correctamente`, `Account ${email} added successfully`), 'success');
      await loadGdriveAccounts();
    } catch (err) {
      notify(ti(`Error al añadir cuenta: ${err}`, `Error adding account: ${err}`), 'error');
    } finally {
      setAddingAccount(false);
    }
  };

  const handleSetDefaultAccount = async (email: string) => {
    try {
      await invoke('set_default_gdrive_account', { email });
      setDefaultGdriveAccount(email);
      notify(ti(`Cuenta ${email} establecida como predeterminada`, `Account ${email} set as default`), 'info');
    } catch (err) {
      notify(`${ti('Error', 'Error')}: ${err}`, 'error');
    }
  };

  const handleRemoveAccount = async (email: string) => {
    try {
      await invoke('remove_gdrive_account', { email });
      notify(ti(`Cuenta ${email} eliminada`, `Account ${email} removed`), 'info');
      await loadGdriveAccounts();
    } catch (err) {
      notify(`${ti('Error', 'Error')}: ${err}`, 'error');
    }
  };

  // Export / import configuration
  const [exportingConfig, setExportingConfig] = useState(false);
  const [importingConfig, setImportingConfig] = useState(false);
  // Set while the post-import auto-install loop is working through
  // catalog_games — shows which game is currently being reinstalled so a
  // 40-game backup doesn't look frozen for several minutes.
  const [installingCatalogGame, setInstallingCatalogGame] = useState<string | null>(null);

  const handleExportConfig = async () => {
    setExportingConfig(true);
    try {
      const dump: Record<string, string> = {};
      for (let i = 0; i < localStorage.length; i++) {
        const key = localStorage.key(i);
        if (key) dump[key] = localStorage.getItem(key) ?? '';
      }
      let manualAppIds: string[] = [];
      try {
        manualAppIds = JSON.parse(dump.rl_manual_games ?? '[]').map((g: { id: string }) => g.id);
      } catch { }
      // Everything Ragnarok has installed (catalog + manual + Bypass) minus
      // the manual ones (already covered separately via their .lua files) —
      // this is the list the new PC will need to reinstall from the catalog.
      const managedIds = await invoke<string[]>('get_ragnarok_managed_app_ids', { steamPath }).catch(() => []);
      const catalogGames = managedIds
        .filter(id => !manualAppIds.includes(id))
        .map(id => ({ app_id: id, name: games.find(g => g.id === id)?.name || id }));
      const savePath = await save({
        defaultPath: 'ragnarok-config-backup.zip',
        filters: [{ name: 'Ragnarok Config', extensions: ['zip'] }],
      });
      if (!savePath) { setExportingConfig(false); return; }
      await invoke('export_ragnarok_config', {
        steamPath,
        manualAppIds,
        catalogGames,
        settingsJson: JSON.stringify(dump),
        savePath,
      });
      notify(ti('Configuración exportada correctamente.', 'Configuration exported successfully.'), 'success');
    } catch (err) {
      notify(`${ti('Error al exportar', 'Export error')}: ${err}`, 'error');
    } finally {
      setExportingConfig(false);
    }
  };

  const handleImportConfig = async () => {
    const zipPath = await open({
      directory: false,
      multiple: false,
      filters: [{ name: 'Ragnarok Config', extensions: ['zip'] }],
    });
    if (!zipPath || typeof zipPath !== 'string') return;
    setImportingConfig(true);
    try {
      const result = await invoke<{ settings_json: string; catalog_games: { app_id: string; name: string }[] }>(
        'import_ragnarok_config', { steamPath, zipPath }
      );
      const parsed = JSON.parse(result.settings_json) as Record<string, string>;
      for (const [k, v] of Object.entries(parsed)) {
        localStorage.setItem(k, v);
      }
      notify(ti('Configuración importada.', 'Configuration imported.'), 'success');

      if (result.catalog_games.length > 0) {
        notify(
          ti(`Instalando ${result.catalog_games.length} juego(s) del catálogo...`, `Installing ${result.catalog_games.length} catalog game(s)...`),
          'info'
        );
        let failed = 0;
        for (const g of result.catalog_games) {
          setInstallingCatalogGame(g.name);
          try {
            await invoke('install_game', { appId: g.app_id, appName: g.name, steamPath, ...readTicketSourceSettings() });
          } catch {
            failed++;
          }
        }
        setInstallingCatalogGame(null);
        notify(
          failed === 0
            ? ti('Todos los juegos del catálogo se reinstalaron.', 'All catalog games were reinstalled.')
            : ti(`${result.catalog_games.length - failed}/${result.catalog_games.length} juegos reinstalados (${failed} fallaron — probá instalarlos a mano desde el catálogo).`,
                 `${result.catalog_games.length - failed}/${result.catalog_games.length} games reinstalled (${failed} failed — try installing those manually from the catalog).`),
          failed === 0 ? 'success' : 'error'
        );
      }
      notify(ti('Reinicia la aplicación para aplicar todos los cambios.', 'Restart the app to apply all changes.'), 'info');
    } catch (err) {
      notify(`${ti('Error al importar', 'Import error')}: ${err}`, 'error');
    } finally {
      setImportingConfig(false);
    }
  };

  const handleCheck = async () => {
    setChecking(true);
    setCheckedInfo(null);
    try {
      const info = await invoke<UpdateInfo>('check_for_updates');
      setCheckedInfo(info);
      if (info.has_update) onShowUpdate(info);
    } catch (e) {
      // An empty catch here left `checkedInfo` null, and the panel renders
      // nothing at all for null: the user pressed the button, watched a
      // spinner, and got back exactly what was there before. No error, no
      // "you're up to date" — the only readings available were "the button is
      // broken" or, worse, "there are no updates".
      notify(
        `${ti('No se pudo comprobar si hay actualizaciones', 'Could not check for updates')}: ${e}`,
        'error'
      );
    }
    setChecking(false);
  };

  const currentVersion = checkedInfo?.current_version ?? __APP_VERSION__;

  const sectionVariants = {
    hidden: { opacity: 0, y: 16 },
    show: (i: number) => ({
      opacity: 1,
      y: 0,
      transition: { delay: i * 0.06, duration: 0.45, ease: [0.16, 1, 0.3, 1] },
    }),
  };

  return (
    <div className="max-w-6xl xl:columns-2 gap-6">

      {/* Section helper */}
      {/* ─── LANGUAGE ─── */}
      <motion.div id="settings-language" custom={0} variants={sectionVariants} initial="hidden" animate="show" className="space-y-2.5 break-inside-avoid mb-7">
        <h3 className="flex items-center gap-2.5 text-[11px] font-black uppercase tracking-[0.25em] text-gray-300 px-1"><span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />{t.settings.language}</h3>
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] hover:border-white/[0.14] transition-colors duration-300 px-6 py-5 space-y-4">
          <div className="flex items-center gap-3.5">
            <div className="w-10 h-10 rounded-xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
              <Globe size={16} className="text-accent" />
            </div>
            <div>
              <h4 className="font-black text-white/90 text-[15px] tracking-tight">{t.settings.language}</h4>
              <p className="text-xs font-medium text-gray-500 mt-0.5">{t.settings.languageDesc}</p>
            </div>
          </div>
          {/* A row of pill buttons stopped scaling past 2 languages — a grid
              of cards handles 5 (and future additions) without crowding or
              wrapping awkwardly. */}
          <div className="grid grid-cols-3 sm:grid-cols-5 gap-2">
            {LANGUAGE_OPTIONS.map(({ code, label }) => (
              <motion.button
                key={code}
                onClick={() => setLang(code)}
                whileHover={{ y: -2 }}
                whileTap={{ scale: 0.96 }}
                className={`relative flex flex-col items-center gap-1.5 py-3 rounded-xl font-black transition-colors duration-300 border ${
                  lang === code
                    ? 'bg-accent/15 border-accent/50 text-white'
                    : 'bg-white/[0.03] border-white/[0.08] text-gray-400 hover:bg-white/[0.06] hover:text-white hover:border-white/[0.15]'
                }`}
              >
                {lang === code && (
                  <motion.div
                    layoutId="lang-active-ring"
                    className="absolute inset-0 rounded-xl border-2 border-accent"
                    transition={{ type: 'spring', stiffness: 400, damping: 30 }}
                  />
                )}
                <FlagIcon code={code} className="w-7 h-[18px]" />
                <span className="text-[9px] uppercase tracking-widest">{label}</span>
              </motion.button>
            ))}
          </div>
        </div>
      </motion.div>

      {/* ─── SIDEBAR ─── */}
      <motion.div id="settings-sidebar" custom={1} variants={sectionVariants} initial="hidden" animate="show" className="space-y-2.5 break-inside-avoid mb-7">
        <h3 className="flex items-center gap-2.5 text-[11px] font-black uppercase tracking-[0.25em] text-gray-300 px-1"><span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />{ti('Barra Lateral', 'Sidebar')}</h3>
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] hover:border-white/[0.14] transition-colors duration-300 px-6 py-5 flex items-center justify-between gap-4 flex-wrap">
          <div className="flex items-center gap-3.5">
            <div className="w-10 h-10 rounded-xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
              <PanelLeft size={16} className="text-accent" />
            </div>
            <div>
              <h4 className="font-black text-white/90 text-[15px] tracking-tight">{ti('Modo de colapso', 'Collapse mode')}</h4>
              <p className="text-xs font-medium text-gray-500 mt-0.5">
                {ti('Cómo se colapsa y expande la barra lateral', 'How the sidebar collapses and expands')}
              </p>
            </div>
          </div>
          <div className="flex gap-2 shrink-0">
            {([
              { key: 'arrow' as const, label: ti('Flecha', 'Arrow') },
              { key: 'hover' as const, label: ti('Hover', 'Hover') },
            ]).map(opt => (
              <button
                key={opt.key}
                onClick={() => setSidebarMode(opt.key)}
                className={`px-4 py-2 rounded-full font-black text-xs uppercase tracking-widest transition-all duration-300 hover:-translate-y-0.5 ${
                  sidebarMode === opt.key
                    ? 'bg-accent text-white shadow-lg shadow-accent/25'
                    : 'bg-white/[0.04] text-gray-400 hover:bg-white/[0.07] hover:text-white border border-white/[0.08]'
                }`}
              >
                {opt.label}
              </button>
            ))}
          </div>
        </div>
      </motion.div>

      <AchievementToastSettingsCard />

      {/* ─── ALMACENAMIENTO ─── */}
      <motion.div id="settings-storage" custom={2} variants={sectionVariants} initial="hidden" animate="show" className="space-y-2.5 break-inside-avoid mb-7">
        <h3 className="flex items-center gap-2.5 text-[11px] font-black uppercase tracking-[0.25em] text-gray-300 px-1"><span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />{ti('Almacenamiento', 'Storage')}</h3>
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] hover:border-white/[0.14] transition-colors duration-300 px-6 py-5 flex items-center justify-between gap-4 flex-wrap">
          <div className="flex items-center gap-3.5">
            <div className="w-10 h-10 rounded-xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
              <HardDrive size={16} className="text-accent" />
            </div>
            <div>
              <h4 className="font-black text-white/90 text-[15px] tracking-tight">
                {ti('Caché de portadas', 'Cover art cache')}
              </h4>
              <p className="text-xs font-medium text-gray-500 mt-0.5">
                {imageCache
                  ? ti(
                      `${imageCache.files.toLocaleString()} imágenes · ${(imageCache.bytes / 1048576).toFixed(0)} MB`,
                      `${imageCache.files.toLocaleString()} images · ${(imageCache.bytes / 1048576).toFixed(0)} MB`
                    )
                  : ti('Calculando…', 'Calculating…')}
              </p>
            </div>
          </div>
          {/* Safe to offer freely: every cover is re-downloaded on demand the
              next time its card renders, so the worst case is one small
              request, not lost data. */}
          <button
            onClick={async () => {
              setClearingCache(true);
              try {
                const freed = await invoke<number>('clear_image_cache');
                notify(
                  ti(
                    `Liberados ${(freed / 1048576).toFixed(0)} MB`,
                    `Freed ${(freed / 1048576).toFixed(0)} MB`
                  ),
                  'success'
                );
                await refreshImageCache();
              } catch (e) {
                notify(`${e}`, 'error');
              } finally {
                setClearingCache(false);
              }
            }}
            disabled={clearingCache || !imageCache || imageCache.files === 0}
            className="shrink-0 flex items-center gap-2 px-4 py-2 rounded-full font-black text-xs uppercase tracking-widest bg-white/[0.04] text-gray-400 border border-white/[0.08] hover:bg-white/[0.07] hover:text-white transition-all duration-300 hover:-translate-y-0.5 disabled:opacity-40 disabled:pointer-events-none"
          >
            <Trash2 size={13} />
            {clearingCache ? ti('Vaciando…', 'Clearing…') : ti('Vaciar', 'Clear')}
          </button>
        </div>

        {/* The app records why things failed — an archive entry refused for an
            unsafe path, a catalog source that timed out, a file a bypass
            couldn't write. That used to go to stderr, which a release build
            has no console for, so it went nowhere. This is how the user (and
            anyone helping them) gets to it. */}
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] hover:border-white/[0.14] transition-colors duration-300 px-6 py-5 flex items-center justify-between gap-4 flex-wrap">
          <div className="flex items-center gap-3.5">
            <div className="w-10 h-10 rounded-xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
              <FileText size={16} className="text-accent" />
            </div>
            <div>
              <h4 className="font-black text-white/90 text-[15px] tracking-tight">
                {ti('Registro de diagnóstico', 'Diagnostics log')}
              </h4>
              <p className="text-xs font-medium text-gray-500 mt-0.5">
                {ti(
                  'Qué falló y por qué — útil para reportar un problema.',
                  'What failed and why — useful when reporting a problem.'
                )}
              </p>
            </div>
          </div>
          <div className="shrink-0 flex items-center gap-2">
            <button
              onClick={async () => {
                try {
                  await invoke<string>('open_diagnostics_log');
                } catch (e) {
                  notify(`${e}`, 'error');
                }
              }}
              className="flex items-center gap-2 px-4 py-2 rounded-full font-black text-xs uppercase tracking-widest bg-white/[0.04] text-gray-400 border border-white/[0.08] hover:bg-white/[0.07] hover:text-white transition-all duration-300 hover:-translate-y-0.5"
            >
              <FileText size={13} />
              {ti('Ragnarok', 'Ragnarok')}
            </button>
            {/* The plugin keeps its own log inside Steam's folder, and the
                command to open it had been registered but never called. When
                the plugin is the thing misbehaving, that is the file that
                says why. */}
            <button
              onClick={async () => {
                try {
                  await invoke<string>('open_opensteamtool_logs', { steamPath });
                } catch (e) {
                  notify(`${e}`, 'error');
                }
              }}
              disabled={!steamPath}
              className="flex items-center gap-2 px-4 py-2 rounded-full font-black text-xs uppercase tracking-widest bg-white/[0.04] text-gray-400 border border-white/[0.08] hover:bg-white/[0.07] hover:text-white transition-all duration-300 hover:-translate-y-0.5 disabled:opacity-40 disabled:pointer-events-none"
            >
              <FileText size={13} />
              {ti('Plugin', 'Plugin')}
            </button>
          </div>
        </div>
      </motion.div>

      {/* ─── FUENTE DE JUEGOS ─── */}
      <motion.div id="settings-source" custom={2} variants={sectionVariants} initial="hidden" animate="show" className="space-y-2.5 break-inside-avoid mb-7">
        <h3 className="flex items-center gap-2.5 text-[11px] font-black uppercase tracking-[0.25em] text-gray-300 px-1"><span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />{ti('Fuente de juegos', 'Game source')}</h3>
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] hover:border-white/[0.14] transition-colors duration-300 px-6 py-5 space-y-4">
          <div className="flex items-center gap-3.5">
            <div className="w-10 h-10 rounded-xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
              <Package size={16} className="text-accent" />
            </div>
            <div>
              <h4 className="font-black text-white/90 text-[15px] tracking-tight">
                {ti('Catálogo y descargas', 'Catalog and downloads')}
              </h4>
              <p className="text-xs font-medium text-gray-500 mt-0.5">
                {ti(
                  'Elige de dónde sale la lista de juegos y los archivos al instalar.',
                  'Choose where the game list and install files come from.'
                )}
              </p>
            </div>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
            {([
              ['ryuu', 'Ryuu', ti('Gratis y sin límite.', 'Free, no limit.')],
              ['hubcap', 'Hubcap', ti('Requiere clave y tiene un límite diario de descargas. Si se agota, usa Ryuu.', 'Needs a key and has a daily download limit. When it runs out, Ryuu is used.')],
            ] as const).map(([value, label, hint]) => (
              <button
                key={value}
                onClick={() => chooseCatalogSource(value)}
                className={`text-left rounded-xl px-3.5 py-3 border transition-all duration-200 ${
                  catalogSource === value
                    ? 'bg-accent/10 border-accent/40'
                    : 'bg-white/[0.03] border-white/[0.08] hover:bg-white/[0.06]'
                }`}
              >
                <p className={`text-sm font-black ${catalogSource === value ? 'text-accent' : 'text-white/80'}`}>{label}</p>
                <p className="text-[11px] font-medium text-gray-500 mt-0.5 leading-snug">{hint}</p>
              </button>
            ))}
          </div>

          <div className="space-y-2">
            <div className="flex items-center justify-between gap-3 flex-wrap">
              <p className="text-[10px] font-black uppercase tracking-widest text-gray-500">
                {ti('Clave de API de Hubcap', 'Hubcap API key')}
              </p>
              {/* Where the key comes from. Without it the only instruction was
                  "paste your key", with nowhere to get one — and the same page
                  is where an expired key is renewed. */}
              <button
                onClick={() => {
                  import('@tauri-apps/api/shell').then(({ open }) => {
                    open('https://hubcapmanifest.com/api-keys/stats').catch(console.error);
                  });
                }}
                className="shrink-0 flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-white/[0.05] border border-white/10 text-[10px] font-black uppercase tracking-widest text-gray-400 hover:text-accent hover:border-accent/30 transition-colors"
              >
                {ti('Conseguir o renovar clave', 'Get or renew a key', {
                  pt: 'Pegar ou renovar chave',
                  fr: 'Obtenir ou renouveler la clé',
                  ru: 'Получить или обновить ключ',
                })}
                <ExternalLink size={11} />
              </button>
            </div>
            <p className="text-[10px] font-medium text-gray-600">
              hubcapmanifest.com/api-keys/stats
            </p>
            <div className="flex gap-2">
              <input
                type={showHubcapKey ? 'text' : 'password'}
                value={hubcapKey}
                spellCheck={false}
                autoComplete="off"
                placeholder="smm_…"
                onChange={e => {
                  const v = e.target.value.trim();
                  setHubcapKey(v);
                  setHubcapUsage(null);
                  setHubcapUsageError(null);
                  try { localStorage.setItem('rl_hubcap_api_key', v); } catch { }
                  window.dispatchEvent(new Event(HUBCAP_USAGE_EVENT));
                }}
                className="flex-1 min-w-0 bg-black/40 border border-white/10 rounded-xl px-3 py-2 text-xs font-mono text-white/90 outline-none focus:border-accent/50 transition-colors"
              />
              <button
                onClick={() => setShowHubcapKey(s => !s)}
                className="shrink-0 px-3 py-2 rounded-xl bg-white/[0.04] border border-white/[0.08] text-[11px] font-black uppercase tracking-widest text-gray-400 hover:text-white hover:bg-white/[0.07] transition-all"
              >
                {showHubcapKey ? ti('Ocultar', 'Hide') : ti('Mostrar', 'Show')}
              </button>
              <button
                onClick={checkHubcapKey}
                disabled={checkingHubcap || !hubcapKeyValid}
                className="shrink-0 flex items-center gap-1.5 px-3 py-2 rounded-xl bg-accent/10 border border-accent/30 text-[11px] font-black uppercase tracking-widest text-accent hover:bg-accent/20 transition-all disabled:opacity-40 disabled:pointer-events-none"
              >
                <RefreshCw size={12} className={checkingHubcap ? 'animate-spin' : ''} />
                {ti('Verificar', 'Check')}
              </button>
            </div>

            {hubcapKey && !hubcapKeyValid && (
              <p className="text-[11px] font-medium text-amber-400">
                {ti(
                  'El formato no parece correcto: tiene que empezar con smm_ y seguir con 96 caracteres.',
                  'That does not look right: it should start with smm_ followed by 96 characters.'
                )}
              </p>
            )}

            {catalogSource === 'hubcap' && !hubcapKeyValid && (
              <p className="text-[11px] font-medium text-red-400">
                {ti(
                  'Elegiste Hubcap pero no hay una clave válida: mientras tanto se sigue usando Ryuu.',
                  'Hubcap is selected but there is no valid key: Ryuu stays in use until you add one.'
                )}
              </p>
            )}

            {hubcapUsageError && (
              <p className="text-[11px] font-medium text-red-400">{hubcapUsageError}</p>
            )}

            {hubcapUsage && (() => {
              const empty = hubcapUsage.remaining <= 0 || !hubcapUsage.can_make_requests;
              const pct = hubcapUsage.daily_limit > 0
                ? Math.min(100, Math.round((hubcapUsage.daily_usage / hubcapUsage.daily_limit) * 100))
                : 100;
              // Hubcap sends a naive UTC timestamp; without a zone designator
              // the browser would read it as local time and skew the date.
              const expires = hubcapUsage.expires_at
                ? new Date(/[zZ]|[+-]\d\d:?\d\d$/.test(hubcapUsage.expires_at) ? hubcapUsage.expires_at : `${hubcapUsage.expires_at}Z`)
                : null;
              return (
                <div className={`rounded-xl border px-4 py-3 space-y-2.5 ${empty ? 'bg-red-500/[0.06] border-red-500/25' : 'bg-green-500/[0.06] border-green-500/20'}`}>
                  <div className="flex items-end justify-between gap-3">
                    <div>
                      <p className={`text-[10px] font-black uppercase tracking-widest ${empty ? 'text-red-400' : 'text-green-400'}`}>
                        {ti('Descargas restantes hoy', 'Downloads left today')}
                      </p>
                      <p className="text-2xl font-black text-white/90 tabular-nums leading-none mt-1">
                        {hubcapUsage.remaining}
                        <span className="text-sm font-bold text-gray-500"> / {hubcapUsage.daily_limit}</span>
                      </p>
                    </div>
                    {hubcapUsage.username && (
                      <p className="text-[11px] font-medium text-gray-500 truncate">
                        {ti('Cuenta', 'Account')}: <span className="text-gray-300">{hubcapUsage.username}</span>
                      </p>
                    )}
                  </div>
                  <div className="h-1.5 w-full rounded-full bg-white/[0.06] overflow-hidden">
                    <div
                      className={`h-full rounded-full ${empty ? 'bg-red-500' : 'bg-green-500'}`}
                      style={{ width: `${pct}%` }}
                    />
                  </div>
                  {empty && (
                    <p className="text-[11px] font-medium text-red-300">
                      {ti(
                        'No quedan descargas de Hubcap por hoy: las instalaciones usan Ryuu hasta que se reinicie el límite.',
                        'No Hubcap downloads left today: installs use Ryuu until the limit resets.'
                      )}
                    </p>
                  )}
                  {expires && !Number.isNaN(expires.getTime()) && (
                    <p className={`text-[11px] font-medium ${
                      hubcapUsage.expired
                        ? 'text-red-300'
                        : hubcapUsage.days_left !== null && hubcapUsage.days_left <= 3
                          ? 'text-amber-300'
                          : 'text-gray-500'
                    }`}>
                      {hubcapUsage.expired
                        ? ti('La clave venció el', 'Key expired on')
                        : ti('La clave vence el', 'Key expires on')}{' '}
                      {expires.toLocaleString(lang === 'es' ? 'es-ES' : 'en-US', { dateStyle: 'long', timeStyle: 'short' })}
                      {hubcapUsage.expired && ` — ${ti('renuévala en hubcapmanifest.com y pégala arriba.', 'renew it at hubcapmanifest.com and paste it above.')}`}
                    </p>
                  )}
                </div>
              );
            })()}
          </div>
        </div>
      </motion.div>

      {/* ─── PARTIDAS ─── */}
      {/* ─── UPDATES ─── */}
      <motion.div id="settings-updates" custom={2} variants={sectionVariants} initial="hidden" animate="show" className="space-y-2.5 break-inside-avoid mb-7">
        <h3 className="flex items-center gap-2.5 text-[11px] font-black uppercase tracking-[0.25em] text-gray-300 px-1"><span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />{t.settings.updates}</h3>
        <div className="relative overflow-hidden rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-5 space-y-4">
          <div className="absolute -right-12 -top-12 w-48 h-48 bg-accent/5 blur-[60px] rounded-full" />
          <div className="relative z-10 flex items-center gap-4">
            <div className="w-12 h-12 rounded-2xl bg-accent flex items-center justify-center shrink-0 shadow-lg shadow-accent/30">
              <Zap size={22} className="text-white" />
            </div>
            <div className="space-y-1.5">
              <h4 className="text-base font-black text-white/90 tracking-tight">Ragnarok Launcher</h4>
              <div className="flex items-center gap-2 flex-wrap">
                <span className="px-3 py-1 rounded-full bg-white/[0.05] border border-white/10 text-[9px] font-black uppercase tracking-widest text-gray-400">v{currentVersion}</span>
                {checkedInfo && !checkedInfo.has_update && (
                  <span className="px-3 py-1 rounded-full bg-green-500/10 border border-green-500/25 text-[9px] font-black uppercase tracking-widest text-green-400">✓ Estable</span>
                )}
                {(pendingUpdate ?? (checkedInfo?.has_update ? checkedInfo : null)) && (
                  <span className="px-3 py-1 rounded-full bg-accent/10 border border-accent/30 text-[9px] font-black uppercase tracking-widest text-accent animate-pulse">⬆ Update Available</span>
                )}
              </div>
            </div>
          </div>

          {checkedInfo && !checkedInfo.has_update && (
            <div className="relative z-10 flex items-center gap-3 px-4 py-3 rounded-xl bg-green-500/5 border border-green-500/15">
              <CheckCircle size={16} className="text-green-500 shrink-0" />
              <div>
                <p className="text-sm font-bold text-green-400">{t.update.noUpdate}</p>
                <p className="text-xs font-medium text-gray-500 mt-0.5">{t.update.noUpdateSub(checkedInfo.current_version)}</p>
              </div>
            </div>
          )}

          <div className="relative z-10 space-y-2.5">
            <button
              onClick={handleCheck}
              disabled={checking}
              className="w-full py-3.5 bg-accent hover:brightness-110 disabled:opacity-40 text-white font-black text-sm rounded-xl transition-all duration-300 uppercase tracking-widest flex items-center justify-center gap-3 shadow-xl shadow-accent/20 hover:-translate-y-0.5"
            >
              <RefreshCw size={16} className={checking ? 'animate-spin' : ''} />
              {checking ? t.update.checking : t.update.checkBtn}
            </button>
            {(pendingUpdate ?? (checkedInfo?.has_update ? checkedInfo : null)) && (
              <button
                onClick={() => onShowUpdate(pendingUpdate ?? checkedInfo ?? undefined)}
                className="w-full py-3 bg-accent/10 hover:bg-accent/20 border border-accent/30 text-accent font-black text-sm rounded-xl transition-all uppercase tracking-widest flex items-center justify-center gap-2 hover:-translate-y-0.5"
              >
                <Download size={14} />
                Instalar v{(pendingUpdate ?? checkedInfo)?.latest_version}
              </button>
            )}
          </div>
        </div>
      </motion.div>

      {/* ─── APPEARANCE ─── */}
      <motion.div id="settings-appearance" custom={3} variants={sectionVariants} initial="hidden" animate="show" className="space-y-2.5 break-inside-avoid mb-7">
        <h3 className="flex items-center gap-2.5 text-[11px] font-black uppercase tracking-[0.25em] text-gray-300 px-1"><span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />{t.appearance.title}</h3>

        {/* Accent color */}
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-4 space-y-3.5">
          <div className="flex items-center gap-3.5">
            <div className="w-9 h-9 rounded-xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
              <Zap size={16} className="text-accent" />
            </div>
            <div>
              <h4 className="font-black text-white/90 text-[15px] tracking-tight">{t.appearance.accent}</h4>
              <p className="text-xs font-medium text-gray-500 mt-0.5">{t.appearance.accentDesc}</p>
            </div>
          </div>
          <div className="flex items-center gap-3 flex-wrap">
            {ACCENT_PRESETS.map(color => (
              <motion.button
                key={color}
                onClick={() => setAccent(color)}
                title={color}
                style={{ backgroundColor: color }}
                whileHover={{ scale: 1.25 }}
                whileTap={{ scale: 0.95 }}
                className={`w-8 h-8 rounded-full shadow-lg transition-opacity duration-200 ${accent === color ? 'ring-2 ring-white ring-offset-2 ring-offset-[#0a0a0d] scale-110' : 'opacity-70 hover:opacity-100'}`}
              />
            ))}
            <input
              type="color"
              value={accent}
              onChange={e => setAccent(e.target.value)}
              title="Color personalizado"
              className="w-8 h-8 rounded-full cursor-pointer bg-transparent border-0 p-0 overflow-hidden hover:scale-125 transition-transform"
            />
          </div>
        </div>

        {/* Dynamic background */}
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-4 space-y-3">
          <div className="flex items-center gap-3.5">
            <div className="w-9 h-9 rounded-xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
              <Maximize2 size={16} className="text-accent" />
            </div>
            <div>
              <h4 className="font-black text-white/90 text-[15px] tracking-tight">{t.appearance.background}</h4>
              <p className="text-xs font-medium text-gray-500 mt-0.5">{t.appearance.backgroundDesc}</p>
            </div>
          </div>
          <div className="flex gap-2">
            <button
              onClick={async () => {
                const selected = await open({
                  multiple: false,
                  filters: [{ name: 'Media', extensions: ['mp4', 'webm', 'jpg', 'png', 'jpeg', 'gif'] }]
                });
                if (selected && typeof selected === 'string') {
                  setBgInput(selected);
                  setBgPath(selected);
                }
              }}
              className="px-4 py-2.5 rounded-xl bg-black/40 border border-white/10 hover:border-accent/50 text-gray-400 hover:text-accent text-sm transition-all flex items-center justify-center"
              title="Buscar archivo"
            >
              <Search size={16} />
            </button>
            <input
              type="text"
              value={bgInput}
              onChange={e => setBgInput(e.target.value)}
              placeholder={t.appearance.backgroundPlaceholder}
              className="flex-1 bg-black/40 border border-white/10 focus:border-accent/50 focus:ring-1 focus:ring-accent/25 rounded-xl py-2.5 px-4 text-sm outline-none transition-all font-mono placeholder:text-gray-700 text-gray-300"
            />
            <button
              onClick={() => { setBgPath(bgInput); }}
              className="px-4 py-2.5 rounded-xl bg-accent/10 border border-accent/25 text-accent font-black text-xs uppercase tracking-widest hover:bg-accent/20 transition-all hover:-translate-y-0.5"
            >
              Apply
            </button>
            {bgPath && (
              <button
                onClick={() => { setBgInput(''); setBgPath(''); }}
                className="px-4 py-2.5 rounded-xl bg-white/[0.04] border border-white/[0.08] text-gray-400 font-black text-xs uppercase tracking-widest hover:bg-white/[0.07] transition-all"
              >
                {t.appearance.backgroundClear}
              </button>
            )}
          </div>
          {bgPath && (
            <p className="text-[10px] text-accent/70 font-mono truncate flex items-center gap-1.5"><Play size={8} /> {bgPath}</p>
          )}
          {!bgPath && (
            <p className="text-[10px] text-gray-600 font-mono">
              Auto: {new Date().getMonth() >= 11 || new Date().getMonth() === 0 ? '❄️ Winter' :
                new Date().getMonth() >= 9 ? '🎃 Halloween / 🍂 Autumn' :
                  new Date().getMonth() >= 5 ? '☀️ Summer' :
                    new Date().getMonth() >= 2 ? '🌸 Spring' : '❄️ Winter'}
            </p>
          )}
        </div>

      </motion.div>

      {/* ─── GOOGLE DRIVE ─── */}
      <motion.div id="settings-drive" custom={4} variants={sectionVariants} initial="hidden" animate="show" className="space-y-2.5 break-inside-avoid mb-7">
        <h3 className="flex items-center gap-2.5 text-[11px] font-black uppercase tracking-[0.25em] text-gray-300 px-1"><span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />Google Drive</h3>
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-5 space-y-4">
          {/* Respaldos de Partidas — Local vs Nube */}
          <div className="flex items-center gap-3">
            <div className="w-9 h-9 rounded-xl bg-accent/15 border border-accent/25 flex items-center justify-center shrink-0">
              <DownloadCloud size={16} className="text-accent" />
            </div>
            <div>
              <h5 className="font-black text-[13px] text-white/90">{ti('Respaldos de Partidas', 'Save Backups')}</h5>
              <p className="text-[11px] text-gray-500">{ti('Elige dónde guardar tus partidas automáticamente', 'Choose where to save your games automatically')}</p>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <button
              onClick={() => setSaveMode('local')}
              className={`relative text-left rounded-xl border p-3.5 space-y-1 transition-all duration-200 ${saveMode === 'local' ? 'bg-accent/[0.08] border-accent/40' : 'bg-white/[0.02] border-white/[0.08] hover:bg-white/[0.04]'}`}
            >
              {saveMode === 'local' && <span className="absolute top-3 right-3 w-2 h-2 rounded-full bg-accent" />}
              <HardDrive size={16} className={saveMode === 'local' ? 'text-accent' : 'text-gray-500'} />
              <p className="text-[12px] font-black text-white/90">{ti('Local', 'Local')}</p>
              <p className="text-[10px] text-gray-500 leading-snug">{ti('Guarda en disco, sin internet necesario', 'Saves to disk, no internet needed')}</p>
              <p className="text-[9px] text-gray-600 font-bold uppercase tracking-wider">{ti('10 copias máx por juego', 'Up to 10 copies per game')}</p>
            </button>
            <button
              onClick={() => setSaveMode('cloud')}
              className={`relative text-left rounded-xl border p-3.5 space-y-1 transition-all duration-200 ${saveMode === 'cloud' ? 'bg-accent/[0.08] border-accent/40' : 'bg-white/[0.02] border-white/[0.08] hover:bg-white/[0.04]'}`}
            >
              {saveMode === 'cloud' && <span className="absolute top-3 right-3 w-2 h-2 rounded-full bg-accent" />}
              <Cloud size={16} className={saveMode === 'cloud' ? 'text-accent' : 'text-gray-500'} />
              <p className="text-[12px] font-black text-white/90">{ti('Nube (Drive)', 'Cloud (Drive)')}</p>
              <p className="text-[10px] text-gray-500 leading-snug">{ti('Sube a Google Drive, libera espacio local', 'Uploads to Google Drive, frees local space')}</p>
              <p className="text-[9px] text-gray-600 font-bold uppercase tracking-wider">{ti('Requiere conexión a internet', 'Requires internet connection')}</p>
            </button>
          </div>

          {defaultGdriveAccount && (
            <div className="flex items-center gap-2.5 rounded-xl bg-accent/[0.06] border border-accent/20 px-3.5 py-2.5">
              <div className="w-6 h-6 rounded-full bg-accent/25 flex items-center justify-center shrink-0">
                <span className="text-[9px] font-black text-accent">{defaultGdriveAccount.charAt(0).toUpperCase()}</span>
              </div>
              <div className="min-w-0">
                <p className="text-[11px] font-bold text-white/90 truncate">{defaultGdriveAccount}</p>
                <p className="text-[9px] text-accent font-bold uppercase tracking-wider">{ti('Predeterminada', 'Default')}</p>
              </div>
            </div>
          )}

          <ul className="space-y-1.5 text-[11px] font-semibold">
            <li className="flex items-center gap-2 text-gray-400">
              <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${gdriveConnected ? 'bg-accent' : 'bg-gray-600'}`} />
              {gdriveConnected ? ti('Google Drive conectado', 'Google Drive connected') : ti('Google Drive no conectado', 'Google Drive not connected')}
            </li>
            <li className="flex items-center gap-2 text-gray-400">
              <span className="w-1.5 h-1.5 rounded-full shrink-0 bg-gray-600" />
              {ti('Control por juego disponible en el panel de guardado', 'Per-game control available in the saves panel')}
            </li>
            {saveMode === 'cloud' && (
              <li className="flex items-center gap-2 text-accent font-black">
                <span className="w-1.5 h-1.5 rounded-full shrink-0 bg-accent" />
                {ti('Modo Nube — las saves se borran del disco tras subir a Drive', 'Cloud Mode — saves are deleted from disk after uploading to Drive')}
              </li>
            )}
          </ul>

          <div className="h-px bg-white/[0.06]" />

          <div className="flex items-center justify-between">
            <h5 className="font-black text-[11px] uppercase tracking-widest text-gray-300">{ti('Cuentas de Google Drive', 'Google Drive Accounts')}</h5>
            <button
              onClick={handleAddAccount}
              disabled={addingAccount}
              className="px-3 py-1.5 rounded-xl bg-accent hover:brightness-110 text-white font-bold text-[10px] uppercase tracking-wider transition-all disabled:opacity-40 flex items-center gap-1.5 shadow-lg shadow-accent/20 hover:-translate-y-0.5"
            >
              {addingAccount ? <RefreshCw size={12} className="animate-spin" /> : <UserPlus size={12} />}
              {ti('Añadir Cuenta', 'Add Account')}
            </button>
          </div>
          {gdriveAccounts.length === 0 ? (
            <p className="text-[11px] text-gray-500 text-center py-2">{ti('Sin cuentas configuradas', 'No accounts configured')}</p>
          ) : (
            <div className="space-y-2">
              <AnimatePresence initial={false}>
                {gdriveAccounts.map(email => (
                  <motion.div
                    key={email}
                    layout
                    initial={{ opacity: 0, scale: 0.96, y: -6 }}
                    animate={{ opacity: 1, scale: 1, y: 0 }}
                    exit={{ opacity: 0, scale: 0.96 }}
                    transition={{ type: 'spring', stiffness: 400, damping: 32 }}
                    className={`flex items-center gap-2.5 rounded-xl px-3.5 py-2.5 border transition-colors ${email === defaultGdriveAccount ? 'bg-accent/10 border-accent/25' : 'bg-white/[0.02] border-white/[0.06] hover:bg-white/[0.04]'}`}
                  >
                    <div className={`w-7 h-7 rounded-full flex items-center justify-center shrink-0 ${email === defaultGdriveAccount ? 'bg-accent/25' : 'bg-white/10'}`}>
                      <span className={`text-[10px] font-black ${email === defaultGdriveAccount ? 'text-accent' : 'text-gray-400'}`}>
                        {email.charAt(0).toUpperCase()}
                      </span>
                    </div>
                    <div className="flex-1 min-w-0">
                      <p className={`text-[11px] font-bold truncate ${email === defaultGdriveAccount ? 'text-white/90' : 'text-gray-300'}`}>{email}</p>
                      {email === defaultGdriveAccount && (
                        <p className="text-[9px] text-accent font-bold uppercase tracking-wider">{ti('Predeterminada', 'Default')}</p>
                      )}
                    </div>
                    <div className="flex gap-1.5 shrink-0">
                      {email !== defaultGdriveAccount && (
                        <button
                          onClick={() => handleSetDefaultAccount(email)}
                          className="px-2.5 py-1 rounded-lg bg-white/[0.04] hover:bg-accent border border-white/[0.08] hover:border-accent text-[9px] font-bold text-gray-400 hover:text-white transition-all"
                        >
                          {ti('Usar', 'Use')}
                        </button>
                      )}
                      <button
                        onClick={() => handleRemoveAccount(email)}
                        className="px-2.5 py-1 rounded-lg bg-white/[0.04] hover:bg-red-500/20 border border-white/[0.08] hover:border-red-500/30 text-[9px] font-bold text-gray-400 hover:text-red-400 transition-all"
                      >
                        <Trash2 size={11} />
                      </button>
                    </div>
                  </motion.div>
                ))}
              </AnimatePresence>
            </div>
          )}
        </div>
      </motion.div>

      {/* ─── EXPORT / IMPORT CONFIG ─── */}
      <motion.div id="settings-backup" custom={5} variants={sectionVariants} initial="hidden" animate="show" className="space-y-2.5 break-inside-avoid mb-7">
        <h3 className="flex items-center gap-2.5 text-[11px] font-black uppercase tracking-[0.25em] text-gray-300 px-1"><span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />{ti('Configuración', 'Configuration')}</h3>
        <div className="relative overflow-hidden bg-[#0d0e12] border border-white/[0.08] hover:border-accent/25 rounded-2xl px-5 py-4 flex items-center justify-between shadow-xl gap-4 flex-wrap group transition-all duration-500">
          <div className="absolute inset-x-0 top-0 h-[2px] bg-gradient-to-r from-transparent via-accent to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-500" />
          <div className="absolute -right-10 -top-10 w-32 h-32 bg-accent/10 blur-[50px] rounded-full group-hover:scale-125 transition-transform duration-500 pointer-events-none" />
          <div className="relative z-10 flex items-center gap-3.5">
            <div className="w-10 h-10 rounded-xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0">
              <Package size={16} className="text-accent" />
            </div>
            <div>
              <h4 className="font-black text-white text-sm">{ti('Exportar / Importar Configuración', 'Export / Import Configuration')}</h4>
              <p className="text-xs text-gray-500 mt-0.5">
                {ti('Ajustes, caches y todos tus juegos en un solo archivo — al importar en otra PC quedan restaurados tal cual, sin volver a descargarlos.', 'Settings, caches and every one of your games in a single file — importing on another PC restores them exactly, with nothing to re-download.')}
              </p>
              {installingCatalogGame && (
                <p className="text-[10px] text-accent font-bold mt-1.5 flex items-center gap-1.5">
                  <RefreshCw size={10} className="animate-spin" />
                  {ti(`Instalando ${installingCatalogGame}...`, `Installing ${installingCatalogGame}...`)}
                </p>
              )}
            </div>
          </div>
          <div className="relative z-10 flex items-center gap-2 shrink-0">
            <button
              onClick={handleImportConfig}
              disabled={importingConfig}
              className="px-4 py-2.5 rounded-xl bg-white/5 hover:bg-white/10 border border-white/10 hover:border-white/20 text-xs text-gray-300 hover:text-white font-black uppercase tracking-wider transition-all disabled:opacity-50 flex items-center gap-2"
            >
              <UploadCloud size={13} className={importingConfig ? 'animate-pulse' : ''} />
              {importingConfig ? ti('Importando...', 'Importing...') : ti('Importar', 'Import')}
            </button>
            <button
              onClick={handleExportConfig}
              disabled={exportingConfig}
              className="px-4 py-2.5 rounded-xl bg-accent/10 hover:bg-accent/20 border border-accent/20 hover:border-accent/40 text-xs text-accent font-black uppercase tracking-wider transition-all disabled:opacity-50 flex items-center gap-2"
            >
              <DownloadCloud size={13} className={exportingConfig ? 'animate-pulse' : ''} />
              {exportingConfig ? ti('Exportando...', 'Exporting...') : ti('Exportar', 'Export')}
            </button>
          </div>
        </div>
      </motion.div>
    </div>
  );
};

// --- Bypass ---

type MediafireFile = {
  filename: string;
  size: string;
  created: string;
  download_url: string;
};

type BypassSource = 'mediafire' | 'gdrive';
type BypassFile = MediafireFile & { source: BypassSource };
type DownloadState = { status: 'idle' | 'downloading' | 'completed' | 'error'; percentage: number };

const BYPASS_EXT_COLORS: Record<string, { bg: string; shadow: string }> = {
  zip: { bg: 'bg-blue-500', shadow: 'shadow-blue-500/30' },
  rar: { bg: 'bg-violet-500', shadow: 'shadow-violet-500/30' },
  '7z': { bg: 'bg-orange-500', shadow: 'shadow-orange-500/30' },
  exe: { bg: 'bg-emerald-500', shadow: 'shadow-emerald-500/30' },
};
const getBypassExtColor = (ext: string) => BYPASS_EXT_COLORS[ext.toLowerCase()] ?? { bg: 'bg-accent', shadow: 'shadow-accent/30' };
// Formats apply_bypass_file can extract — .exe is deliberately excluded,
// it's never auto-run.
const BYPASS_AUTO_APPLY_EXTS = ['zip', 'rar', '7z'];

// Best-effort match of a Bypass release filename against an installed
// game's title — release names carry version/scene-group noise the game
// catalog doesn't, so this strips that noise and looks for the longest
// installed-game name contained inside what's left. Never trusted blindly:
// the user always confirms the detected game before any file gets touched.
// Scene releases sometimes spell a word with leetspeak digits ("Bl4ck" for
// "Black"). Only substitutes a digit that's immediately followed by a
// letter — a trailing sequel number glued onto a title ("MaxPayne3") has
// nothing after the digit, so it's left alone; requiring only a LETTER
// AFTER (not before) is what tells the two apart, since a plain version
// number like "23" always has a digit or nothing after each digit, never
// a letter. An earlier version gated on "token contains a letter anywhere"
// instead, which wrongly turned "MaxPayne3" into "maxpaynee".
const LEET_MAP: Record<string, string> = { '0': 'o', '1': 'i', '3': 'e', '4': 'a', '5': 's', '7': 't' };
const deleetify = (s: string): string =>
  s.replace(/[013457](?=[a-zA-Z])/g, d => LEET_MAP[d] ?? d);

// Fills in word breaks the filename never had at all — "ValleyCW" (no
// separator between the real word and the tacked-on release tag) or
// "MaxPayne3" (sequel number glued straight onto the title, no space). Runs
// AFTER deleetify so it can't interfere with leet-digit detection (which
// needs the digit sitting directly against a letter to fire). Only inserts
// a space at an unambiguous implicit boundary — lowercase→uppercase
// (camelCase) or letter→digit — never guesses inside an all-lowercase or
// all-caps run, so "GTAIV" is untouched (still relies on its own alias).
const splitGluedWords = (s: string): string =>
  s.replace(/([a-z])([A-Z])/g, '$1 $2').replace(/([a-zA-Z])(\d)/g, '$1 $2');

// A few release-scene acronyms so common they're effectively the game's
// other name (not a general aliasing system — just the ones actually seen
// causing misses, expand only with similarly unambiguous entries).
const BYPASS_TITLE_ALIASES: [RegExp, string][] = [
  [/\bgtaiv\b/gi, 'grand theft auto iv'],
  [/\bgta\b/gi, 'grand theft auto'],
  [/\bbo1\b/gi, 'call of duty black ops'],
  [/\bbo2\b/gi, 'call of duty black ops ii'],
  [/\bbo3\b/gi, 'call of duty black ops iii'],
  [/\bbo6\b/gi, 'call of duty black ops 6'],
];

const stripBypassNoise = (filename: string): string => {
  let s = filename
    .replace(/\.(zip|rar|7z|exe)$/i, '')
    // Scene-style release names use '.', '-', '_' as word separators
    // ("Bl4ck.Myth.Wukong.CRACKFIXandFIX-voices38") — normalizeSearch's
    // own "strip special chars" pass just DELETES them (not replace with
    // space), which used to glue the whole filename into one run-on word
    // with no boundaries at all, e.g. "bl4ckmythwukongcrackfixandfix...".
    // That let any short catalog game name that coincidentally appeared
    // as a raw character sequence anywhere in that blob match — the
    // actual cause of games getting the wrong cover art. Converting these
    // to spaces first keeps real word boundaries intact.
    .replace(/[.\-_]/g, ' ')
    .replace(/\[[^\]]*\]/g, ' ')
    .replace(/\([^)]*\)/g, ' ')
    .replace(/\b(bypass|crack|cracked|repack|portable|update|fix|fixed|goldberg|steamworks|v\d+(?:\.\d+)*|build\.?\d+)\b/gi, ' ');
  s = deleetify(s);
  // Aliases run BEFORE splitGluedWords on purpose — "BO1" must still be one
  // token when /\bbo1\b/ tries to match it. Splitting it into "bo 1" first
  // broke that alias (regressed the Call of Duty Black Ops fix).
  for (const [pattern, replacement] of BYPASS_TITLE_ALIASES) {
    s = s.replace(pattern, replacement);
  }
  s = splitGluedWords(s);
  return normalizeSearch(s);
};

type NormalizedGame = { game: Game; norm: string };

// Single generic descriptor words that show up constantly as SUFFIXES in
// release filenames ("... Enhanced Bypass.rar") without meaning "this file
// is about a game literally named that" — the actual case that slipped
// through: a catalog game genuinely named "Enhanced" wrongly matched a GTA V
// file. Genuine single-word game titles ("Unravel", "DOOM", "Hades") are
// NOT in this list and are allowed to match — the risk was specifically
// generic words, not shortness by itself.
const GENERIC_SINGLE_WORDS = new Set([
  'enhanced', 'edition', 'remastered', 'remaster', 'deluxe', 'ultimate',
  'definitive', 'complete', 'goty', 'gold', 'extended', 'standard',
  'premium', 'anniversary', 'collection', 'bundle', 'demo', 'trial',
]);

// Normalizing every catalog game's name is the expensive part of matching —
// do it ONCE per game list (via useMemo at the call site), not once per
// bypass file. findBypassGameMatch below just scans this pre-built index.
// A wrong cover is worse than no cover, so this still trades some recall
// for precision on purpose — just narrower than blocking every single-word
// name outright.
const isEligibleNorm = (norm: string): boolean =>
  norm.length >= 6 && (norm.includes(' ') || !GENERIC_SINGLE_WORDS.has(norm));

// The catalog sometimes stores a longer official name than what a scene
// release's filename ever spells out — a trailing "<word(s)> Edition"
// ("Diablo II: Resurrected – Infernal Edition") or a leading storefront
// brand ("EA SPORTS™ FIFA 23") the community naming convention drops
// entirely. Since our match check is "does the filename contain the
// catalog name", a catalog name that's LONGER than the filename can never
// match at all — so also index a trimmed variant alongside the full one.
// The full name is still tried first (added first, and it's longer, so it
// wins the longest-match tiebreak whenever the filename really does spell
// out the edition/brand too).
const stripCatalogNoise = (norm: string): string =>
  norm
    // Deliberately capped at ONE descriptor word (+ optional leading "the")
    // before "edition" — an earlier version allowed up to 3 words and ate
    // the "IV" out of "Grand Theft Auto IV: The Complete Edition", silently
    // collapsing it to plain "Grand Theft Auto".
    .replace(/\s+(?:the\s+)?\w+\s+edition$/i, '')
    // Leading storefront/franchise branding the community naming convention
    // routinely drops — "STAR WARS Jedi: Survivor™" vs a filename that just
    // says "Jedi survivor". Same reasoning as the edition-suffix strip above:
    // only ever ADDS an alternate, shorter candidate, never replaces the
    // full name, so a filename that DOES spell out the brand still prefers
    // that longer, more specific match.
    .replace(/^(?:ea sports|star wars)\s+/i, '')
    .trim();

// A plain English plural/singular flip of the LAST word only — "Gotham
// Knights" <-> "Gotham Knight" — deliberately NOT general typo tolerance
// (no edit-distance fuzzing of arbitrary letters). Skips words under 3
// chars and words ending in "ss" ("Chess") to avoid mangling those.
const pluralVariant = (norm: string): string | null => {
  const words = norm.split(' ');
  const last = words[words.length - 1];
  if (last.length < 3) return null;
  words[words.length - 1] = last.endsWith('s') && !last.endsWith('ss') ? last.slice(0, -1) : `${last}s`;
  return words.join(' ');
};

const buildNormalizedGameIndex = (list: Game[]): NormalizedGame[] => {
  const out: NormalizedGame[] = [];
  const addIfEligible = (game: Game, norm: string) => {
    if (isEligibleNorm(norm)) out.push({ game, norm });
  };
  for (const g of list) {
    // Catalog names sometimes carry a hyphen INSIDE a word ("Middle-earth™:
    // Shadow of War™", "Half-Life", "Spider-Man") — normalizeSearch on its
    // own just deletes it (no space), gluing it into "middleearth", while a
    // filename's own "Middle-earth" goes through stripBypassNoise's
    // dot/hyphen-to-space pass and becomes "middle earth". Converting the
    // hyphen (and underscore — "Watch_Dogs" is Ubisoft's own official
    // stylization) to a space here first keeps both sides consistent. Same
    // reasoning for splitGluedWords: the catalog's own OFFICIAL stylization
    // ("Dragon Ball FighterZ") has an internal capital the filename side
    // splits into "Fighter Z" — without applying the same split here, that
    // legitimate title stopped matching its own filename.
    const full = normalizeSearch(splitGluedWords((g.name || '').replace(/[-_]/g, ' ')));
    addIfEligible(g, full);
    const trimmed = stripCatalogNoise(full);
    if (trimmed !== full) addIfEligible(g, trimmed);
    const plural = pluralVariant(full);
    if (plural && plural !== full) addIfEligible(g, plural);
  }
  return out;
};

// True if `needle` appears in `target` as a standalone run of whole words —
// bounded by spaces or the string's edges, not stitched onto neighboring
// letters. Both strings are already normalizeSearch'd (only [a-z0-9\s]
// survive), so a space is the only possible word boundary character here.
const hasWordBoundaryMatch = (target: string, needle: string): boolean => {
  let idx = target.indexOf(needle);
  while (idx !== -1) {
    const before = idx === 0 ? ' ' : target[idx - 1];
    const afterIdx = idx + needle.length;
    const after = afterIdx >= target.length ? ' ' : target[afterIdx];
    if (before === ' ' && after === ' ') return true;
    idx = target.indexOf(needle, idx + 1);
  }
  return false;
};

const findBypassGameMatch = (filename: string, index: NormalizedGame[]): Game | null => {
  const target = stripBypassNoise(filename);
  if (!target) return null;
  let best: Game | null = null;
  let bestLen = 0;
  for (const { game, norm } of index) {
    if (norm.length > bestLen && hasWordBoundaryMatch(target, norm)) {
      best = game;
      bestLen = norm.length;
    }
  }
  return best;
};

const BypassView = ({ games, steamPath }: { games: Game[]; steamPath: string }) => {
  const { lang } = useLanguage();
  const { notify } = useNotify();
  const es = lang === 'es';
  const [search, setSearch] = useState('');
  // Auto-apply writes real files into the game's install folder, so what it
  // needs is "Steam actually downloaded this" — NOT the Library's broader
  // Installed badge. That badge comes from getAllInstalledIds, which unions
  // in Ragnarok's own apps sidecar, and a game lands in that sidecar the
  // instant "Instalar" is clicked — before Steam has downloaded a single
  // byte. Filtering on it put a MATCH badge and an enabled Auto-apply on
  // Assassin's Creed IV: Black Flag for a user who had never downloaded it,
  // and the apply then failed with the backend's own "No se encontró una
  // instalación completa de este juego en Steam" — find_game_folder
  // disagreeing with the badge the UI had just shown. get_really_installed_app_ids
  // is the ACF-only answer to exactly that question (see its doc comment).
  const [reallyInstalledIds, setReallyInstalledIds] = useState<Set<string>>(new Set());
  useEffect(() => {
    if (!steamPath) return;
    let cancelled = false;
    invoke<string[]>('get_really_installed_app_ids', { steamPath })
      .then(ids => { if (!cancelled) setReallyInstalledIds(new Set(ids)); })
      .catch(() => { if (!cancelled) setReallyInstalledIds(new Set()); });
    return () => { cancelled = true; };
  }, [steamPath]);
  const installedGames = useMemo(() => games.filter(g => reallyInstalledIds.has(g.id)), [games, reallyInstalledIds]);
  const installedIndex = useMemo(() => buildNormalizedGameIndex(installedGames), [installedGames]);
  // Built off the render path, after the tab has painted.
  //
  // This was a `useMemo` over the whole catalog — 69,560 games, each run
  // through two regexes and a `normalize('NFD')`, producing roughly 150,000
  // entries — evaluated synchronously while React was rendering. Clicking the
  // Bypass tab froze the window outright, and with no spinner, since the
  // skeletons only cover the network fetch. It also depended on `games` by
  // identity, so every `setGames` anywhere in the app paid the cost again.
  //
  // Nothing needs it immediately: it only picks a cover image. The far smaller
  // `installedIndex` still drives the auto-apply match, so the list is usable
  // the moment it paints and covers fill in a tick later.
  const [catalogIndex, setCatalogIndex] = useState<NormalizedGame[]>([]);
  useEffect(() => {
    if (games.length === 0) {
      setCatalogIndex([]);
      return;
    }
    let cancelled = false;
    // Keyed on length rather than identity: the index only maps names to
    // covers, and rebuilding it every time some unrelated field of `games`
    // changes is exactly the cost this is avoiding.
    const handle = window.setTimeout(() => {
      if (!cancelled) setCatalogIndex(buildNormalizedGameIndex(games));
    }, 0);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [games.length]);
  const [confirmApply, setConfirmApply] = useState<{ file: BypassFile; game: Game } | null>(null);
  const [applyTargets, setApplyTargets] = useState<Record<string, string>>({});
  // The archive password every file from this feed uses when it's protected
  // — most filenames literally carry "CW.FIX" as a watermark, confirming
  // it's the source's standard password, not something that varies per file.
  const BYPASS_ARCHIVE_PASSWORD = 'CW.FIX';
  const [pendingPasswordDownload, setPendingPasswordDownload] = useState<BypassFile | null>(null);

  const [mediafireFiles, setMediafireFiles] = useState<MediafireFile[]>([]);
  const [mediafireLoading, setMediafireLoading] = useState(true);
  const [mediafireError, setMediafireError] = useState(false);

  const [gdriveFiles, setGdriveFiles] = useState<MediafireFile[]>([]);
  const [gdriveLoading, setGdriveLoading] = useState(true);
  const [gdriveError, setGdriveError] = useState(false);

  const [downloads, setDownloads] = useState<Record<string, DownloadState>>({});

  const fetchMediafire = useCallback(() => {
    setMediafireLoading(true);
    setMediafireError(false);
    invoke<MediafireFile[]>('fetch_mediafire_bypass')
      .then(setMediafireFiles)
      .catch(err => { console.error("Failed to fetch bypass files:", err); setMediafireError(true); })
      .finally(() => setMediafireLoading(false));
  }, []);

  const fetchGdrive = useCallback(() => {
    setGdriveLoading(true);
    setGdriveError(false);
    invoke<MediafireFile[]>('fetch_gdrive_bypass')
      .then(setGdriveFiles)
      .catch(err => { console.error("Failed to fetch gdrive bypass files:", err); setGdriveError(true); })
      .finally(() => setGdriveLoading(false));
  }, []);

  // The lists saved by the last visit go up first, before anything goes over
  // the network; the fetches below then refresh them. Every launch used to
  // open this tab on skeletons until both sources had answered.
  useEffect(() => {
    invoke<{ mediafire: MediafireFile[]; gdrive: MediafireFile[] } | null>('get_cached_bypass')
      .then(cached => {
        if (!cached) return;
        setMediafireFiles(prev => (prev.length > 0 ? prev : cached.mediafire));
        setGdriveFiles(prev => (prev.length > 0 ? prev : cached.gdrive));
      })
      .catch(() => { });
  }, []);

  useEffect(() => { fetchMediafire(); }, [fetchMediafire]);
  useEffect(() => { fetchGdrive(); }, [fetchGdrive]);

  // Google Drive sizes arrive after the list (see fetch_gdrive_bypass).
  useEffect(() => {
    let unlistenFn: (() => void) | null = null;
    let cancelled = false;
    listen<Record<string, string>>('bypass_sizes', e => {
      const sizes = e.payload;
      setGdriveFiles(prev => prev.map(f => (sizes[f.download_url] ? { ...f, size: sizes[f.download_url] } : f)));
    }).then(fn => { if (cancelled) fn(); else unlistenFn = fn; });
    return () => { cancelled = true; unlistenFn?.(); };
  }, []);

  // Single listener for every in-flight direct download — the backend tags
  // each progress event with the filename, so one subscription covers every
  // row regardless of how many are downloading at once.
  useEffect(() => {
    let unlistenFn: (() => void) | null = null;
    let cancelled = false;
    listen<{ filename: string; percentage: number; status: string }>('bypass_download_progress', e => {
      const { filename, percentage, status } = e.payload;
      setDownloads(prev => ({ ...prev, [filename]: { status: status === 'completed' ? 'completed' : 'downloading', percentage } }));
    }).then(fn => { if (cancelled) fn(); else unlistenFn = fn; });
    return () => { cancelled = true; unlistenFn?.(); };
  }, []);

  const handleDownload = async (f: BypassFile) => {
    if (downloads[f.filename]?.status === 'downloading') return;

    // download_bypass_file writes directly to save_path (a full file path,
    // not a directory) — it was previously being handed just the bare
    // Downloads folder with nothing joined onto it, so the backend tried to
    // create a file where a directory already existed and kept failing/
    // retrying, which looked like a stuck 0% / infinite loop. Ask the user
    // where to save via a native dialog instead, which both fixes that and
    // gives them the destination picker they were missing.
    const defaultDir = await downloadDir().catch(() => undefined);
    const ext = (getExtension(f.filename) ?? 'FILE').toLowerCase();
    const savePath = await save({
      defaultPath: defaultDir ? `${defaultDir}${f.filename}` : f.filename,
      filters: ext !== 'file' ? [{ name: ext.toUpperCase(), extensions: [ext] }] : undefined,
    });
    if (!savePath) return; // user cancelled the dialog

    setDownloads(prev => ({ ...prev, [f.filename]: { status: 'downloading', percentage: 0 } }));
    try {
      await invoke('download_bypass_file', { url: f.download_url, filename: f.filename, savePath });
      setDownloads(prev => ({ ...prev, [f.filename]: { status: 'completed', percentage: 100 } }));
      notify(es ? `${f.filename} descargado` : `${f.filename} downloaded`, 'success');
    } catch (err) {
      setDownloads(prev => ({ ...prev, [f.filename]: { status: 'error', percentage: 0 } }));
      notify(es ? `Error al descargar: ${err}` : `Download failed: ${err}`, 'error');
    }
  };

  // Entry point for the "Auto" button: tries to match the release filename
  // to an installed game, then asks for confirmation before touching any
  // file — falls back to the manual save-dialog download when there's no
  // confident match or the format isn't one auto-apply can handle yet.
  const handleAutoApplyClick = (f: BypassFile) => {
    if (downloads[f.filename]?.status === 'downloading') return;
    const ext = (getExtension(f.filename) ?? 'FILE').toLowerCase();
    const match = bypassMatches.get(f.filename)?.auto ?? null;
    if (!match) {
      notify(es ? 'No se detectó ningún juego instalado que coincida — descargando manualmente.' : 'No matching installed game detected — downloading manually.', 'error');
      handleDownload(f);
      return;
    }
    if (!BYPASS_AUTO_APPLY_EXTS.includes(ext)) {
      notify(es ? `El auto-apply no soporta .${ext.toUpperCase()} — descargando manualmente.` : `Auto-apply doesn't support .${ext.toUpperCase()} — downloading manually.`, 'error');
      handleDownload(f);
      return;
    }
    setConfirmApply({ file: f, game: match });
  };

  const runAutoApply = async (f: BypassFile, game: Game) => {
    setConfirmApply(null);
    setApplyTargets(prev => ({ ...prev, [f.filename]: game.name }));
    setDownloads(prev => ({ ...prev, [f.filename]: { status: 'downloading', percentage: 0 } }));
    try {
      const result = await invoke<string>('apply_bypass_file', {
        steamPath, appId: game.id, url: f.download_url, filename: f.filename,
      });
      setDownloads(prev => ({ ...prev, [f.filename]: { status: 'completed', percentage: 100 } }));
      notify(result, 'success');
    } catch (err) {
      setDownloads(prev => ({ ...prev, [f.filename]: { status: 'error', percentage: 0 } }));
      notify(es ? `Error al aplicar: ${err}` : `Apply failed: ${err}`, 'error');
    }
  };

  const formatSize = (bytesStr: string) => {
    const bytes = parseInt(bytesStr, 10);
    if (isNaN(bytes)) return bytesStr;
    if (bytes >= 1_073_741_824) return `${(bytes / 1_073_741_824).toFixed(1)} GB`;
    if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(1)} MB`;
    return `${(bytes / 1024).toFixed(0)} KB`;
  };

  const getExtension = (filename: string) => {
    const parts = filename.split('.');
    return parts.length > 1 ? parts.pop()?.toUpperCase() : 'FILE';
  };

  // One unified, chronologically-sorted list — no source tabs.
  const combined: BypassFile[] = useMemo(() => {
    const tagged: BypassFile[] = [
      ...mediafireFiles.map(f => ({ ...f, source: 'mediafire' as BypassSource })),
      ...gdriveFiles.map(f => ({ ...f, source: 'gdrive' as BypassSource })),
    ];
    return tagged.sort((a, b) => (b.created || '').localeCompare(a.created || ''));
  }, [mediafireFiles, gdriveFiles]);

  // Runs the (still O(files × catalog)) game-matching pass ONCE whenever the
  // file list or catalog actually changes, instead of on every render — this
  // used to run inline per-row, and since `downloads` updates several times
  // a second during an active download, every row was re-scanning the whole
  // catalog on every progress tick. That was the actual source of the lag,
  // not React/motion overhead.
  const bypassMatches = useMemo(() => {
    const map = new Map<string, { auto: Game | null; cover: Game | null }>();
    for (const f of combined) {
      const ext = (getExtension(f.filename) ?? 'FILE').toLowerCase();
      const auto = BYPASS_AUTO_APPLY_EXTS.includes(ext) ? findBypassGameMatch(f.filename, installedIndex) : null;
      const cover = auto ?? findBypassGameMatch(f.filename, catalogIndex);
      map.set(f.filename, { auto, cover });
    }
    return map;
  }, [combined, installedIndex, catalogIndex]);

  // Mediafire dates look like "2026-09-20 18:22:11"; Drive gives none.
  const createdAt = (f: BypassFile): number | null => {
    const t = Date.parse(f.created.replace(' ', 'T'));
    return Number.isFinite(t) ? t : null;
  };
  const isNewFile = (f: BypassFile) => {
    const t = createdAt(f);
    return t !== null && Date.now() - t < 3 * 86_400_000;
  };
  // "Persona.3.Reload.Crack.Only-voices38.rar" → "Persona 3 Reload", for
  // files no catalog game matched.
  const cleanBypassName = (filename: string) =>
    filename
      .replace(/\.(zip|rar|7z)$/i, '')
      .replace(/[._]+/g, ' ')
      .replace(/\b(CW ?FIX|Crack Only|voices\d+|FIX)\b/gi, '')
      .replace(/[\s-]+$/g, '')
      .replace(/\s+/g, ' ')
      .trim();

  type BypassFilter = 'all' | 'mine' | 'new';
  const [bypassFilter, setBypassFilter] = useState<BypassFilter>('all');
  const mineCount = useMemo(() => combined.filter(f => bypassMatches.get(f.filename)?.auto).length, [combined, bypassMatches]);
  const newCount = useMemo(() => combined.filter(isNewFile).length, [combined]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    let list = combined;
    if (bypassFilter === 'mine') list = list.filter(f => bypassMatches.get(f.filename)?.auto);
    else if (bypassFilter === 'new') list = list.filter(isNewFile);
    if (!q) return list;
    return list.filter(f =>
      f.filename.toLowerCase().includes(q) ||
      (bypassMatches.get(f.filename)?.cover?.name ?? '').toLowerCase().includes(q)
    );
  }, [combined, search, bypassFilter, bypassMatches]);

  // Anything to show — the saved list or either source — goes up at once;
  // skeletons only while there is nothing at all. It waited for both sources.
  const refreshing = mediafireLoading || gdriveLoading;
  const stillLoading = refreshing && combined.length === 0;
  const bothErrored = !mediafireLoading && !gdriveLoading && mediafireError && gdriveError;
  const partialError = (mediafireError || gdriveError) && !bothErrored;
  const retryAll = () => { fetchMediafire(); fetchGdrive(); };

  return (
    <div className="space-y-5">
      {/* ── Header ── */}
      <motion.div
        initial={{ opacity: 0, y: 14 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.35, ease: 'easeOut' }}
        className="relative overflow-hidden rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-6"
      >
        <div
          className="absolute inset-0 pointer-events-none opacity-[0.04]"
          style={{ backgroundImage: 'linear-gradient(rgba(255,255,255,0.6) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.6) 1px, transparent 1px)', backgroundSize: '28px 28px' }}
        />
        <div className="absolute -right-14 -top-14 w-48 h-48 bg-accent/10 blur-[60px] rounded-full pointer-events-none" />
        <div className="relative z-10 flex items-center justify-between gap-4">
          <div className="flex items-center gap-5">
            <div className="relative w-14 h-14 shrink-0 flex items-center justify-center">
              <motion.div
                className="absolute inset-0 rounded-2xl bg-accent blur-md"
                animate={{ opacity: [0.55, 0.2, 0.55] }}
                transition={{ duration: 2.2, repeat: Infinity, ease: 'easeInOut' }}
              />
              <div className="relative z-10 w-14 h-14 rounded-2xl bg-accent border border-accent/40 flex items-center justify-center shadow-lg shadow-accent/25">
                <ShieldOff size={22} className="text-white" />
              </div>
            </div>
            <div>
              <h3 className="text-xl font-black text-white/90 tracking-tight">Bypass</h3>
              <p className="text-xs font-semibold text-gray-400 mt-0.5">
                {stillLoading
                  ? (es ? 'Cargando descargas…' : 'Loading downloads…')
                  : (es ? `${combined.length} descargas disponibles` : `${combined.length} downloads available`)}
                {refreshing && !stillLoading && (
                  <span className="inline-flex items-center gap-1 ml-2 text-gray-500">
                    <RefreshCw size={10} className="animate-spin" />
                    {es ? 'actualizando' : 'updating'}
                  </span>
                )}
              </p>
            </div>
          </div>
          {!refreshing && (
            <motion.button
              whileHover={{ scale: 1.08, rotate: 90 }}
              whileTap={{ scale: 0.92 }}
              onClick={retryAll}
              title={es ? 'Recargar' : 'Refresh'}
              className="shrink-0 w-9 h-9 rounded-xl bg-white/[0.05] hover:bg-white/[0.1] border border-white/10 flex items-center justify-center text-gray-400 hover:text-white transition-colors"
            >
              <RefreshCw size={14} />
            </motion.button>
          )}
        </div>
      </motion.div>

      {/* ── Search ── */}
      {!stillLoading && combined.length > 0 && (
        <motion.div
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.3, ease: 'easeOut', delay: 0.05 }}
          className="relative group"
        >
          <div className="absolute inset-y-0 left-4 flex items-center pointer-events-none text-gray-500 group-focus-within:text-accent transition-colors duration-300">
            <Search size={15} />
          </div>
          <input
            type="text"
            value={search}
            onChange={e => setSearch(e.target.value)}
            placeholder={es ? `Buscar en ${combined.length} descargas...` : `Search ${combined.length} downloads...`}
            spellCheck={false}
            autoComplete="off"
            className="w-full bg-[#0d0e12] border border-white/[0.08] focus:border-accent/40 rounded-2xl py-3.5 pl-12 pr-10 text-xs text-white/90 placeholder:text-gray-600 outline-none transition-all duration-300 font-bold shadow-inner"
          />
          {search && (
            <button
              onClick={() => setSearch('')}
              className="absolute inset-y-0 right-3 flex items-center text-gray-500 hover:text-white transition-colors"
            >
              <X size={14} />
            </button>
          )}
        </motion.div>
      )}

      {/* ── Filters ── */}
      {!stillLoading && combined.length > 0 && (mineCount > 0 || newCount > 0) && (
        <div className="flex items-center gap-1.5 flex-wrap">
          <LibraryPill active={bypassFilter === 'all'} onClick={() => setBypassFilter('all')} layoutId="bypass-filter-pill">
            {es ? 'Todos' : 'All'} <span className="tabular-nums opacity-70">{combined.length}</span>
          </LibraryPill>
          {mineCount > 0 && (
            <LibraryPill active={bypassFilter === 'mine'} onClick={() => setBypassFilter('mine')} layoutId="bypass-filter-pill">
              {es ? 'Para tus juegos' : 'For your games'} <span className="tabular-nums opacity-70">{mineCount}</span>
            </LibraryPill>
          )}
          {newCount > 0 && (
            <LibraryPill active={bypassFilter === 'new'} onClick={() => setBypassFilter('new')} layoutId="bypass-filter-pill">
              {es ? 'Nuevos' : 'New'} <span className="tabular-nums opacity-70">{newCount}</span>
            </LibraryPill>
          )}
        </div>
      )}

      {/* ── Partial source error banner ── */}
      {partialError && !stillLoading && (
        <motion.div
          initial={{ opacity: 0, y: 6 }}
          animate={{ opacity: 1, y: 0 }}
          className="flex items-center justify-between gap-3 rounded-2xl bg-red-500/[0.06] border border-red-500/20 px-4 py-3"
        >
          <span className="text-[11px] font-bold text-red-400 flex items-center gap-2">
            <AlertTriangle size={13} />
            {mediafireError
              ? (es ? 'Mediafire no cargó correctamente.' : 'Mediafire failed to load.')
              : (es ? 'Google Drive no cargó correctamente.' : 'Google Drive failed to load.')}
          </span>
          <button
            onClick={retryAll}
            className="shrink-0 flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[9px] font-black uppercase tracking-widest text-gray-300 hover:text-white transition-all"
          >
            <RefreshCw size={11} />
            {es ? 'Reintentar' : 'Retry'}
          </button>
        </motion.div>
      )}

      {/* ── File list ── */}
      {stillLoading ? (
        <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4">
          {[1, 2, 3, 4, 5, 6, 7, 8].map(i => (
            <motion.div
              key={i}
              className="h-64 rounded-2xl bg-[#0d0e12] border border-white/[0.08]"
              animate={{ opacity: [0.5, 0.9, 0.5] }}
              transition={{ duration: 1.4, repeat: Infinity, ease: 'easeInOut', delay: (i % 4) * 0.15 }}
            />
          ))}
        </div>
      ) : bothErrored ? (
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] py-14 flex flex-col items-center gap-3">
          <div className="w-11 h-11 rounded-full bg-red-500/10 border border-red-500/20 flex items-center justify-center">
            <AlertTriangle size={18} className="text-red-400" />
          </div>
          <p className="text-gray-400 text-xs font-bold text-center">{es ? 'No se pudieron cargar los archivos.' : 'Could not load files.'}</p>
          <button
            onClick={retryAll}
            className="flex items-center gap-2 px-4 py-2 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[10px] font-black uppercase tracking-widest text-gray-300 hover:text-white transition-all"
          >
            <RefreshCw size={12} /> {es ? 'Reintentar' : 'Retry'}
          </button>
        </div>
      ) : combined.length === 0 ? (
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] py-14 text-center text-gray-500 text-xs font-bold">
          {es ? 'No hay archivos disponibles en este momento.' : 'No files available at the moment.'}
        </div>
      ) : filtered.length === 0 ? (
        <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] py-14 text-center text-gray-500 text-xs font-bold">
          {es ? `Sin resultados para "${search}".` : `No results for "${search}".`}
        </div>
      ) : (
        <motion.div
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.3, ease: 'easeOut', delay: 0.1 }}
          className="space-y-4"
        >
          <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4">
            {filtered.map((f, idx) => {
              const dl = downloads[f.filename] ?? { status: 'idle' as const, percentage: 0 };
              const ext = getExtension(f.filename) ?? 'FILE';
              const extColor = getBypassExtColor(ext);
              const isDone = dl.status === 'completed';
              const isErr = dl.status === 'error';
              const isDl = dl.status === 'downloading';
              const applyTarget = applyTargets[f.filename];
              const isApplying = applyTarget !== undefined;
              // Precomputed once in bypassMatches (see above) instead of
              // rescanning the whole catalog on every render.
              const { auto: autoMatch, cover: coverMatch } = bypassMatches.get(f.filename) ?? { auto: null, cover: null };
              // "Apply" becomes the main action when the game is installed;
              // it used to be an unlabelled lightning icon.
              const showApply = !!autoMatch && BYPASS_AUTO_APPLY_EXTS.includes(ext.toLowerCase()) && !isApplying && dl.status === 'idle';
              const startDownload = () => {
                if (BYPASS_AUTO_APPLY_EXTS.includes(ext.toLowerCase())) setPendingPasswordDownload(f);
                else handleDownload(f);
              };
              const created = createdAt(f);

              return (
                <motion.div
                  key={`${f.source}-${f.filename}`}
                  initial={{ opacity: 0, y: 10 }}
                  animate={{ opacity: 1, y: 0 }}
                  whileHover={{ scale: 1.02, y: -3 }}
                  transition={{ duration: 0.3, ease: 'easeOut', delay: Math.min(idx, 16) * 0.03, scale: { type: 'spring', stiffness: 400, damping: 20 } }}
                  className="relative bg-[#0d0e12] border border-white/[0.08] rounded-2xl overflow-hidden group hover:border-accent/40 transition-colors duration-300 shadow-lg flex flex-col"
                  style={{ contentVisibility: 'auto', containIntrinsicSize: 'auto 260px' }}
                >
                  <div className="pointer-events-none absolute -inset-px rounded-2xl opacity-0 group-hover:opacity-100 transition-opacity duration-500 bg-accent/[0.08] blur-xl -z-10" />

                  {/* Cover */}
                  <div className="relative h-32 w-full overflow-hidden shrink-0">
                    {coverMatch ? (
                      <>
                        <CachedImage
                          appId={coverMatch.id}
                          alt={coverMatch.name}
                          className="absolute inset-0 w-full h-full object-cover transition-transform duration-700 ease-out group-hover:scale-110 z-10"
                        />
                        <div className="absolute inset-0 bg-gradient-to-t from-black via-black/40 to-black/10 z-20" />
                      </>
                    ) : (
                      <>
                        <div className={`absolute inset-0 ${extColor.bg}`} />
                        <div
                          className="absolute inset-0 opacity-[0.08]"
                          style={{ backgroundImage: 'linear-gradient(rgba(255,255,255,0.6) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.6) 1px, transparent 1px)', backgroundSize: '16px 16px' }}
                        />
                        <div className="absolute inset-0 bg-gradient-to-t from-black/50 via-transparent to-black/10 z-20" />
                        <div className="absolute inset-0 flex items-center justify-center z-10">
                          <span className="text-3xl font-black uppercase text-white/25 tracking-widest">{ext}</span>
                        </div>
                      </>
                    )}

                    {isNewFile(f) && (
                      <div className="absolute top-2.5 left-2.5 z-30">
                        <span className="px-2 py-1 bg-accent text-white text-[9px] font-black uppercase tracking-widest rounded-full shadow-lg shadow-accent/30">
                          {es ? 'Nuevo' : 'New'}
                        </span>
                      </div>
                    )}
                    <div className="absolute top-2.5 right-2.5 z-30">
                      <span className="px-2 py-1 bg-black/60 backdrop-blur-sm border border-white/15 text-white/80 text-[9px] font-black uppercase tracking-widest rounded-full">
                        {ext}
                      </span>
                    </div>
                    {autoMatch && (
                      <div className="absolute bottom-2.5 left-2.5 z-30" title={es ? `Coincide con ${autoMatch.name}, que tienes instalado` : `Matches ${autoMatch.name}, which you have installed`}>
                        <span className="flex items-center gap-1 px-2 py-1 bg-emerald-500/20 backdrop-blur-sm border border-emerald-400/40 text-emerald-200 text-[9px] font-black uppercase tracking-widest rounded-full">
                          <CheckCircle2 size={9} /> {es ? 'Tienes el juego' : 'You have it'}
                        </span>
                      </div>
                    )}
                  </div>

                  {/* Info */}
                  <div className="p-3 flex flex-col gap-2.5 flex-1">
                    {/* The game's name as the title, the release's file name
                        under it — the file name alone was the title before. */}
                    <div className="min-w-0" title={f.filename}>
                      <p className="text-[13px] font-black text-white/90 truncate leading-snug">
                        {coverMatch?.name ?? cleanBypassName(f.filename)}
                      </p>
                      <p className="text-[10px] text-gray-600 font-semibold truncate mt-0.5">{f.filename}</p>
                      <p className="text-[10px] text-gray-400 flex items-center gap-1.5 mt-1.5 font-semibold">
                        {f.size !== '-' && (
                          <>
                            <span>{formatSize(f.size)}</span>
                            <span className="text-gray-700">•</span>
                          </>
                        )}
                        {created !== null && (
                          <>
                            <span className="truncate">{publishedAgo(new Date(created).toISOString(), es)}</span>
                            <span className="text-gray-700">•</span>
                          </>
                        )}
                        <span>{f.source === 'mediafire' ? 'Mediafire' : 'Drive'}</span>
                      </p>
                    </div>

                    <div className="mt-auto flex items-center gap-2">
                      {/* Shown on every file whose format CAN be auto-applied
                          (not just the ones that currently match an installed
                          game), so the action is discoverable instead of
                          appearing out of nowhere once a game happens to be
                          installed. Disabled — with a tooltip explaining why —
                          until the matching game is actually on disk. */}
                      {showApply && autoMatch ? (
                        <>
                          <button
                            onClick={() => handleAutoApplyClick(f)}
                            title={es ? `Aplicar automáticamente a ${autoMatch.name}` : `Auto-apply to ${autoMatch.name}`}
                            className="flex-1 flex items-center justify-center gap-1.5 px-3 py-2.5 rounded-full bg-accent text-white text-[9px] font-black uppercase tracking-widest hover:brightness-110 hover:-translate-y-0.5 shadow-lg shadow-accent/25 transition-all duration-300"
                          >
                            <Zap size={11} /> {es ? 'Aplicar' : 'Apply'}
                          </button>
                          <button
                            onClick={startDownload}
                            title={es ? 'Solo descargar' : 'Download only'}
                            className="w-9 h-9 shrink-0 flex items-center justify-center rounded-full bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-gray-300 hover:text-white transition-colors"
                          >
                            <Download size={13} />
                          </button>
                        </>
                      ) : (
                      <button
                        onClick={startDownload}
                        disabled={isDl}
                        className={`relative overflow-hidden flex-1 flex items-center justify-center gap-1.5 px-3 py-2.5 rounded-full transition-all duration-300 text-[9px] font-black uppercase tracking-widest shadow-lg ${
                          isDone
                            ? 'bg-emerald-500/15 border border-emerald-500/30 text-emerald-400 shadow-emerald-500/10'
                            : isErr
                              ? 'bg-red-500/15 border border-red-500/30 text-red-400 hover:bg-red-500/25 shadow-red-500/10'
                              : 'bg-accent text-white hover:brightness-110 hover:-translate-y-0.5 shadow-accent/25'
                        }`}
                      >
                        {isDl && dl.percentage > 0 && (
                          <motion.div
                            className="absolute inset-0 bg-white/20"
                            initial={{ width: 0 }}
                            animate={{ width: `${dl.percentage}%` }}
                            transition={{ duration: 0.2, ease: 'easeOut' }}
                          />
                        )}
                        {isDl && dl.percentage === 0 && (
                          <motion.div
                            className="absolute inset-0 bg-white/10"
                            animate={{ opacity: [0.3, 0.7, 0.3] }}
                            transition={{ duration: 1.2, repeat: Infinity, ease: 'easeInOut' }}
                          />
                        )}
                        <span className="relative z-10 flex items-center gap-1.5 truncate">
                          {isDone ? (
                            <><CheckCircle2 size={11} className="shrink-0" /> <span className="truncate">{isApplying ? (es ? `Aplicado a ${applyTarget}` : `Applied to ${applyTarget}`) : (es ? 'Listo' : 'Done')}</span></>
                          ) : isErr ? (
                            <><RefreshCw size={11} className="shrink-0" /> {es ? 'Reintentar' : 'Retry'}</>
                          ) : isDl ? (
                            dl.percentage > 0
                              ? <>{dl.percentage}%</>
                              : <><RefreshCw size={11} className="animate-spin shrink-0" /> {isApplying ? (es ? 'Aplicando…' : 'Applying…') : (es ? 'Descargando…' : 'Downloading…')}</>
                          ) : (
                            <><Download size={11} className="group-hover:translate-y-0.5 transition-transform shrink-0" /> {es ? 'Descargar' : 'Download'}</>
                          )}
                        </span>
                      </button>
                      )}
                    </div>
                  </div>
                </motion.div>
              );
            })}
          </div>

          {/* Footer count */}
          <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] px-5 py-3 flex items-center justify-between">
            <span className="text-[9px] font-black uppercase tracking-widest text-gray-500">
              {search ? `${filtered.length} / ${combined.length} ${es ? 'archivos' : 'files'}` : `${combined.length} ${es ? 'archivos totales' : 'total files'}`}
            </span>
            <div className="flex items-center gap-3">
              <span className="flex items-center gap-1.5 text-[9px] font-bold text-gray-500">
                <span className="w-1.5 h-1.5 rounded-full bg-blue-500" />Mediafire
              </span>
              <span className="flex items-center gap-1.5 text-[9px] font-bold text-gray-500">
                <span className="w-1.5 h-1.5 rounded-full bg-emerald-500" />Drive
              </span>
            </div>
          </div>
        </motion.div>
      )}

      {/* ── Password heads-up before a manual download ──
          Portal'd straight to document.body: the tab-switch wrapper
          (App.tsx's `<motion.div key={activeTab} animate={{ x: 0 }}>`)
          sets an inline `transform` even at rest, and per the CSS spec any
          ancestor with `transform` becomes the containing block for
          `position: fixed` descendants — this modal's fixed backdrop was
          being confined to that content pane instead of the real viewport,
          leaving the sidebar/header uncovered by the dark/blur overlay. */}
      {createPortal(
      <AnimatePresence>
        {pendingPasswordDownload && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-[100] flex items-center justify-center bg-black/70 backdrop-blur-sm p-4"
            onClick={() => setPendingPasswordDownload(null)}
          >
            <motion.div
              initial={{ opacity: 0, y: 12, scale: 0.97 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 12, scale: 0.97 }}
              transition={{ duration: 0.2, ease: 'easeOut' }}
              onClick={e => e.stopPropagation()}
              className="relative w-full max-w-sm rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-6 shadow-2xl overflow-hidden text-center"
            >
              <div
                className="absolute inset-0 pointer-events-none opacity-[0.04]"
                style={{ backgroundImage: 'linear-gradient(rgba(255,255,255,0.6) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.6) 1px, transparent 1px)', backgroundSize: '24px 24px' }}
              />
              <div className="absolute -top-16 left-1/2 -translate-x-1/2 w-48 h-48 bg-accent/15 blur-[70px] rounded-full pointer-events-none" />

              <div className="relative z-10 flex flex-col items-center">
                <div className="relative w-14 h-14 mb-4">
                  <motion.div
                    className="absolute inset-0 rounded-2xl bg-accent blur-md"
                    animate={{ opacity: [0.5, 0.15, 0.5] }}
                    transition={{ duration: 2.2, repeat: Infinity, ease: 'easeInOut' }}
                  />
                  <div className="relative z-10 w-14 h-14 rounded-2xl bg-accent border border-accent/40 flex items-center justify-center shadow-lg shadow-accent/25">
                    <Lock size={22} className="text-white" />
                  </div>
                </div>

                <h4 className="text-sm font-black text-white/90 uppercase tracking-wide mb-1.5">
                  {es ? 'Si pide contraseña al extraer' : 'If it asks for a password when extracting'}
                </h4>
                <p className="text-[11px] text-gray-400 font-semibold leading-relaxed mb-5 max-w-[26rem]">
                  {es
                    ? 'Este archivo puede venir protegido con contraseña. Si al descomprimirlo te la pide, usá:'
                    : 'This file may be password-protected. If you get prompted when extracting it, use:'}
                </p>

                <button
                  onClick={() => { navigator.clipboard.writeText(BYPASS_ARCHIVE_PASSWORD); notify(es ? 'Contraseña copiada' : 'Password copied', 'success'); }}
                  title={es ? 'Copiar' : 'Copy'}
                  className="group relative w-full flex items-center justify-center gap-3 px-4 py-3.5 mb-6 rounded-xl bg-black/40 border border-accent/25 hover:border-accent/50 transition-colors"
                >
                  <span className="text-accent font-black text-lg tracking-[0.2em]">{BYPASS_ARCHIVE_PASSWORD}</span>
                  <Copy size={13} className="text-gray-500 group-hover:text-accent transition-colors" />
                </button>

                <div className="flex items-center gap-2 w-full">
                  <button
                    onClick={() => setPendingPasswordDownload(null)}
                    className="flex-1 py-2.5 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[10px] font-black uppercase tracking-widest text-gray-300 hover:text-white transition-all"
                  >
                    {es ? 'Cancelar' : 'Cancel'}
                  </button>
                  <button
                    onClick={() => { const f = pendingPasswordDownload; setPendingPasswordDownload(null); if (f) handleDownload(f); }}
                    className="flex-[1.4] py-2.5 rounded-xl bg-accent text-white hover:brightness-110 hover:-translate-y-0.5 text-[10px] font-black uppercase tracking-widest transition-all shadow-lg shadow-accent/25"
                  >
                    {es ? 'Descargar' : 'Download'}
                  </button>
                </div>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>,
      document.body
      )}

      {/* ── Auto-apply confirmation ── (same portal reasoning as above) */}
      {createPortal(
      <AnimatePresence>
        {confirmApply && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-[100] flex items-center justify-center bg-black/70 backdrop-blur-sm p-4"
            onClick={() => setConfirmApply(null)}
          >
            <motion.div
              initial={{ opacity: 0, y: 12, scale: 0.97 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 12, scale: 0.97 }}
              transition={{ duration: 0.2, ease: 'easeOut' }}
              onClick={e => e.stopPropagation()}
              className="relative w-full max-w-sm rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-6 shadow-2xl overflow-hidden text-center"
            >
              <div
                className="absolute inset-0 pointer-events-none opacity-[0.04]"
                style={{ backgroundImage: 'linear-gradient(rgba(255,255,255,0.6) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.6) 1px, transparent 1px)', backgroundSize: '24px 24px' }}
              />
              <div className="absolute -top-16 left-1/2 -translate-x-1/2 w-48 h-48 bg-accent/15 blur-[70px] rounded-full pointer-events-none" />

              <div className="relative z-10 flex flex-col items-center">
                <div className="relative w-14 h-14 mb-4">
                  <motion.div
                    className="absolute inset-0 rounded-2xl bg-accent blur-md"
                    animate={{ opacity: [0.5, 0.15, 0.5] }}
                    transition={{ duration: 2.2, repeat: Infinity, ease: 'easeInOut' }}
                  />
                  <div className="relative z-10 w-14 h-14 rounded-2xl bg-accent border border-accent/40 flex items-center justify-center shadow-lg shadow-accent/25">
                    <Zap size={22} className="text-white" />
                  </div>
                </div>

                <h4 className="text-sm font-black text-white/90 uppercase tracking-wide mb-1.5">
                  {es ? '¿Aplicar este bypass?' : 'Apply this bypass?'}
                </h4>
                <p className="text-[11px] text-gray-400 font-semibold leading-relaxed mb-4 max-w-[26rem]">
                  {es
                    ? <>Se detectó que <span className="text-white/80">{confirmApply.file.filename}</span> es para:</>
                    : <>Detected that <span className="text-white/80">{confirmApply.file.filename}</span> is for:</>}
                </p>

                <div className="w-full flex items-center gap-3 p-2.5 mb-5 rounded-xl bg-black/40 border border-accent/25">
                  <div className="relative w-12 h-16 rounded-lg overflow-hidden shrink-0 bg-black/40">
                    <CachedImage
                      appId={confirmApply.game.id}
                      alt={confirmApply.game.name}
                      className="absolute inset-0 w-full h-full object-cover"
                    />
                  </div>
                  <span className="flex-1 text-left text-sm font-black text-accent leading-snug">
                    {confirmApply.game.name}
                  </span>
                </div>

                <p className="text-[10px] text-gray-500 font-semibold leading-relaxed mb-5 max-w-[26rem]">
                  {es
                    ? 'Los archivos que se vayan a sobrescribir se respaldan antes de aplicar. Si esto es para otro juego, cancelá y descargalo manualmente.'
                    : 'Any file about to be overwritten is backed up before applying. If this is for a different game, cancel and download it manually instead.'}
                </p>

                <div className="flex items-center gap-2 w-full">
                  <button
                    onClick={() => setConfirmApply(null)}
                    className="flex-1 py-2.5 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[10px] font-black uppercase tracking-widest text-gray-300 hover:text-white transition-all"
                  >
                    {es ? 'Cancelar' : 'Cancel'}
                  </button>
                  <button
                    onClick={() => runAutoApply(confirmApply.file, confirmApply.game)}
                    className="flex-[1.4] py-2.5 rounded-xl bg-accent text-white hover:brightness-110 hover:-translate-y-0.5 text-[10px] font-black uppercase tracking-widest transition-all shadow-lg shadow-accent/25"
                  >
                    {es ? 'Sí, aplicar' : 'Yes, apply'}
                  </button>
                </div>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>,
      document.body
      )}
    </div>
  );
};

// --- Steam Workshop ---

interface WorkshopItemInfo {
  item_id: string;
  app_id: string;
  title: string;
  preview_url: string;
  file_size: number;
}

interface InstalledWorkshopItem {
  item_id: string;
  title: string;
  preview_url: string;
  size_bytes: number;
}

interface StaticVersionInfo {
  build_id: string | null;
  version_label: string | null;
}

const formatBytes = (bytes: number): string => {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  let i = 0;
  let n = bytes;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(n >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
};

// Scoped to one specific game (opened from its GameCard), so unlike a
// standalone Workshop tab there's no cross-referencing needed to figure out
// which library game a resolved item belongs to — we already know. Still
// resolves the item first (rather than installing blind) so the user can
// catch a pasted-the-wrong-link mistake before anything gets written to disk,
// flagged if the item's own consumer_app_id disagrees with this game's id.
const WorkshopModal = memo(({ game, steamPath, onClose }: {
  game: Game;
  steamPath: string;
  onClose: () => void;
}) => {
  const { lang } = useLanguage();
  const { notify } = useNotify();
  const es = lang === 'es';
  const displayName = game.name?.trim() && !game.name.startsWith('Game ID:') ? game.name : `App ${game.id}`;

  const [linkInput, setLinkInput] = useState('');
  const [resolving, setResolving] = useState(false);
  const [item, setItem] = useState<WorkshopItemInfo | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installedPath, setInstalledPath] = useState<string | null>(null);
  // Ticks up while installing so the button can explain WHY it's taking a
  // while instead of just spinning silently — a SteamCMD-backed download can
  // genuinely take a minute or more the first time (its own self-update).
  const [installElapsed, setInstallElapsed] = useState(0);
  useEffect(() => {
    if (!installing) { setInstallElapsed(0); return; }
    const id = setInterval(() => setInstallElapsed(s => s + 1), 1000);
    return () => clearInterval(id);
  }, [installing]);

  const [installedMods, setInstalledMods] = useState<InstalledWorkshopItem[]>([]);
  const [loadingMods, setLoadingMods] = useState(true);
  const [uninstallingId, setUninstallingId] = useState<string | null>(null);

  const loadInstalledMods = async () => {
    setLoadingMods(true);
    try {
      const list = await invoke<InstalledWorkshopItem[]>('list_installed_workshop_items', { steamPath, appId: game.id });
      setInstalledMods(list);
    } catch {
      setInstalledMods([]);
    } finally {
      setLoadingMods(false);
    }
  };

  useEffect(() => { loadInstalledMods(); }, [game.id, steamPath]);

  const mismatched = item !== null && item.app_id !== '0' && item.app_id !== game.id;

  const handleResolve = async () => {
    if (!linkInput.trim()) return;
    setResolving(true);
    setItem(null);
    setInstalledPath(null);
    try {
      const info = await invoke<WorkshopItemInfo>('resolve_workshop_item', { input: linkInput.trim() });
      setItem(info);
    } catch (err) {
      notify(`Error: ${err}`, 'error');
    } finally {
      setResolving(false);
    }
  };

  const handleInstall = async () => {
    if (!item) return;
    setInstalling(true);
    try {
      const result = await invoke<{ path: string; warning: string | null }>('install_workshop_item', {
        steamPath,
        appId: game.id,
        itemId: item.item_id,
        title: item.title,
        previewUrl: item.preview_url,
      });
      setInstalledPath(result.path);
      // A .gma that couldn't be extracted is on disk but won't load. Saying
      // "installed" there sends the user looking for a mod the game will
      // never show.
      if (result.warning) {
        notify(result.warning, 'warning', { persistent: true });
      } else {
        notify(es ? '¡Mod instalado!' : 'Mod installed!', 'success');
      }
      loadInstalledMods();
    } catch (err) {
      notify(`Error: ${err}`, 'error');
    } finally {
      setInstalling(false);
    }
  };

  const handleUninstallMod = async (itemId: string) => {
    setUninstallingId(itemId);
    // The backend always drops the index entry even if some files on disk
    // couldn't be removed (e.g. the game still had them open) — it reports
    // that as an Err, but the item genuinely is gone from Ragnarok's own
    // tracking either way, so the list should reflect that regardless of
    // whether this call resolves or rejects.
    setInstalledMods(prev => prev.filter(m => m.item_id !== itemId));
    try {
      await invoke('uninstall_workshop_item', { steamPath, appId: game.id, itemId });
      notify(es ? 'Mod eliminado.' : 'Mod removed.', 'success');
    } catch (err) {
      notify(`${err}`, 'error');
    } finally {
      setUninstallingId(null);
    }
  };

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,0.75)', backdropFilter: 'blur(8px)' }}
      onClick={onClose}
    >
      <motion.div
        initial={{ scale: 0.96, opacity: 0, y: 10 }}
        animate={{ scale: 1, opacity: 1, y: 0 }}
        exit={{ scale: 0.96, opacity: 0, y: 10 }}
        transition={{ duration: 0.3, ease: 'easeOut' }}
        className="relative w-full max-w-3xl bg-[#0d0e12] border border-white/[0.08] rounded-3xl shadow-2xl overflow-hidden flex flex-col"
        style={{ maxHeight: '88vh' }}
        onClick={e => e.stopPropagation()}
      >
        <div className="absolute top-0 left-0 right-0 h-[2px] bg-gradient-to-r from-accent via-accent/30 to-transparent" />

        <div className="flex items-start justify-between gap-4 px-7 py-6 border-b border-white/[0.06] shrink-0">
          <div className="flex items-center gap-4 min-w-0">
            <div className="w-12 h-12 rounded-2xl bg-accent/10 border border-accent/20 flex items-center justify-center shrink-0 text-accent">
              <Puzzle size={22} />
            </div>
            <div className="min-w-0">
              <h3 className="font-black text-lg text-white uppercase tracking-wider flex items-center gap-2.5">
                Workshop
                <span className="px-2 py-0.5 rounded-md bg-amber-500/15 border border-amber-500/30 text-amber-400 text-[9px] font-black tracking-widest">
                  BETA
                </span>
              </h3>
              <p className="text-xs text-gray-500 font-bold truncate">{displayName}</p>
            </div>
          </div>
          <button onClick={onClose} className="w-9 h-9 rounded-full bg-white/[0.05] hover:bg-white/10 border border-white/10 flex items-center justify-center text-gray-400 hover:text-white transition-colors shrink-0">
            <X size={16} />
          </button>
        </div>

        {/* One line, not a floating italic paragraph: it was the loudest thing
            in the window and said the least. */}
        <div className="px-7 pt-4 shrink-0">
          <p className="flex items-start gap-2 text-[11px] text-amber-400/90 font-medium leading-relaxed bg-amber-500/[0.06] border border-amber-500/20 rounded-xl px-3.5 py-2.5">
            <AlertTriangle size={13} className="shrink-0 mt-[1px]" />
            <span>
              {es
                ? 'Función en beta: puede fallar según el juego o el mod. Depende de un servicio externo, no de Valve.'
                : 'Beta feature: it can fail depending on the game or mod. It relies on a third-party service, not Valve.'}
            </span>
          </p>
        </div>

        <div className="p-7 grid grid-cols-1 lg:grid-cols-[1.05fr_1fr] gap-7 overflow-y-auto custom-scrollbar">
          {/* Left: add a mod */}
          <div className="space-y-3.5 min-w-0">
            <h4 className="flex items-center gap-2.5 text-[10px] font-black uppercase tracking-[0.25em] text-gray-300">
              <span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />
              {es ? 'Agregar mod' : 'Add mod'}
            </h4>

            {/* The field and its button on one line, with no card around them:
                the panel used to be a box inside a box inside the modal. */}
            <div className="space-y-2.5">
              <div className="flex gap-2">
                <input
                  type="text"
                  value={linkInput}
                  onChange={e => setLinkInput(e.target.value)}
                  onKeyDown={e => { if (e.key === 'Enter') handleResolve(); }}
                  placeholder={es ? 'Pegá el link o el ID del ítem' : 'Paste the item link or ID'}
                  className="flex-1 min-w-0 bg-black/40 border border-white/10 rounded-xl px-4 py-3 text-[12px] text-white placeholder-gray-600 focus:outline-none focus:border-accent/50 focus:ring-1 focus:ring-accent/25 transition-all"
                />
                <button
                  onClick={handleResolve}
                  disabled={resolving || !linkInput.trim()}
                  className="shrink-0 px-5 rounded-xl bg-accent hover:brightness-110 text-white font-black text-[11px] uppercase tracking-widest transition-all flex items-center gap-2 disabled:opacity-40 disabled:pointer-events-none shadow-lg shadow-accent/20"
                >
                  {resolving ? <RefreshCw size={13} className="animate-spin" /> : <Search size={13} />}
                  {es ? 'Buscar' : 'Search'}
                </button>
              </div>
              <p className="text-[10px] text-gray-600 font-medium leading-relaxed">
                {es
                  ? 'steamcommunity.com/sharedfiles/filedetails/?id=… o solo el número.'
                  : 'steamcommunity.com/sharedfiles/filedetails/?id=… or just the number.'}
              </p>
            </div>

            <AnimatePresence>
              {item && (
                <motion.div
                  initial={{ opacity: 0, y: 8 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -8 }}
                  transition={{ duration: 0.25, ease: 'easeOut' }}
                  className="rounded-2xl bg-white/[0.03] border border-white/[0.08] p-4 space-y-3.5"
                >
                  <div className="flex items-center gap-3.5">
                    {item.preview_url && (
                      <div className="relative w-16 h-16 rounded-xl overflow-hidden shrink-0 bg-black/60 border border-white/10 shadow-md">
                        <img src={item.preview_url} alt={item.title} className="absolute inset-0 w-full h-full object-cover" />
                      </div>
                    )}
                    <div className="min-w-0">
                      <p className="text-[13px] font-black text-white/90 tracking-tight truncate">{item.title || `Item ${item.item_id}`}</p>
                      <p className="text-[10px] text-gray-500 font-mono mt-0.5">{formatBytes(item.file_size)} · {item.item_id}</p>
                    </div>
                  </div>

                  {mismatched && (
                    <p className="flex items-start gap-2 text-[11px] text-amber-400/90 font-semibold bg-amber-500/10 border border-amber-500/20 rounded-xl p-3">
                      <AlertTriangle size={13} className="shrink-0 mt-[1px]" />
                      <span>
                        {es
                          ? `Este mod dice ser para el AppID ${item.app_id}, no para ${displayName} (${game.id}). Revisá el link antes de instalar.`
                          : `This mod claims to be for AppID ${item.app_id}, not ${displayName} (${game.id}). Double-check the link before installing.`}
                      </span>
                    </p>
                  )}

                  <button
                    onClick={handleInstall}
                    disabled={installing}
                    className="w-full py-3 bg-accent hover:brightness-110 hover:-translate-y-0.5 disabled:opacity-50 disabled:translate-y-0 text-white font-black text-[11px] rounded-xl transition-all uppercase tracking-widest flex items-center justify-center gap-2 shadow-lg shadow-accent/20"
                  >
                    {installing ? (
                      <><RefreshCw size={14} className="animate-spin" /> {es ? 'Instalando…' : 'Installing…'}</>
                    ) : (
                      <><DownloadCloud size={14} /> {es ? 'Instalar' : 'Install'}</>
                    )}
                  </button>

                  {installing && (
                    <p className="text-[10px] text-gray-500 text-center leading-relaxed">
                      {installElapsed < 10
                        ? (es ? 'Descargando…' : 'Downloading…')
                        : (es
                            ? 'Puede tardar un par de minutos la primera vez: SteamCMD se actualiza solo antes de bajar nada.'
                            : 'Can take a couple of minutes the first time: SteamCMD updates itself before downloading anything.')}
                    </p>
                  )}

                  {installedPath && (
                    <div className="p-3 rounded-xl bg-emerald-500/10 border border-emerald-500/20 text-emerald-400 text-[10px] font-mono flex items-center gap-2">
                      <CheckCircle2 size={13} className="shrink-0" />
                      <span className="break-all">{installedPath}</span>
                    </div>
                  )}
                </motion.div>
              )}
            </AnimatePresence>
          </div>

          {/* Right: installed mods */}
          <div className="space-y-3.5 flex flex-col min-h-0 min-w-0">
            <h4 className="flex items-center gap-2.5 text-[10px] font-black uppercase tracking-[0.25em] text-gray-300">
              <span className="w-1 h-3.5 rounded-full bg-accent shadow-[0_0_8px_var(--accent-color-hex)]" />
              {es ? 'Mods instalados' : 'Installed mods'}
              {installedMods.length > 0 && (
                <span className="px-2 py-0.5 rounded-full bg-white/[0.06] border border-white/10 text-[9px] text-gray-400 tabular-nums">
                  {installedMods.length}
                </span>
              )}
            </h4>

            {loadingMods ? (
              <div className="space-y-2.5">
                {[0, 1].map(i => <div key={i} className="h-[66px] rounded-xl bg-white/[0.04] animate-pulse" />)}
              </div>
            ) : installedMods.length === 0 ? (
              // Dashed, so an empty list reads as "nothing here yet" instead of
              // a solid panel that looks like something failed to load.
              <div className="flex-1 flex flex-col items-center justify-center gap-2.5 text-center rounded-2xl border border-dashed border-white/[0.10] px-6 py-10">
                <Puzzle size={26} className="text-white/15" />
                <p className="text-[11px] text-gray-500 font-medium max-w-[220px] leading-relaxed">
                  {es ? 'Todavía no instalaste ningún mod para este juego.' : "You haven't installed any mods for this game yet."}
                </p>
              </div>
            ) : (
              <div className="space-y-2.5 overflow-y-auto custom-scrollbar pr-1" style={{ maxHeight: '420px' }}>
                {installedMods.map(mod => (
                  <div
                    key={mod.item_id}
                    className="group flex items-center gap-3 bg-white/[0.02] hover:bg-white/[0.04] border border-white/[0.06] hover:border-white/[0.12] rounded-xl p-3 transition-colors"
                  >
                    {mod.preview_url && (
                      <div className="relative w-12 h-12 rounded-lg overflow-hidden shrink-0 bg-black/60 border border-white/10">
                        <img src={mod.preview_url} alt={mod.title} className="absolute inset-0 w-full h-full object-cover" />
                      </div>
                    )}
                    <div className="min-w-0 flex-1">
                      <p className="text-[12px] font-black text-white/90 truncate">{mod.title || `Item ${mod.item_id}`}</p>
                      <p className="text-[10px] text-gray-500 font-mono">{formatBytes(mod.size_bytes)}</p>
                    </div>
                    <button
                      onClick={() => handleUninstallMod(mod.item_id)}
                      disabled={uninstallingId === mod.item_id}
                      title={es ? 'Eliminar' : 'Remove'}
                      className="shrink-0 w-8 h-8 rounded-full bg-white/5 hover:bg-red-500/20 border border-white/10 hover:border-red-500/30 flex items-center justify-center text-gray-500 hover:text-red-400 transition-all disabled:opacity-50 opacity-60 group-hover:opacity-100"
                    >
                      {uninstallingId === mod.item_id ? <RefreshCw size={13} className="animate-spin" /> : <Trash2 size={13} />}
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>      </motion.div>
    </motion.div>
  );
});


const CATEGORY_ICONS: Record<string, any> = {
  cache: Cpu,
  download: DownloadCloud,
  install: HardDrive,
  steam: Settings,
  crash: AlertOctagon,
  feature: ZapIcon,
  other: HelpCircle,
};

// Tracks open support tickets (Discord threads spun off from the webhook
// message) so the app can poll discord_check_thread_replies for each one,
// and so the Support tab can show a "Tus Tickets" history with the replies
// that came back — not just a one-off toast when a reply arrives.
type SupportThreadReply = { id: string; author: string; content: string; timestamp: string; attachments: string[]; self?: boolean };
type SupportThreadRecord = {
  threadId: string;
  lastMessageId: string;
  createdAt: number;
  threadName: string;
  replies: SupportThreadReply[];
  archived: boolean;
  unreadCount: number;
  originalMessage: string;
  originalCategories: string[];
};
const loadSupportThreads = (): SupportThreadRecord[] => {
  try {
    const parsed = JSON.parse(localStorage.getItem('rl_support_threads') ?? '[]');
    // Back-compat: records saved before newer fields existed.
    return (Array.isArray(parsed) ? parsed : []).map((t: any) => ({
      threadId: t.threadId,
      lastMessageId: t.lastMessageId,
      createdAt: t.createdAt,
      threadName: t.threadName ?? 'Ticket',
      replies: Array.isArray(t.replies) ? t.replies.map((r: any) => ({ ...r, attachments: Array.isArray(r.attachments) ? r.attachments : [] })) : [],
      archived: !!t.archived,
      unreadCount: typeof t.unreadCount === 'number' ? t.unreadCount : 0,
      originalMessage: t.originalMessage ?? '',
      originalCategories: Array.isArray(t.originalCategories) ? t.originalCategories : [],
    }));
  } catch { return []; }
};
// "hace 5m" / "hace 3h" style relative time, matching the notification bell.
const relativeTime = (ms: number): string => {
  const diff = Date.now() - ms;
  if (diff < 60_000) return 'ahora';
  if (diff < 3_600_000) return `hace ${Math.floor(diff / 60_000)}m`;
  if (diff < 86_400_000) return `hace ${Math.floor(diff / 3_600_000)}h`;
  return `hace ${Math.floor(diff / 86_400_000)}d`;
};
// A short two-tone chime for "developer replied" — synthesized so there's no
// audio asset to bundle/load.
const playSupportReplySound = () => {
  try {
    const ctx = new (window.AudioContext || (window as any).webkitAudioContext)();
    const now = ctx.currentTime;
    [880, 1320].forEach((freq, i) => {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = 'sine';
      osc.frequency.value = freq;
      const start = now + i * 0.12;
      gain.gain.setValueAtTime(0, start);
      gain.gain.linearRampToValueAtTime(0.18, start + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.001, start + 0.32);
      osc.connect(gain).connect(ctx.destination);
      osc.start(start);
      osc.stop(start + 0.34);
    });
    setTimeout(() => ctx.close().catch(() => {}), 700);
  } catch { /* Web Audio unavailable — skip the sound, notification still shows */ }
};
const saveSupportThreads = (threads: SupportThreadRecord[]) => {
  localStorage.setItem('rl_support_threads', JSON.stringify(threads));
  window.dispatchEvent(new Event('rl-support-threads-updated'));
};

const SupportView = () => {
  const { t, lang } = useLanguage();
  const ti = useTranslateInline();
  const { notify } = useNotify();
  // Pre-filled once from a crash detected on startup (see the App
  // component's pending-crash-report check) — read-and-clear so it's only
  // ever offered this one time, even if Support gets remounted later.
  const [message, setMessage] = useState(() => {
    const pending = localStorage.getItem('rl_pending_crash_report');
    if (pending) localStorage.removeItem('rl_pending_crash_report');
    return pending ?? '';
  });
  const [selected, setSelected] = useState<string[]>([]);
  const [reportAttachments, setReportAttachments] = useState<string[]>([]);
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState(false);
  const [tickets, setTickets] = useState<SupportThreadRecord[]>(() => loadSupportThreads());
  const [expandedTicket, setExpandedTicket] = useState<string | null>(null);
  const [ticketSearch, setTicketSearch] = useState('');
  const [replyText, setReplyText] = useState('');
  const [replyAttachments, setReplyAttachments] = useState<string[]>([]);
  const [sendingReply, setSendingReply] = useState(false);
  const [replyError, setReplyError] = useState<string | null>(null);
  const [resolvingTicket, setResolvingTicket] = useState<string | null>(null);
  const [lightboxImg, setLightboxImg] = useState<string | null>(null);

  const handleAttachFile = async () => {
    const picked = await open({
      multiple: true,
      filters: [
        { name: ti('Imágenes', 'Images'), extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp'] },
        { name: ti('Registros de texto', 'Text logs'), extensions: ['txt', 'log', 'json'] },
        { name: ti('Archivos comprimidos', 'Archives'), extensions: ['zip', 'rar', '7z'] },
        { name: ti('Todos los archivos', 'All files'), extensions: ['*'] },
      ],
    });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    setReplyAttachments(prev => [...prev, ...paths].slice(0, 5));
  };

  const handleAttachReportFile = async () => {
    const picked = await open({
      multiple: true,
      filters: [
        { name: ti('Imágenes', 'Images'), extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp'] },
        { name: ti('Todos los archivos', 'All files'), extensions: ['*'] },
      ],
    });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    setReportAttachments(prev => [...prev, ...paths].slice(0, 3));
  };

  const handleSendReply = async (ticket: SupportThreadRecord) => {
    if (!replyText.trim() && replyAttachments.length === 0) return;
    setSendingReply(true);
    setReplyError(null);
    try {
      const sentReply = await invoke<{ id: string; timestamp: string; attachments: string[] }>('discord_send_thread_reply', {
        threadId: ticket.threadId,
        content: replyText.trim(),
        attachmentPaths: replyAttachments,
      });
      const threads = loadSupportThreads();
      const idx = threads.findIndex(t => t.threadId === ticket.threadId);
      if (idx !== -1) {
        threads[idx] = {
          ...threads[idx],
          lastMessageId: sentReply.id,
          replies: [...threads[idx].replies, {
            id: sentReply.id,
            author: ti('Tú', 'You'),
            content: replyText.trim(),
            timestamp: sentReply.timestamp,
            attachments: sentReply.attachments,
            self: true,
          }],
        };
        saveSupportThreads(threads);
      }
      setReplyText('');
      setReplyAttachments([]);
    } catch (err) {
      // Keep the draft (text + attachments) so "Reintentar" can just resend
      // it — nothing typed is lost on a failed send.
      setReplyError(ti(`No se pudo enviar: ${err}`, `Failed to send: ${err}`));
    } finally {
      setSendingReply(false);
    }
  };

  const handleResolveTicket = async (ticket: SupportThreadRecord) => {
    setResolvingTicket(ticket.threadId);
    try {
      await invoke('discord_archive_thread', { threadId: ticket.threadId });
    } catch (err) {
      // Still close it locally even if the Discord-side archive call failed
      // (e.g. no connection) — the user asked to be done with it either way.
      notify(ti(`No se pudo archivar el hilo en Discord (se cerró igual localmente): ${err}`, `Could not archive the Discord thread (closed locally anyway): ${err}`), 'error');
    } finally {
      const threads = loadSupportThreads();
      const idx = threads.findIndex(t => t.threadId === ticket.threadId);
      if (idx !== -1) {
        threads[idx] = { ...threads[idx], archived: true };
        saveSupportThreads(threads);
      }
      setResolvingTicket(null);
    }
  };

  // Purely a local dismissal — it doesn't touch the Discord thread itself,
  // just stops this device from tracking/polling it.
  const handleDeleteTicket = (threadId: string) => {
    const threads = loadSupportThreads().filter(t => t.threadId !== threadId);
    saveSupportThreads(threads);
    if (expandedTicket === threadId) setExpandedTicket(null);
  };

  const handleToggleTicket = (ticket: SupportThreadRecord) => {
    const opening = expandedTicket !== ticket.threadId;
    setExpandedTicket(opening ? ticket.threadId : null);
    setReplyError(null);
    if (opening && ticket.unreadCount > 0) {
      const threads = loadSupportThreads();
      const idx = threads.findIndex(t => t.threadId === ticket.threadId);
      if (idx !== -1) {
        threads[idx] = { ...threads[idx], unreadCount: 0 };
        saveSupportThreads(threads);
      }
    }
  };

  const isImageAttachment = (nameOrUrl: string) => /\.(png|jpe?g|gif|webp|bmp)(\?|$)/i.test(nameOrUrl);
  const attachmentFileName = (nameOrUrl: string) => nameOrUrl.split('/').pop()?.split('?')[0] ?? nameOrUrl;

  // The background poller (in the main App component) updates
  // rl_support_threads whenever it checks for replies — refresh from
  // localStorage whenever that happens so replies show up here live,
  // not just as a bell notification.
  useEffect(() => {
    const refresh = () => setTickets(loadSupportThreads());
    refresh();
    window.addEventListener('rl-support-threads-updated', refresh);
    return () => window.removeEventListener('rl-support-threads-updated', refresh);
  }, []);

  useEffect(() => {
    if (!lightboxImg) return;
    const handler = (e: KeyboardEvent) => { if (e.key === 'Escape') setLightboxImg(null); };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [lightboxImg]);

  const CATEGORY_KEYS = ['cache', 'download', 'install', 'steam', 'crash', 'feature', 'other'] as const;

  // What to try before sending, per issue type. Most of these reports are
  // fixed by a button the app already has.
  const SUPPORT_TIPS: Record<string, { es: string; en: string }> = {
    cache: {
      es: 'Cierra y vuelve a abrir el programa. Si sigue sin cargar, prueba una vez con una VPN: a veces el proveedor de internet bloquea el catálogo.',
      en: 'Close and reopen the app. If it still does not load, try a VPN once: some internet providers block the catalog.',
    },
    download: {
      es: 'Abre la ficha del juego: el apartado Estado dice si falta Steam, el plugin o un manifiesto.',
      en: "Open the game's page: its Status section says whether Steam, the plugin or a manifest is missing.",
    },
    install: {
      es: 'En Biblioteca, menú ⋯ del juego → "Reparar / reinstalar", o "Reparar todos" si tiene un aviso.',
      en: 'In Library, the game\'s ⋯ menu → "Repair / reinstall", or "Repair all" if it shows a notice.',
    },
    steam: {
      es: 'En Inicio: "Ejecutar reparador" si Steam no abre, o "Reparar Plugin".',
      en: 'On Home: "Run repair" if Steam will not open, or "Repair Plugin".',
    },
    crash: {
      es: 'Deja marcado "Adjuntar el registro del programa": es lo único que dice por qué se cerró.',
      en: 'Keep "Attach the app log" checked: it is the only thing that says why it closed.',
    },
  };
  // The last lines of ragnarok.log go with the report unless unticked.
  const [attachLog, setAttachLog] = useState(true);

  const toggle = (id: string) =>
    setSelected(prev => prev.includes(id) ? prev.filter(c => c !== id) : [...prev, id]);

  const handleSend = async () => {
    const webhook = DISCORD_SUPPORT_WEBHOOK;
    if (!webhook) return;
    if (selected.length === 0 && !message.trim()) {
      notify(ti('Selecciona un tipo de problema y escribe una descripción para poder enviar.', 'Select an issue type and write a description before sending.'), 'error');
      return;
    }
    if (selected.length === 0) {
      notify(ti('Selecciona al menos un tipo de problema para poder enviar.', 'Select at least one issue type before sending.'), 'error');
      return;
    }
    if (!message.trim()) {
      notify(ti('Escribe una descripción para poder enviar.', 'Write a description before sending.'), 'error');
      return;
    }
    setSending(true);
    try {
      const categoryLabels = CATEGORY_KEYS.filter(k => selected.includes(k)).map(k => t.support.categories[k]);
      const labels = categoryLabels.map(l => `• ${l}`).join('\n');

      // A short code both sides can quote, shown in the report, the thread
      // name and the footer. No look-alike characters (0/O, 1/I).
      const ticketId = 'RL-' + Array.from({ length: 4 }, () => 'ABCDEFGHJKMNPQRSTUVWXYZ23456789'[Math.floor(Math.random() * 31)]).join('');
      // The colour says how urgent it reads at a glance in the channel.
      const SEVERITY: Array<[string, number]> = [
        ['crash', 0xef4444], ['download', 0xf59e0b], ['install', 0xf59e0b], ['steam', 0xf59e0b],
        ['cache', 0x8b5cf6], ['feature', 0x3b82f6], ['other', 0x6b7280],
      ];
      const color = (SEVERITY.find(([k]) => (selected as string[]).includes(k)) ?? SEVERITY[SEVERITY.length - 1])[1];
      const sys = await invoke<{ os: string; ram_gb: number; cpu: string }>('support_system_summary').catch(() => null);

      const isImage = (path: string) => /\.(png|jpe?g|gif|webp)$/i.test(path);
      const imageCount = reportAttachments.filter(isImage).length;
      const otherCount = reportAttachments.length - imageCount;
      const attachSummary = [
        imageCount ? ti(`${imageCount} captura${imageCount > 1 ? 's' : ''}`, `${imageCount} screenshot${imageCount > 1 ? 's' : ''}`) : '',
        otherCount ? ti(`${otherCount} archivo${otherCount > 1 ? 's' : ''}`, `${otherCount} file${otherCount > 1 ? 's' : ''}`) : '',
        attachLog ? ti('registro en el hilo', 'log in the thread') : '',
      ].filter(Boolean).join(' · ');

      const text = message.trim();
      const body = {
        embeds: [{
          title: `🐛 Reporte de soporte · ${ticketId}`,
          color,
          // The description has room for 4096 characters; a field only takes
          // 1024, and a longer message made Discord refuse the whole report.
          description: `>>> ${text.length > 3500 ? text.slice(0, 3500) + '…' : text}`,
          fields: [
            { name: '📋 Categorías', value: labels || '—', inline: false },
            { name: '🧩 Versión', value: `v${__APP_VERSION__}`, inline: true },
            { name: '📦 Fuente', value: readCatalogSource().catalogSource === 'hubcap' ? 'Hubcap' : 'Ryuu', inline: true },
            { name: '🌐 Idioma', value: lang.toUpperCase(), inline: true },
            ...(sys ? [
              { name: '🪟 Sistema', value: sys.os, inline: true },
              { name: '🧠 Memoria', value: `${sys.ram_gb} GB`, inline: true },
              { name: '⚙️ Procesador', value: sys.cpu.slice(0, 100), inline: true },
            ] : []),
            ...(attachSummary ? [{ name: '📎 Adjuntos', value: attachSummary, inline: false }] : []),
          ],
          // Filled in by the backend with the first screenshot's name.
          ...(imageCount ? { image: { url: 'attachment://__FIRST_IMAGE__' } } : {}),
          footer: { text: `Ragnarok Launcher v${__APP_VERSION__} · ${ticketId}` },
          timestamp: new Date().toISOString(),
        }],
      };
      const logPath = attachLog ? await invoke<string>('support_log_excerpt').catch(() => null) : null;
      // Goes through a Rust command instead of a plain fetch() so it can
      // also attach files — a raw JSON POST to the webhook can't include
      // attachments, which is why there used to be no way to attach a
      // screenshot to the very first report (only replies to an already-
      // open ticket could). ?wait=true still makes the webhook return the
      // posted message (id + channel_id), needed to spin this into its own
      // Discord thread so developer replies can be polled back into the app.
      const posted = await invoke<{ id: string; channel_id: string }>('discord_send_support_webhook', {
        webhookUrl: webhook,
        payloadJson: JSON.stringify(body),
        attachmentPaths: reportAttachments,
      }).catch(err => { throw new Error(String(err)); });
      if (posted?.id && posted?.channel_id) {
        try {
          const threadName = `${ticketId} · ${labels ? labels.replace(/\n/g, ', ').replace(/• /g, '') : 'Reporte'} — ${message.trim().slice(0, 50)}`;
          const threadId = await invoke<string>('discord_create_support_thread', {
            channelId: posted.channel_id,
            messageId: posted.id,
            threadName,
          });
          // The log goes into the thread rather than the report itself: Discord
          // prints a preview of a .txt right in the channel, and 300 lines of
          // it buried the report it belonged to.
          if (logPath) {
            await invoke('discord_send_thread_reply', {
              threadId,
              content: `📎 **Registro del programa** · ${ticketId}\nÚltimas líneas, sin claves ni tokens.`,
              attachmentPaths: [logPath],
            }).catch(() => { });
          }
          const threads = loadSupportThreads();
          threads.push({
            threadId,
            lastMessageId: posted.id,
            createdAt: Date.now(),
            threadName,
            replies: [],
            archived: false,
            unreadCount: 0,
            originalMessage: message.trim(),
            originalCategories: categoryLabels,
          });
          saveSupportThreads(threads);
        } catch (threadErr) {
          // The ticket itself went through, so this is not a failed send —
          // but it is no longer the silent non-event it used to be.
          //
          // Replies are read out of the Discord thread this call creates. No
          // thread means there is nowhere for an answer to land: the user
          // waits for a reply that can never arrive, and support sees a
          // report with no way to respond to it. The only trace was a
          // console.error, and a release build has no console — the same
          // reason diag.rs exists.
          // Without a thread the log has nowhere to go but the channel, which
          // is still better than losing it.
          if (logPath) {
            invoke('discord_send_support_webhook', {
              webhookUrl: webhook,
              payloadJson: JSON.stringify({ content: `📎 **Registro del programa** · ${ticketId}` }),
              attachmentPaths: [logPath],
            }).catch(() => { });
          }
          console.error('Failed to create support thread:', threadErr);
          invoke('log_diagnostic', {
            message: `[support] No se pudo crear el hilo de respuestas: ${threadErr}`,
          }).catch(() => { });
          // The build-configuration case gets its own wording. Telling the
          // person who compiled it to "reach us on Discord" is useless
          // advice — the fix is in their hands, not ours.
          const notConfigured = String(threadErr).includes('DISCORD_API_BASE');
          notify(
            notConfigured
              ? String(threadErr)
              : ti(
                  'Tu reporte se envió, pero no se pudo abrir el canal de respuestas: no vas a recibir la contestación dentro del programa. Escribinos por Discord y mencioná este reporte.',
                  "Your report was sent, but the reply channel couldn't be opened: you won't get the answer inside the app. Reach us on Discord and mention this report."
                ),
            'error'
          );
        }
      }
      setSent(true);
      setMessage('');
      setSelected([]);
      setReportAttachments([]);
      notify(t.support.sent, 'success');
      setTimeout(() => setSent(false), 4000);
    } catch (err) {
      notify(`Error: ${err}`, 'error');
    }
    setSending(false);
  };

  const canSend = !!DISCORD_SUPPORT_WEBHOOK && message.trim().length > 0 && selected.length > 0;

  if (sent) {
    return (
      <motion.div
        initial={{ opacity: 0, scale: 0.9 }}
        animate={{ opacity: 1, scale: 1 }}
        className="flex flex-col items-center justify-center h-[60vh] gap-6"
      >
        <div className="relative">
          <div className="absolute inset-0 bg-green-500 blur-3xl opacity-30 animate-pulse" />
          <div className="w-24 h-24 rounded-full bg-green-500/10 border-2 border-green-500 flex items-center justify-center relative z-10">
            <CheckCircle2 size={44} className="text-green-400" />
          </div>
        </div>
        <div className="text-center space-y-2">
          <h3 className="text-2xl font-black text-white uppercase tracking-widest">¡Enviado!</h3>
          <p className="text-gray-400 text-sm">{t.support.sent}</p>
        </div>
      </motion.div>
    );
  }

  return (
    <div className="space-y-6 animate-in fade-in slide-in-from-bottom-4 duration-700">

      {/* Header */}
      <motion.div
        initial={{ opacity: 0, y: 14 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.35, ease: 'easeOut' }}
        className="relative overflow-hidden rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-7"
      >
        <div
          className="absolute inset-0 pointer-events-none opacity-[0.04]"
          style={{ backgroundImage: 'linear-gradient(rgba(255,255,255,0.6) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.6) 1px, transparent 1px)', backgroundSize: '28px 28px' }}
        />
        <div className="absolute -right-14 -top-14 w-56 h-56 bg-accent/10 blur-[70px] rounded-full pointer-events-none" />
        <div className="relative z-10 flex items-center gap-5">
          <div className="relative w-16 h-16 shrink-0 flex items-center justify-center">
            <motion.div
              className="absolute inset-0 rounded-2xl bg-accent blur-lg"
              animate={{ opacity: [0.55, 0.2, 0.55] }}
              transition={{ duration: 2.2, repeat: Infinity, ease: 'easeInOut' }}
            />
            <div className="relative z-10 w-16 h-16 rounded-2xl bg-accent border border-accent/40 flex items-center justify-center shadow-lg shadow-accent/25">
              <MessageSquare size={26} className="text-white" />
            </div>
          </div>
          <div>
            <h3 className="text-xl font-black text-white/90 tracking-tight">Centro de Soporte</h3>
            <p className="text-sm text-gray-400 font-medium mt-1">{t.support.subtitle}</p>
          </div>
        </div>
      </motion.div>

      {/* Form on the left, tickets on the right: it was one narrow column
          with the tickets pushing the form further down. */}
      <div className="grid grid-cols-1 xl:grid-cols-[minmax(0,1fr)_minmax(0,440px)] gap-6 items-start">
        <div className="space-y-6 min-w-0">
          {/* Categories */}
          <div className="space-y-4">
            <div className="flex items-center gap-4">
              <div className="h-[1px] flex-1 bg-gradient-to-r from-white/10 to-transparent" />
              <h3 className="text-xs font-black uppercase tracking-[0.3em] text-gray-500 flex items-center gap-2">
                <Bug size={12} />
                {t.support.typeLabel}
              </h3>
              <div className="h-[1px] flex-1 bg-gradient-to-l from-white/10 to-transparent" />
            </div>
            <div className="relative overflow-hidden bg-[#0d0e12] border border-white/[0.08] rounded-2xl p-4">
              <div className="absolute -left-10 -bottom-16 w-48 h-48 bg-accent/[0.06] blur-[70px] rounded-full pointer-events-none" />
              <div className="relative z-10 grid grid-cols-1 sm:grid-cols-2 2xl:grid-cols-3 gap-2.5">
                {CATEGORY_KEYS.map(key => {
                  const active = selected.includes(key);
                  const Icon = CATEGORY_ICONS[key] ?? HelpCircle;
                  return (
                    <motion.button
                      key={key}
                      onClick={() => toggle(key)}
                      whileHover={{ y: -2 }}
                      whileTap={{ scale: 0.98 }}
                      className={`flex items-center gap-3 px-4 py-3 rounded-xl transition-colors duration-300 text-left border group relative overflow-hidden ${
                        active
                          ? 'bg-accent/15 border-accent/50 text-white shadow-lg shadow-accent/20'
                          : 'bg-white/[0.03] border-white/[0.08] text-gray-300 hover:bg-white/[0.06] hover:border-white/[0.15] hover:text-white'
                      }`}
                    >
                      {active && <div className="absolute inset-0 bg-gradient-to-r from-accent/10 to-transparent" />}
                      <div className={`w-9 h-9 rounded-xl flex items-center justify-center shrink-0 transition-all duration-300 relative z-10 ${
                        active ? 'bg-accent text-white shadow-md shadow-accent/40' : 'bg-white/[0.06] text-gray-400 group-hover:bg-white/[0.1] group-hover:text-white'
                      }`}>
                        <Icon size={16} />
                      </div>
                      <span className="text-sm font-bold relative z-10">{t.support.categories[key]}</span>
                      {active && (
                        <div className="ml-auto relative z-10">
                          <CheckCircle2 size={16} className="text-accent" />
                        </div>
                      )}
                    </motion.button>
                  );
                })}
              </div>
            </div>
          </div>

          {/* Quick fixes for the chosen issue types */}
          {selected.some(k => SUPPORT_TIPS[k]) && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.3, ease: 'easeOut' }}
              className="rounded-2xl bg-accent/[0.06] border border-accent/25 p-5 space-y-2.5"
            >
              <p className="text-[10px] font-black uppercase tracking-widest text-accent flex items-center gap-2">
                <Zap size={12} />
                {ti('Antes de enviar, prueba esto', 'Before sending, try this')}
              </p>
              <ul className="space-y-1.5">
                {selected.filter(k => SUPPORT_TIPS[k]).map(k => (
                  <li key={k} className="text-[13px] text-gray-300 leading-snug flex gap-2">
                    <span className="text-accent shrink-0">•</span>
                    {ti(SUPPORT_TIPS[k].es, SUPPORT_TIPS[k].en)}
                  </li>
                ))}
              </ul>
            </motion.div>
          )}

          {/* Description */}
          <div className="space-y-4">
            <div className="flex items-center gap-4">
              <div className="h-[1px] flex-1 bg-gradient-to-r from-white/10 to-transparent" />
              <h3 className="text-xs font-black uppercase tracking-[0.3em] text-gray-500">{t.support.descLabel}</h3>
              <div className="h-[1px] flex-1 bg-gradient-to-l from-white/10 to-transparent" />
            </div>
            <div className="relative bg-[#0d0e12] border border-white/[0.08] rounded-2xl p-2">
              <textarea
                value={message}
                onChange={e => setMessage(e.target.value)}
                placeholder={t.support.descPlaceholder}
                rows={5}
                className="w-full bg-black/40 border border-white/10 focus:border-accent/50 focus:ring-1 focus:ring-accent/25 rounded-xl p-5 text-sm transition-all duration-300 resize-none placeholder:text-gray-600 outline-none text-gray-200"
              />
              {message.length > 0 && (
                <div className="absolute bottom-4 right-4 text-[10px] text-gray-600 font-mono">
                  {message.length} chars
                </div>
              )}
            </div>

            {/* Attachments */}
            <div className="space-y-2">
              {reportAttachments.length > 0 && (
                <div className="flex flex-wrap gap-2">
                  {reportAttachments.map((path, i) => (
                    <div key={path + i} className="relative group">
                      {isImageAttachment(path) ? (
                        <div className="w-16 h-16 rounded-lg overflow-hidden border border-white/10">
                          <img src={convertFileSrc(path)} alt="" className="w-full h-full object-cover" />
                        </div>
                      ) : (
                        <div className="w-16 h-16 rounded-lg border border-white/10 bg-white/[0.04] flex flex-col items-center justify-center gap-0.5 px-1">
                          <FileText size={16} className="text-gray-500 shrink-0" />
                          <span className="text-[7px] text-gray-500 font-bold truncate max-w-full">{attachmentFileName(path)}</span>
                        </div>
                      )}
                      <button
                        onClick={() => setReportAttachments(prev => prev.filter((_, idx) => idx !== i))}
                        className="absolute inset-0 bg-black/60 opacity-0 group-hover:opacity-100 flex items-center justify-center transition-opacity rounded-lg"
                      >
                        <X size={14} className="text-white" />
                      </button>
                    </div>
                  ))}
                </div>
              )}
              <button
                onClick={handleAttachReportFile}
                disabled={reportAttachments.length >= 3}
                className="flex items-center gap-2 px-4 py-2.5 rounded-xl bg-white/[0.05] hover:bg-white/[0.09] border border-white/[0.08] text-[11px] font-bold text-gray-400 hover:text-white transition-all disabled:opacity-40 disabled:cursor-not-allowed hover:-translate-y-0.5"
              >
                <ImagePlus size={14} />
                {ti('Adjuntar captura', 'Attach screenshot')}
              </button>

              {/* The app log, secrets covered on the backend before it is written. */}
              <button
                onClick={() => setAttachLog(v => !v)}
                className="w-full flex items-start gap-3 rounded-xl bg-[#0d0e12] border border-white/[0.08] hover:border-white/[0.14] px-4 py-3 text-left transition-colors"
              >
                <span className={`mt-0.5 w-5 h-5 shrink-0 rounded-md border flex items-center justify-center transition-colors ${attachLog ? 'bg-accent border-accent' : 'border-white/20 bg-black/30'}`}>
                  {attachLog && <Check size={12} strokeWidth={3} className="text-white" />}
                </span>
                <span className="min-w-0">
                  <span className="block text-[13px] font-bold text-white/90">
                    {ti('Adjuntar el registro del programa', 'Attach the app log')}
                    <span className="ml-2 text-[9px] font-black uppercase tracking-widest text-accent">{ti('Recomendado', 'Recommended')}</span>
                  </span>
                  <span className="block text-[11px] text-gray-500 mt-0.5 leading-snug">
                    {ti(
                      'Las últimas líneas de lo que hizo el programa: dicen qué falló. Tus claves y tokens se tapan antes de enviarlo.',
                      'The last lines of what the app did: they say what failed. Your keys and tokens are covered before it is sent.'
                    )}
                  </span>
                </span>
              </button>
            </div>
          </div>

          {/* Send Button */}
          <div className="space-y-3">
            {DISCORD_SUPPORT_WEBHOOK && !canSend && (
              <div className="flex items-center gap-2.5 rounded-xl bg-amber-500/10 border border-amber-500/25 px-4 py-3">
                <AlertTriangle size={15} className="text-amber-400 shrink-0" />
                <p className="text-xs font-bold text-amber-300">
                  {selected.length === 0 && !message.trim()
                    ? ti('Selecciona un tipo de problema y escribe una descripción para poder enviar.', 'Select an issue type and write a description before sending.')
                    : selected.length === 0
                      ? ti('Selecciona al menos un tipo de problema para poder enviar.', 'Select at least one issue type before sending.')
                      : ti('Escribe una descripción para poder enviar.', 'Write a description before sending.')}
                </p>
              </div>
            )}
            <button
              onClick={handleSend}
              disabled={!DISCORD_SUPPORT_WEBHOOK || sending}
              className={`w-full py-4 bg-accent hover:brightness-110 disabled:cursor-not-allowed text-white font-black text-sm rounded-2xl transition-all duration-300 uppercase tracking-widest flex items-center justify-center gap-3 shadow-lg shadow-accent/25 hover:-translate-y-0.5 ${!canSend ? 'opacity-40 grayscale-[0.3]' : ''} disabled:opacity-30`}
            >
              {sending ? (
                <><div className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" /> Enviando...</>
              ) : (
                <><Send size={16} /> {t.support.sendBtn}</>
              )}
            </button>
            {!DISCORD_SUPPORT_WEBHOOK && (
              <p className="text-xs text-yellow-500/60 text-center">{t.support.noWebhook}</p>
            )}
            {canSend && (
              <p className="text-xs text-gray-600 text-center">
                {selected.length} categoría{selected.length !== 1 ? 's' : ''} seleccionada{selected.length !== 1 ? 's' : ''}
              </p>
            )}
          </div>
        </div>
        <div className="min-w-0 space-y-4 xl:sticky xl:top-4">
          {/* Tus Tickets — history of sent reports + any developer replies */}
          {tickets.length > 0 && (
            <div className="space-y-4">
              <div className="flex items-center gap-4">
                <div className="h-[1px] flex-1 bg-gradient-to-r from-white/10 to-transparent" />
                <h3 className="text-xs font-black uppercase tracking-[0.3em] text-gray-500 flex items-center gap-2">
                  <MessageSquare size={12} />
                  {ti('Tus Tickets', 'Your Tickets')}
                </h3>
                <div className="h-[1px] flex-1 bg-gradient-to-l from-white/10 to-transparent" />
              </div>

              {tickets.length > 3 && (
                <div className="relative">
                  <div className="absolute inset-y-0 left-4 flex items-center pointer-events-none text-gray-500">
                    <Search size={14} />
                  </div>
                  <input
                    type="text"
                    value={ticketSearch}
                    onChange={e => setTicketSearch(e.target.value)}
                    placeholder={ti('Buscar en tus tickets...', 'Search your tickets...')}
                    spellCheck={false}
                    autoComplete="off"
                    className="w-full bg-[#0d0e12] border border-white/[0.08] focus:border-accent/50 focus:ring-1 focus:ring-accent/25 rounded-xl py-3 pl-10 pr-4 text-xs text-white/90 placeholder:text-gray-600 outline-none transition-all font-bold"
                  />
                </div>
              )}

              <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] divide-y divide-white/[0.06] overflow-hidden">
                {(() => {
                  const q = ticketSearch.trim().toLowerCase();
                  const filteredTickets = q
                    ? tickets.filter(t => t.threadName.toLowerCase().includes(q) || t.originalMessage.toLowerCase().includes(q))
                    : tickets;
                  if (filteredTickets.length === 0) {
                    return (
                      <p className="text-xs text-gray-600 font-bold text-center py-8">
                        {ti(`Sin resultados para "${ticketSearch}".`, `No results for "${ticketSearch}".`)}
                      </p>
                    );
                  }
                  return [...filteredTickets]
                    .sort((a, b) => {
                      const lastA = a.replies.length > 0 ? new Date(a.replies[a.replies.length - 1].timestamp).getTime() : a.createdAt;
                      const lastB = b.replies.length > 0 ? new Date(b.replies[b.replies.length - 1].timestamp).getTime() : b.createdAt;
                      return lastB - lastA;
                    })
                    .map(ticket => {
                  const isOpen = expandedTicket === ticket.threadId;
                  return (
                    <div key={ticket.threadId} className="group/ticket">
                      <div
                        role="button"
                        onClick={() => handleToggleTicket(ticket)}
                        className="w-full flex items-center gap-4 px-5 py-4 text-left hover:bg-white/[0.03] transition-colors cursor-pointer"
                      >
                        <div className={`w-9 h-9 rounded-xl flex items-center justify-center shrink-0 ${ticket.archived ? 'bg-white/[0.06] border border-white/10' : 'bg-accent/15 border border-accent/25'}`}>
                          <MessageSquare size={15} className={ticket.archived ? 'text-gray-500' : 'text-accent'} />
                        </div>
                        <div className="flex-1 min-w-0">
                          <p className="text-sm font-bold text-white/90 truncate">{ticket.threadName}</p>
                          <p className="text-[10px] text-gray-500 font-semibold mt-0.5">
                            {relativeTime(ticket.createdAt)}
                          </p>
                        </div>
                        <span className={`shrink-0 px-2.5 py-1 rounded-full text-[9px] font-black uppercase tracking-wider ${
                          ticket.archived ? 'bg-white/[0.06] text-gray-500' : 'bg-emerald-500/10 text-emerald-400 border border-emerald-500/20'
                        }`}>
                          {ticket.archived ? ti('Cerrado', 'Closed') : ti('Abierto', 'Open')}
                        </span>
                        {ticket.unreadCount > 0 && (
                          <span className="shrink-0 w-5 h-5 rounded-full bg-accent text-white text-[9px] font-black flex items-center justify-center">
                            {ticket.unreadCount}
                          </span>
                        )}
                        <button
                          onClick={e => { e.stopPropagation(); handleDeleteTicket(ticket.threadId); }}
                          title={ti('Quitar ticket', 'Remove ticket')}
                          className="shrink-0 w-7 h-7 rounded-lg flex items-center justify-center text-gray-600 hover:text-red-400 hover:bg-red-500/10 opacity-0 group-hover/ticket:opacity-100 transition-all"
                        >
                          <Trash2 size={13} />
                        </button>
                        <ChevronDown size={14} className={`shrink-0 text-gray-500 transition-transform ${isOpen ? 'rotate-180' : ''}`} />
                      </div>
                      <AnimatePresence>
                        {isOpen && (
                          <motion.div
                            initial={{ height: 0, opacity: 0 }}
                            animate={{ height: 'auto', opacity: 1 }}
                            exit={{ height: 0, opacity: 0 }}
                            transition={{ duration: 0.2, ease: 'easeOut' }}
                            className="overflow-hidden"
                          >
                            <div className="px-5 pb-4 space-y-2.5">
                              {ticket.originalMessage && (
                                <div className="rounded-xl bg-white/[0.02] border border-dashed border-white/[0.1] px-4 py-3">
                                  <div className="flex items-center justify-between gap-2 mb-1.5">
                                    <span className="text-[9px] font-black uppercase tracking-widest text-gray-500">
                                      {ti('Tu reporte inicial', 'Your original report')}
                                    </span>
                                    <span className="text-[9px] text-gray-600 font-bold">{relativeTime(ticket.createdAt)}</span>
                                  </div>
                                  {ticket.originalCategories.length > 0 && (
                                    <div className="flex flex-wrap gap-1.5 mb-2">
                                      {ticket.originalCategories.map(cat => (
                                        <span key={cat} className="px-2 py-0.5 rounded-full bg-white/[0.05] border border-white/10 text-[9px] font-bold text-gray-400 uppercase tracking-wider">{cat}</span>
                                      ))}
                                    </div>
                                  )}
                                  <p className="text-xs text-gray-300 leading-relaxed whitespace-pre-wrap">{ticket.originalMessage}</p>
                                </div>
                              )}
                              {ticket.replies.length === 0 ? (
                                <p className="text-xs text-gray-600 font-semibold py-2">
                                  {ti('Todavía no hay respuestas.', 'No replies yet.')}
                                </p>
                              ) : (
                                ticket.replies.map(reply => (
                                  <div key={reply.id} className={`rounded-xl border px-4 py-3 ${reply.self ? 'bg-accent/[0.06] border-accent/20' : 'bg-white/[0.03] border-white/[0.06]'}`}>
                                    <div className="flex items-center justify-between gap-2 mb-1">
                                      <span className={`text-[11px] font-black ${reply.self ? 'text-white/80' : 'text-accent'}`}>{reply.author}</span>
                                      <span className="text-[9px] text-gray-600 font-bold" title={new Date(reply.timestamp).toLocaleString()}>{relativeTime(new Date(reply.timestamp).getTime())}</span>
                                    </div>
                                    {reply.content && (
                                      <p className="text-xs text-gray-300 leading-relaxed whitespace-pre-wrap">{reply.content}</p>
                                    )}
                                    {reply.attachments.length > 0 && (
                                      <div className="flex flex-wrap gap-2 mt-2">
                                        {reply.attachments.map(url => (
                                          isImageAttachment(url) ? (
                                            <img
                                              key={url}
                                              src={url}
                                              alt="attachment"
                                              onClick={() => setLightboxImg(url)}
                                              className="w-24 h-24 object-cover rounded-lg border border-white/10 cursor-pointer hover:opacity-80 transition-opacity"
                                            />
                                          ) : (
                                            <a
                                              key={url}
                                              href={url}
                                              target="_blank"
                                              rel="noreferrer"
                                              className="flex items-center gap-2 px-3 py-2 rounded-lg bg-white/[0.04] hover:bg-white/[0.08] border border-white/10 text-[11px] font-bold text-gray-300 hover:text-white transition-colors max-w-[180px]"
                                            >
                                              <FileText size={13} className="shrink-0 text-gray-500" />
                                              <span className="truncate">{attachmentFileName(url)}</span>
                                            </a>
                                          )
                                        ))}
                                      </div>
                                    )}
                                  </div>
                                ))
                              )}

                              {/* Reply composer */}
                              {!ticket.archived && (
                                <div className="pt-2 space-y-2">
                                  {replyAttachments.length > 0 && (
                                    <div className="flex flex-wrap gap-2">
                                      {replyAttachments.map((path, i) => (
                                        <div key={path + i} className="relative group">
                                          {isImageAttachment(path) ? (
                                            <div className="w-14 h-14 rounded-lg overflow-hidden border border-white/10">
                                              <img src={convertFileSrc(path)} alt="" className="w-full h-full object-cover" />
                                            </div>
                                          ) : (
                                            <div className="w-14 h-14 rounded-lg border border-white/10 bg-white/[0.04] flex flex-col items-center justify-center gap-0.5 px-1">
                                              <FileText size={16} className="text-gray-500 shrink-0" />
                                              <span className="text-[7px] text-gray-500 font-bold truncate max-w-full">{attachmentFileName(path)}</span>
                                            </div>
                                          )}
                                          <button
                                            onClick={() => setReplyAttachments(prev => prev.filter((_, idx) => idx !== i))}
                                            className="absolute inset-0 bg-black/60 opacity-0 group-hover:opacity-100 flex items-center justify-center transition-opacity rounded-lg"
                                          >
                                            <X size={14} className="text-white" />
                                          </button>
                                        </div>
                                      ))}
                                    </div>
                                  )}
                                  {replyError && (
                                    <div className="flex items-center justify-between gap-2 rounded-xl bg-red-500/[0.08] border border-red-500/20 px-3 py-2">
                                      <span className="text-[11px] font-bold text-red-400 flex items-center gap-1.5 min-w-0">
                                        <AlertTriangle size={12} className="shrink-0" />
                                        <span className="truncate">{replyError}</span>
                                      </span>
                                      <button
                                        onClick={() => handleSendReply(ticket)}
                                        disabled={sendingReply}
                                        className="shrink-0 flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[9px] font-black uppercase tracking-widest text-gray-300 hover:text-white transition-all disabled:opacity-50"
                                      >
                                        <RefreshCw size={11} className={sendingReply ? 'animate-spin' : ''} />
                                        {ti('Reintentar', 'Retry')}
                                      </button>
                                    </div>
                                  )}
                                  <div className="flex items-end gap-2">
                                    <textarea
                                      value={replyText}
                                      onChange={e => setReplyText(e.target.value)}
                                      placeholder={ti('Escribe una respuesta...', 'Write a reply...')}
                                      rows={2}
                                      className="flex-1 bg-black/30 border border-white/10 focus:border-accent/40 rounded-xl px-3 py-2 text-xs text-gray-200 placeholder:text-gray-600 outline-none resize-none transition-colors"
                                    />
                                    <button
                                      onClick={handleAttachFile}
                                      title={ti('Adjuntar archivo', 'Attach file')}
                                      className="shrink-0 w-9 h-9 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 flex items-center justify-center text-gray-400 hover:text-white transition-all"
                                    >
                                      <ImagePlus size={15} />
                                    </button>
                                    <button
                                      onClick={() => handleSendReply(ticket)}
                                      disabled={sendingReply || (!replyText.trim() && replyAttachments.length === 0)}
                                      className="shrink-0 w-9 h-9 rounded-xl bg-accent hover:brightness-110 disabled:opacity-30 disabled:cursor-not-allowed flex items-center justify-center text-white transition-all"
                                    >
                                      {sendingReply ? <RefreshCw size={14} className="animate-spin" /> : <Send size={14} />}
                                    </button>
                                  </div>
                                  <button
                                    onClick={() => handleResolveTicket(ticket)}
                                    disabled={resolvingTicket === ticket.threadId}
                                    className="flex items-center gap-1.5 text-[10px] font-black uppercase tracking-wider text-gray-500 hover:text-emerald-400 transition-colors disabled:opacity-50"
                                  >
                                    {resolvingTicket === ticket.threadId ? (
                                      <RefreshCw size={11} className="animate-spin" />
                                    ) : (
                                      <CheckCircle2 size={11} />
                                    )}
                                    {ti('Marcar como resuelto', 'Mark as resolved')}
                                  </button>
                                </div>
                              )}
                            </div>
                          </motion.div>
                        )}
                      </AnimatePresence>
                    </div>
                  );
                  });
                })()}
              </div>
            </div>
          )}
          {tickets.length === 0 && (
            <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-6 text-center space-y-2">
              <div className="w-11 h-11 mx-auto rounded-full bg-accent/10 border border-accent/20 flex items-center justify-center">
                <MessageSquare size={18} className="text-accent" />
              </div>
              <p className="text-sm font-black text-white/90">{ti('Tus tickets', 'Your tickets')}</p>
              <p className="text-xs text-gray-500 leading-relaxed">
                {ti('Cuando envíes un reporte aparecerá aquí, con las respuestas del equipo.', "Once you send a report it shows up here, with the team's replies.")}
              </p>
            </div>
          )}
        </div>
      </div>

      {/* Attachment lightbox */}
      <AnimatePresence>
        {lightboxImg && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={() => setLightboxImg(null)}
            className="fixed inset-0 z-[300] flex items-center justify-center p-8 cursor-zoom-out"
            style={{ background: 'radial-gradient(ellipse at center, rgba(20,20,26,0.92) 0%, rgba(0,0,0,0.96) 75%)', backdropFilter: 'blur(20px)' }}
          >
            <motion.button
              initial={{ opacity: 0, scale: 0.8 }}
              animate={{ opacity: 1, scale: 1 }}
              whileHover={{ scale: 1.08 }}
              whileTap={{ scale: 0.92 }}
              onClick={() => setLightboxImg(null)}
              title={ti('Cerrar', 'Close')}
              className="absolute top-6 right-6 w-11 h-11 rounded-full bg-white/[0.08] hover:bg-white/[0.16] border border-white/15 flex items-center justify-center text-white transition-colors z-10"
            >
              <X size={20} />
            </motion.button>
            <motion.div
              initial={{ scale: 0.92, opacity: 0, y: 10 }}
              animate={{ scale: 1, opacity: 1, y: 0 }}
              transition={{ type: 'spring', stiffness: 300, damping: 28 }}
              onClick={e => e.stopPropagation()}
              className="relative cursor-default"
            >
              <img
                src={lightboxImg}
                alt=""
                className="max-w-[85vw] max-h-[85vh] rounded-2xl border border-white/10 shadow-2xl"
                style={{ boxShadow: '0 40px 100px rgba(0,0,0,0.7), 0 0 0 1px rgba(255,255,255,0.06)' }}
              />
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
};

// --- Achievements ---

interface AchievementInfo {
  name: string;
  display_name: string;
  description: string;
  hidden: boolean;
  icon: string;
  icongray: string;
  earned: boolean;
  earned_time: number;
  global_percent: number | null;
}

interface AchievementHistorySummary {
  app_id: string;
  total: number;
  earned: number;
}

const AchievementsView = ({ steamPath, games = [] }: { steamPath: string; games?: Game[] }) => {
  const { t, lang } = useLanguage();
  const ti = useTranslateInline();
  const { notify } = useNotify();

  const [apiKey, setApiKey] = useState(() => localStorage.getItem('rl_steam_api_key') ?? '');
  const [apiKeyDraft, setApiKeyDraft] = useState(apiKey);
  // Folded to one line once a key is saved; it took a whole card for good.
  const [editingKey, setEditingKey] = useState(() => !(localStorage.getItem('rl_steam_api_key') ?? '').trim());
  const [achFilter, setAchFilter] = useState<'all' | 'earned' | 'locked'>('all');
  // Games checked in the background that turned out to have no data: "none"
  // when Steam lists no achievements, "unknown" when it could not be read.
  const [noData, setNoData] = useState<Record<string, 'none' | 'unknown'>>({});
  const [scanning, setScanning] = useState<{ done: number; total: number } | null>(null);
  // The game on screen, for requests that finish after the user moved on.
  const selectedRef = useRef<string | null>(null);

  const [loadingGames, setLoadingGames] = useState(true);
  const [installedIds, setInstalledIds] = useState<string[]>([]);
  const [names, setNames] = useState<Record<string, string>>({});
  const [historyMap, setHistoryMap] = useState<Record<string, { total: number; earned: number }>>({});
  const [search, setSearch] = useState('');

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [achievements, setAchievements] = useState<AchievementInfo[] | null>(null);
  const [achLoading, setAchLoading] = useState(false);
  const [installingAch, setInstallingAch] = useState(false);
  const [achError, setAchError] = useState<string | null>(null);
  const [togglingName, setTogglingName] = useState<string | null>(null);

  const [showHistory, setShowHistory] = useState(false);
  const [historyList, setHistoryList] = useState<AchievementHistorySummary[]>([]);

  // Escape closes it, and the page behind it stops scrolling while it is open.
  //
  // Both are patterns this file already uses for its other overlays (the
  // screenshot lightbox, the game-detail modal, DonateModal) — this one just
  // never got them. Without the scroll lock, reaching the end of a long
  // history list carried the wheel through to the Achievements view behind the
  // backdrop, which reads as the page coming apart.
  useEffect(() => {
    if (!showHistory) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setShowHistory(false);
    };
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onKey);
      document.body.style.overflow = previousOverflow;
    };
  }, [showHistory]);
  // --- Full-history modal: search, per-game expansion and its lazy cache ---
  const [historySearch, setHistorySearch] = useState('');
  const [expandedHistoryId, setExpandedHistoryId] = useState<string | null>(null);
  // appId -> achievements, populated the first time a row is expanded and kept
  // afterwards so re-opening the same row is instant (no second IPC round-trip)
  const [historyAchCache, setHistoryAchCache] = useState<Record<string, AchievementInfo[]>>({});
  // A Set, not a single id.
  //
  // Hover prefetch fires one request per row the mouse passes over, but this
  // held only the newest. Hovering row A, then row B, then clicking A left A
  // with `rowLoading === false` (the id was B's), no cached data and no error
  // — so the panel fell through to "No hay detalles guardados para este juego"
  // while A's request was still in flight. It corrected itself on arrival, but
  // the message was simply false.
  const [historyAchLoading, setHistoryAchLoading] = useState<Set<string>>(new Set());
  const [historyAchErrors, setHistoryAchErrors] = useState<Record<string, string>>({});

  const saveApiKey = () => {
    const trimmed = apiKeyDraft.trim();
    localStorage.setItem('rl_steam_api_key', trimmed);
    setApiKey(trimmed);
    if (trimmed) setEditingKey(false);
    notify(ti('API Key guardada.', 'API key saved.'), 'success');
  };

  const refreshHistory = useCallback(async () => {
    try {
      const list = await invoke<AchievementHistorySummary[]>('list_achievement_history', { steamPath });
      setHistoryList(list);
      const map: Record<string, { total: number; earned: number }> = {};
      list.forEach(h => { map[h.app_id] = { total: h.total, earned: h.earned }; });
      setHistoryMap(map);
      return list;
    } catch {
      return [];
    }
  }, [steamPath]);

  // Initial load: installed games + achievement history summary + display names for both
  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoadingGames(true);
      try {
        const [installedSet, history] = await Promise.all([
          invoke<string[]>('get_really_installed_app_ids', { steamPath }),
          invoke<AchievementHistorySummary[]>('list_achievement_history', { steamPath }),
        ]);
        if (cancelled) return;

        const installed = Array.from(installedSet);
        const map: Record<string, { total: number; earned: number }> = {};
        history.forEach(h => { map[h.app_id] = { total: h.total, earned: h.earned }; });

        const unionIds = Array.from(new Set([...installed, ...history.map(h => h.app_id)]));
        // Names this app already knows — the catalog and the name cache — go
        // up at once. The list used to wait for Steam to name every game over
        // the network before showing any of them.
        const known: Record<string, string> = { ...getCleanNameCache() };
        for (const g of games) {
          const n = g.name?.trim();
          if (n && !n.startsWith('Game ID:')) known[g.id] = n;
        }
        const namesMap: Record<string, string> = Object.fromEntries(unionIds.map(id => [id, known[id] ?? `App ${id}`]));

        if (cancelled) return;
        setInstalledIds(installed);
        setHistoryList(history);
        setHistoryMap(map);
        setNames(namesMap);

        const missing = unionIds.filter(id => !known[id]);
        if (missing.length > 0) {
          invoke<Record<string, [string, boolean]>>('fetch_game_names', { appIds: missing })
            .then(raw => {
              if (cancelled) return;
              setNames(prev => {
                const next = { ...prev };
                for (const id of missing) {
                  const n = raw[id]?.[0]?.trim();
                  if (n) next[id] = n;
                }
                return next;
              });
            })
            .catch(() => { });
        }
      } catch (err) {
        if (!cancelled) notify(`${ti('Error al cargar juegos', 'Error loading games')}: ${err}`, 'error');
      } finally {
        if (!cancelled) setLoadingGames(false);
      }
    })();
    return () => { cancelled = true; };
  }, [steamPath]);

  const stats = useMemo(() => {
    const entries = Object.values(historyMap);
    const gamesTracked = entries.length;
    const sumEarned = entries.reduce((acc, e) => acc + e.earned, 0);
    const sumTotal = entries.reduce((acc, e) => acc + e.total, 0);
    const withTotal = entries.filter(e => e.total > 0);
    const avgProgress = withTotal.length > 0
      ? Math.round(withTotal.reduce((acc, e) => acc + (e.earned / e.total) * 100, 0) / withTotal.length)
      : 0;
    const fullyCompleted = entries.filter(e => e.total > 0 && e.earned === e.total).length;
    return { gamesTracked, sumEarned, sumTotal, avgProgress, fullyCompleted };
  }, [historyMap]);

  // Most earned first, then the rest by name; games with no achievements last.
  const filteredGames = useMemo(() => {
    const list = installedIds.map(id => ({ id, name: names[id] ?? `App ${id}` }));
    const q = search.trim();
    const shown = q ? list.filter(g => matchesSearch(g.name, q) || g.id.startsWith(q)) : list;
    const rank = (id: string) => (noData[id] ? -1 : historyMap[id]?.earned ?? 0);
    return shown.sort((a, b) => (rank(b.id) - rank(a.id)) || a.name.localeCompare(b.name));
  }, [installedIds, names, search, historyMap, noData]);

  const selectGame = async (id: string) => {
    selectedRef.current = id;
    setSelectedId(id);
    setAchievements(null);
    setAchError(null);
    setAchFilter('all');
    setAchLoading(true);
    // What was saved the last time goes up at once; the full read below then
    // brings it up to date. The panel used to sit on skeletons meanwhile.
    invoke<AchievementInfo[]>('get_cached_game_achievements', { steamPath, appId: id })
      .then(list => {
        if (selectedRef.current === id && list.length > 0) setAchievements(prev => prev ?? list);
      })
      .catch(() => { });
    try {
      const result = await invoke<AchievementInfo[]>('get_game_achievements', {
        steamPath, appId: id, apiKey, forceRefresh: false,
      });
      if (selectedRef.current !== id) return;
      setAchievements(result);
      refreshHistory();
    } catch (err) {
      if (selectedRef.current !== id) return;
      setAchievements(prev => {
        // Keep what was saved if the refresh failed; say why only when there
        // is nothing to show.
        if (!prev || prev.length === 0) setAchError(String(err));
        return prev;
      });
    } finally {
      if (selectedRef.current === id) setAchLoading(false);
    }
  };

  // A game opens on arrival instead of an empty "pick a game" panel.
  useEffect(() => {
    if (loadingGames || selectedRef.current || filteredGames.length === 0) return;
    void selectGame(filteredGames[0].id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadingGames]);

  // Games never opened here said "Sin revisar todavía" until clicked. They are
  // checked in the background now, two at a time, once the list is up — with
  // the schema cached on disk, each is a read of Steam's local files.
  useEffect(() => {
    if (loadingGames) return;
    const pending = installedIds.filter(id => !historyMap[id]);
    if (pending.length === 0) return;
    let cancelled = false;
    (async () => {
      const queue = [...pending];
      let done = 0;
      setScanning({ done, total: pending.length });
      const worker = async () => {
        while (queue.length > 0 && !cancelled) {
          const id = queue.shift()!;
          try {
            const list = await invoke<AchievementInfo[]>('get_game_achievements', { steamPath, appId: id, apiKey, forceRefresh: false });
            if (list.length === 0) setNoData(prev => ({ ...prev, [id]: 'none' }));
          } catch (err) {
            const none = /no tiene logros/i.test(String(err));
            setNoData(prev => ({ ...prev, [id]: none ? 'none' : 'unknown' }));
          }
          done += 1;
          if (!cancelled) setScanning({ done, total: pending.length });
        }
      };
      await Promise.all([worker(), worker()]);
      if (!cancelled) {
        await refreshHistory();
        setScanning(null);
      }
    })();
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadingGames, steamPath]);

  const handleForceRefresh = async () => {
    if (!selectedId) return;
    setAchLoading(true);
    setAchError(null);
    try {
      const result = await invoke<AchievementInfo[]>('get_game_achievements', {
        steamPath, appId: selectedId, apiKey, forceRefresh: true,
      });
      setAchievements(result);
      refreshHistory();
      notify(ti('Logros actualizados.', 'Achievements refreshed.'), 'success');
    } catch (err) {
      notify(`${ti('Error al actualizar', 'Error refreshing')}: ${err}`, 'error');
    } finally {
      setAchLoading(false);
    }
  };

  const toggleAchievement = async (ach: AchievementInfo) => {
    if (!selectedId || togglingName) return;
    const newEarned = !ach.earned;
    setTogglingName(ach.name);
    // `togglingName` gates EVERY achievement of every game, not just this one,
    // so leaving it set is not a small leak: it disables the whole panel until
    // the app restarts. The clear used to sit at the end of the function, which
    // an `invoke` that never settles skips entirely — and this project has hit
    // three separate Rust panics that left a promise unresolved. A `finally`
    // costs nothing and removes the whole class of outcome.
    //
    // The timeout is the other half: a hung helper should surface as an error
    // the user can act on, not as a spinner with no end. The Rust side already
    // caps ach_helper at 15 seconds, so 30 here only catches the case where
    // the command itself never returns.
    const withTimeout = <T,>(p: Promise<T>, ms: number): Promise<T> =>
      Promise.race([
        p,
        new Promise<T>((_, reject) =>
          setTimeout(
            () => reject(new Error(ti('Steam no respondió a tiempo.', 'Steam did not respond in time.'))),
            ms
          )
        ),
      ]);

    try {
      try {
        await withTimeout(
          invoke('sync_steam_achievement', {
            steamPath, appId: selectedId, achievementName: ach.name, earned: newEarned,
          }),
          30_000
        );
        notify(
          newEarned
            ? ti('Logro desbloqueado en Steam.', 'Achievement unlocked on Steam.')
            : ti('Logro bloqueado en Steam.', 'Achievement locked on Steam.'),
          'success'
        );
      } catch (err) {
        notify(`${ti('No se pudo sincronizar con Steam', 'Could not sync with Steam')}: ${err}`, 'error');
      }
      try {
        await withTimeout(
          invoke('set_local_achievement', { appId: selectedId, achievementName: ach.name, earned: newEarned }),
          15_000
        );
      } catch { /* best-effort local persistence */ }

      setAchievements(prev => prev?.map(a => a.name === ach.name
        ? { ...a, earned: newEarned, earned_time: newEarned ? Math.floor(Date.now() / 1000) : 0 }
        : a) ?? null);
      refreshHistory();
    } finally {
      setTogglingName(null);
    }
  };

  // A game running under the Steam emulator only records achievements once the
  // emulator has a schema to match them against. Without it nothing unlocks and
  // nothing is even written, so the game looks like it simply has none — which
  // is indistinguishable, from the player's side, from the feature being broken.
  const handleInstallAchievements = async () => {
    if (!selectedId || installingAch) return;
    setInstallingAch(true);
    try {
      const msg = await invoke<string>('install_emulator_achievements', {
        steamPath, appId: selectedId,
      });
      notify(msg, 'success');
    } catch (err) {
      notify(`${ti('No se pudieron instalar los logros', 'Could not install achievements')}: ${err}`, 'error');
    } finally {
      setInstallingAch(false);
    }
  };

  // --- Full-history modal helpers ---

  const historyStats = useMemo(() => {
    const sumTotal = historyList.reduce((acc, h) => acc + h.total, 0);
    const sumEarned = historyList.reduce((acc, h) => acc + h.earned, 0);
    return {
      games: historyList.length,
      sumTotal,
      sumEarned,
      pct: sumTotal > 0 ? Math.round((sumEarned / sumTotal) * 100) : 0,
      completed: historyList.filter(h => h.total > 0 && h.earned === h.total).length,
    };
  }, [historyList]);

  // Most-complete first so the interesting rows read at the top; ties by name.
  const historyRows = useMemo(() => {
    const rows = historyList.map(h => ({
      ...h,
      name: names[h.app_id] ?? `App ${h.app_id}`,
      pct: h.total > 0 ? Math.round((h.earned / h.total) * 100) : 0,
    }));
    const q = historySearch.trim();
    const filtered = q
      ? rows.filter(r => matchesSearch(r.name, q) || r.app_id.startsWith(q))
      : rows;
    return filtered.sort((a, b) => (b.pct - a.pct) || a.name.localeCompare(b.name));
  }, [historyList, names, historySearch]);

  // Read-only: pulls from the on-disk schema cache, never touches Steam.
  /// Loads one game's achievements into the cache, at most once.
  ///
  /// Separated from the click handler so the same work can begin earlier:
  /// reading a game's schema is a filesystem round trip, and doing it only on
  /// expand meant the first open of every row sat on loading skeletons. Rows
  /// now request their data on hover, so by the time the click lands it is
  /// usually already in the cache and the panel opens filled.
  const ensureHistoryAchievements = async (appId: string) => {
    if (historyAchCache[appId] || historyAchLoading.has(appId)) return;
    setHistoryAchErrors(prev => {
      if (!(appId in prev)) return prev;
      const next = { ...prev };
      delete next[appId];
      return next;
    });
    setHistoryAchLoading(prev => new Set(prev).add(appId));
    try {
      const list = await invoke<AchievementInfo[]>('get_cached_game_achievements', { steamPath, appId });
      setHistoryAchCache(prev => ({ ...prev, [appId]: list }));
    } catch (err) {
      setHistoryAchErrors(prev => ({ ...prev, [appId]: String(err) }));
    } finally {
      setHistoryAchLoading(prev => {
        const next = new Set(prev);
        next.delete(appId);
        return next;
      });
    }
  };

  const toggleHistoryGame = async (appId: string) => {
    if (expandedHistoryId === appId) {
      setExpandedHistoryId(null);
      return;
    }
    setExpandedHistoryId(appId);
    await ensureHistoryAchievements(appId);
  };

  const formatEarnedDate = (unix: number) => {
    if (!unix) return null;
    try {
      return new Date(unix * 1000).toLocaleDateString(lang, {
        year: 'numeric', month: 'short', day: 'numeric',
      });
    } catch {
      return new Date(unix * 1000).toLocaleDateString();
    }
  };

  const closeHistory = () => {
    setShowHistory(false);
    setExpandedHistoryId(null);
  };

  const openHistory = () => {
    setShowHistory(true);
    setHistorySearch('');
    setExpandedHistoryId(null);
    // The initial load already filled historyList, and recomputing it is the
    // most expensive thing this tab does — it re-reads every tracked game's
    // achievement state. The modal opens on what is already there and only
    // refreshes if that is genuinely empty; anything unlocked since is picked
    // up the next time the tab is opened.
    if (historyList.length === 0) refreshHistory();
  };

  const selectedName = selectedId ? (names[selectedId] ?? `App ${selectedId}`) : '';

  const statBoxes = [
    { label: ti('Juegos con logros', 'Games tracked'), value: stats.gamesTracked, icon: Gamepad2 },
    { label: ti('Logros conseguidos', 'Achievements earned'), value: `${stats.sumEarned}/${stats.sumTotal}`, icon: Trophy },
    { label: ti('Promedio de progreso', 'Average progress'), value: `${stats.avgProgress}%`, icon: Star, accent: true },
    { label: ti('100% completados', '100% completed'), value: stats.fullyCompleted, icon: CheckCircle2 },
  ];

  return (
    <div className="space-y-6 pb-10">

      {/* Header */}
      <motion.div
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.35, ease: 'easeOut' }}
        className="relative overflow-hidden rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-8"
      >
        <div className="absolute top-0 right-0 w-64 h-64 bg-accent/10 blur-[80px] rounded-full pointer-events-none" />
        <div className="relative z-10 flex flex-col md:flex-row items-start md:items-center gap-6">
          <div className="w-16 h-16 rounded-2xl bg-accent flex items-center justify-center shrink-0">
            <Trophy size={28} className="text-white" />
          </div>
          <div className="flex-1 min-w-0">
            <h3 className="text-xl font-black text-white/90 tracking-tight">{t.sidebar.achievements}</h3>
            <p className="text-sm text-gray-500 mt-1">
              {ti('Se reflejan directamente en Steam', 'Reflected directly on Steam')}
              {scanning && (
                <span className="inline-flex items-center gap-1.5 ml-3 text-[11px] text-gray-500">
                  <RefreshCw size={11} className="animate-spin" />
                  {ti(`Revisando juegos ${scanning.done}/${scanning.total}`, `Checking games ${scanning.done}/${scanning.total}`)}
                </span>
              )}
            </p>
          </div>
          <button
            onClick={openHistory}
            className="shrink-0 px-5 py-3 bg-white/[0.04] hover:bg-white/[0.07] border border-white/[0.08] hover:border-white/20 text-white/90 text-xs font-black uppercase tracking-widest rounded-xl transition-all duration-300 flex items-center gap-2"
          >
            <Trophy size={14} />
            {ti('Ver historial completo', 'View full history')}
          </button>
        </div>
      </motion.div>

      {/* Stats */}
      <div className="flex flex-col md:flex-row gap-4">
        {statBoxes.map((s, i) => (
          <motion.div
            key={s.label}
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.35, ease: 'easeOut', delay: i * 0.06 }}
            className="flex-1 rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-6 flex flex-col gap-3 hover:border-white/[0.14] transition-colors duration-500"
          >
            <div className="flex items-center justify-between">
              <span className="text-[9px] uppercase tracking-widest text-gray-500 font-black">{s.label}</span>
              <s.icon size={16} className={s.accent ? 'text-accent opacity-70' : 'text-gray-600'} />
            </div>
            <span className={`text-4xl font-black tracking-tight ${s.accent ? 'text-accent' : 'text-white/90'}`}>
              {s.value}
            </span>
          </motion.div>
        ))}
      </div>

      {/* Two-column body */}
      <div className="flex flex-col lg:flex-row gap-5 items-start">

        {/* LEFT column */}
        <div className="w-full lg:w-[340px] shrink-0 flex flex-col gap-4">
          {/* API key card — one line once a key is saved */}
          {!editingKey ? (
            <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] px-4 py-3 flex items-center gap-3">
              <CheckCircle2 size={15} className="text-emerald-400 shrink-0" />
              <div className="min-w-0 flex-1">
                <p className="text-[9px] font-black uppercase tracking-widest text-gray-500">Steam Web API Key</p>
                <p className="text-[11px] font-semibold text-gray-300">{ti('Guardada', 'Saved')}</p>
              </div>
              <button
                onClick={() => { setApiKeyDraft(apiKey); setEditingKey(true); }}
                className="shrink-0 px-3 py-1.5 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[10px] font-black uppercase tracking-widest text-gray-300 hover:text-white transition-colors"
              >
                {ti('Cambiar', 'Change')}
              </button>
            </div>
          ) : (
          <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-5 space-y-3">
            <label className="text-[9px] font-black uppercase tracking-[0.3em] text-gray-500">
              {ti('Steam Web API Key', 'Steam Web API Key')}
            </label>
            <div className="flex items-center gap-2">
              <input
                type="password"
                value={apiKeyDraft}
                onChange={e => setApiKeyDraft(e.target.value)}
                placeholder="XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
                spellCheck={false}
                autoComplete="off"
                className="flex-1 min-w-0 bg-black/40 border border-white/10 focus:border-accent/50 focus:ring-1 focus:ring-accent/25 rounded-xl py-2.5 px-4 text-xs font-mono outline-none transition-all placeholder:text-gray-700 text-gray-200"
              />
              <button
                onClick={saveApiKey}
                className="shrink-0 px-4 py-2.5 bg-accent hover:brightness-110 hover:-translate-y-0.5 text-white text-[11px] font-black rounded-xl uppercase tracking-wider transition-all duration-200"
              >
                {ti('Guardar', 'Save')}
              </button>
            </div>
            <a
              href="https://steamcommunity.com/dev/apikey"
              target="_blank"
              rel="noreferrer"
              className="text-[11px] text-gray-500 hover:text-accent transition-colors underline decoration-dotted block"
            >
              {ti('Consigue tu API Key aquí', 'Get your API key here')}
            </a>
          </div>
          )}

          {/* Search */}
          <div className="relative">
            <div className="absolute inset-y-0 left-4 flex items-center pointer-events-none text-gray-500">
              <Search size={14} />
            </div>
            <input
              type="text"
              value={search}
              onChange={e => setSearch(e.target.value)}
              placeholder={ti('Buscar juego...', 'Search game...')}
              spellCheck={false}
              autoComplete="off"
              className="w-full bg-black/40 border border-white/10 rounded-xl py-3 pl-11 pr-4 text-sm focus:outline-none focus:border-accent/50 focus:ring-1 focus:ring-accent/25 transition-all text-gray-200"
            />
          </div>

          {/* Game list */}
          <div className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-3">
            <div className="max-h-[480px] overflow-y-auto custom-scrollbar space-y-1.5 pr-1">
              {loadingGames ? (
                [1, 2, 3, 4].map(i => <div key={i} className="h-14 rounded-xl bg-white/[0.03] animate-pulse" />)
              ) : filteredGames.length === 0 ? (
                <div className="text-center py-10 text-gray-500 text-xs font-bold px-2">
                  {installedIds.length === 0
                    ? ti('No se encontraron juegos instalados.', 'No installed games found.')
                    : ti('Sin resultados para tu búsqueda.', 'No results for your search.')}
                </div>
              ) : (
                filteredGames.map((g, i) => {
                  const hist = historyMap[g.id];
                  const pct = hist && hist.total > 0 ? Math.round((hist.earned / hist.total) * 100) : 0;
                  const active = selectedId === g.id;
                  return (
                    <motion.button
                      key={g.id}
                      initial={{ opacity: 0, y: 8 }}
                      animate={{ opacity: 1, y: 0 }}
                      transition={{ duration: 0.3, ease: 'easeOut', delay: Math.min(i * 0.035, 0.35) }}
                      onClick={() => selectGame(g.id)}
                      className={`w-full flex items-center gap-3 p-2.5 rounded-xl border transition-colors duration-300 text-left ${
                        active
                          ? 'bg-accent/[0.12] border-accent/40'
                          : 'bg-white/[0.02] border-white/[0.06] hover:bg-white/[0.05] hover:border-white/10'
                      }`}
                    >
                      <CachedImage appId={g.id} alt={g.name} className="w-12 h-8 rounded-lg object-cover shrink-0 bg-black/40" />
                      <div className="min-w-0 flex-1">
                        <p className={`text-xs font-bold truncate transition-colors duration-300 ${active ? 'text-white/90' : 'text-gray-300'}`}>{g.name}</p>
                        {hist ? (
                          <div className="mt-1.5 flex items-center gap-2">
                            <div className="flex-1 h-1 bg-white/10 rounded-full overflow-hidden">
                              <motion.div
                                className="h-full bg-accent rounded-full"
                                initial={{ width: 0 }}
                                animate={{ width: `${pct}%` }}
                                transition={{ duration: 0.6, ease: 'easeOut' }}
                              />
                            </div>
                            <span className="text-[10px] text-gray-500 font-mono shrink-0">{hist.earned}/{hist.total}</span>
                          </div>
                        ) : noData[g.id] ? (
                          <p className="text-[10px] text-gray-600 mt-1">
                            {noData[g.id] === 'none'
                              ? ti('Sin logros de Steam', 'No Steam achievements')
                              : ti('Sin datos: añade una API Key', 'No data: add an API key')}
                          </p>
                        ) : (
                          <p className="text-[10px] text-gray-600 mt-1 flex items-center gap-1">
                            <RefreshCw size={9} className="animate-spin" />
                            {ti('Revisando…', 'Checking…')}
                          </p>
                        )}
                      </div>
                    </motion.button>
                  );
                })
              )}
            </div>
          </div>
        </div>

        {/* RIGHT column */}
        <div className="flex-1 min-w-0 w-full rounded-2xl bg-[#0d0e12] border border-white/[0.08] min-h-[480px] flex flex-col">
          {!selectedId ? (
            <div className="flex-1 flex flex-col items-center justify-center gap-4 py-20 text-center px-6">
              <div className="w-16 h-16 rounded-2xl bg-white/[0.03] border border-white/[0.08] flex items-center justify-center">
                <Trophy size={26} className="text-gray-600" />
              </div>
              <div>
                <h4 className="text-sm font-black text-white/90 uppercase tracking-widest">{ti('Elige un juego', 'Pick a game')}</h4>
                <p className="text-xs text-gray-500 mt-2 max-w-xs">
                  {ti('Selecciona uno de la lista para ver y gestionar sus logros.', 'Select one from the list to view and manage its achievements.')}
                </p>
              </div>
            </div>
          ) : (
            <>
              {/* Detail header */}
              <div className="flex items-center gap-4 p-6 border-b border-white/[0.06]">
                <CachedImage appId={selectedId} alt={selectedName} className="w-16 h-10 rounded-lg object-cover shrink-0 bg-black/40" />
                <div className="min-w-0 flex-1">
                  <h4 className="text-sm font-black text-white/90 truncate">{selectedName}</h4>
                  {achievements && achievements.length > 0 && (() => {
                    const earned = achievements.filter(a => a.earned).length;
                    const pct = Math.round((earned / achievements.length) * 100);
                    return (
                      <div className="mt-1.5 flex items-center gap-3 max-w-md">
                        <div className="flex-1 h-1.5 bg-white/10 rounded-full overflow-hidden">
                          <motion.div
                            className="h-full bg-accent rounded-full"
                            initial={{ width: 0 }}
                            animate={{ width: `${pct}%` }}
                            transition={{ duration: 0.6, ease: 'easeOut' }}
                          />
                        </div>
                        <span className="text-[11px] text-gray-400 font-bold tabular-nums shrink-0">
                          {earned}/{achievements.length} · {pct}%
                        </span>
                      </div>
                    );
                  })()}
                </div>
                <button
                  onClick={handleInstallAchievements}
                  disabled={installingAch}
                  className="shrink-0 h-9 px-3 rounded-xl bg-white/[0.04] hover:bg-white/[0.07] border border-white/[0.08] flex items-center gap-1.5 text-[11px] font-bold text-gray-400 hover:text-white transition-all disabled:opacity-40"
                  title={ti(
                    'Instala la lista de logros dentro del juego para que el emulador pueda contarlos',
                    'Installs the achievement list into the game so the emulator can track them'
                  )}
                >
                  <Download size={13} className={installingAch ? 'animate-pulse' : ''} />
                  {ti('Activar logros', 'Enable achievements')}
                </button>
                <button
                  onClick={handleForceRefresh}
                  disabled={achLoading}
                  className="shrink-0 w-9 h-9 rounded-xl bg-white/[0.04] hover:bg-white/[0.07] border border-white/[0.08] flex items-center justify-center text-gray-400 hover:text-white transition-all disabled:opacity-40"
                  title={ti('Actualizar', 'Refresh')}
                >
                  <RefreshCw size={14} className={achLoading ? 'animate-spin' : ''} />
                </button>
              </div>

              {/* Filter */}
              {achievements && achievements.length > 0 && (
                <div className="flex items-center gap-1.5 flex-wrap px-5 pt-4">
                  <LibraryPill active={achFilter === 'all'} onClick={() => setAchFilter('all')} layoutId="ach-filter-pill">
                    {ti('Todos', 'All')} <span className="tabular-nums opacity-70">{achievements.length}</span>
                  </LibraryPill>
                  <LibraryPill active={achFilter === 'earned'} onClick={() => setAchFilter('earned')} layoutId="ach-filter-pill">
                    {ti('Conseguidos', 'Earned')} <span className="tabular-nums opacity-70">{achievements.filter(a => a.earned).length}</span>
                  </LibraryPill>
                  <LibraryPill active={achFilter === 'locked'} onClick={() => setAchFilter('locked')} layoutId="ach-filter-pill">
                    {ti('Pendientes', 'Locked')} <span className="tabular-nums opacity-70">{achievements.filter(a => !a.earned).length}</span>
                  </LibraryPill>
                </div>
              )}

              {/* Detail body */}
              <div className="flex-1 overflow-y-auto custom-scrollbar p-5 space-y-2.5">
                {achLoading && !achievements ? (
                  [1, 2, 3, 4, 5].map(i => <div key={i} className="h-16 rounded-xl bg-white/[0.03] animate-pulse" />)
                ) : achError ? (
                  <div className="text-center py-16 px-6 space-y-3">
                    <AlertTriangle size={28} className="mx-auto text-yellow-500/60" />
                    <p className="text-xs text-gray-400 max-w-sm mx-auto">{achError}</p>
                  </div>
                ) : achievements && achievements.length > 0 ? (
                  achievements
                    .filter(a => achFilter === 'all' || (achFilter === 'earned' ? a.earned : !a.earned))
                    .map((ach, i) => (
                    <motion.div
                      key={ach.name}
                      initial={{ opacity: 0, y: 8 }}
                      animate={{ opacity: 1, y: 0 }}
                      transition={{ duration: 0.3, ease: 'easeOut', delay: Math.min(i * 0.03, 0.3) }}
                      className={`flex items-start gap-4 p-4 rounded-xl border transition-colors duration-300 ${
                        ach.earned ? 'bg-white/[0.03] border-white/[0.08]' : 'bg-black/20 border-white/[0.06]'
                      }`}
                    >
                      {(ach.earned ? ach.icon : (ach.icongray || ach.icon)) ? (
                        <img
                          src={ach.earned ? ach.icon : (ach.icongray || ach.icon)}
                          alt=""
                          className={`w-11 h-11 rounded-xl shrink-0 transition-all duration-300 ${ach.earned ? '' : 'opacity-40 grayscale'}`}
                        />
                      ) : (
                        <div className="w-11 h-11 rounded-xl bg-white/[0.04] border border-white/[0.08] flex items-center justify-center shrink-0">
                          <Trophy size={16} className="text-gray-700" />
                        </div>
                      )}
                      <div className="min-w-0 flex-1">
                        <p className={`text-sm font-bold transition-colors duration-300 ${ach.earned ? 'text-white/90' : 'text-gray-400'}`}>{ach.display_name}</p>
                        <p className="text-xs text-gray-500 mt-0.5 leading-relaxed">
                          {ach.hidden && !ach.earned ? ti('Logro oculto', 'Hidden achievement') : (ach.description || '—')}
                        </p>
                        {(typeof ach.global_percent === 'number' || (ach.earned && ach.earned_time > 0)) && (
                          <p className="text-[10px] text-gray-600 mt-1.5 flex items-center gap-2 flex-wrap">
                            {ach.earned && ach.earned_time > 0 && (
                              <span className="text-accent/80 font-bold">
                                {ti('Conseguido el', 'Earned on')} {formatEarnedDate(ach.earned_time)}
                              </span>
                            )}
                            {typeof ach.global_percent === 'number' && (
                              <span>{ach.global_percent.toFixed(1)}% {ti('de jugadores lo tienen', 'of players have this')}</span>
                            )}
                          </p>
                        )}
                      </div>
                      <button
                        onClick={() => toggleAchievement(ach)}
                        disabled={togglingName === ach.name}
                        title={ach.earned ? ti('Bloquear', 'Lock') : ti('Desbloquear', 'Unlock')}
                        className={`shrink-0 w-9 h-9 rounded-xl border flex items-center justify-center transition-colors duration-300 disabled:opacity-40 ${
                          ach.earned
                            ? 'bg-accent/15 border-accent/40 text-accent hover:bg-accent/25'
                            : 'bg-white/[0.04] border-white/[0.08] text-gray-600 hover:border-white/20 hover:text-gray-400'
                        }`}
                      >
                        {togglingName === ach.name ? (
                          <RefreshCw size={14} className="animate-spin" />
                        ) : (
                          <AnimatePresence mode="wait" initial={false}>
                            <motion.span
                              key={ach.earned ? 'earned' : 'unearned'}
                              initial={{ scale: 0.4, opacity: 0 }}
                              animate={{ scale: 1, opacity: 1 }}
                              exit={{ scale: 0.4, opacity: 0 }}
                              transition={{ type: 'spring', stiffness: 420, damping: 22 }}
                              className="flex items-center justify-center"
                            >
                              <CheckCircle2 size={16} />
                            </motion.span>
                          </AnimatePresence>
                        )}
                      </button>
                    </motion.div>
                  ))
                ) : (
                  <div className="text-center py-16 text-gray-500 text-xs font-bold">
                    {ti('Este juego no tiene logros de Steam.', 'This game has no Steam achievements.')}
                  </div>
                )}
              </div>
            </>
          )}
        </div>
      </div>

      {/* Full history modal */}
      {/* Portal outside AnimatePresence, not inside it: AnimatePresence needs
          its direct children to be motion components, and a portal is not one
          — nesting them the other way round made the modal never render. */}
      {createPortal(
        <AnimatePresence>
          {showHistory && (
          /* In document.body because an animated ancestor carries a CSS
             transform, and a transformed ancestor makes position:fixed resolve
             against it rather than the viewport — which left this backdrop
             short of the top of the window. */
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-[200] flex items-center justify-center bg-black/60 backdrop-blur-md p-6"
            onClick={closeHistory}
          >
            <motion.div
              initial={{ scale: 0.97, y: 12, opacity: 0 }}
              animate={{ scale: 1, y: 0, opacity: 1 }}
              exit={{ scale: 0.97, y: 12, opacity: 0 }}
              transition={{ duration: 0.25, ease: 'easeOut' }}
              className="rounded-2xl bg-[#0d0e12] border border-white/[0.08] w-[1080px] max-w-[94vw] max-h-[88vh] flex flex-col shadow-2xl overflow-hidden"
              onClick={e => e.stopPropagation()}
            >
              {/* Modal header + summary */}
              <div className="shrink-0 p-7 pb-6 border-b border-white/[0.06] space-y-6">
                <div className="flex items-start justify-between gap-4">
                  <div className="flex items-center gap-4 min-w-0">
                    <div className="w-11 h-11 rounded-xl bg-accent/15 border border-accent/30 flex items-center justify-center shrink-0">
                      <Trophy size={20} className="text-accent" />
                    </div>
                    <div className="min-w-0">
                      <h3 className="font-black text-white/90 uppercase tracking-widest text-base">
                        {ti('Historial completo', 'Full history')}
                      </h3>
                      <p className="text-[11px] text-gray-500 mt-1 font-medium">
                        {ti('Toca un juego para ver sus logros', 'Tap a game to see its achievements')}
                      </p>
                    </div>
                  </div>
                  <button
                    onClick={closeHistory}
                    className="shrink-0 w-10 h-10 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 flex items-center justify-center text-gray-400 hover:text-white transition-all"
                  >
                    <X size={18} />
                  </button>
                </div>

                {historyList.length > 0 && (
                  <div className="space-y-4">
                    <div className="grid grid-cols-3 gap-3">
                      {[
                        { label: ti('Juegos', 'Games'), value: `${historyStats.games}`, accent: false },
                        { label: ti('Logros', 'Achievements'), value: `${historyStats.sumEarned}/${historyStats.sumTotal}`, accent: false },
                        { label: ti('Completados', 'Completed'), value: `${historyStats.completed}`, accent: false },
                      ].map(s => (
                        <div key={s.label} className="rounded-xl bg-white/[0.03] border border-white/[0.06] px-4 py-3">
                          <p className="text-[9px] font-black uppercase tracking-widest text-gray-500">{s.label}</p>
                          <p className={`mt-1 text-lg font-black tracking-tight ${s.accent ? 'text-accent' : 'text-white/90'}`}>{s.value}</p>
                        </div>
                      ))}
                    </div>
                    <div className="flex items-center gap-3">
                      <span className="text-[9px] font-black uppercase tracking-widest text-gray-500 shrink-0">
                        {ti('Progreso global', 'Overall progress')}
                      </span>
                      <div className="flex-1 h-1.5 bg-white/10 rounded-full overflow-hidden">
                        <motion.div
                          className="h-full bg-accent rounded-full"
                          initial={{ width: 0 }}
                          animate={{ width: `${historyStats.pct}%` }}
                          transition={{ duration: 0.7, ease: 'easeOut' }}
                        />
                      </div>
                      <span className="text-xs font-black text-accent shrink-0 tabular-nums">{historyStats.pct}%</span>
                    </div>
                  </div>
                )}

                {historyList.length > 5 && (
                  <div className="relative">
                    <div className="absolute inset-y-0 left-4 flex items-center pointer-events-none text-gray-500">
                      <Search size={14} />
                    </div>
                    <input
                      type="text"
                      value={historySearch}
                      onChange={e => setHistorySearch(e.target.value)}
                      placeholder={ti('Filtrar juegos...', 'Filter games...')}
                      spellCheck={false}
                      autoComplete="off"
                      className="w-full bg-black/40 border border-white/10 rounded-xl py-2.5 pl-11 pr-4 text-sm focus:outline-none focus:border-accent/50 focus:ring-1 focus:ring-accent/25 transition-all text-gray-200 placeholder:text-gray-600"
                    />
                  </div>
                )}
              </div>

              {/* Rows */}
              <div className="flex-1 overflow-y-auto custom-scrollbar p-4 space-y-2">
                {historyList.length === 0 ? (
                  <div className="text-center py-14 px-6 space-y-3">
                    <div className="w-14 h-14 mx-auto rounded-2xl bg-white/[0.04] border border-white/[0.08] flex items-center justify-center">
                      <Trophy size={22} className="text-gray-600" />
                    </div>
                    <p className="text-gray-500 text-xs font-bold max-w-sm mx-auto leading-relaxed">
                      {ti('Todavía no hay logros registrados. Abre un juego en esta pestaña para empezar.', "No achievements tracked yet. Open a game in this tab to get started.")}
                    </p>
                  </div>
                ) : historyRows.length === 0 ? (
                  <div className="text-center py-14 text-gray-500 text-xs font-bold">
                    {ti('Sin resultados para tu búsqueda.', 'No results for your search.')}
                  </div>
                ) : (
                  historyRows.map((h, i) => {
                    const isOpen = expandedHistoryId === h.app_id;
                    const isDone = h.total > 0 && h.earned === h.total;
                    const rowAchievements = historyAchCache[h.app_id];
                    const rowError = historyAchErrors[h.app_id];
                    const rowLoading = historyAchLoading.has(h.app_id) && !rowAchievements;
                    const remaining = Math.max(h.total - h.earned, 0);
                    // Unlocked first (most recent at the top), then the ones still missing
                    const sortedAchievements = rowAchievements
                      ? rowAchievements.slice().sort((a, b) => {
                          if (a.earned !== b.earned) return a.earned ? -1 : 1;
                          if (a.earned && b.earned) return (b.earned_time || 0) - (a.earned_time || 0);
                          return 0;
                        })
                      : null;
                    return (
                      <motion.div
                        key={h.app_id}
                        initial={{ opacity: 0, y: 8 }}
                        animate={{ opacity: 1, y: 0 }}
                        transition={{ duration: 0.3, ease: 'easeOut', delay: Math.min(i * 0.03, 0.3) }}
                        className={`rounded-xl border overflow-hidden transition-colors duration-300 ${
                          isOpen
                            ? 'bg-white/[0.05] border-white/15'
                            : isDone
                              ? 'bg-white/[0.02] border-accent/25 hover:bg-white/[0.04]'
                              : 'bg-white/[0.02] border-white/[0.06] hover:bg-white/[0.04] hover:border-white/10'
                        }`}
                      >
                        <button
                          onClick={() => toggleHistoryGame(h.app_id)}
                          // Start fetching before the click: by the time the
                          // row opens the data is usually already cached.
                          onMouseEnter={() => { void ensureHistoryAchievements(h.app_id); }}
                          onFocus={() => { void ensureHistoryAchievements(h.app_id); }}
                          className="w-full flex items-center gap-3.5 p-3 text-left"
                        >
                          <CachedImage appId={h.app_id} alt={h.name} className="w-16 h-10 rounded-lg object-cover shrink-0 bg-black/40" />
                          <div className="min-w-0 flex-1">
                            <div className="flex items-center gap-2 min-w-0">
                              <p className="text-[13px] font-bold text-white/90 truncate">{h.name}</p>
                              {isDone && (
                                <span className="shrink-0 rounded-full bg-accent/15 border border-accent/30 text-accent text-[9px] font-black uppercase tracking-widest px-2 py-0.5">
                                  {ti('Completo', 'Complete')}
                                </span>
                              )}
                            </div>
                            <div className="mt-2 flex items-center gap-3">
                              <div className="flex-1 h-1.5 bg-white/10 rounded-full overflow-hidden">
                                <motion.div
                                  className="h-full bg-accent rounded-full"
                                  initial={{ width: 0 }}
                                  animate={{ width: `${h.pct}%` }}
                                  transition={{ duration: 0.5, ease: 'easeOut' }}
                                />
                              </div>
                              <span className={`text-[11px] font-black shrink-0 tabular-nums w-10 text-right ${isDone ? 'text-accent' : 'text-gray-400'}`}>
                                {h.pct}%
                              </span>
                              <span className="text-[10px] text-gray-500 font-mono shrink-0 w-16 text-right">{h.earned}/{h.total}</span>
                            </div>
                          </div>
                          <motion.span
                            animate={{ rotate: isOpen ? 180 : 0 }}
                            transition={{ duration: 0.25, ease: 'easeOut' }}
                            className="shrink-0 text-gray-500 flex items-center justify-center"
                          >
                            <ChevronDown size={16} />
                          </motion.span>
                        </button>

                        <AnimatePresence initial={false}>
                          {isOpen && (
                            <motion.div
                              initial={{ height: 0, opacity: 0 }}
                              animate={{ height: 'auto', opacity: 1 }}
                              exit={{ height: 0, opacity: 0 }}
                              transition={{ duration: 0.28, ease: 'easeOut' }}
                              className="overflow-hidden"
                            >
                              <div className="px-3 pb-3">
                                <div className="rounded-xl bg-black/30 border border-white/[0.06] p-3 space-y-2.5">
                                  <div className="flex items-center justify-between gap-3 px-1">
                                    <span className="text-[9px] font-black uppercase tracking-widest text-gray-500">
                                      {ti('Logros', 'Achievements')}
                                    </span>
                                    <span className="text-[10px] font-bold text-gray-500">
                                      {isDone
                                        ? ti('Todos desbloqueados', 'All unlocked')
                                        : `${ti('faltan', 'missing')} ${remaining} ${ti('de', 'of')} ${h.total}`}
                                    </span>
                                  </div>

                                  {rowLoading ? (
                                    <div className="space-y-2">
                                      {[1, 2, 3].map(n => (
                                        <motion.div
                                          key={n}
                                          className="h-14 rounded-xl bg-white/[0.03]"
                                          animate={{ opacity: [0.4, 0.8, 0.4] }}
                                          transition={{ duration: 1.4, repeat: Infinity, ease: 'easeInOut', delay: n * 0.12 }}
                                        />
                                      ))}
                                    </div>
                                  ) : rowError ? (
                                    <div className="flex items-start gap-3 px-1 py-4">
                                      <AlertTriangle size={16} className="text-yellow-500/60 shrink-0 mt-0.5" />
                                      <p className="text-[11px] text-gray-400 leading-relaxed">
                                        {ti(
                                          'No hay una lista de logros guardada para este juego. Selecciónalo en la lista para descargarla.',
                                          'No cached achievement list for this game. Select it in the list to download it.'
                                        )}
                                      </p>
                                    </div>
                                  ) : sortedAchievements && sortedAchievements.length > 0 ? (
                                    <div className="max-h-[300px] overflow-y-auto custom-scrollbar space-y-1.5 pr-1">
                                      {sortedAchievements.map(ach => {
                                        const iconSrc = ach.earned ? ach.icon : (ach.icongray || ach.icon);
                                        const when = ach.earned ? formatEarnedDate(ach.earned_time) : null;
                                        return (
                                          <div
                                            key={ach.name}
                                            className={`flex items-start gap-3 p-2.5 rounded-xl border transition-colors duration-300 ${
                                              ach.earned ? 'bg-white/[0.04] border-white/[0.08]' : 'bg-black/20 border-white/[0.05]'
                                            }`}
                                          >
                                            {iconSrc ? (
                                              <img
                                                src={iconSrc}
                                                alt=""
                                                className={`w-9 h-9 rounded-lg shrink-0 ${ach.earned ? '' : 'opacity-40 grayscale'}`}
                                              />
                                            ) : (
                                              <div className="w-9 h-9 rounded-lg bg-white/[0.04] border border-white/[0.08] flex items-center justify-center shrink-0">
                                                <Trophy size={13} className="text-gray-700" />
                                              </div>
                                            )}
                                            <div className="min-w-0 flex-1">
                                              <p className={`text-[12px] font-bold truncate ${ach.earned ? 'text-white/90' : 'text-gray-400'}`}>
                                                {ach.display_name || ach.name}
                                              </p>
                                              <p className="text-[11px] text-gray-500 mt-0.5 leading-relaxed">
                                                {ach.hidden && !ach.earned ? ti('Logro oculto', 'Hidden achievement') : (ach.description || '—')}
                                              </p>
                                              {ach.earned && (
                                                <p className="text-[10px] text-accent/80 font-bold mt-1">
                                                  {when
                                                    ? `${ti('Desbloqueado el', 'Unlocked on')} ${when}`
                                                    : ti('Desbloqueado', 'Unlocked')}
                                                </p>
                                              )}
                                            </div>
                                            <div
                                              className={`shrink-0 w-7 h-7 rounded-full border flex items-center justify-center ${
                                                ach.earned
                                                  ? 'bg-accent/15 border-accent/30 text-accent'
                                                  : 'bg-white/[0.04] border-white/10 text-gray-600'
                                              }`}
                                              title={ach.earned ? ti('Desbloqueado', 'Unlocked') : ti('Bloqueado', 'Locked')}
                                            >
                                              {ach.earned ? <CheckCircle2 size={14} /> : <Lock size={12} />}
                                            </div>
                                          </div>
                                        );
                                      })}
                                    </div>
                                  ) : (
                                    <div className="text-center py-6 text-gray-500 text-[11px] font-bold">
                                      {ti('No hay detalles guardados para este juego.', 'No cached details for this game.')}
                                    </div>
                                  )}
                                </div>
                              </div>
                            </motion.div>
                          )}
                        </AnimatePresence>
                      </motion.div>
                    );
                  })
                )}
              </div>
            </motion.div>
          </motion.div>
          )}
        </AnimatePresence>,
        document.body
      )}
    </div>
  );
};

// --- Onboarding ---

type OnboardingStep = { icon: any; title: string; titleEn: string; body: string; bodyEn: string };

const ONBOARDING_STEPS: OnboardingStep[] = [
  {
    icon: Zap,
    title: 'Bienvenido a Ragnarok Launcher',
    titleEn: 'Welcome to Ragnarok Launcher',
    body: 'Te va a llevar un minuto entender cómo funciona todo — es un paso a paso rápido antes de arrancar.',
    bodyEn: "It'll take a minute to get the hang of it — a quick walkthrough before you dive in.",
  },
  {
    icon: Download,
    title: '1. Instalá el Plugin',
    titleEn: '1. Install the Plugin',
    body: 'Andá a Inicio y presioná "Instalar Plugin" una sola vez. Sin esto, Steam no puede leer los juegos que agregues — es el primer paso siempre.',
    bodyEn: 'Go to Home and press "Install Plugin" once. Without this, Steam can\'t read any game you add — it always comes first.',
  },
  {
    icon: Gamepad2,
    title: '2. Elegí un juego',
    titleEn: '2. Pick a game',
    body: 'Buscá un juego en Games o el Catálogo y presioná "Instalar". Después andá a Inicio y presioná "Reiniciar Steam" — sin este paso, Steam no va a mostrar el juego recién agregado.',
    bodyEn: 'Search for a game in Games or the Catalog and press "Install". Then go to Home and press "Restart Steam" — without this step, Steam won\'t show the game you just added.',
  },
  {
    icon: ShieldOff,
    title: '3. Bypass',
    titleEn: '3. Bypass',
    body: 'Si un juego que ya tenés comprado necesita un archivo de bypass, lo encontrás en la pestaña Bypass. Si pide contraseña, probá "CW.FIX" — es la que usa esta fuente casi siempre.',
    bodyEn: 'If a game you already own needs a bypass file, you\'ll find it in the Bypass tab. If it asks for a password, try "CW.FIX" — it\'s what this source almost always uses.',
  },
  {
    icon: Trophy,
    title: '¡Listo!',
    titleEn: "You're all set!",
    body: 'Ya sabés lo básico. Si algo falla, la pestaña Soporte manda un ticket directo por Discord.',
    bodyEn: 'You know the basics now. If something breaks, the Support tab sends a ticket straight to Discord.',
  },
];

const OnboardingModal = ({ onClose }: { onClose: () => void }) => {
  const { lang, setLang } = useLanguage();
  const es = lang === 'es';
  const [step, setStep] = useState(0);
  const isLast = step === ONBOARDING_STEPS.length - 1;
  const current = ONBOARDING_STEPS[step];
  const Icon = current.icon;

  return createPortal(
    <AnimatePresence>
      <motion.div
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        className="fixed inset-0 z-[300] flex items-center justify-center bg-black/70 backdrop-blur-sm p-4"
      >
        <motion.div
          initial={{ opacity: 0, y: 12, scale: 0.97 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: 12, scale: 0.97 }}
          transition={{ duration: 0.2, ease: 'easeOut' }}
          className="relative w-full max-w-md rounded-2xl bg-[#0d0e12] border border-white/[0.08] p-7 shadow-2xl overflow-hidden text-center"
        >
          <div
            className="absolute inset-0 pointer-events-none opacity-[0.04]"
            style={{ backgroundImage: 'linear-gradient(rgba(255,255,255,0.6) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.6) 1px, transparent 1px)', backgroundSize: '24px 24px' }}
          />
          <div className="absolute -top-16 left-1/2 -translate-x-1/2 w-48 h-48 bg-accent/15 blur-[70px] rounded-full pointer-events-none" />

          <button
            onClick={onClose}
            className="absolute top-4 right-4 z-20 text-gray-600 hover:text-white transition-colors"
            title={es ? 'Saltar' : 'Skip'}
          >
            <X size={16} />
          </button>

          <div className="relative z-10 flex flex-col items-center">
            <motion.div
              key={step}
              initial={{ opacity: 0, scale: 0.9 }}
              animate={{ opacity: 1, scale: 1 }}
              transition={{ duration: 0.25, ease: 'easeOut' }}
              className="relative w-16 h-16 mb-5"
            >
              <div className="absolute inset-0 rounded-2xl bg-accent blur-md opacity-40" />
              <div className="relative z-10 w-16 h-16 rounded-2xl bg-accent border border-accent/40 flex items-center justify-center shadow-lg shadow-accent/25">
                <Icon size={26} className="text-white" />
              </div>
            </motion.div>

            {step === 0 && (
              <motion.div
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ duration: 0.25, ease: 'easeOut' }}
                className="flex items-center gap-2 mb-5"
              >
                <button
                  onClick={() => setLang('es')}
                  className={`px-4 py-1.5 rounded-full text-[10px] font-black uppercase tracking-widest border transition-all ${
                    lang === 'es'
                      ? 'bg-accent/15 border-accent/50 text-white'
                      : 'bg-white/[0.03] border-white/[0.08] text-gray-500 hover:text-white hover:border-white/[0.15]'
                  }`}
                >
                  <span className="inline-flex items-center gap-1.5">
                    <FlagIcon code="es" className="w-4 h-[11px]" />
                    Español
                  </span>
                </button>
                <button
                  onClick={() => setLang('en')}
                  className={`px-4 py-1.5 rounded-full text-[10px] font-black uppercase tracking-widest border transition-all ${
                    lang === 'en'
                      ? 'bg-accent/15 border-accent/50 text-white'
                      : 'bg-white/[0.03] border-white/[0.08] text-gray-500 hover:text-white hover:border-white/[0.15]'
                  }`}
                >
                  <span className="inline-flex items-center gap-1.5">
                    <FlagIcon code="en" className="w-4 h-[11px]" />
                    English
                  </span>
                </button>
              </motion.div>
            )}

            <motion.div key={`text-${step}`} initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.25, ease: 'easeOut' }}>
              <h4 className="text-base font-black text-white/90 uppercase tracking-wide mb-2">
                {es ? current.title : current.titleEn}
              </h4>
              <p className="text-[12px] text-gray-400 font-semibold leading-relaxed max-w-xs mx-auto mb-6">
                {es ? current.body : current.bodyEn}
              </p>
            </motion.div>

            <div className="flex items-center gap-1.5 mb-6">
              {ONBOARDING_STEPS.map((_, i) => (
                <div
                  key={i}
                  className={`h-1.5 rounded-full transition-all duration-300 ${i === step ? 'w-6 bg-accent' : 'w-1.5 bg-white/15'}`}
                />
              ))}
            </div>

            <div className="flex items-center gap-2 w-full">
              {step > 0 && (
                <button
                  onClick={() => setStep(s => s - 1)}
                  className="px-4 py-2.5 rounded-xl bg-white/5 hover:bg-white/10 border border-white/10 text-xs text-gray-300 hover:text-white font-black uppercase tracking-wider transition-all flex items-center gap-1.5"
                >
                  <ChevronLeft size={13} />
                  {es ? 'Atrás' : 'Back'}
                </button>
              )}
              <button
                onClick={() => (isLast ? onClose() : setStep(s => s + 1))}
                className="flex-1 py-2.5 rounded-xl bg-accent text-white hover:brightness-110 hover:-translate-y-0.5 text-[11px] font-black uppercase tracking-widest transition-all shadow-lg shadow-accent/25 flex items-center justify-center gap-1.5"
              >
                {isLast ? (es ? 'Empezar' : 'Get Started') : (es ? 'Siguiente' : 'Next')}
                {!isLast && <ChevronRight size={13} />}
              </button>
            </div>
          </div>
        </motion.div>
      </motion.div>
    </AnimatePresence>,
    document.body
  );
};

// --- Main ---

// Module-level (not per-effect) guard for the F6 handler below — shared by
// every listener instance regardless of how many end up registered, so a
// single physical keypress can never produce more than one toggle/toast
// even if two listeners briefly coexist (e.g. a stale one left over from a
// dev hot-reload) and both receive the same native keydown event.
let f6ToggleLockAt = 0;

const MainContent = () => {
  const { t } = useLanguage();
  const ti = useTranslateInline();
  const { bgPath } = useTheme();
  const [activeTab, setActiveTab] = useState('Home');

  // First-ever launch only — walks through Instalar Plugin → elegir un
  // juego → Bypass, since several support tickets came from people stuck
  // on exactly these basic steps. Replayable later from Ajustes.
  const [showOnboarding, setShowOnboarding] = useState(() => !localStorage.getItem('rl_onboarding_seen'));
  const closeOnboarding = () => {
    localStorage.setItem('rl_onboarding_seen', 'true');
    setShowOnboarding(false);
  };

  // F6 "panic key" — hides the Zona +18 sidebar entry and, if you're on it
  // right now, bounces you back to Home instantly. Persisted so it stays
  // hidden across restarts until toggled back with F6 again.
  const [adultsHidden, setAdultsHidden] = useState(() => localStorage.getItem('rl_adults_hidden') === 'true');
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key !== 'F6' || e.repeat) return;
      e.preventDefault();
      const now = Date.now();
      if (now - f6ToggleLockAt < 400) return;
      f6ToggleLockAt = now;
      setAdultsHidden(prev => {
        const next = !prev;
        localStorage.setItem('rl_adults_hidden', String(next));
        notify(
          next ? ti('Zona +18 oculta (F6 para mostrarla de nuevo)', 'Zona +18 hidden (press F6 to show it again)')
               : ti('Zona +18 visible de nuevo', 'Zona +18 visible again'),
          'info',
          { key: 'adults-f6-toggle' }
        );
        return next;
      });
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, []);
  useEffect(() => {
    if (adultsHidden) setActiveTab(prev => (prev === 'Adults' ? 'Home' : prev));
  }, [adultsHidden]);
  // Quick links from the tray popup (see TrayPopup.tsx / tray_navigate).
  useEffect(() => {
    const unlisten = listen<string>('tray-navigate', (e) => setActiveTab(e.payload));
    return () => { unlisten.then((f) => f()); };
  }, []);

  const [sidebarMode, setSidebarModeRaw] = useState<'hover' | 'arrow'>(
    () => (localStorage.getItem('rl_sidebar_mode') as 'hover' | 'arrow') ?? 'arrow'
  );
  const setSidebarMode = (m: 'hover' | 'arrow') => { setSidebarModeRaw(m); localStorage.setItem('rl_sidebar_mode', m); };
  const [sidebarCollapsed, setSidebarCollapsedRaw] = useState<boolean>(
    () => (localStorage.getItem('rl_sidebar_collapsed') ?? 'true') === 'true'
  );
  const setSidebarCollapsed = (v: boolean) => { setSidebarCollapsedRaw(v); localStorage.setItem('rl_sidebar_collapsed', String(v)); };
  const [sidebarHovering, setSidebarHovering] = useState(false);
  const sidebarExpanded = sidebarMode === 'hover' ? sidebarHovering : !sidebarCollapsed;
  const [steamPath, setSteamPath] = useState('C:\\Program Files (x86)\\Steam');
  const [games, setGames] = useState<Game[]>([]);
  // True until the very first catalog load resolves — `games.length` starts
  // at 0 either way, so without this the Home stats briefly flashed "0"
  // before the real totals were in, which read as if the app was broken.
  const [catalogLoading, setCatalogLoading] = useState(true);
  // Catalog syncs in flight — a catalog switch runs one after startup's.
  const [catalogSyncs, setCatalogSyncs] = useState(0);

  // Steam's best sellers and most played for the Games filters. The backend
  // keeps them for six hours; asking again on that schedule keeps an app that
  // lives in the tray for days from showing last week's charts.
  const [steamRankings, setSteamRankings] = useState<SteamRankings | null>(null);
  useEffect(() => {
    const load = () => {
      invoke<SteamRankings>('get_steam_rankings')
        .then(r => { if (r.top_sellers.length > 0 || r.most_played.length > 0) setSteamRankings(r); })
        .catch(() => { });
    };
    load();
    const timer = window.setInterval(load, 6 * 3_600_000);
    return () => clearInterval(timer);
  }, []);
  // Manifest-code servers go down and get blocked; asking all three in turn
  // costs nothing and saves a download that would otherwise never start. Set
  // up on every launch, and quietly skipped if the user wrote their own
  // manifest.lua.
  useEffect(() => {
    if (!steamPath) return;
    invoke<string>('ensure_manifest_chain', { steamPath }).catch(() => { });
  }, [steamPath]);
  const [steamInstallChoices, setSteamInstallChoices] = useState<string[]>([]);
  const [selectedGame, setSelectedGame] = useState<Game | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<UpdateInfo | null>(null);
  const [showDonate, setShowDonate] = useState(false);
  const [showUpdateOverlay, setShowUpdateOverlay] = useState(false);
  const { notify } = useNotify();

  // The catalog's own nsfw tag comes from a loosely-curated external source
  // and is sometimes wrong (non-adult games landing in the +18 tab, or the
  // reverse). check_adult_content_batch asks Steam's own store API for the
  // official content_descriptors instead — corrections get cached here so a
  // game only ever needs to be checked once.
  const [nsfwOverrides, setNsfwOverrides] = useState<Record<string, boolean>>(() => {
    try { return JSON.parse(localStorage.getItem('rl_nsfw_overrides') ?? '{}'); } catch { return {}; }
  });
  const applyNsfwOverrides = useCallback((updates: Record<string, boolean>) => {
    setNsfwOverrides(prev => {
      const next = { ...prev, ...updates };
      localStorage.setItem('rl_nsfw_overrides', JSON.stringify(next));
      return next;
    });
  }, []);
  // Games whose catalog nsfw tag is incorrect — force them to the normal tab.
  const NSFW_FALSE_OVERRIDE = new Set(['3949550', '3751260']);
  const isEffectivelyNsfw = useCallback(
    (g: Game) => NSFW_FALSE_OVERRIDE.has(g.id) ? false : (nsfwOverrides[g.id] ?? g.nsfw ?? false),
    [nsfwOverrides]
  );
  // Memoized so CatalogView (wrapped in React.memo) actually gets a stable
  // `games` prop across renders that don't touch the catalog — passing a
  // fresh games.filter(...) array literal inline defeated memo every time
  // (new array reference every render), which kept re-firing CatalogView's
  // own `useEffect(() => setPage(1), [q, selectedCategory, games])` and was
  // the real cause of the "Maximum update depth exceeded" warning / the lag
  // when switching tabs or opening the game-detail panel — both trigger
  // MainContent re-renders (bell/notifications/preload-progress state, etc.)
  // that have nothing to do with the catalog itself.
  const visibleCatalogGames = useMemo(() => games.filter(g => !isEffectivelyNsfw(g)), [games, isEffectivelyNsfw]);
  const adultsCatalogGames = useMemo(() => games.filter(g => isEffectivelyNsfw(g)), [games, isEffectivelyNsfw]);

  const [nsfwCheck, setNsfwCheck] = useState<{ checked: number; total: number } | null>(null);
  const handleVerifyNsfwClassification = useCallback(async () => {
    if (nsfwCheck) return;
    const pending = games.filter(g => isEffectivelyNsfw(g) && !(g.id in nsfwOverrides));
    if (pending.length === 0) {
      notify('Todos los juegos en Zona +18 ya fueron verificados con Steam.', 'info');
      return;
    }
    setNsfwCheck({ checked: 0, total: pending.length });
    let corrected = 0;
    const CHUNK = 40;
    // Accumulated locally and committed once at the end. Applying each chunk
    // as it arrived changed `nsfwOverrides` ~75 times for a full catalog, and
    // every one of those re-filtered all 69,560 games twice and handed
    // CatalogView a new `games` array — which fires its own
    // `setPage(1)` effect. The grid jumped back to page one about seventy-five
    // times while the user was trying to browse it.
    const collected: Record<string, boolean> = {};
    for (let i = 0; i < pending.length; i += CHUNK) {
      const chunk = pending.slice(i, i + CHUNK).map(g => g.id);
      try {
        const result = await invoke<Record<string, boolean>>('check_adult_content_batch', { appIds: chunk });
        Object.assign(collected, result);
        corrected += Object.values(result).filter(v => !v).length;
      } catch { /* keep going with the next chunk */ }
      setNsfwCheck({ checked: Math.min(i + CHUNK, pending.length), total: pending.length });
    }
    if (Object.keys(collected).length > 0) applyNsfwOverrides(collected);
    setNsfwCheck(null);
    notify(
      corrected > 0
        ? `Revisión completa: ${corrected} juego${corrected !== 1 ? 's' : ''} movido${corrected !== 1 ? 's' : ''} a Games.`
        : 'Revisión completa: la clasificación de Steam coincide con la del catálogo.',
      'success'
    );
  }, [games, nsfwOverrides, nsfwCheck, isEffectivelyNsfw, applyNsfwOverrides, notify]);

  const [notifications, setNotifications] = useState<{ id: string; type: 'update' | 'new_games' | 'support_reply' | 'new_bypass' | 'hubcap'; message: string; read: boolean; timestamp: number }[]>([]);
  const [bellOpen, setBellOpen] = useState(false);
  const bellRef = useRef<HTMLDivElement>(null);

  const addNotification = useCallback((type: 'update' | 'new_games' | 'support_reply' | 'new_bypass' | 'hubcap', message: string) => {
    setNotifications(prev => {
      if (type === 'update' && prev.some(n => n.type === 'update' && !n.read)) return prev;
      return [{ id: `${type}-${Date.now()}`, type, message, read: false, timestamp: Date.now() }, ...prev];
    });
  }, []);

  const dismissNotification = useCallback((id: string) => {
    setNotifications(prev => prev.filter(n => n.id !== id));
  }, []);

  // Hubcap notices: the day's downloads used up, and a key that has expired or
  // is about to. Until now both only showed as a quiet badge colour, and a
  // user found out when installs had silently been going through Ryuu.
  //
  // Checked at startup, after every install and every half hour; each notice at
  // most once a day, so someone who already knows is not told again on every
  // check. Read through a ref so the effect runs once, not on every render.
  const hubcapNoticeTools = useRef({ notify, ti, addNotification });
  hubcapNoticeTools.current = { notify, ti, addNotification };
  useEffect(() => {
    const onceToday = (id: string, message: string, kind: 'error' | 'info') => {
      const storageKey = `rl_hubcap_notice_${id}`;
      const today = new Date().toISOString().slice(0, 10);
      try {
        if (localStorage.getItem(storageKey) === today) return;
        localStorage.setItem(storageKey, today);
      } catch { }
      hubcapNoticeTools.current.notify(message, kind);
      hubcapNoticeTools.current.addNotification('hubcap', message);
    };

    const check = async () => {
      const { ti } = hubcapNoticeTools.current;
      const { catalogSource, hubcapApiKey } = readCatalogSource();
      if (!HUBCAP_KEY_RE.test(hubcapApiKey)) return;
      const renew = ti(
        'Renuévala en hubcapmanifest.com y pégala en Ajustes → Fuente de juegos.',
        'Renew it at hubcapmanifest.com and paste it in Settings → Game source.'
      );
      try {
        const usage = await fetchHubcapUsage(hubcapApiKey, true);
        if (usage.expired) {
          onceToday('expired', ti(`Tu clave de Hubcap venció. ${renew}`, `Your Hubcap key has expired. ${renew}`), 'error');
        } else if (usage.days_left !== null && usage.days_left <= 3) {
          const when = usage.days_left === 0
            ? ti('hoy', 'today')
            : ti(`en ${usage.days_left} día${usage.days_left === 1 ? '' : 's'}`, `in ${usage.days_left} day${usage.days_left === 1 ? '' : 's'}`);
          onceToday('expiring', ti(`Tu clave de Hubcap vence ${when}. ${renew}`, `Your Hubcap key expires ${when}. ${renew}`), 'info');
        }
        // Only when Hubcap is the chosen source: with Ryuu chosen, Hubcap is
        // just a backup and running out of it changes nothing for the user.
        if (catalogSource === 'hubcap' && (usage.remaining <= 0 || !usage.can_make_requests)) {
          onceToday(
            'quota',
            ti(
              `Se acabaron tus ${usage.daily_limit} descargas de Hubcap por hoy. Hasta que se reinicie el límite, las instalaciones usan Ryuu.`,
              `Your ${usage.daily_limit} Hubcap downloads for today are used up. Until the limit resets, installs use Ryuu.`
            ),
            'error'
          );
        }
      } catch (err) {
        if (String(err).includes(HUBCAP_KEY_REJECTED)) {
          onceToday('expired', ti(`Hubcap rechazó tu clave: lo más probable es que haya vencido. ${renew}`, `Hubcap refused your key: it has most likely expired. ${renew}`), 'error');
        }
      }
    };

    void check();
    const onInstall = () => { void check(); };
    window.addEventListener(HUBCAP_USAGE_EVENT, onInstall);
    const timer = window.setInterval(() => { void check(); }, 30 * 60_000);
    return () => {
      window.removeEventListener(HUBCAP_USAGE_EVENT, onInstall);
      clearInterval(timer);
    };
  }, []);

  const unreadCount = notifications.filter(n => !n.read).length;

  // Poll any open support tickets (Discord threads) for developer replies —
  // runs regardless of which tab is active so replies surface via the bell
  // even if the user isn't on the Support tab when one arrives.
  useEffect(() => {
    const checkSupportReplies = async () => {
      const threads = loadSupportThreads();
      if (threads.length === 0) return;
      const updated: SupportThreadRecord[] = [];
      let newReplyCount = 0;
      let changed = false;
      for (const thread of threads) {
        // Archived tickets are done — keep them in the history (so "Tus
        // Tickets" still shows the closed conversation) but stop polling.
        if (thread.archived) { updated.push(thread); continue; }
        try {
          const result = await invoke<{ replies: SupportThreadReply[]; archived: boolean }>(
            'discord_check_thread_replies',
            { threadId: thread.threadId, afterMessageId: thread.lastMessageId }
          );
          let lastMessageId = thread.lastMessageId;
          for (const reply of result.replies) {
            const preview = reply.content.trim() ? reply.content.trim().slice(0, 100) : '(archivo adjunto)';
            addNotification('support_reply', `${reply.author}: ${preview}`);
            lastMessageId = reply.id;
            newReplyCount++;
          }
          if (result.replies.length > 0 || result.archived !== thread.archived) changed = true;
          updated.push({
            ...thread,
            lastMessageId,
            replies: result.replies.length > 0 ? [...thread.replies, ...result.replies] : thread.replies,
            archived: result.archived,
            unreadCount: thread.unreadCount + result.replies.length,
          });
        } catch {
          // Transient network/API error — keep the thread for the next poll.
          updated.push(thread);
        }
      }
      // Only touch localStorage/dispatch the refresh event when something
      // actually changed — this now runs every few seconds for near-instant
      // delivery, so skipping no-op writes keeps that cheap.
      if (changed) saveSupportThreads(updated);
      if (newReplyCount > 0) playSupportReplySound();
      return newReplyCount > 0 || changed;
    };

    // True push (a persistent Discord Gateway WebSocket) would need a much
    // heavier always-connected client plus a privileged intent the bot owner
    // has to enable manually in Discord's dev portal, so this stays on REST.
    //
    // What changed is the cadence. A flat 3-second poll ran forever, for every
    // open ticket, whether or not anyone was on the other end and whether or
    // not the window was even visible — measured at roughly 14 MB an hour per
    // ticket, which is real money on a metered or satellite connection.
    //
    // A live conversation still feels instant: the interval resets to 3s the
    // moment anything arrives. It only stretches once a ticket has been quiet,
    // which is exactly when nobody is waiting on it.
    const FAST_MS = 3_000;
    const MAX_MS = 60_000;
    let delay = FAST_MS;
    let timer: number | undefined;

    const tick = async () => {
      let active = false;
      try {
        active = (await checkSupportReplies()) ?? false;
      } catch {
        // A failed poll is not activity; let the backoff keep growing rather
        // than hammering a server that is refusing us.
      }
      // Nothing to say while the window is hidden — the user cannot see the
      // bell anyway, and a minimised launcher was the worst offender here.
      const hidden = typeof document !== 'undefined' && document.hidden;
      delay = active && !hidden ? FAST_MS : Math.min(delay * 2, MAX_MS);
      timer = window.setTimeout(tick, delay);
    };

    // Coming back to the window is the strongest hint that the user wants to
    // see replies now, so drop straight back to the fast cadence.
    const onVisible = () => {
      if (document.hidden) return;
      delay = FAST_MS;
      if (timer !== undefined) clearTimeout(timer);
      void tick();
    };
    document.addEventListener('visibilitychange', onVisible);

    void tick();
    return () => {
      if (timer !== undefined) clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [addNotification]);

  const buildNewGamesMessage = useCallback((newGames: Game[]) => {
    const n = newGames.length;
    if (n === 0) return null;
    const nameCache = getCleanNameCache();
    const names = newGames
      .map(g => {
        const raw = g.name?.trim() ?? '';
        if (!raw || raw.startsWith('Game ID:')) return nameCache[g.id] ?? null;
        return raw;
      })
      .filter(Boolean) as string[];
    if (names.length === 0) return t.notifications.newGames(n);
    if (n === 1) return `Nuevo juego: ${names[0]}`;
    if (n <= 3) return `${n} juegos nuevos: ${names.join(', ')}`;
    return `${n} juegos nuevos · ${names.slice(0, 2).join(', ')} y ${n - 2} más`;
  }, [t.notifications]);

  useEffect(() => {
    const handleClick = (e: MouseEvent) => {
      if (bellRef.current && !bellRef.current.contains(e.target as Node)) setBellOpen(false);
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, []);

  // Unions every real Steam-installed AppID (any game — including ones the
  // Ryuu catalog has never heard of, e.g. a game the user genuinely bought)
  // with Ragnarok's own catalog-aware install detection. Without the second
  // call, a legitimately-owned game outside the catalog never appears in
  // `games` at all, which meant Library only ever showed Ragnarok-managed
  // titles — DLC Manager/Unlock couldn't be pointed at anything else.
  const getAllInstalledIds = useCallback(async (path: string) => {
    const [catalogAware, allReal, managed, sizes] = await Promise.all([
      invoke<string[]>('get_installed_app_ids', { steamPath: path }).catch(() => [] as string[]),
      invoke<string[]>('get_really_installed_app_ids', { steamPath: path }).catch(() => [] as string[]),
      invoke<string[]>('get_ragnarok_managed_app_ids', { steamPath: path }).catch(() => [] as string[]),
      // Leído junto a los demás en el mismo Promise.all: es otra lectura de
      // los mismos appmanifest que ya se están recorriendo, así que no agrega
      // una espera propia.
      invoke<Record<string, number>>('get_installed_sizes', { steamPath: path }).catch(() => ({})),
    ]);
    return { all: new Set([...catalogAware, ...allReal]), managed: new Set(managed), sizes };
  }, []);

  // Shared by the instant offline-cache paint and the real network sync —
  // folds Steam/Ragnarok's locally-known installed-app IDs into a raw
  // catalog array, and appends any installed app the catalog doesn't know
  // about at all — either manually-added (via "Agregar Lua Manualmente",
  // still Ragnarok-managed) or a real Steam game the user owns outside
  // Ragnarok entirely, marked `foreign` so GameCard can hide destructive
  // actions for it (see the `foreign` field's doc comment on Game).
  const mergeCatalogWithInstalled = useCallback((
    catalog: Game[],
    installedIds: Set<string>,
    managedIds: Set<string>,
    sizes: Record<string, number> = {},
  ) => {
    const nameCache = getCleanNameCache();
    // One card per game even when a catalog lists an id twice: React keys the
    // grid by id, and a repeated key leaves stale copies of a card on screen.
    const seenIds = new Set<string>();
    catalog = catalog.filter(g => !seenIds.has(g.id) && (seenIds.add(g.id), true));
    const withStatus = catalog.map(g => ({
      ...g,
      status: installedIds.has(g.id) ? 'Installed' : g.status,
      name: nameCache[g.id] ?? g.name,
      // El backend siempre mandaba size_bytes en null, así que el chip de
      // tamaño de la tarjeta nunca se dibujaba para ningún juego. Acá se
      // completa con el SizeOnDisk real del appmanifest.
      size_bytes: sizes[g.id] ?? g.size_bytes,
    }));
    const catalogIds = new Set(catalog.map(g => g.id));
    const extraGames: Game[] = Array.from(installedIds)
      .filter(id => !catalogIds.has(id))
      .map(id => ({
        id,
        name: nameCache[id] ?? `Game ID: ${id}`,
        status: 'Installed',
        foreign: !managedIds.has(id),
        size_bytes: sizes[id],
      }));
    return extraGames.length > 0 ? [...withStatus, ...extraGames] : withStatus;
  }, []);

  // Paints the Library/Home instantly from whatever catalog was last
  // successfully synced — no network call at all, just a disk read plus a
  // fully-local installed-apps scan — so the UI never has to sit blank
  // while sync_games' network fetch (up to 30s+60s+90s of retries when the
  // connection is slow or down) is still in flight. The real loadCatalog
  // below always runs afterward and overwrites `games` with fresh data once
  // it resolves, so this is purely a "don't make them wait" head start.
  // `replace` is for a catalog switch: the list on screen belongs to the other
  // catalog, so it is swapped out even when there is nothing saved to show.
  const loadCachedCatalogFast = useCallback(async (path: string, replace = false) => {
    try {
      const cachedPath = await invoke<string | null>('get_cached_catalog_path', { catalogSource: readCatalogSource().catalogSource });
      if (!cachedPath) {
        // No saved copy of the chosen catalog yet — the first launch after
        // switching to Hubcap. The installed games are still known locally,
        // and leaving the Library at "0 juegos" until a multi-minute catalog
        // download finishes reads as every game having been removed.
        const { all: installedIds, managed: managedIds, sizes } = await getAllInstalledIds(path);
        const installedOnly = mergeCatalogWithInstalled([], installedIds, managedIds, sizes);
        setGames(prev => (prev.length > 0 && !replace ? prev : installedOnly));
        return false;
      }
      const { convertFileSrc } = await import('@tauri-apps/api/tauri');
      const res = await fetch(convertFileSrc(cachedPath));
      if (!res.ok) return false;
      const catalog: Game[] = await res.json();
      if (!Array.isArray(catalog) || catalog.length === 0) return false;
      const { all: installedIds, managed: managedIds, sizes } = await getAllInstalledIds(path);
      setGames(mergeCatalogWithInstalled(catalog, installedIds, managedIds, sizes));
      setCatalogLoading(false);
      return true;
    } catch {
      return false;
    }
  }, [mergeCatalogWithInstalled, getAllInstalledIds]);

  const loadCatalog = useCallback(async (path: string, force = false) => {
    setCatalogSyncs(n => n + 1);
    try {
      // sync_games returns the path to the catalog JSON file on disk (not the
      // array itself) — the full catalog is tens of MB, too large to pass
      // efficiently over Tauri's IPC channel directly.
      const requested = readCatalogSource();
      const catalogPath = await invoke<string>('sync_games', { githubUrl: '', force, ...requested });
      const { convertFileSrc } = await import('@tauri-apps/api/tauri');
      const res = await fetch(convertFileSrc(catalogPath));
      if (!res.ok) throw new Error(`Failed to read catalog file: HTTP ${res.status}`);
      const catalog: Game[] = await res.json();
      const { all: installedIds, managed: managedIds, sizes } = await getAllInstalledIds(path);
      const allGames = mergeCatalogWithInstalled(catalog, installedIds, managedIds, sizes);
      // A catalog that finishes after the user picked the other source is
      // stale. Switching Ryuu → Hubcap starts overlapping loads, and Ryuu's —
      // seconds, against Hubcap's minutes — used to land last and put Ryuu's
      // list back on screen with Hubcap selected.
      if (readCatalogSource().catalogSource !== requested.catalogSource) {
        return { total: catalog.length, newGames: [] };
      }
      setGames(allGames);
      if (force && requested.catalogSource === 'hubcap') {
        notify(
          ti(`Catálogo de Hubcap listo: ${catalog.length.toLocaleString()} juegos.`, `Hubcap catalog ready: ${catalog.length.toLocaleString()} games.`),
          'success'
        );
      }
      // Which catalog is actually on screen. Ajustes compares against this, so
      // picking an option that already looks selected still reloads when the
      // list showing is the other one.
      try { localStorage.setItem('rl_loaded_catalog_source', requested.catalogSource); } catch { }
      const newGames = allGames.filter(g => g.is_new);
      return { total: catalog.length, newGames };
    } catch (err) {
      // sync_games already retries through several fetch strategies
      // (including a DNS-over-HTTPS bypass for ISP-level DNS blocking, a
      // real pattern reported by users in some countries) before giving up
      // — if we're here, all of those failed. A VPN sidesteps ISP-level
      // blocking entirely, so surface that as an actionable next step
      // instead of a bare "failed" message the user has to ask support to
      // decode.
      // Hubcap failures are almost never the ISP: a rejected key, a rate
      // limit, a response in an unexpected shape. Pointing those users at a
      // VPN sends them after the wrong problem.
      if (readCatalogSource().catalogSource === 'hubcap') {
        notify(
          ti(
            `No se pudo cargar el catálogo de Hubcap: ${err}. Se sigue mostrando la lista anterior.`,
            `Could not load the Hubcap catalog: ${err}. The previous list is still shown.`
          ),
          'error'
        );
      } else {
        notify(
          `${t.notify.catalogErr}${err ? `: ${err}` : ''} — ${ti(
            'Si esto persiste, puede ser un bloqueo de tu proveedor de internet: prueba activar una VPN una vez y reabrir el programa.',
            'If this persists, your internet provider may be blocking it: try turning on a VPN once and reopening the app.'
          )}`,
          'error'
        );
      }
      // The catalog fetch failing shouldn't mean Library/Home show NOTHING —
      // detecting installed games needs no network at all (real Steam ACFs +
      // Ragnarok's own sidecar). Without this, a first-ever launch with no
      // internet (so loadCachedCatalogFast also has no cache to fall back
      // on) left `games` permanently empty for the whole session, hiding
      // even games the user already has installed — reported as "the
      // launcher acts like it has no internet, none of my installed games
      // show up either". Only fills in when still empty, so a genuinely
      // successful earlier load (or the fast cache path) is never clobbered.
      try {
        const { all: installedIds, managed: managedIds, sizes } = await getAllInstalledIds(path);
        if (installedIds.size > 0) {
          setGames(prev => prev.length > 0 ? prev : mergeCatalogWithInstalled([], installedIds, managedIds, sizes));
        }
      } catch { }
      return { total: 0, newGames: [] };
    } finally {
      setCatalogSyncs(n => n - 1);
    }
  }, [notify, t.notify.catalogErr, ti, mergeCatalogWithInstalled, getAllInstalledIds]);

  // Hubcap's catalog is saved every 2,000 games while it downloads. Showing
  // each save fills Games as the download goes, instead of leaving it empty
  // for the minutes Hubcap's rate limit stretches a first download to.
  const catalogCheckpointAt = useRef(0);
  useEffect(() => {
    if (!steamPath) return;
    let alive = true;
    let unlisten: (() => void) | undefined;
    listen<CatalogProgress>('catalog_progress', e => {
      if (!e.payload.checkpoint || readCatalogSource().catalogSource !== 'hubcap') return;
      const now = Date.now();
      if (now - catalogCheckpointAt.current < 15_000) return;
      catalogCheckpointAt.current = now;
      void loadCachedCatalogFast(steamPath);
    }).then(fn => {
      if (alive) unlisten = fn;
      else fn();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [steamPath, loadCachedCatalogFast]);

  // No catalog on screen yet: only installed games, none of which carries a
  // catalog image. Games shows a loading notice instead of the Library again.
  const catalogMissing = useMemo(() => !games.some(g => g.image), [games]);

  useEffect(() => {
    const load = async () => {
      // A previous pick from the picker below always wins over fresh
      // detection — otherwise it would ask again on every single launch.
      const savedOverride = localStorage.getItem('rl_steam_path_override');
      let path = 'C:\\Program Files (x86)\\Steam';
      try { path = await invoke<string>('get_steam_path'); } catch { }
      setSteamPath(savedOverride || path);

      // Fire-and-forget: paints Library/Home from the last successful sync
      // immediately, without waiting on the update check or the real
      // network-backed catalog sync below. loadCatalog() always runs after
      // and overwrites `games` with fresh data once it resolves.
      loadCachedCatalogFast(path);

      if (!savedOverride) {
        try {
          const installs = await invoke<string[]>('detect_steam_installations');
          if (installs.length > 1) setSteamInstallChoices(installs);
        } catch { }
      }

      // If the last auto-update attempt failed after this process had already
      // exited (installer blocked by an antivirus/AppLocker policy, or the
      // new exe never appeared), a marker file explains what happened — this
      // is the only chance to show it, since the app would otherwise just
      // silently offer "update available" again with no explanation.
      try {
        const failureMsg = await invoke<string | null>('check_previous_update_failure');
        if (failureMsg) notify(failureMsg, 'error');
      } catch { }

      // The report is still collected and left waiting in Support's
      // description field, but nothing announces it any more.
      //
      // The notice fired on things that are not crashes and that the user
      // cannot act on — Task Manager, a power cut, an antivirus stop, or the
      // PC restarting while the app sat in the tray — so it mostly read as
      // "the app is broken" when it was not. It also had no detail to offer
      // in those cases beyond "something happened".
      //
      // Kept as a silent read rather than removed outright for one reason:
      // this call is what deletes the backend's pending-report file. Dropping
      // it would leave that file on disk forever, and the report would be
      // stale by the time anyone opened Support.
      try {
        const crashReport = await invoke<string | null>('get_pending_crash_report');
        if (crashReport) {
          localStorage.setItem('rl_pending_crash_report', crashReport);
        }
      } catch { }

      try {
        const info = await invoke<UpdateInfo>('check_for_updates');
        if (info.has_update) {
          setPendingUpdate(info);
          setShowUpdateOverlay(true);
          addNotification('update', t.notifications.update(info.latest_version));
        }
      } catch { }

      const { newGames: initialNew } = await loadCatalog(path);
      setCatalogLoading(false);
      if (initialNew.length > 0) {
        const msg = buildNewGamesMessage(initialNew);
        if (msg) addNotification('new_games', msg);
      }
    };
    load();
  }, []);

  // Warms the image cache for INSTALLED games only. Everything else loads
  // lazily, per card, as the user scrolls.
  //
  // This used to pass `games.map(g => g.id)` — the entire Ryuu catalog. That
  // is 69,560 titles, one header image each, ~48 KB apiece, and it filled
  // %APPDATA%\com.ragnarok.launcher\image_cache with 66,116 JPEGs totalling
  // 3.1 GB. A user reported the launcher eating 3 GB shortly after installing
  // it, and this was the whole of it — for a machine with four games actually
  // installed.
  //
  // Nothing was lost by narrowing it: GameImage already fetches its own image
  // through get_cached_image_path when a card mounts, with in-flight dedup,
  // writing to the same folder. The preload was only front-loading downloads
  // for titles the user would never scroll past. Installed games stay worth
  // warming because Home and Library show them the moment the app opens.
  useEffect(() => {
    if (games.length === 0) return;
    const appIds = games.filter(g => g.status === 'Installed').map(g => g.id);
    if (appIds.length === 0) return;

    // No progress listener: the `preload` state it fed was never rendered
    // anywhere, so every one of its events re-rendered the whole MainContent
    // tree to display nothing. Reinstate it here if a progress bar is ever
    // actually wanted on screen.
    invoke('preload_images', { appIds }).catch(() => { });
  }, [games.length]);

  // El catálogo ya no se refresca solo.
  //
  // Antes esto consultaba `get_github_game_count` cada dos minutos y, si el
  // repo tenía más juegos que la lista en memoria, disparaba un
  // `loadCatalog(force)` completo — una descarga de ~17 MB y una reescritura
  // del caché entera, en segundo plano, sin que nadie la pidiera. Se saca
  // por pedido explícito: el catálogo se carga al arrancar y se actualiza
  // únicamente con el botón "Sincronizar catálogo" de Ajustes.



  // Repairs any library entry Steam is showing as "0 B", on its own.
  //
  // `repair_install_sizes` was written for exactly this and then never called
  // from anywhere — registered as a command and orphaned. The only automatic
  // path was the post-install poll, which watches for Steam to write its ACF
  // for about two minutes and then gives up; a user who takes longer to press
  // Install in Steam's own dialog, or who installs the game later, ends up with
  // an entry stuck at 0 B forever. That is the several-games-at-0-B state that
  // showed up in Steam's storage manager.
  //
  // Safe to run unattended: it reads each manifest first and only touches the
  // ones actually reporting zero, so real Steam installs are never rewritten,
  // and an app whose size no source knows is left alone rather than written
  // back as 0. Once shortly after startup, then hourly, so an entry that missed
  // the post-install window still gets corrected without the user asking.
  useEffect(() => {
    if (!steamPath) return;
    const repair = () => {
      invoke<number>('repair_install_sizes', { steamPath })
        .then(n => { if (n > 0) console.log(`[sizes] ${n} manifiesto(s) en 0 B corregidos.`); })
        .catch(() => { });
    };
    const startup = window.setTimeout(repair, 20_000);
    const interval = window.setInterval(repair, 3_600_000);
    return () => { clearTimeout(startup); clearInterval(interval); };
  }, [steamPath]);

  // Notifies via the bell when a new Bypass file shows up (Mediafire or
  // Google Drive), same idea as the "new games" check above — runs
  // regardless of which tab is active, and the actual fetches are cheap
  // now thanks to the 3-minute cache added on the backend. The first run
  // ever just records what's already there without notifying (nobody wants
  // a notification for every file that already existed before this shipped);
  // only files that show up AFTER that baseline trigger one.
  useEffect(() => {
    const BYPASS_SEEN_KEY = 'rl_bypass_seen_files';
    const checkNewBypass = async () => {
      try {
        const [mediafire, gdrive] = await Promise.all([
          invoke<MediafireFile[]>('fetch_mediafire_bypass').catch(() => []),
          invoke<MediafireFile[]>('fetch_gdrive_bypass').catch(() => []),
        ]);
        const allNames = [...mediafire, ...gdrive].map(f => f.filename);
        if (allNames.length === 0) return;

        const seenRaw = localStorage.getItem(BYPASS_SEEN_KEY);
        if (seenRaw === null) {
          localStorage.setItem(BYPASS_SEEN_KEY, JSON.stringify(allNames));
          return;
        }
        const seen = new Set<string>(JSON.parse(seenRaw));
        const newOnes = allNames.filter(name => !seen.has(name));
        if (newOnes.length > 0) {
          const msg = newOnes.length === 1
            ? `Nuevo bypass: ${newOnes[0]}`
            : newOnes.length <= 3
              ? `${newOnes.length} bypass nuevos: ${newOnes.join(', ')}`
              : `${newOnes.length} bypass nuevos · ${newOnes.slice(0, 2).join(', ')} y ${newOnes.length - 2} más`;
          addNotification('new_bypass', msg);
        }
        localStorage.setItem(BYPASS_SEEN_KEY, JSON.stringify(allNames));
      } catch { }
    };
    checkNewBypass();
    const interval = setInterval(checkNewBypass, 120_000);
    return () => clearInterval(interval);
  }, [addNotification]);

  // Background: batch-fetch names for games that still show "Game ID: X"
  useEffect(() => {
    if (games.length === 0) return;
    const unnamed = games.filter(g => {
      const name = g.name?.trim() ?? '';
      return !name || name.startsWith('Game ID:');
    });
    if (unnamed.length === 0) return;

    let cancelled = false;
    // Fallback: clear skeletons after 90s (enough for 200+ throttled requests)
    const fallbackTimer = setTimeout(() => {
      if (cancelled) return;
      setGames(prev => prev.map(g =>
        g.name?.startsWith('Game ID:') ? { ...g, name: `App ${g.id}` } : g
      ));
    }, 90_000);

    (async () => {
      // Skip IDs already confirmed unresolvable in a previous session
      const unresolvable: Set<string> = new Set(
        JSON.parse(localStorage.getItem('rl_unresolvable') ?? '[]')
      );
      const ids = unnamed.map(g => g.id).filter(id => !unresolvable.has(id));
      if (ids.length === 0) {
        clearTimeout(fallbackTimer);
        setGames(prev => prev.map(g =>
          g.name?.startsWith('Game ID:') ? { ...g, name: `App ${g.id}` } : g
        ));
        return;
      }

      const names = await fetchGameNames(ids);
      clearTimeout(fallbackTimer);
      if (cancelled) return;

      const cache = getCleanNameCache();
      const newUnresolvable: string[] = [];
      let hasNew = false;

      setGames(prev => prev.map(g => {
        if (!g.name?.startsWith('Game ID:')) return g;
        const resolved = names[g.id]?.[0];
        if (resolved) {
          cache[g.id] = resolved;
          hasNew = true;
          return { ...g, name: resolved };
        }
        // Permanently unresolvable — skip next session
        if (!unresolvable.has(g.id)) newUnresolvable.push(g.id);
        return { ...g, name: `App ${g.id}` };
      }));

      if (hasNew) localStorage.setItem('rl_names', JSON.stringify(cache));
      if (newUnresolvable.length > 0) {
        const updated = [...Array.from(unresolvable), ...newUnresolvable];
        localStorage.setItem('rl_unresolvable', JSON.stringify(updated));
      }
    })();
    return () => { cancelled = true; clearTimeout(fallbackTimer); };
  }, [games.length]);

  // Estado de DRM por juego, cacheado en disco.
  //
  // La fuente es el propio `drm_notice` de Steam, que es exactamente el texto
  // que su ficha muestra como "Incorporates 3rd-party DRM: Denuvo
  // Anti-tamper". `fetch_game_names` ya lo venía leyendo y devolviendo como
  // segundo elemento de la tupla, y el frontend tomaba sólo el nombre con
  // `names[id]?.[0]` — el dato llegaba y se tiraba.
  //
  // Se guarda en localStorage porque el DRM de un juego no cambia de un día
  // para otro: una vez preguntado, no se vuelve a preguntar nunca.
  const drmCacheRef = useRef<Record<string, string> | null>(null);
  const readDrmCache = useCallback((): Record<string, string> => {
    if (drmCacheRef.current) return drmCacheRef.current;
    // La clave lleva versión, y hace falta.
    //
    // Esta caché ya guardó dos formatos distintos que hoy son basura: primero
    // booleanos (la tarjeta dibuja el valor tal cual, y React no renderiza
    // nada para un booleano, así que salía una píldora ámbar vacía), y
    // después etiquetas genéricas como "DRM" de cuando el cartel marcaba
    // cualquier anti-tamper. Sanear por tipo no alcanzaba para la segunda:
    // "DRM" es una cadena perfectamente válida, así que sobrevivía la
    // limpieza y seguía apareciendo en juegos que ya no deberían marcarse.
    //
    // Subir la versión de la clave descarta ambas de una, y lo que se pierde
    // es una consulta por juego que se rehace sola.
    const clean: Record<string, string> = {};
    try {
      localStorage.removeItem('rl_drm');
      const raw = JSON.parse(localStorage.getItem(DRM_CACHE_KEY) ?? '{}');
      if (raw && typeof raw === 'object') {
        for (const [id, value] of Object.entries(raw as Record<string, unknown>)) {
          if (typeof value === 'string') clean[id] = value;
        }
      }
    } catch {
      localStorage.removeItem(DRM_CACHE_KEY);
    }
    drmCacheRef.current = clean;
    return clean;
  }, []);

  const drmInFlight = useRef<Set<string>>(new Set());

  const resolveDrmFor = useCallback(async (ids: string[]) => {
    const cache = readDrmCache();

    // Lo ya conocido se aplica sin pedir nada, y pisa lo que hubiera puesto
    // el catálogo: la ficha de Steam es la fuente exacta.
    if (ids.some(id => id in cache)) {
      setGames(prev => {
        let changed = false;
        const next = prev.map(g => {
          if (g.id in cache && g.drm !== cache[g.id]) {
            changed = true;
            return { ...g, drm: cache[g.id] };
          }
          return g;
        });
        return changed ? next : prev;
      });
    }

    const missing = ids.filter(id => !(id in cache) && !drmInFlight.current.has(id));
    if (missing.length === 0) return;
    // Acotado: una página de la grilla, no el catálogo entero.
    const batch = missing.slice(0, 40);
    batch.forEach(id => drmInFlight.current.add(id));

    try {
      const res = await fetchGameNames(batch);
      for (const id of batch) {
        // Un AppID que Steam no resuelve se marca como "sin DRM" igual: si no,
        // se vuelve a preguntar en cada scroll para siempre.
        cache[id] = res[id]?.[1] ?? '';
      }
      localStorage.setItem(DRM_CACHE_KEY, JSON.stringify(cache));
      setGames(prev => {
        let changed = false;
        const next = prev.map(g => {
          if (g.id in cache && g.drm !== cache[g.id]) {
            changed = true;
            return { ...g, drm: cache[g.id] };
          }
          return g;
        });
        return changed ? next : prev;
      });
    } catch {
      // Sin conexión: se reintenta en el próximo scroll.
      batch.forEach(id => drmInFlight.current.delete(id));
    }
  }, [readDrmCache]);

  const handleInstall = useCallback(async (appId: string) => {
    const game = games.find((g) => g.id === appId);
    const appName = game?.name ?? appId;
    notify(`${t.notify.preparing}${appId}`, 'info');
    try {
      // Refreshed whether the install worked or not: a failed attempt can
      // still have spent one of the day's Hubcap downloads.
      await invoke('install_game', { appId, appName, steamPath, ...readTicketSourceSettings() })
        .finally(() => window.dispatchEvent(new Event(HUBCAP_USAGE_EVENT)));
      notify(t.notify.injected, 'success');
      // Mark as installed in local state
      setGames((prev) =>
        prev.map((g) => (g.id === appId ? { ...g, status: 'Installed' } : g))
      );
      // Best-effort: stop Steam's own cloud sync from fighting the local save
      // for this app. Without this, Steam can decide the (empty/stale) cloud
      // copy for a non-owned/emulated app is authoritative and wipe local
      // progress on a later launch — never block the install on this.
      invoke('disable_steam_cloud_for_app', { steamPath, appId }).catch(() => {});
    } catch (err) {
      notify(`${t.notify.installErr}${err}`, 'error');
    }
  }, [games, notify, steamPath, t.notify]);

  const handleUninstall = useCallback((appId: string) => {
    setGames((prev) =>
      prev.map((g) => (g.id === appId ? { ...g, status: undefined } : g))
    );
  }, []);

  // What was running the last time we looked, so a game that disappears from
  // the list can be recognised as a session that just ended. Kept in a ref
  // rather than in the effect body because `games` changes often and a fresh
  // Set on every re-render would forget what was running a second ago.
  const runningGamesRef = useRef<Set<string>>(new Set());
  const backupBusyRef = useRef(false);

  // Backs up saves without depending on Google Drive, and at the moment that
  // actually matters: when a play session ends.
  //
  // Two things were wrong before. It opened with `is_gdrive_connected` and
  // returned early when that was false — no Drive, no backup of any kind —
  // even though do_local_backup and its `backup_saves_local` command were
  // already written, registered and unused. Anyone who never connected Drive,
  // or whose connection broke, had no protection at all while the code to
  // protect them sat there.
  //
  // And it only ran on window focus, which never happens for the person who
  // closes the game and walks away. Polling for a game that WAS running and
  // no longer is catches that, and covers games started from Steam directly
  // too, not just from the Play button here.
  useEffect(() => {
    if (!steamPath) return;
    const sp = steamPath;

    // Only games the user opted into, same as before — turning autosync on for
    // everything is not this fix's call to make.
    const eligible = () =>
      games.filter(
        g => g.status === 'Installed' && localStorage.getItem(`autosync_${g.id}`) === 'true'
      );

    const backupOne = async (game: Game, reason: string) => {
      const modTime = await invoke<number>('get_save_modified_time', {
        appId: game.id,
        steamPath: sp,
      }).catch(() => 0);
      const last = parseInt(localStorage.getItem(`last_auto_backup_${game.id}`) || '0');
      if (!(modTime > 0 && modTime > last)) return;

      // Drive when it is connected, disk when it is not. A local copy is not
      // as good as an off-machine one, but it is the difference between losing
      // a save to a bad patch and keeping it.
      const toDrive = await invoke<boolean>('is_gdrive_connected').catch(() => false);
      if (toDrive) {
        await invoke('backup_game_saves', { appId: game.id, steamPath: sp });
        notify(`Nube: ${game.name} guardado (${reason})`, 'success');
      } else {
        await invoke('backup_saves_local', { appId: game.id, steamPath: sp });
        notify(`Copia local: ${game.name} guardado (${reason})`, 'success');
      }
      localStorage.setItem(`last_auto_backup_${game.id}`, String(modTime));
    };

    // The guard matters: every pass walks the whole installed list with an
    // await inside, and two alt-tabs in quick succession used to start two
    // overlapping passes that raced each other writing last_auto_backup.
    const runPass = async (targets: Game[], reason: string) => {
      if (backupBusyRef.current || targets.length === 0) return;
      backupBusyRef.current = true;
      try {
        for (const game of targets) {
          try {
            await backupOne(game, reason);
          } catch (e) {
            console.error('Auto-backup failed', game.id, e);
          }
        }
      } finally {
        backupBusyRef.current = false;
      }
    };

    const checkExits = async () => {
      const ids = eligible().map(g => g.id);
      if (ids.length === 0) return;

      let nowRunning: string[];
      try {
        nowRunning = await invoke<string[]>('running_game_app_ids', { appIds: ids, steamPath: sp });
      } catch {
        return; // never let a failed probe clear what we knew was running
      }

      const previous = runningGamesRef.current;
      const closed = [...previous].filter(id => !nowRunning.includes(id));
      runningGamesRef.current = new Set(nowRunning);

      const games_ = closed
        .map(id => games.find(g => g.id === id))
        .filter((g): g is Game => Boolean(g));
      await runPass(games_, 'al cerrar el juego');
    };

    const onFocus = () => { void runPass(eligible(), 'al volver'); };
    window.addEventListener('focus', onFocus);
    const startup = setTimeout(() => { void runPass(eligible(), 'al iniciar'); }, 5000);
    const poll = setInterval(() => { void checkExits(); }, 30_000);

    return () => {
      window.removeEventListener('focus', onFocus);
      clearTimeout(startup);
      clearInterval(poll);
    };
  }, [games, steamPath, notify]);

  return (
    <div className="flex h-screen text-white font-sans overflow-hidden relative" style={{ backgroundColor: 'var(--rl-bg-primary)' }}>
      {showOnboarding && <OnboardingModal onClose={closeOnboarding} />}
      <DynamicBackground customPath={bgPath} />
      <motion.aside
        animate={{ width: sidebarExpanded ? 280 : 80 }}
        transition={{ duration: 0.3, ease: 'easeOut' }}
        onMouseEnter={() => { if (sidebarMode === 'hover') setSidebarHovering(true); }}
        onMouseLeave={() => { if (sidebarMode === 'hover') setSidebarHovering(false); }}
        className="bg-[#08080b]/80 backdrop-blur-3xl border-r border-white/[0.06] flex flex-col pt-6 shrink-0 relative z-20"
        style={{ width: sidebarExpanded ? 280 : 80 }}
      >
        {/* Single ambient glow behind sidebar */}
        <div className="absolute top-0 left-0 w-full h-64 bg-accent/5 blur-[100px] pointer-events-none" />

        {/* Arrow-toggle control (only shown in manual/arrow mode) */}
        {sidebarMode === 'arrow' && (
          <motion.button
            onClick={() => setSidebarCollapsed(!sidebarCollapsed)}
            whileHover={{ scale: 1.08 }}
            whileTap={{ scale: 0.92 }}
            transition={{ duration: 0.15, ease: 'easeOut' }}
            title={sidebarCollapsed ? 'Expandir' : 'Colapsar'}
            className="absolute -right-4 top-9 z-30 w-9 h-9 rounded-full bg-[#111319] border border-white/10 hover:border-accent/40 flex items-center justify-center text-gray-400 hover:text-accent shadow-lg shadow-black/40 transition-colors"
          >
            <motion.div
              animate={{ rotate: sidebarCollapsed ? 180 : 0 }}
              transition={{ duration: 0.3, ease: 'easeOut' }}
              className="flex items-center justify-center"
            >
              <ChevronLeft size={16} />
            </motion.div>
          </motion.button>
        )}

        <motion.div
          initial={{ opacity: 0, y: -8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.35, ease: 'easeOut' }}
          className={`mb-5 flex items-center gap-3.5 group cursor-pointer relative z-10 mt-1 ${sidebarExpanded ? 'px-8' : 'px-0 justify-center'}`}
        >
          <div className="relative shrink-0">
            <div className="absolute inset-0 bg-accent/20 blur-xl rounded-full opacity-0 group-hover:opacity-100 transition-opacity duration-500" />
            <img src={logo} alt="Ragnarok Logo" className="w-11 h-11 object-contain relative z-10 group-hover:scale-105 transition-transform duration-300" />
          </div>
          <AnimatePresence initial={false}>
            {sidebarExpanded && (
              <motion.div
                initial={{ opacity: 0, width: 0 }}
                animate={{ opacity: 1, width: 'auto' }}
                exit={{ opacity: 0, width: 0 }}
                transition={{ duration: 0.2, ease: 'easeOut' }}
                className="flex flex-col pt-1 overflow-hidden whitespace-nowrap"
              >
                <h1 className="text-[20px] font-black tracking-widest uppercase text-white/90 leading-none">
                  Ragnarok
                </h1>
                <div className="flex items-center gap-2 mt-1.5">
                  <span className="text-[9px] tracking-[0.4em] font-black text-accent uppercase opacity-90 group-hover:opacity-100 transition-opacity">
                    Launcher
                  </span>
                </div>
              </motion.div>
            )}
          </AnimatePresence>
        </motion.div>
        <motion.nav
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.35, ease: 'easeOut', delay: 0.05 }}
          className="flex-1 overflow-y-auto no-scrollbar relative z-10 pb-4"
        >
          <SidebarItem icon={Home} label={t.sidebar.home} active={activeTab === 'Home'} onClick={() => setActiveTab('Home')} collapsed={!sidebarExpanded} />
          <SidebarItem icon={Gamepad2} label={t.sidebar.games} active={activeTab === 'Games'} onClick={() => setActiveTab('Games')} collapsed={!sidebarExpanded} />
          {!adultsHidden && (
            <SidebarItem icon={Heart} label={t.sidebar.adults} active={activeTab === 'Adults'} onClick={() => setActiveTab('Adults')} collapsed={!sidebarExpanded} />
          )}
          <SidebarItem icon={Library} label={t.sidebar.library} active={activeTab === 'Library'} onClick={() => setActiveTab('Library')} collapsed={!sidebarExpanded} />
          <SidebarItem icon={Globe} label={t.sidebar.online} active={activeTab === 'Online Games'} onClick={() => setActiveTab('Online Games')} collapsed={!sidebarExpanded} />

          <SidebarItem icon={ShieldOff} label={t.sidebar.bypass} active={activeTab === 'Bypass'} onClick={() => setActiveTab('Bypass')} collapsed={!sidebarExpanded} />
          <SidebarItem icon={SwitchEmulatorsIcon} label={t.sidebar.emulators} active={activeTab === 'Emulators'} onClick={() => setActiveTab('Emulators')} collapsed={!sidebarExpanded} />
          <SidebarItem icon={Cpu} label={t.sidebar.specs} active={activeTab === 'Specs'} onClick={() => setActiveTab('Specs')} collapsed={!sidebarExpanded} />
          <SidebarItem icon={Trophy} label={t.sidebar.achievements} active={activeTab === 'Achievements'} onClick={() => setActiveTab('Achievements')} collapsed={!sidebarExpanded} />
          <SidebarItem label={t.sidebar.system} active={false} onClick={() => { }} section collapsed={!sidebarExpanded} />
          <SidebarItem icon={Settings} label={t.sidebar.settings} active={activeTab === 'Settings'} onClick={() => setActiveTab('Settings')} collapsed={!sidebarExpanded} />
          <SidebarItem icon={LifeBuoy} label={t.sidebar.support} active={activeTab === 'Support'} onClick={() => setActiveTab('Support')} collapsed={!sidebarExpanded} />

          {/* Discord Button */}
          {sidebarExpanded ? (
          <div className="px-3 pt-3 pb-2 relative z-10">
            <motion.button
              whileHover={{ y: -1 }}
              whileTap={{ scale: 0.98 }}
              transition={{ duration: 0.15, ease: 'easeOut' }}
              onClick={(e) => {
                e.preventDefault();
                import('@tauri-apps/api/shell').then(({ open }) => {
                  open('https://discord.gg/wa2wmPUC5d').catch(console.error);
                });
              }}
              className="group relative w-full flex items-center gap-3 rounded-xl bg-[#5865F2] hover:bg-[#4752C4] shadow-[0_4px_16px_rgba(88,101,242,0.35)] hover:shadow-[0_6px_22px_rgba(88,101,242,0.5)] transition-colors duration-200 px-4 py-3 overflow-hidden"
            >
              {/* Shimmer effect */}
              <div className="absolute inset-0 -translate-x-full group-hover:translate-x-full transition-transform duration-700 bg-gradient-to-r from-transparent via-white/10 to-transparent pointer-events-none" />
              {/* Discord logo */}
              <svg viewBox="0 0 24 24" width="18" height="18" fill="white" className="shrink-0 drop-shadow-sm">
                <path d="M20.317 4.3698a19.7913 19.7913 0 00-4.8851-1.5152.0741.0741 0 00-.0785.0371c-.211.3753-.4447.8648-.6083 1.2495-1.8447-.2762-3.68-.2762-5.4868 0-.1636-.3933-.4058-.8742-.6177-1.2495a.077.077 0 00-.0785-.037 19.7363 19.7363 0 00-4.8852 1.515.0699.0699 0 00-.0321.0277C.5334 9.0458-.319 13.5799.0992 18.0578a.0824.0824 0 00.0312.0561c2.0528 1.5076 4.0413 2.4228 5.9929 3.0294a.0777.0777 0 00.0842-.0276c.4616-.6304.8731-1.2952 1.226-1.9942a.076.076 0 00-.0416-.1057c-.6528-.2476-1.2743-.5495-1.8722-.8923a.077.077 0 01-.0076-.1277c.1258-.0943.2517-.1923.3718-.2914a.0743.0743 0 01.0776-.0105c3.9278 1.7933 8.18 1.7933 12.0614 0a.0739.0739 0 01.0785.0095c.1202.099.246.1981.3728.2924a.077.077 0 01-.0066.1276 12.2986 12.2986 0 01-1.873.8914.0766.0766 0 00-.0407.1067c.3604.698.7719 1.3628 1.225 1.9932a.076.076 0 00.0842.0286c1.961-.6067 3.9495-1.5219 6.0023-3.0294a.077.077 0 00.0313-.0552c.5004-5.177-.8382-9.6739-3.5485-13.6604a.061.061 0 00-.0312-.0286zM8.02 15.3312c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9555-2.4189 2.157-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.9555 2.4189-2.1569 2.4189zm7.9748 0c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9554-2.4189 2.1569-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.946 2.4189-2.1568 2.4189Z" />
              </svg>
              {/* Text */}
              <div className="flex flex-col text-left leading-none">
                <span className="text-[12px] font-extrabold text-white tracking-wide">Únete al Discord</span>
                <span className="text-[9px] font-medium text-white/60 mt-0.5">discord.gg/wa2wmPUC5d</span>
              </div>
              <ChevronRight size={13} className="ml-auto shrink-0 text-white/50 group-hover:text-white group-hover:translate-x-0.5 transition-all duration-200" />
            </motion.button>
          </div>
          ) : (
          <div className="px-2.5 pt-3 pb-2 relative z-10 flex justify-center">
            <motion.button
              whileHover={{ y: -1, scale: 1.05 }}
              whileTap={{ scale: 0.95 }}
              transition={{ duration: 0.15, ease: 'easeOut' }}
              title="Únete al Discord"
              onClick={(e) => {
                e.preventDefault();
                import('@tauri-apps/api/shell').then(({ open }) => {
                  open('https://discord.gg/wa2wmPUC5d').catch(console.error);
                });
              }}
              className="w-10 h-10 flex items-center justify-center rounded-full bg-[#5865F2] hover:bg-[#4752C4] shadow-[0_4px_16px_rgba(88,101,242,0.35)] transition-colors duration-200"
            >
              <svg viewBox="0 0 24 24" width="16" height="16" fill="white" className="shrink-0 drop-shadow-sm">
                <path d="M20.317 4.3698a19.7913 19.7913 0 00-4.8851-1.5152.0741.0741 0 00-.0785.0371c-.211.3753-.4447.8648-.6083 1.2495-1.8447-.2762-3.68-.2762-5.4868 0-.1636-.3933-.4058-.8742-.6177-1.2495a.077.077 0 00-.0785-.037 19.7363 19.7363 0 00-4.8852 1.515.0699.0699 0 00-.0321.0277C.5334 9.0458-.319 13.5799.0992 18.0578a.0824.0824 0 00.0312.0561c2.0528 1.5076 4.0413 2.4228 5.9929 3.0294a.0777.0777 0 00.0842-.0276c.4616-.6304.8731-1.2952 1.226-1.9942a.076.076 0 00-.0416-.1057c-.6528-.2476-1.2743-.5495-1.8722-.8923a.077.077 0 01-.0076-.1277c.1258-.0943.2517-.1923.3718-.2914a.0743.0743 0 01.0776-.0105c3.9278 1.7933 8.18 1.7933 12.0614 0a.0739.0739 0 01.0785.0095c.1202.099.246.1981.3728.2924a.077.077 0 01-.0066.1276 12.2986 12.2986 0 01-1.873.8914.0766.0766 0 00-.0407.1067c.3604.698.7719 1.3628 1.225 1.9932a.076.076 0 00.0842.0286c1.961-.6067 3.9495-1.5219 6.0023-3.0294a.077.077 0 00.0313-.0552c.5004-5.177-.8382-9.6739-3.5485-13.6604a.061.061 0 00-.0312-.0286zM8.02 15.3312c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9555-2.4189 2.157-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.9555 2.4189-2.1569 2.4189zm7.9748 0c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9554-2.4189 2.1569-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.946 2.4189-2.1568 2.4189Z" />
              </svg>
            </motion.button>
          </div>
          )}
        </motion.nav>

        {/* Support banner — opens DonateModal */}
        {sidebarExpanded ? (
        <motion.div
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.35, ease: 'easeOut', delay: 0.1 }}
          className="px-4 pb-4 relative z-10"
        >
          <motion.button
            whileHover={{ y: -1 }}
            whileTap={{ scale: 0.98 }}
            transition={{ duration: 0.15, ease: 'easeOut' }}
            onClick={(e) => {
              e.preventDefault();
              setShowDonate(true);
            }}
            /* Compacted from a tall card (icon block + heading + paragraph +
               button row, ~150px) to a single row: with 12 nav entries the
               sidebar was overflowing and the last options needed scrolling
               to reach. */
            className="group relative w-full flex items-center gap-3 rounded-xl bg-white/[0.02] border border-white/[0.06] hover:border-accent/25 hover:bg-white/[0.03] transition-colors duration-300 overflow-hidden px-3 py-2.5"
          >
            <div className="w-8 h-8 rounded-lg shrink-0 flex items-center justify-center bg-accent shadow-md shadow-accent/20 transition-transform duration-300 group-hover:scale-105">
              <svg viewBox="0 0 24 24" width="15" height="15" fill="white" stroke="none">
                <path d="M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z" />
              </svg>
            </div>
            <div className="flex flex-col text-left leading-none min-w-0">
              <span className="text-[12px] font-extrabold text-white/90 tracking-tight truncate">
                {ti('Apoya el Desarrollo', 'Support Development')}
              </span>
              <span className="text-[9px] font-medium text-gray-500 mt-0.5 truncate">
                {DONATION_METHODS
                  ? ti(`Donar con ${DONATION_METHODS}`, `Donate with ${DONATION_METHODS}`)
                  : ti('Ver cómo donar', 'See how to donate')}
              </span>
            </div>
            <ChevronRight size={13} className="ml-auto shrink-0 text-gray-600 group-hover:text-accent group-hover:translate-x-0.5 transition-all duration-200" />
          </motion.button>
        </motion.div>
        ) : (
        <div className="px-2.5 pb-4 relative z-10 flex justify-center">
          <motion.button
            whileHover={{ scale: 1.08 }}
            whileTap={{ scale: 0.95 }}
            transition={{ duration: 0.15, ease: 'easeOut' }}
            title={ti('Apoya el Desarrollo', 'Support Development')}
            onClick={(e) => {
              e.preventDefault();
              setShowDonate(true);
            }}
            className="w-10 h-10 rounded-full flex items-center justify-center bg-accent shadow-md shadow-accent/20 transition-transform duration-300"
          >
            <svg viewBox="0 0 24 24" width="15" height="15" fill="white" stroke="none">
              <path d="M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z" />
            </svg>
          </motion.button>
        </div>
        )}

        {/* Version Badge */}
        <motion.div
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.35, ease: 'easeOut', delay: 0.15 }}
          className={`relative z-10 flex ${sidebarExpanded ? 'px-4 pb-5' : 'px-2.5 pb-5 justify-center'}`}
        >
          {sidebarExpanded ? (
            <div className="rounded-full bg-white/[0.05] border border-white/10 px-3.5 py-2 flex items-center gap-2.5">
              <div className="relative shrink-0">
                <div className="absolute inset-0 bg-accent rounded-full blur-sm opacity-60 animate-pulse" />
                <div className="w-1.5 h-1.5 rounded-full bg-accent relative z-10" />
              </div>
              <div className="flex items-center gap-2 min-w-0">
                <span className="text-[8px] font-black uppercase tracking-[0.3em] text-gray-600">v</span>
                <span className="text-xs font-black text-white/90 tracking-wide">{__APP_VERSION__}</span>
              </div>
            </div>
          ) : (
            <div className="relative shrink-0" title={`v${__APP_VERSION__}`}>
              <div className="absolute inset-0 bg-accent rounded-full blur-sm opacity-60 animate-pulse" />
              <div className="w-1.5 h-1.5 rounded-full bg-accent relative z-10" />
            </div>
          )}
        </motion.div>
      </motion.aside>

      <main className="flex-1 flex flex-col overflow-hidden relative z-10">
        <header className="h-16 flex justify-between items-center px-8 border-b border-white/[0.06] shrink-0">
          <AnimatePresence mode="wait">
            <motion.h2
              key={activeTab}
              initial={{ opacity: 0, y: -8 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: 8 }}
              transition={{ duration: 0.25, ease: 'easeOut' }}
              className="text-2xl font-black tracking-tight text-white/90 flex items-center gap-3"
            >
              {activeTab === 'Emulators' && <SwitchEmulatorsIcon size={30} className="rounded-lg" />}
              {activeTab === 'Emulators' ? t.sidebar.emulators
                // The tab ids are English; the title is not. It used to print
                // the id itself, so a Spanish interface read "Library".
                : ({
                    Home: t.sidebar.home, Games: t.sidebar.games, Adults: t.sidebar.adults,
                    Library: t.sidebar.library, Bypass: t.sidebar.bypass, Fix: t.sidebar.fix,
                    'Online Games': t.sidebar.online,
                    Specs: t.sidebar.specs, Achievements: t.sidebar.achievements,
                    Settings: t.sidebar.settings, Support: t.sidebar.support,
                  } as Record<string, string>)[activeTab] ?? activeTab}
              <HubcapUsageBadge />
            </motion.h2>
          </AnimatePresence>
          <CatalogLoadStatus />
          {/* Bell */}
          <div className="relative" ref={bellRef}>
            <motion.button
              whileHover={{ scale: 1.03 }}
              whileTap={{ scale: 0.97 }}
              transition={{ duration: 0.15, ease: 'easeOut' }}
              onClick={() => setBellOpen(o => !o)}
              className="relative w-10 h-10 flex items-center justify-center rounded-full hover:bg-white/[0.05] transition-colors text-gray-500 hover:text-white"
            >
              <Bell size={19} />
              {unreadCount > 0 && (
                <span className="absolute top-1.5 right-1.5 w-4 h-4 bg-accent rounded-full text-[9px] font-black flex items-center justify-center text-white leading-none">
                  {unreadCount > 9 ? '9+' : unreadCount}
                </span>
              )}
            </motion.button>
            <AnimatePresence>
              {bellOpen && (
                <motion.div
                  initial={{ opacity: 0, y: -8, scale: 0.97 }}
                  animate={{ opacity: 1, y: 0, scale: 1 }}
                  exit={{ opacity: 0, y: -8, scale: 0.97 }}
                  transition={{ duration: 0.2, ease: 'easeOut' }}
                  className="absolute right-0 top-13 z-50 w-[340px]"
                  style={{ filter: 'drop-shadow(0 16px 48px rgba(0,0,0,0.6))' }}
                >
                  <div className="bg-[#0a0a0d]/95 backdrop-blur-2xl border border-white/[0.08] rounded-2xl overflow-hidden">

                    {/* Header */}
                    <div className="flex items-center justify-between px-4 py-3.5 border-b border-white/[0.06]">
                      <div className="flex items-center gap-2.5">
                        <Bell size={13} className="text-gray-500" />
                        <span className="text-[10px] font-black uppercase tracking-[0.2em] text-gray-400">{t.notifications.title}</span>
                        {unreadCount > 0 && (
                          <span className="px-1.5 py-0.5 rounded-full bg-accent text-white text-[9px] font-black leading-none">
                            {unreadCount}
                          </span>
                        )}
                      </div>
                      {unreadCount > 0 && (
                        <button
                          onClick={() => setNotifications(prev => prev.map(n => ({ ...n, read: true })))}
                          className="text-[9px] font-black uppercase tracking-wider text-gray-500 hover:text-white transition-colors"
                        >
                          {t.notifications.markRead}
                        </button>
                      )}
                    </div>

                    {/* Body */}
                    <div className="max-h-[320px] overflow-y-auto custom-scrollbar">
                      {notifications.length === 0 ? (
                        <div className="flex flex-col items-center gap-3 py-12 px-6">
                          <div className="w-12 h-12 rounded-2xl bg-white/[0.03] border border-white/[0.06] flex items-center justify-center">
                            <Bell size={18} className="text-gray-700" />
                          </div>
                          <p className="text-[11px] text-gray-600 font-bold text-center">{t.notifications.empty}</p>
                        </div>
                      ) : (
                        <div className="py-1">
                          <AnimatePresence initial={false}>
                            {notifications.map((n, i) => {
                              const isUpdate = n.type === 'update';
                              const isSupportReply = n.type === 'support_reply';
                              const isNewBypass = n.type === 'new_bypass';
                              const isHubcap = n.type === 'hubcap';
                              const relTime = (() => {
                                const diff = Date.now() - n.timestamp;
                                if (diff < 60_000) return 'ahora';
                                if (diff < 3_600_000) return `hace ${Math.floor(diff / 60_000)}m`;
                                if (diff < 86_400_000) return `hace ${Math.floor(diff / 3_600_000)}h`;
                                return `hace ${Math.floor(diff / 86_400_000)}d`;
                              })();
                              return (
                                <motion.div
                                  key={n.id}
                                  layout
                                  initial={{ opacity: 0, y: -6 }}
                                  animate={{ opacity: 1, y: 0 }}
                                  exit={{ opacity: 0, x: 24 }}
                                  transition={{ duration: 0.2, ease: 'easeOut' }}
                                  className={`group relative flex items-start gap-3 px-4 py-3 transition-colors ${
                                    i < notifications.length - 1 ? 'border-b border-white/[0.04]' : ''
                                  } ${n.read ? 'opacity-40' : 'hover:bg-white/[0.03]'}`}
                                >
                                  {/* Icon */}
                                  <div className={`mt-0.5 w-8 h-8 rounded-xl flex items-center justify-center shrink-0 ${
                                    isUpdate ? 'bg-red-500/10 border border-red-500/15' : isSupportReply ? 'bg-sky-500/10 border border-sky-500/15' : isNewBypass ? 'bg-violet-500/10 border border-violet-500/15' : isHubcap ? 'bg-amber-500/10 border border-amber-500/15' : 'bg-emerald-500/10 border border-emerald-500/15'
                                  }`}>
                                    {isUpdate
                                      ? <Download size={14} className="text-red-400" />
                                      : isSupportReply
                                        ? <MessageSquare size={14} className="text-sky-400" />
                                        : isNewBypass
                                          ? <ShieldOff size={14} className="text-violet-400" />
                                          : isHubcap
                                            ? <AlertTriangle size={14} className="text-amber-400" />
                                            : <Zap size={14} className="text-emerald-400" />}
                                  </div>

                                  {/* Text */}
                                  <div className="flex-1 min-w-0 pt-0.5">
                                    <div className="flex items-center gap-1.5 mb-1">
                                      <span className={`text-[8px] font-black uppercase tracking-widest px-1.5 py-0.5 rounded-full ${
                                        isUpdate
                                          ? 'bg-red-500/10 text-red-400'
                                          : isSupportReply
                                            ? 'bg-sky-500/10 text-sky-400'
                                            : isNewBypass
                                              ? 'bg-violet-500/10 text-violet-400'
                                              : isHubcap
                                                ? 'bg-amber-500/10 text-amber-400'
                                              : 'bg-emerald-500/10 text-emerald-400'
                                      }`}>
                                        {isUpdate ? 'Update' : isSupportReply ? 'Soporte' : isNewBypass ? 'Bypass' : isHubcap ? 'Hubcap' : 'Juegos'}
                                      </span>
                                      {!n.read && (
                                        <span className={`w-1.5 h-1.5 rounded-full ${isUpdate ? 'bg-accent' : isSupportReply ? 'bg-sky-400' : 'bg-emerald-400'}`} />
                                      )}
                                    </div>
                                    <p className="text-[11px] font-semibold text-gray-200 leading-snug">{n.message}</p>
                                    <p className="text-[9px] text-gray-600 mt-1 font-bold">{relTime}</p>
                                  </div>

                                  {/* Dismiss */}
                                  <button
                                    onClick={() => dismissNotification(n.id)}
                                    className="opacity-0 group-hover:opacity-100 transition-opacity mt-1 p-0.5 text-gray-600 hover:text-gray-300 rounded"
                                  >
                                    <X size={12} />
                                  </button>
                                </motion.div>
                              );
                            })}
                          </AnimatePresence>
                        </div>
                      )}
                    </div>

                    {/* Footer */}
                    {notifications.length > 0 && (
                      <div className="px-4 py-2.5 border-t border-white/[0.06] flex items-center justify-end">
                        <button
                          onClick={() => setNotifications([])}
                          className="text-[9px] font-black uppercase tracking-wider text-gray-600 hover:text-gray-400 transition-colors"
                        >
                          Borrar todo
                        </button>
                      </div>
                    )}
                  </div>
                </motion.div>
              )}
            </AnimatePresence>
          </div>
        </header>
        <div className="flex-1 overflow-y-auto p-8 custom-scrollbar">
          <AnimatePresence mode="wait">
            <motion.div
              key={activeTab}
              initial={{ opacity: 0, x: 10 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -10 }}
              transition={{ duration: 0.2 }}
            >
              {activeTab === 'Home' && <HomeView games={games} steamPath={steamPath} onNavigate={setActiveTab} loading={catalogLoading} setGames={setGames} />}
              {activeTab === 'Games' && (catalogMissing
                ? <CatalogPending syncing={catalogLoading || catalogSyncs > 0} />
                : <CatalogView games={visibleCatalogGames} onSelect={setSelectedGame} onNeedDrm={resolveDrmFor} rankings={steamRankings ?? undefined} />)}
              {activeTab === 'Adults' && !adultsHidden && (
                <div>
                  <div className="flex items-center justify-between gap-3 mb-4 flex-wrap">
                    <p className="text-xs text-gray-500 font-semibold">
                      {ti(
                        'La clasificación viene del catálogo y a veces se equivoca — verificala contra Steam.',
                        'Classification comes from the catalog and is sometimes wrong — verify it against Steam.'
                      )}
                    </p>
                    <button
                      onClick={handleVerifyNsfwClassification}
                      disabled={!!nsfwCheck}
                      className="shrink-0 flex items-center gap-2 px-4 py-2 rounded-xl bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 text-[10px] font-black uppercase tracking-widest text-gray-300 hover:text-white transition-all disabled:opacity-60"
                    >
                      <RefreshCw size={12} className={nsfwCheck ? 'animate-spin' : ''} />
                      {nsfwCheck
                        ? `${ti('Revisando', 'Checking')}... ${nsfwCheck.checked}/${nsfwCheck.total}`
                        : ti('Revisar Clasificación', 'Verify Classification')}
                    </button>
                  </div>
                  <CatalogView
                    games={adultsCatalogGames}
                    onSelect={setSelectedGame}
                    onNeedDrm={resolveDrmFor}
                  />
                </div>
              )}
              {activeTab === 'Emulators' && <EmulatorsView />}
              {activeTab === 'Specs' && <SpecMatcherView games={games} steamPath={steamPath} />}
              {activeTab === 'Library' && (
                <LibraryView
                  games={games}
                  onInstall={handleInstall}
                  steamPath={steamPath}
                  onUninstall={handleUninstall}
                  onNeedDrm={resolveDrmFor}
                />
              )}
              {activeTab === 'Online Games' && <OnlineGamesView />}
              {activeTab === 'Settings' && (
                <SettingsView
                  pendingUpdate={pendingUpdate}
                  onShowUpdate={(info) => {
                    // Only open with something to show. Flipping the flag on
                    // its own renders nothing, which is indistinguishable
                    // from a dead button.
                    const shown = info ?? pendingUpdate;
                    if (!shown) return;
                    setPendingUpdate(shown);
                    setShowUpdateOverlay(true);
                  }}
                  onSyncCatalog={async () => {
                    // The chosen catalog's saved copy goes up at once; the sync
                    // after it only continues or checks for changes.
                    await loadCachedCatalogFast(steamPath, true);
                    await loadCatalog(steamPath, true);
                  }}
                  steamPath={steamPath}
                  sidebarMode={sidebarMode}
                  setSidebarMode={setSidebarMode}
                  games={games}
                />
              )}
              {activeTab === 'Fix' && <FixView steamPath={steamPath} />}
              {activeTab === 'Bypass' && <BypassView games={games} steamPath={steamPath} />}
              {activeTab === 'Achievements' && <AchievementsView steamPath={steamPath} games={games} />}
              {activeTab === 'Support' && <SupportView />}
            </motion.div>
          </AnimatePresence>
        </div>
      </main>

      {/* Game Detail — full screen overlay */}
      <AnimatePresence>
        {selectedGame && (
          <GameDetailView
            game={selectedGame}
            steamPath={steamPath}
            rank={rankInCharts(steamRankings, selectedGame.id)}
            onClose={() => setSelectedGame(null)}
            onInstall={handleInstall}
            onNameResolved={(id, name) => {
              setGames(prev => prev.map(g => g.id === id ? { ...g, name } : g));
              try {
                const cache = getCleanNameCache();
                cache[id] = name;
                localStorage.setItem('rl_names', JSON.stringify(cache));
              } catch { }
            }}
          />
        )}
      </AnimatePresence>

      {/* Update countdown overlay */}
      <AnimatePresence>
        <AnimatePresence>
          {showDonate && <DonateModal onClose={() => setShowDonate(false)} />}
        </AnimatePresence>

        {showUpdateOverlay && pendingUpdate && (
          <UpdateCountdownOverlay
            info={pendingUpdate}
            onCancel={() => setShowUpdateOverlay(false)}
          />
        )}
      </AnimatePresence>

      {/* Steam install picker — only ever appears when more than one real
          Steam client was detected on this PC and the user hasn't already
          picked one in a previous launch. */}
      <AnimatePresence>
        {steamInstallChoices.length > 0 && (
          <SteamInstallPickerModal
            choices={steamInstallChoices}
            defaultPath={steamPath}
            onSelect={(path) => {
              localStorage.setItem('rl_steam_path_override', path);
              setSteamPath(path);
              setSteamInstallChoices([]);
            }}
            onSkip={() => setSteamInstallChoices([])}
          />
        )}
      </AnimatePresence>
    </div>
  );
};

const App = () => {
  const [showSplash, setShowSplash] = useState(true);

  // MainContent se monta un frame después del splash, no a la vez.
  //
  // Montarlo simultáneamente adelantaba su trabajo (que es el objetivo), pero
  // su montaje bloquea el hilo principal lo suficiente como para robarle al
  // splash su primer pintado: el usuario veía aparecer la barra ya empezada.
  // Un frame de diferencia deja que el splash se dibuje en 0% y recién ahí
  // arranca lo pesado, sin perder nada del solapamiento: sigue corriendo
  // durante toda la animación en vez de esperar a que termine.
  const [mountMain, setMountMain] = useState(false);
  useEffect(() => {
    const raf = requestAnimationFrame(() => setMountMain(true));
    return () => cancelAnimationFrame(raf);
  }, []);

  // Mostrar la ventana solo cuando React ya pintó.
  //
  // La ventana nace oculta (visible: false en tauri.conf.json) porque antes
  // aparecía en cuanto arrancaba el proceso y se quedaba negra el tiempo que
  // tardara el webview en levantar y parsear el bundle. useEffect corre
  // después del commit pero no necesariamente después del primer pintado,
  // así que el rAF anidado espera al frame siguiente: para cuando se muestra,
  // hay algo dibujado. Si esto falla, el failsafe de Rust la muestra igual a
  // los 8 s.
  useEffect(() => {
    let raf = 0;
    import('@tauri-apps/api/window').then(({ appWindow }) => {
      raf = requestAnimationFrame(() => {
        raf = requestAnimationFrame(() => {
          appWindow.show();
          appWindow.setTitle(`Ragnarok Launcher v${__APP_VERSION__}`);
        });
      });
    }).catch(console.error);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <ThemeProvider>
      <LanguageProvider>
        <NotificationProvider>
          {/* MainContent va debajo del splash en lugar de esperar a que
              termine: su arranque real —catálogo en caché, escaneo de
              instalados, tamaños— corre durante la animación, así que cuando
              el splash se va la app ya está poblada. El splash queda encima y
              se desvanece sobre ella. */}
          <div className="w-full h-full">
            {mountMain && <MainContent />}
          </div>
          <AnimatePresence>
            {showSplash && (
              <motion.div
                key="splash"
                exit={{ opacity: 0, filter: "blur(10px)" }}
                transition={{ duration: 0.5, ease: "easeInOut" }}
                className="fixed inset-0 z-50"
              >
                <SplashNuevo onComplete={() => setShowSplash(false)} />
              </motion.div>
            )}
          </AnimatePresence>
        </NotificationProvider>
      </LanguageProvider>
    </ThemeProvider>
  );
};

export default App;
