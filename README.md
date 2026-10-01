# Ragnarok Launcher 🚀

<div align="center">

![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)
![Tauri](https://img.shields.io/badge/Tauri-v1-orange.svg)
![React](https://img.shields.io/badge/React-18-61dafb.svg)
![Rust](https://img.shields.io/badge/Rust-1.70+-dea584.svg)
![TypeScript](https://img.shields.io/badge/TypeScript-5-3178c6.svg)

**A modern, blazing-fast, and open-source game launcher built with Tauri, Rust, and React.**

</div>

---

## ✨ Features

- **Blazing Fast Native Backend**: Built with Rust and Tauri for minimal memory footprint and maximum performance.
- **Modern UI**: Smooth interface built with React, TypeScript, and modern styling.
- **Smart Catalog**: Multi-tier game catalog fallback system for maximum reliability.
- **Local & Cloud Save Backups**: Automatic game save versioning and backup management.
- **Integrated Download Manager**: Resilient multi-part chunk downloading with retry logic and mirror failover.
- **Emulator & Tool Support**: Built-in support for launchers, emulators, and workshop tools.

---

## 🛠️ Tech Stack

- **Backend**: Rust (`src-tauri/`) using [Tauri](https://tauri.app/)
- **Frontend**: React 18, TypeScript, Vite, TailwindCSS / Vanilla CSS
- **IPC**: Type-safe Tauri commands and events

---

## 🚀 Getting Started

### Prerequisites

Make sure you have installed:
1. **Node.js** (v18+ recommended)
2. **Rust & Cargo** ([rustup.rs](https://rustup.rs/))
3. **C++ Build Tools** (Visual Studio Build Tools with C++ workload on Windows)

### Installation

1. Clone the repository:
   ```bash
   git clone https://github.com/RagnarokManifests/ragnarok-launcher.git
   cd ragnarok-launcher
   ```

2. Install frontend dependencies:
   ```bash
   npm install
   ```

3. Run in development mode:
   ```bash
   npm run tauri dev
   ```

### Building for Production

To compile the optimized production executable and installer:

```bash
npm run tauri build
```

The resulting binaries will be placed in `src-tauri/target/release/bundle/`.

---

## 📜 License & Copyleft Notice

This project is licensed under the **GNU General Public License v3.0 (GPL-3.0)**.

> ### ⚠️ Copyleft Notice ("No Closed-Source Derivative Works")
> Under the terms of the **GNU GPLv3**:
> - Anyone is free to read, use, modify, and distribute this software.
> - **Copyleft Requirement**: If you modify this project, incorporate parts of its code, or create a derivative software product, **YOUR ENTIRE PROGRAM MUST ALSO BE OPEN-SOURCE under the GNU GPLv3 license**.
> - **You CANNOT take this code and include it in a closed-source, private, or proprietary application.**
> - Doing so is a direct violation of international copyright law.

See the full [LICENSE](LICENSE) file for complete legal details.
