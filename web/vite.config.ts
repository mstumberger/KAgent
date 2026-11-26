import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  // `pnpm dev` runs against the daemon that is up (ui-settings.md section 7). The
  // daemon checks the Host header: `changeOrigin` sends it as 127.0.0.1:7878.
  server: {
    proxy: {
      '/api': { target: 'http://127.0.0.1:7878', changeOrigin: true },
      '/ws': { target: 'http://127.0.0.1:7878', ws: true, changeOrigin: true },
      '/live': { target: 'http://127.0.0.1:7878', ws: true, changeOrigin: true },
    },
  },
})
