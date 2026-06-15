# ECNU-VPN Agent Rules

- This repository is synced across machines, so keep the working tree light.
- For any Node-based install, build, packaging, or test workflow in this repository, use `pnpm` exclusively. Do not use `npm` or `npx`.
- Treat any `package-lock.json` as stale unless the user explicitly asks to keep it; the canonical lockfile is `pnpm-lock.yaml`.
- After any test, build, packaging, or verification run, remove generated artifacts from synced directories as soon as the result is no longer needed.
- Treat these as cleanup-first artifacts unless the user explicitly asks to keep them:
  - `webui/node_modules/`
  - `webui/native/bin/`
  - `webui/dist-electron/`
  - temporary Electron/Vite/dev logs under `webui/` (`*.log`)
  - temporary screenshots or debug captures under `webui/` (`*.png`)
  - stale `.claude/worktrees/` directories and other throwaway local build outputs
- Before finishing a task, check for leftover generated artifacts in synced directories and delete them if they are no longer needed.
- Do not commit cleanup-only artifact changes unless the user asks for that commit explicitly.
