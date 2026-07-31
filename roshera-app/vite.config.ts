import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'path'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  optimizeDeps: {
    include: ['postprocessing', '@react-three/postprocessing'],
  },
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://localhost:8081',
      // Agent Client Protocol. Without this the browser's /acp calls are
      // served by Vite itself, 404, and the Blackboard silently falls back to
      // the legacy single-shot path — the agent looks connected and answers
      // "Command processed." Streaming must not be buffered: session/new and
      // every session/update arrive on an SSE body, so a proxy that waits for
      // a complete response would hang the turn rather than stream it.
      '/acp': {
        target: 'http://localhost:8081',
        changeOrigin: true,
        configure: (proxy) => {
          proxy.on('proxyRes', (proxyRes) => {
            if ((proxyRes.headers['content-type'] ?? '').includes('text/event-stream')) {
              delete proxyRes.headers['content-length']
              proxyRes.headers['cache-control'] = 'no-cache, no-transform'
            }
          })
        },
      },
      '/ws': {
        target: 'ws://localhost:8081',
        ws: true,
      },
    },
  },
})
