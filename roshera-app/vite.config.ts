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
      '/ws': {
        target: 'ws://localhost:8081',
        ws: true,
      },
    },
  },
})
