import { defineConfig } from 'vite';

// Tauri dev expects a fixed port (tauri.conf.json → build.devUrl). 1450 so it runs beside CarMD Utils (1440).
export default defineConfig({
  clearScreen: false,
  server: { port: 1450, strictPort: true },
  build: { target: 'es2022' },
});
