import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import adonisjs from '@adonisjs/vite/client'
import { imagetools } from 'vite-imagetools'

export default defineConfig({
  plugins: [
    react(),
    adonisjs({
      entryPoints: ['inertia/app.tsx'],
      reload: ['resources/views/**/*.edge'],
    }),
    imagetools(),
  ],

  /**
   * Define aliases for importing modules from
   * your frontend code
   */
  resolve: {
    alias: {
      '~/': `${import.meta.dirname}/inertia/`,
      '@generated': `${import.meta.dirname}/.adonisjs/client/`,
    },
  },

  server: {
    watch: {
      ignored: ['**/storage/**', '**/tmp/**'],
    },
  },
})
