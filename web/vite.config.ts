import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import path from 'node:path'

const apiProxyTarget =
  process.env.VITE_API_PROXY_TARGET ??
  process.env.FORGE_E2E_BACKEND_BASE_URL ??
  'http://localhost:8080'

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    setupFiles: './src/test/setup.ts',
  },
  server: {
    ...(process.env.FORGE_E2E_WEB_PORT
      ? {
          host: '127.0.0.1',
          port: Number(process.env.FORGE_E2E_WEB_PORT),
          strictPort: true,
        }
      : {}),
    proxy: {
      '/api': {
        target: apiProxyTarget,
        changeOrigin: true,
        ws: true,
      },
      '/mcp': {
        target: apiProxyTarget,
        changeOrigin: true,
      },
    },
  },
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
})
